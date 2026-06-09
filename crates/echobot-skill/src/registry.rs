//! `SkillRegistry` — discovers skills across project + managed +
//! built-in roots, with project skills taking precedence.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use echobot_core::models::LLMMessage;
use echobot_tools::BaseTool;

use crate::models::{Skill, SkillRuntimeState};
use crate::parsing::{
    extract_active_skill_names_from_history, extract_explicit_skill_tokens, parse_skill_file,
};
use crate::tools::{ActivateSkillTool, ListSkillResourcesTool, ReadSkillResourceTool};

/// Default search-root folder names. The runtime layer extends this
/// list with extra roots; the agent sees them in `discover`.
pub const DEFAULT_SEARCH_ROOT_NAMES: &[&str] = &[
    "skills",
    ".echobot/skills",
    ".agents/skills",
    "echobot/skills",
];

/// Registry of skills discovered from disk.
#[derive(Default)]
pub struct SkillRegistry {
    skills: std::collections::HashMap<String, Skill>,
    /// The roots that were searched (in priority order).
    pub search_roots: Vec<PathBuf>,
    /// Warnings collected while discovering skills.
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for SkillRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillRegistry")
            .field("names", &self.names())
            .field("search_roots", &self.search_roots)
            .field("warnings", &self.warnings)
            .finish()
    }
}

impl SkillRegistry {
    /// Creates a registry pre-populated with `skills`.
    pub fn new(skills: Vec<Skill>, search_roots: Vec<PathBuf>, warnings: Vec<String>) -> Self {
        let mut registry = Self {
            skills: std::collections::HashMap::new(),
            search_roots,
            warnings,
        };
        for skill in skills {
            registry.register(skill);
        }
        registry
    }

    /// Discovers skills by walking `project_root` and the standard
    /// search roots.
    pub fn discover(
        project_root: impl AsRef<Path>,
        client_name: &str,
        extra_roots: Option<Vec<PathBuf>>,
        include_user_roots: bool,
    ) -> SkillRegistry {
        let project_root = project_root.as_ref();
        let project_root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.to_path_buf());
        let mut search_roots: Vec<PathBuf> = build_default_search_roots(
            &project_root,
            client_name,
            include_user_roots,
        );
        if let Some(extra) = extra_roots {
            for root in extra.into_iter().rev() {
                let canonical = root.canonicalize().unwrap_or(root);
                search_roots.insert(0, canonical);
            }
        }

        let mut skills: Vec<Skill> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut seen_names: std::collections::HashMap<String, PathBuf> =
            std::collections::HashMap::new();

