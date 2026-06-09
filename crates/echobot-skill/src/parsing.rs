//! `SKILL.md` parser + helpers for extracting active skills from
//! chat history.

use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;

use echobot_core::models::{message_content_to_text, LLMMessage};

use crate::models::Skill;

static ACTIVE_SKILL_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"<active_skill name="([^"]+)">"#).expect("valid active-skill regex"));
static LEGACY_SKILL_NAME_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^Skill name:\s*(.+?)\s*$").expect("valid legacy-skill regex")
});
static EXPLICIT_SKILL_TOKEN_PATTERN: Lazy<Regex> = Lazy::new(|| {
    // Look-aheads are not supported by the `regex` crate, so we match
    // (but do not capture) the trailing character. The skill name is
    // captured in group 1.
    Regex::new(r"(?:^|[\s(])(?:/|\$)([a-z0-9_-]{1,64})(?:$|[\s),.!?])")
        .expect("valid token regex")
});

const MULTILINE_FRONTMATTER_MARKERS: &[&str] = &["|", "|-", "|+", ">", ">-", ">+"];

/// Parses a `SKILL.md` file into a [`Skill`].
pub fn parse_skill_file(path: impl AsRef<Path>) -> Result<Skill, String> {
    let skill_file: PathBuf = path.as_ref().to_path_buf();
    let content = std::fs::read_to_string(&skill_file)
        .map_err(|e| format!("Cannot read {}: {e}", skill_file.display()))?;
    let content = strip_utf8_bom(&content);
    let (frontmatter_lines, body) = split_frontmatter(content)?;
    let name = read_frontmatter_value(&frontmatter_lines, "name", false)?;
    let description = read_frontmatter_value(&frontmatter_lines, "description", true)?;
    if name.is_empty() {
        return Err("Missing name in frontmatter".to_string());
    }
    if description.is_empty() {
        return Err("Missing description in frontmatter".to_string());
    }
    Ok(Skill {
        name,
        description,
        directory: skill_file
            .parent()
            .ok_or_else(|| "SKILL.md has no parent directory".to_string())?
            .to_path_buf(),
        skill_file,
        body: body.trim().to_string(),
        frontmatter: frontmatter_lines.join("\n").trim().to_string(),
    })
}

/// Extracts explicit `/skill-name` or `$skill-name` tokens from `text`.
pub fn extract_explicit_skill_tokens(text: &str) -> Vec<String> {
    EXPLICIT_SKILL_TOKEN_PATTERN
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Walks `history` and returns the active skill names mentioned in
/// tool / system messages, filtered to the registered skill names.
pub fn extract_active_skill_names_from_history(
    history: &[LLMMessage],
    available_skill_names: &[String],
) -> Vec<String> {
    let available: std::collections::HashSet<&str> = available_skill_names
        .iter()
        .map(String::as_str)
        .collect();
    let mut found: Vec<String> = Vec::new();
    for message in history {
        for name in extract_active_skill_names_from_message(message) {
            if available.contains(name.as_str()) && !found.iter().any(|n| n == &name) {
                found.push(name);
            }
        }
    }
    found
}

fn strip_utf8_bom(content: &str) -> &str {
    if let Some(stripped) = content.strip_prefix('\u{feff}') {
        stripped
    } else {
        content
    }
}

fn split_frontmatter(content: &str) -> Result<(Vec<String>, String), String> {
    let normalized = content.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return Err("Invalid SKILL.md frontmatter".to_string());
    }
    let mut end_index: Option<usize> = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end_index = Some(i);
            break;
        }
    }
    let end_index = end_index.ok_or_else(|| "Invalid SKILL.md frontmatter".to_string())?;
    let frontmatter_lines: Vec<String> = lines[1..end_index]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let body = lines
        .get(end_index + 1..)
        .map(|slice| slice.join("\n"))
        .unwrap_or_default();
    Ok((frontmatter_lines, body))
}

fn read_frontmatter_value(
    frontmatter_lines: &[String],
    key: &str,
    allow_multiline: bool,
) -> Result<String, String> {
    let mut index = 0;
    while index < frontmatter_lines.len() {
        let entry = match parse_frontmatter_entry(&frontmatter_lines[index]) {
            Some(e) => e,
            None => {
                index += 1;
                continue;
            }
        };
        let (current_key, raw_value) = entry;
        if current_key != key {
            index += 1;
            continue;
        }
        if MULTILINE_FRONTMATTER_MARKERS.contains(&raw_value.as_str()) {
            if !allow_multiline {
                return Err(format!("{key} must be a single-line value"));
            }
            return Ok(read_multiline_frontmatter_value(frontmatter_lines, index + 1));
        }
        return Ok(strip_optional_quotes(&raw_value));
    }
    Ok(String::new())
}

fn parse_frontmatter_entry(line: &str) -> Option<(String, String)> {
    if line.trim().is_empty() {
        return None;
    }
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let colon = line.find(':')?;
    let key = line[..colon].trim().to_string();
    let value = line[colon + 1..].trim().to_string();
    Some((key, value))
}

fn read_multiline_frontmatter_value(frontmatter_lines: &[String], start_index: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = start_index;
    while index < frontmatter_lines.len() {
        let line = &frontmatter_lines[index];
        if line.starts_with(' ') || line.starts_with('\t') {
            let cleaned = line.trim();
            if !cleaned.is_empty() {
                parts.push(cleaned.to_string());
            }
            index += 1;
            continue;
        }
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        break;
    }
    parts.join(" ").trim().to_string()
}

fn strip_optional_quotes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.chars().next().unwrap_or(' ');
        let last = trimmed.chars().last().unwrap_or(' ');
        if first == last && (first == '"' || first == '\'') {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn extract_active_skill_names_from_message(message: &LLMMessage) -> Vec<String> {
    use echobot_core::models::MessageRole;
    let text = message_content_to_text(&message.content);
    match message.role {
        MessageRole::System => extract_active_skill_names_from_text(&text),
        MessageRole::Tool => extract_active_skill_names_from_tool_payload(&text),
        _ => Vec::new(),
    }
}

fn extract_active_skill_names_from_text(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for cap in ACTIVE_SKILL_PATTERN.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let cleaned = m.as_str().trim().to_string();
            if !cleaned.is_empty() && !names.iter().any(|n| n == &cleaned) {
                names.push(cleaned);
            }
        }
    }
    for cap in LEGACY_SKILL_NAME_PATTERN.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let cleaned = m.as_str().trim().to_string();
            if !cleaned.is_empty() && !names.iter().any(|n| n == &cleaned) {
                names.push(cleaned);
            }
        }
    }
    names
}

fn extract_active_skill_names_from_tool_payload(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    if obj.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let Some(result) = obj.get("result").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let Some(name) = result.get("name").and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    let kind = result.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let has_directory = result.contains_key("directory");
    let has_content = result.contains_key("content");
    if kind == "skill_activation" || (has_directory && has_content) {
        vec![name.to_string()]
    } else {
        Vec::new()
    }
}
