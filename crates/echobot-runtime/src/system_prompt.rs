//! Default system-prompt construction.
//!
//! Mirrors `echobot/runtime/system_prompt.py`. The result is the markdown
//! blob the agent is initialized with. Sections are conditionally appended
//! based on which features the runtime has enabled (memory, scheduling, ...).
//!
//! The Python version also reads workspace `AGENTS.md` files as a bootstrap
//! section. The same convention is used here, with the file list exposed as
//! [`BOOTSTRAP_FILES`].

use std::path::{Path, PathBuf};

/// Bootstrap files (relative to the workspace) that are appended to the
/// system prompt when present.
pub const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md"];

/// Knobs that control which sections the system prompt includes.
#[derive(Debug, Clone)]
pub struct SystemPromptOptions {
    /// Whether the underlying LLM accepts image inputs.
    pub supports_image_input: bool,
    /// Whether long-term memory is enabled.
    pub enable_project_memory: bool,
    /// Memory workspace (defaults to `<workspace>/.echobot/memory`).
    pub memory_workspace: Option<PathBuf>,
    /// Whether scheduling is enabled (cron + heartbeat sections).
    pub enable_scheduling: bool,
    /// Cron store path. Defaults to `<workspace>/.echobot/cron/jobs.json`.
    pub cron_store_path: Option<PathBuf>,
    /// Heartbeat file path. Defaults to `<workspace>/.echobot/HEARTBEAT.md`.
    pub heartbeat_file_path: Option<PathBuf>,
    /// Heartbeat interval in seconds. Defaults to 1800.
    pub heartbeat_interval_seconds: Option<u64>,
    /// Shell-safety mode label.
    pub shell_safety_mode: String,
    /// Whether file-write tools are enabled.
    pub file_write_enabled: bool,
    /// Whether cron-mutating tools are enabled.
    pub cron_mutation_enabled: bool,
    /// Whether private-network web access is enabled.
    pub web_private_network_enabled: bool,
}

impl Default for SystemPromptOptions {
    fn default() -> Self {
        Self {
            supports_image_input: true,
            enable_project_memory: false,
            memory_workspace: None,
            enable_scheduling: false,
            cron_store_path: None,
            heartbeat_file_path: None,
            heartbeat_interval_seconds: None,
            shell_safety_mode: "danger-full-access".to_string(),
            file_write_enabled: true,
            cron_mutation_enabled: true,
            web_private_network_enabled: false,
        }
    }
}

/// Builds the default system prompt. Sections are joined with `\n\n---\n\n`
/// (matches Python's `"\n\n---\n\n".join(parts)`).
pub fn build_default_system_prompt(
    workspace: impl AsRef<Path>,
    options: &SystemPromptOptions,
) -> String {
    let workspace = workspace.as_ref();
    let mut parts: Vec<String> = Vec::new();
    parts.push(build_identity_section(workspace));
    parts.push(build_operating_rules_section(options));
    if options.enable_project_memory {
        parts.push(build_memory_section(
            workspace,
            options.memory_workspace.as_deref(),
        ));
    }
    if options.enable_scheduling {
        parts.push(build_scheduling_section(
            workspace,
            options.cron_store_path.as_deref(),
            options.heartbeat_file_path.as_deref(),
            options.heartbeat_interval_seconds,
        ));
    }
    parts.push(build_delivery_section(options.supports_image_input));

    let bootstrap = load_bootstrap_files(workspace);
    if !bootstrap.is_empty() {
        parts.push(bootstrap);
    }
    parts.join("\n\n---\n\n")
}

fn build_identity_section(workspace: &Path) -> String {
    let system_name = std::env::consts::OS;
    let session_store = workspace.join(".echobot").join("sessions");
    let lines = [
        "# EchoBot",
        "",
        "You are EchoBot, the full tool-using agent operating inside the user's project workspace.",
        "",
        "## Environment",
        &format!("- OS: {system_name}"),
        &format!("- Workspace: {}", workspace.display()),
        &format!("- Session store: {}", session_store.display()),
        "",
    ];
    lines.join("\n").trim().to_string()
}

fn build_operating_rules_section(options: &SystemPromptOptions) -> String {
    let shell_mode = &options.shell_safety_mode;
    let file_writes_enabled = options.file_write_enabled;
    let cron_mutations_enabled = options.cron_mutation_enabled;
    let private_network_enabled = options.web_private_network_enabled;
    let lines: Vec<String> = vec![
        "## Core Rules".into(),
        "- Use tools, memory, and workspace inspection to get real answers. Do not guess when the answer depends on external state.".into(),
        "- Do not invent file contents, code changes, command output, schedule state, or prior memory.".into(),
        "- If a request depends on project files, code, schedules, or stored memory, inspect them before answering.".into(),
        "- For multi-step work, use `update_plan` to keep a short plan current.".into(),
        "- If you are blocked by missing requirements or ambiguity, use `request_user_input` instead of guessing.".into(),
        format!("- Current shell safety mode: `{shell_mode}`."),
        "- In `read-only` and `workspace-write` shell modes, `run_shell_command` only accepts simple allowlisted commands.".into(),
        format!(
            "- Workspace file writes are currently {}.",
            if file_writes_enabled { "enabled" } else { "disabled" }
        ),
        format!(
            "- Cron mutations are currently {}.",
            if cron_mutations_enabled { "enabled" } else { "disabled" }
        ),
        if private_network_enabled {
            "- Private-network web access is currently enabled for `fetch_web_page`.".into()
        } else {
            "- `fetch_web_page` can only access public web hosts; localhost and private IPs are blocked.".into()
        },
        "- If a tool is blocked by runtime safety settings, explain that clearly instead of pretending it worked.".into(),
        "- When a tool or file gives the answer, base your response on that evidence instead of paraphrasing loosely from memory.".into(),
        "- Preserve exact technical details when they matter: paths, commands, code, JSON, identifiers, timestamps, and error messages.".into(),
        "- Keep responses concise, but do not omit critical caveats, failure details, or uncertainty.".into(),
        "- If the needed evidence is missing, say what is missing instead of pretending it was checked.".into(),
    ];
    lines.join("\n")
}

