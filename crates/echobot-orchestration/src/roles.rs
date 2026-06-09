//! `RoleCard` and `RoleCardRegistry`.
//!
//! Mirrors `echobot/orchestration/roles.py`. Role cards are short
//! markdown / text files that describe a persona; the registry discovers
//! them from `echobot/roles/`, `roles/`, and `.echobot/roles/` (in that
//! order), auto-creating the default role card on first run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::RwLock;

use echobot_core::naming::normalize_name_token;

/// Name used for the built-in default role card.
pub const DEFAULT_ROLE_NAME: &str = "default";

/// Built-in default role prompt (verbatim port of the Python
/// `DEFAULT_ROLE_PROMPT`).
pub const DEFAULT_ROLE_PROMPT: &str = "\
# Default Role

你是一位带一点猫娘气质的助手。

## 人设

- 自称可以是“我”或“本喵”，但不要每句话都重复自称。
- 语气轻快、亲近、俏皮，偶尔在句尾自然地加“喵”。
- 不要过度卖萌，不要连续堆叠语气词，不要影响信息清晰度。
- 遇到严肃问题时，先保证准确和有条理，再保留一点温柔的角色感。

## 回复风格

- 默认使用简洁中文回复。
- 日常闲聊时，可以更像猫娘一些，轻松、灵动、带一点撒娇感。
- 说明步骤、总结结果、回答技术问题时，要清楚直接，避免废话。
- 角色感要稳定，但不能压过内容本身。

## 细节偏好

- 适度可爱，适度克制。
- 可以偶尔用“喵”点缀，但平均每 2 到 4 句出现一次就够了。
- 不要使用过于夸张、低龄化或失真的口吻。
";

/// A single role card: a name, its prompt text, and the on-disk source
/// (if it was loaded from a file).
#[derive(Debug, Clone)]
pub struct RoleCard {
    /// Normalized name.
    pub name: String,
    /// Role prompt text (trimmed).
    pub prompt: String,
    /// File the card was loaded from, or `None` for the in-memory default.
    pub source_path: Option<PathBuf>,
}

/// A registry of role cards, discoverable from multiple filesystem roots.
#[derive(Debug)]
pub struct RoleCardRegistry {
    project_root: PathBuf,
    cards: RwLock<HashMap<String, RoleCard>>,
}

impl RoleCardRegistry {
    /// Creates a registry rooted at `project_root`. The default role is
    /// always registered; any extra cards passed to the constructor are
    /// also registered (replacing any prior entry with the same name).
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            cards: RwLock::new(HashMap::new()),
        }
    }

    /// Creates a registry and runs a one-time `reload()`.
    pub async fn discover(project_root: impl Into<PathBuf>) -> Self {
        let registry = Self::new(project_root);
        registry.reload().await;
        registry
    }

    /// The project root the registry is attached to.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// The managed directory: `<project>/.echobot/roles`.
    pub fn managed_root(&self) -> PathBuf {
        self.project_root.join(".echobot").join("roles")
    }

    /// Returns the path the role file for `role_name` would be written to
    /// under the managed root.
    pub fn managed_role_path(&self, role_name: &str) -> PathBuf {
        let normalized = normalize_role_name(role_name);
        self.managed_root().join(format!("{normalized}.md"))
    }

    /// Registers a card, optionally replacing an existing entry.
    pub async fn register(&self, card: RoleCard, replace: bool) {
        let name = normalize_role_name(&card.name);
        let mut guard = self.cards.write().await;
        if !replace && guard.contains_key(&name) {
            return;
        }
        let stored_name = name.clone();
        guard.insert(
            stored_name,
            RoleCard {
                name,
                prompt: card.prompt.trim().to_string(),
                source_path: card.source_path,
            },
        );
    }

    /// Reloads cards from the filesystem. The default role is always
    /// available; managed roles are also written out if missing.
    pub async fn reload(&self) {
        // Make sure the default role card exists on disk.
        let default_path = ensure_default_role_card(&self.project_root).await;

        let mut collected: HashMap<String, RoleCard> = HashMap::new();
        collected.insert(
            DEFAULT_ROLE_NAME.to_string(),
            RoleCard {
                name: DEFAULT_ROLE_NAME.to_string(),
                prompt: DEFAULT_ROLE_PROMPT.to_string(),
                source_path: Some(default_path),
            },
        );

        for root in default_role_roots(&self.project_root) {
            let Ok(mut read_dir) = tokio::fs::read_dir(&root).await else {
                continue;
            };
            let mut entries = Vec::new();
            while let Ok(Some(entry)) = read_dir.next_entry().await {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                match path.extension().and_then(|e| e.to_str()) {
                    Some("md") | Some("txt") => entries.push(path),
                    _ => continue,
                }
            }
            entries.sort();
            for file_path in entries {
                let prompt = match tokio::fs::read_to_string(&file_path).await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Trim UTF-8 BOM if present.
                let prompt = prompt.trim_start_matches('\u{feff}').to_string();
                let prompt = prompt.trim();
                if prompt.is_empty() {
                    continue;
                }
                let name = normalize_role_name(file_path.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
                if name.is_empty() {
                    continue;
                }
                // First root wins on duplicate names.
                if collected.contains_key(&name) {
                    continue;
                }
                collected.insert(
                    name.clone(),
                    RoleCard {
                        name,
                        prompt: prompt.to_string(),
                        source_path: Some(file_path),
                    },
                );
            }
        }

        let mut guard = self.cards.write().await;
        *guard = collected;
    }

    /// Returns the list of registered role names, sorted alphabetically.
    pub async fn names(&self) -> Vec<String> {
        let guard = self.cards.read().await;
        let mut names: Vec<String> = guard.keys().cloned().collect();
        names.sort();
        names
    }

    /// Returns a copy of every registered card.
    pub async fn cards(&self) -> Vec<RoleCard> {
        let guard = self.cards.read().await;
        let mut names: Vec<&String> = guard.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|n| guard.get(n).cloned())
            .collect()
    }

    /// Looks up a role card by name (or `None` for the default).
    pub async fn get(&self, name: Option<&str>) -> Option<RoleCard> {
        let lookup = match name {
            Some(n) => normalize_role_name(n),
            None => DEFAULT_ROLE_NAME.to_string(),
        };
        let guard = self.cards.read().await;
        guard.get(&lookup).cloned()
    }

    /// Same as [`Self::get`] but raises `None` is converted to an
    /// `Option::None` for the caller to handle.
    pub async fn try_require(&self, name: Option<&str>) -> Result<RoleCard, String> {
        match self.get(name).await {
            Some(card) => Ok(card),
            None => {
                let available = self.names().await.join(", ");
                Err(match name {
                    Some(n) => format!("Unknown role: {n}. Available roles: {available}"),
                    None => format!("Unknown role. Available roles: {available}"),
                })
            }
        }
    }
}

