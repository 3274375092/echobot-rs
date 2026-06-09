//! First-run setup for the EchoBot desktop app.
//!
//! On first launch we copy a bundled `.env.example` to the
//! workspace root so the user has a starting point for their
//! `.env`. We never overwrite an existing `.env`.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::info;

/// Copy `.env.example` from the bundle to `<workspace>/.env` if no
/// `.env` exists there yet. No-op when the destination already
/// exists.
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