fn build_memory_section(workspace: &Path, memory_workspace: Option<&Path>) -> String {
    let resolved = memory_workspace
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| workspace.join(".echobot").join("memory"));
    let memory_file = resolved.join("MEMORY.md");
    let daily_notes = resolved.join("memory");
    let tool_cache = resolved.join("tool_result");
    let lines = [
        "## Memory",
        &format!("- Memory workspace: {}", resolved.display()),
        &format!("- Long-term memory file: {}", memory_file.display()),
        &format!("- Daily notes directory: {}", daily_notes.display()),
        &format!("- Long tool output cache: {}", tool_cache.display()),
        "- Before answering questions about prior work, decisions, dates, preferences, todos, or what the user shared earlier, call `memory_search`.",
        "- Keep durable user preferences and recurring setup notes in `MEMORY.md`.",
        "- Daily notes in `memory/YYYY-MM-DD.md` are raw session memory. `MEMORY.md` should stay curated and compact.",
        "- If a cached tool result points to `tool_result/*.txt`, use the file tools to read the full content when needed.",
        "- If memory search does not provide enough evidence, say that clearly instead of pretending to remember.",
    ];
    lines.join("\n")
}

fn build_scheduling_section(
    workspace: &Path,
    cron_store_path: Option<&Path>,
    heartbeat_file_path: Option<&Path>,
    heartbeat_interval_seconds: Option<u64>,
) -> String {
    let cron_path = cron_store_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| workspace.join(".echobot").join("cron").join("jobs.json"));
    let heartbeat_path = heartbeat_file_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| workspace.join(".echobot").join("HEARTBEAT.md"));
    let interval_text = heartbeat_interval_seconds
        .map(|n| n.to_string())
        .unwrap_or_else(|| "1800".to_string());
    let lines = [
        "## Scheduling",
        &format!("- Cron job store: {}", cron_path.display()),
        &format!("- Heartbeat file: {}", heartbeat_path.display()),
        &format!("- Heartbeat interval: {interval_text} seconds"),
        "- Use the `cron` tool for exact schedules or one-time reminders.",
        "- Prefer `task_type=\"text\"` when the future notification should send fixed wording.",
        "- Use `task_type=\"agent\"` only when the future run must re-check information or perform work at execution time.",
        "- For one-time reminders like 'in 20 seconds' or '20 minutes later', use `cron` with `delay_seconds`.",
        "- Use `every_seconds` only for repeating jobs, not one-time reminders.",
        "- Use `HEARTBEAT.md` for broad periodic checklists and recurring self-checks.",
        "- Keep `HEARTBEAT.md` concise. If it only contains headings or comments, heartbeat will skip it.",
        "- When creating or changing a scheduled job, include the exact schedule or trigger time in the result.",
        "- If the current turn is itself running from cron or heartbeat, complete the requested work and report the result. Do not mutate cron jobs unless explicitly asked and allowed.",
        "- Do not create or edit cron jobs from inside a scheduled task unless the user explicitly asks for that behavior.",
    ];
    lines.join("\n")
}

fn build_delivery_section(supports_image_input: bool) -> String {
    let mut lines: Vec<String> = vec![
        "## Delivery".into(),
        "- When the user asks you to send or attach a local image in the chat, use `send_image_to_user`.".into(),
        "- When the user asks you to send or attach a local file in the chat, use `send_file_to_user`.".into(),
        "- Never claim that an image, file, or attachment was sent unless the corresponding send tool succeeded in this turn.".into(),
        "- If the send tool fails, say that clearly instead of pretending the file or image was delivered.".into(),
    ];
    if supports_image_input {
        lines.insert(
            3,
            "- Use `view_image` only to inspect an image yourself. It does not send anything to the user."
                .into(),
        );
    }
    lines.join("\n")
}

fn load_bootstrap_files(workspace: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for file_name in BOOTSTRAP_FILES {
        let path = workspace.join(file_name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stripped = strip_utf8_bom(&content).trim();
        if stripped.is_empty() {
            continue;
        }
        parts.push(format!("## {file_name}\n\n{stripped}"));
    }
    parts.join("\n\n")
}

fn strip_utf8_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_includes_environment_and_core_rules() {
        let prompt = build_default_system_prompt(".", &SystemPromptOptions::default());
        assert!(prompt.contains("# EchoBot"));
        assert!(prompt.contains("## Core Rules"));
        assert!(prompt.contains("## Delivery"));
    }

    #[test]
    fn scheduling_and_memory_sections_appear_when_enabled() {
        let opts = SystemPromptOptions {
            enable_scheduling: true,
            enable_project_memory: true,
            ..SystemPromptOptions::default()
        };
        let prompt = build_default_system_prompt(".", &opts);
        assert!(prompt.contains("## Scheduling"));
        assert!(prompt.contains("## Memory"));
    }
}
