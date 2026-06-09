//! First-run setup for the EchoBot desktop app.
//!
//! On first launch we copy a bundled `.env.example` to the
//! workspace root so the user has a starting point for their
//! `.env`. We never overwrite an existing `.env`.
//!
//! The desktop binary is usually launched by double-clicking the
//! `.exe` in File Explorer, which means the working directory is
//! **not** the repo root. To handle that gracefully we walk up
//! from the executable looking for the first existing directory
//! that contains either `.env` or `.env.example` (so the user can
//! place the workspace wherever they like).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

/// Resolve the workspace root for the desktop app. Priority:
///
/// 1. `ECHOBOT_WORKSPACE` environment variable, if set.
/// 2. The current working directory (what `cargo run` gives us).
/// 3. The directory containing the running executable.
/// 4. The grandparent of the executable (e.g. when the exe lives
///    in `target/release/`, walk up to the repo root).
///
/// Returns the first directory that contains either `.env` or
/// `.env.example`; falls back to the current working directory.
pub fn resolve_workspace() -> PathBuf {
    if let Ok(custom) = std::env::var("ECHOBOT_WORKSPACE") {
        let p = PathBuf::from(custom);
        if p.is_dir() {
            return p;
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if has_env_marker(&cwd) {
        return cwd;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if has_env_marker(dir) {
                return dir.to_path_buf();
            }
            // Walk up to handle `target/release/echobot-desktop.exe`
            // launched from the repo root.
            for ancestor in dir.ancestors() {
                if has_env_marker(ancestor) {
                    return ancestor.to_path_buf();
                }
            }
            return dir.to_path_buf();
        }
    }
    cwd
}

fn has_env_marker(dir: &Path) -> bool {
    dir.join(".env").is_file() || dir.join(".env.example").is_file()
}

/// Copy `.env.example` from the bundle to `<workspace>/.env` if no
/// `.env` exists there yet. No-op when the destination already
/// exists. Also nudges the user to edit it before re-running.
pub async fn ensure_env_file(workspace: &Path) -> Result<()> {
    let env_path = workspace.join(".env");
    if env_path.exists() {
        return Ok(());
    }
    let example = bundled_env_example();
    tokio::fs::write(&env_path, example.as_bytes())
        .await
        .with_context(|| format!("failed to write {} from bundled example", env_path.display()))?;
    info!(path = %env_path.display(), "first-run: copied .env.example to .env (edit it to add your LLM_API_KEY)");
    Ok(())
}

/// The bundled `.env.example` template. Mirrors the Python
/// project's `.env.example`; we keep the same env var names so
/// users moving between the two implementations don't have to
/// relearn anything.
fn bundled_env_example() -> &'static str {
    // `crates/echobot-desktop/src/first_run.rs` -> repo root
    // -> `EchoBot/.env.example`
    include_str!("../../../../EchoBot/.env.example")
}