        for root in &search_roots {
            if !root.exists() {
                continue;
            }
            for skill_file in walk_files(root) {
                if skill_file.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
                    continue;
                }
                match parse_skill_file(&skill_file) {
                    Ok(skill) => {
                        if let Some(previous) = seen_names.get(&skill.name) {
                            warnings.push(format!(
                                "Duplicate skill ignored: {} from {} (already loaded from {})",
                                skill.name,
                                skill_file.display(),
                                previous.display()
                            ));
                            continue;
                        }
                        seen_names.insert(skill.name.clone(), skill_file.clone());
                        skills.push(skill);
                    }
                    Err(e) => {
                        warnings.push(format!("{}: {}", skill_file.display(), e));
                    }
                }
            }
        }

        skills.sort_by(|a, b| a.name.cmp(&b.name));
        SkillRegistry::new(skills, search_roots, warnings)
    }

    /// Registers a skill explicitly (used by tests).
    pub fn register(&mut self, skill: Skill) {
        self.skills.insert(skill.name.clone(), skill);
    }

    /// Returns a reference to the named skill (or `None`).
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Looks up a skill by name, returning a friendly error if it
    /// is missing.
    pub fn require_skill(&self, raw_name: &str) -> Result<&Skill, String> {
        let name = raw_name.trim();
        if name.is_empty() {
            return Err("name is required".to_string());
        }
        self.skills
            .get(name)
            .ok_or_else(|| format!("Unknown skill: {name}"))
    }

    /// Looks up a skill and ensures it is active in `runtime_state`.
    pub fn require_active_skill<'a>(
        &'a self,
        raw_name: &str,
        runtime_state: &SkillRuntimeState,
    ) -> Result<&'a Skill, String> {
        let skill = self.require_skill(raw_name)?;
        if !runtime_state.is_active(&skill.name) {
            return Err(format!(
                "Skill is not active yet: {}. Activate it first with activate_skill.",
                skill.name
            ));
        }
        Ok(skill)
    }

    /// Returns the sorted list of registered skill names.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.skills.keys().cloned().collect();
        names.sort();
        names
    }

    /// Returns true if the registry has no skills.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Returns the count of registered skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Returns the activate_skill tool (boxed as `BaseTool`), or
    /// `None` if the registry has no skills.
    pub fn create_activate_tool(
        self: Arc<Self>,
        active_skill_names: Option<Vec<String>>,
    ) -> Option<Box<dyn BaseTool>> {
        if self.skills.is_empty() {
            return None;
        }
        let runtime_state = SkillRuntimeState::new(active_skill_names);
        Some(Box::new(ActivateSkillTool::new(self, runtime_state)))
    }

    /// Returns the standard skill tools: activate, list, read.
    pub fn create_tools(
        self: Arc<Self>,
        active_skill_names: Option<Vec<String>>,
    ) -> Vec<Box<dyn BaseTool>> {
        if self.skills.is_empty() {
            return Vec::new();
        }
        let runtime_state = SkillRuntimeState::new(active_skill_names);
        vec![
            Box::new(ActivateSkillTool::new(self.clone(), runtime_state.clone())),
            Box::new(ListSkillResourcesTool::new(self.clone(), runtime_state.clone())),
            Box::new(ReadSkillResourceTool::new(self, runtime_state)),
        ]
    }

    /// Builds the `available_skills` catalog prompt.
    pub fn build_catalog_prompt(&self, active_skill_names: Option<&[String]>) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut lines: Vec<String> = vec![
            "You can use project skills for specialized workflows.".to_string(),
            "Only activate a skill when the task clearly benefits from its instructions.".to_string(),
            "If the user explicitly mentions /skill-name or $skill-name, treat that skill as already active.".to_string(),
            "If a skill is already active in the context, do not activate it again.".to_string(),
        ];
        let current = active_skill_names.unwrap_or(&[]).to_vec();
        if !current.is_empty() {
            let mut sorted = current.clone();
            sorted.sort();
            lines.push(format!("Already active skills: {}", sorted.join(", ")));
        }
        lines.extend([
            "Available skills:".to_string(),
            "<available_skills>".to_string(),
        ]);
        for name in self.names() {
            if let Some(skill) = self.skills.get(&name) {
                lines.push(skill.to_catalog_entry());
            }
        }
        lines.push("</available_skills>".to_string());
        lines.push("Use activate_skill to load a skill's main instructions.".to_string());
        lines.push("Use list_skill_resources only after a skill is active.".to_string());
        lines.push(
            "Use read_skill_resource to load one bundled file only when needed.".to_string(),
        );
        lines.join("\n")
    }

    /// Returns the activation message for a single skill.
    pub fn build_activation_message(&self, skill_name: &str) -> Result<String, String> {
        let skill = self.require_skill(skill_name)?;
        Ok(format!(
            "The user explicitly activated this skill.\n{}",
            skill.to_activation_text()
        ))
    }

    /// Returns activation messages for any explicit skill tokens
    /// (`/foo` or `$foo`) in `user_input`.
    pub fn build_explicit_activation_messages(
        &self,
        user_input: &str,
        active_skill_names: Option<&[String]>,
    ) -> Vec<String> {
        let mut messages: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = active_skill_names
            .unwrap_or(&[])
            .iter()
            .cloned()
            .collect();
        for name in self.explicit_skill_names(user_input) {
            if seen.contains(&name) {
                continue;
            }
            if let Ok(message) = self.build_activation_message(&name) {
                messages.push(message);
                seen.insert(name);
            }
        }
        messages
    }

    /// Returns the active skill names found in `history`.
    pub fn active_skill_names_from_history(&self, history: &[LLMMessage]) -> Vec<String> {
        let available: Vec<String> = self.names();
        extract_active_skill_names_from_history(history, &available)
    }

    /// Returns the explicit skill names mentioned in `text` (with
    /// unknown tokens filtered out).
    pub fn explicit_skill_names(&self, text: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for token in extract_explicit_skill_tokens(text) {
            if self.skills.contains_key(&token) && !found.iter().any(|n| n == &token) {
                found.push(token);
            }
        }
        found
    }
}

fn build_default_search_roots(
    project_root: &Path,
    client_name: &str,
    include_user_roots: bool,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![
        project_root.join("skills"),
        project_root.join(format!(".{client_name}")).join("skills"),
        project_root.join(".agents").join("skills"),
        project_root.join("echobot").join("skills"),
    ];
    if include_user_roots {
        if let Some(home) = home_dir() {
            roots.push(home.join(format!(".{client_name}")).join("skills"));
            roots.push(home.join(".agents").join("skills"));
        }
    }
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
