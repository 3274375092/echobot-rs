//! Skill data model + runtime state.

use std::path::{Path, PathBuf};

/// Folders that contain bundled skill resources. Used by
/// [`Skill::resource_files`] and [`Skill::resolve_resource_path`].
pub const RESOURCE_FOLDERS: &[&str] = &["scripts", "references", "assets", "agents"];

/// A single skill parsed from a `SKILL.md` file.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name (from the frontmatter).
    pub name: String,
    /// Short description (from the frontmatter).
    pub description: String,
    /// Directory the skill lives in (the `SKILL.md`'s parent).
    pub directory: PathBuf,
    /// Path to the `SKILL.md` file.
    pub skill_file: PathBuf,
    /// Markdown body (frontmatter stripped).
    pub body: String,
    /// Raw frontmatter text (between the `---` markers).
    pub frontmatter: String,
}

impl Skill {
    /// Returns all bundled resource files, optionally restricted to a
    /// single folder (`"scripts"`, `"references"`, `"assets"`,
    /// `"agents"`).
    pub fn resource_files(&self, folder_name: Option<&str>) -> Result<Vec<String>, String> {
        let folder_names: Vec<&str> = match folder_name {
            Some(name) => {
                if !RESOURCE_FOLDERS.contains(&name) {
                    return Err(format!("Unknown skill resource folder: {name}"));
                }
                vec![name]
            }
            None => RESOURCE_FOLDERS.to_vec(),
        };
        let mut files: Vec<String> = Vec::new();
        for folder in folder_names {
            let dir = self.directory.join(folder);
            if !dir.exists() {
                continue;
            }
            collect_files(&dir, &self.directory, &mut files);
        }
        files.sort();
        Ok(files)
    }

    /// Returns a one-line summary per folder with at least one file.
    pub fn resource_summary(&self) -> Vec<String> {
        let mut summary: Vec<String> = Vec::new();
        for folder in RESOURCE_FOLDERS {
            let count = self.resource_files(Some(folder)).map(|v| v.len()).unwrap_or(0);
            if count == 0 {
                continue;
            }
            let label = if count == 1 { "file" } else { "files" };
            summary.push(format!("{folder}: {count} {label}"));
        }
        summary
    }

    /// Resolves a relative resource path against the skill directory,
    /// rejecting escapes and non-resource folders.
    pub fn resolve_resource_path(&self, relative_path: &str) -> Result<PathBuf, String> {
        let cleaned = relative_path.replace('\\', "/");
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            return Err("path is required".to_string());
        }
        let target = self.directory.join(trimmed);
        let target = target
            .canonicalize()
            .map_err(|e| format!("Path does not exist: {relative_path} ({e})"))?;
        let skill_root = self
            .directory
            .canonicalize()
            .map_err(|_e| format!("Skill root is invalid: {}", self.directory.display()))?;
        let relative = target
            .strip_prefix(&skill_root)
            .map_err(|_| format!("Path is outside the skill directory: {relative_path}"))?;
        if relative.as_os_str().is_empty() {
            return Err("path is required".to_string());
        }
        let first = relative
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .unwrap_or("");
        if !RESOURCE_FOLDERS.contains(&first) {
            return Err(format!(
                "path must be inside one of: {}",
                RESOURCE_FOLDERS.join(", ")
            ));
        }
        Ok(target)
    }

    /// Catalog entry for the `available_skills` prompt.
    pub fn to_catalog_entry(&self) -> String {
        format!("<skill name=\"{}\">\n{}\n</skill>", self.name, self.description)
    }

    /// Activation text used in the activation message and
    /// `activate_skill` tool response.
    pub fn to_activation_text(&self) -> String {
        let mut lines: Vec<String> = vec![
            format!("<active_skill name=\"{}\">", self.name),
            format!("Skill name: {}", self.name),
            format!("Skill directory: {}", self.directory.display()),
            "Skill instructions:".to_string(),
            self.body.trim().to_string(),
        ];
        let summary = self.resource_summary();
        if !summary.is_empty() {
            lines.push("Resource summary:".to_string());
            for s in summary {
                lines.push(format!("- {s}"));
            }
            lines.push("Bundled files are not loaded yet.".to_string());
            lines.push("Use list_skill_resources to inspect available files.".to_string());
            lines.push("Use read_skill_resource to load one specific file only when needed.".to_string());
        }
        lines.push("</active_skill>".to_string());
        lines.join("\n").trim().to_string()
    }
}

fn collect_files(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        } else if path.is_dir() {
            collect_files(&path, root, out);
        }
    }
}

/// Runtime state: the set of skills that are currently active in a
/// session.
#[derive(Debug, Default, Clone)]
pub struct SkillRuntimeState {
    active_skill_names: Vec<String>,
}

impl SkillRuntimeState {
    /// Creates a new state from a list of initially-active skill names.
    pub fn new(active_skill_names: Option<Vec<String>>) -> Self {
        Self {
            active_skill_names: active_skill_names.unwrap_or_default(),
        }
    }

    /// Activates a skill by name (idempotent).
    pub fn activate(&mut self, skill_name: &str) {
        if !self.active_skill_names.iter().any(|n| n == skill_name) {
            self.active_skill_names.push(skill_name.to_string());
        }
    }

    /// Returns true if the named skill is active.
    pub fn is_active(&self, skill_name: &str) -> bool {
        self.active_skill_names.iter().any(|n| n == skill_name)
    }

    /// Returns the active skill names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names = self.active_skill_names.clone();
        names.sort();
        names
    }
}
