//! Shared CLI flags.
//!
//! Mirrors `echobot/cli/common.py` — every subcommand picks up the
//! runtime flags from here. The flags map 1:1 to the Python versions.

use std::path::PathBuf;

use clap::Args;

use echobot_runtime::bootstrap::RuntimeOptions;

/// The runtime flags shared by every subcommand. Use `#[command(flatten)]`
/// from the clap derive API to inline them.
#[derive(Args, Debug, Clone)]
pub struct CommonRuntimeArgs {
    /// Path to the environment file. Default: `.env`.
    #[arg(long, default_value = ".env")]
    pub env_file: String,

    /// Workspace root. Default: current directory.
    #[arg(long)]
    pub workspace: Option<String>,

    /// Optional model temperature.
    #[arg(long)]
    pub temperature: Option<f32>,

    /// Optional max output tokens.
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Disable the built-in basic tools.
    #[arg(long)]
    pub no_tools: bool,

    /// Disable discovered project skills.
    #[arg(long)]
    pub no_skills: bool,

    /// Disable long-term memory support.
    #[arg(long)]
    pub no_memory: bool,

    /// Disable heartbeat checks for this run.
    #[arg(long)]
    pub no_heartbeat: bool,

    /// Override heartbeat interval in seconds for this run.
    #[arg(long)]
    pub heartbeat_interval: Option<u64>,
}

impl CommonRuntimeArgs {
    /// Builds a [`RuntimeOptions`] from the parsed args.
    pub fn to_runtime_options(&self) -> RuntimeOptions {
        let workspace = build_workspace_path(self.workspace.as_deref());
        let env_file_path = resolve_runtime_path(&self.env_file, workspace.as_ref());
        RuntimeOptions {
            env_file: env_file_path.to_string_lossy().to_string(),
            workspace,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            delegated_ack_enabled: None,
            no_tools: self.no_tools,
            no_skills: self.no_skills,
            no_memory: self.no_memory,
            no_heartbeat: self.no_heartbeat,
            heartbeat_interval: self.heartbeat_interval,
            session: None,
            new_session: None,
        }
    }
}

/// Resolves a free-form workspace path string.
pub fn build_workspace_path(raw: Option<&str>) -> Option<PathBuf> {
    raw.map(|s| {
        let p = PathBuf::from(s);
        match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p,
        }
    })
}

/// Resolves a runtime path (env-file, channel-config) against the workspace
/// when relative.
pub fn resolve_runtime_path(path: &str, workspace: Option<&PathBuf>) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() || workspace.is_none() {
        return p;
    }
    workspace.unwrap().join(p)
}
