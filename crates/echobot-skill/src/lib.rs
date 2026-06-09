//! `echobot-skill` parses project skills (the `SKILL.md` + `references/`
//! format) and maintains a registry that resolves skill lookups across
//! project, managed, and built-in roots, with project skills taking
//! precedence.
//!
//! ## Layout
//!
//! * [`models`]   — `Skill`, `SkillRuntimeState`, and resource-folder
//!   constants.
//! * [`parsing`]  — `SKILL.md` frontmatter parser + active-skill
//!   extraction from message history.
//! * [`registry`] — `SkillRegistry` (discover + activate + tools).
//! * [`tools`]    — `ActivateSkillTool`, `ListSkillResourcesTool`,
//!   `ReadSkillResourceTool`.

pub mod models;
pub mod parsing;
pub mod registry;
pub mod tools;

pub use models::{Skill, SkillRuntimeState, RESOURCE_FOLDERS};
pub use parsing::{
    extract_active_skill_names_from_history, extract_explicit_skill_tokens, parse_skill_file,
};
pub use registry::{SkillRegistry, DEFAULT_SEARCH_ROOT_NAMES};
pub use tools::{ActivateSkillTool, ListSkillResourcesTool, ReadSkillResourceTool};