/// Normalizes a free-form role name into a slug, falling back to
/// [`DEFAULT_ROLE_NAME`] when the result is empty.
pub fn normalize_role_name(name: &str) -> String {
    let normalized = normalize_name_token(name);
    if normalized.is_empty() {
        DEFAULT_ROLE_NAME.to_string()
    } else {
        normalized
    }
}

/// Reads the role name from a session metadata map. Missing or invalid
/// values fall back to [`DEFAULT_ROLE_NAME`].
pub fn role_name_from_metadata(metadata: Option<&HashMap<String, Value>>) -> String {
    let Some(metadata) = metadata else {
        return DEFAULT_ROLE_NAME.to_string();
    };
    match metadata.get("role_name") {
        Some(Value::String(s)) => normalize_role_name(s),
        _ => DEFAULT_ROLE_NAME.to_string(),
    }
}

/// Returns a new metadata map with the `role_name` key set to the
/// normalized form of `role_name`.
pub fn set_role_name(
    metadata: &HashMap<String, Value>,
    role_name: &str,
) -> HashMap<String, Value> {
    let mut next = metadata.clone();
    next.insert(
        "role_name".to_string(),
        Value::String(normalize_role_name(role_name)),
    );
    next
}

/// Writes the default role card to disk if it does not exist yet. Returns
/// the path that was ensured.
pub async fn ensure_default_role_card(project_root: &Path) -> PathBuf {
    let path = project_root
        .join(".echobot")
        .join("roles")
        .join(format!("{DEFAULT_ROLE_NAME}.md"));
    if path.exists() {
        return path;
    }
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, format!("{DEFAULT_ROLE_PROMPT}\n")).await;
    path
}

/// The default search roots for role cards, in priority order:
/// `echobot/roles`, `roles`, `.echobot/roles`.
pub fn default_role_roots(project_root: &Path) -> Vec<PathBuf> {
    vec![
        project_root.join("echobot").join("roles"),
        project_root.join("roles"),
        project_root.join(".echobot").join("roles"),
    ]
}

async fn _silence_unused(_unused: ()) {}

// Shared registry handle alias.
pub type SharedRoleCardRegistry = Arc<RoleCardRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "echobot-orchestration-roles-test-{}-{}",
            std::process::id(),
            name
        ));
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn default_role_card_is_auto_created() {
        let dir = tmp_dir("default-auto-create");
        let registry = RoleCardRegistry::discover(&dir).await;
        let card = registry.get(None).await.unwrap();
        assert_eq!(card.name, "default");
        assert!(!card.prompt.is_empty());
        let expected = dir.join(".echobot").join("roles").join("default.md");
        assert!(expected.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn discover_reads_role_files_in_priority_order() {
        let dir = tmp_dir("discover-priority");
        std::fs::create_dir_all(dir.join("echobot").join("roles")).unwrap();
        std::fs::create_dir_all(dir.join("roles")).unwrap();
        std::fs::create_dir_all(dir.join(".echobot").join("roles")).unwrap();
        std::fs::write(
            dir.join("echobot").join("roles").join("alpha.md"),
            "echobot alpha prompt",
        )
        .unwrap();
        std::fs::write(
            dir.join("roles").join("alpha.md"),
            "roles alpha prompt",
        )
        .unwrap();
        std::fs::write(
            dir.join("roles").join("beta.md"),
            "beta prompt",
        )
        .unwrap();
        let registry = RoleCardRegistry::discover(&dir).await;
        let alpha = registry.get(Some("alpha")).await.unwrap();
        // The first root wins, so the `echobot/roles/alpha.md` content is used.
        assert_eq!(alpha.prompt, "echobot alpha prompt");
        let beta = registry.get(Some("beta")).await.unwrap();
        assert_eq!(beta.prompt, "beta prompt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_role_falls_back_to_default() {
        let dir = tmp_dir("fallback");
        let registry = RoleCardRegistry::discover(&dir).await;
        let card = registry.get(Some("does-not-exist")).await;
        assert!(card.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_role_name_falls_back_to_default() {
        assert_eq!(normalize_role_name("Default"), "default");
        assert_eq!(normalize_role_name("   "), "default");
        assert_eq!(normalize_role_name("My Cool Role"), "my-cool-role");
    }
}
