//! `app` subcommand — the EchoBot HTTP server (axum).
//!
//! Wires the [`echobot_app`] crate on top of the shared
//! [`FullRuntimeContext`], starts the axum server on `--host:--port`, and
//! serves both the JSON API under `/api/*` and the embedded web console
//! under `/web/*`.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Args;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{error, info, warn};

use echobot_app::{create_app, runtime::AppRuntime};

use crate::common::CommonRuntimeArgs;
use crate::runtime_assembly::assemble_runtime;

/// `app` subcommand flags.
#[derive(Args, Debug, Clone)]
pub struct AppArgs {
    #[command(flatten)]
    pub common: CommonRuntimeArgs,

    /// Path to the channel config file. Default: `.echobot/channels.json`.
    #[arg(long, default_value = ".echobot/channels.json")]
    pub channel_config: String,

    /// Bind host for the API server. Default: `127.0.0.1`.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Bind port for the API server. Default: `8000`.
    #[arg(long, default_value_t = 8000)]
    pub port: u16,
}

/// Runs the `app` subcommand. Builds the shared runtime, wraps it in
/// an [`AppRuntime`], serves the axum router, and shuts down gracefully
/// on Ctrl+C.
pub async fn run(args: AppArgs) -> anyhow::Result<()> {
    let _ = args.channel_config; // accepted for flag-surface stability; v1 has no channel manager.

    // 1. Assemble the shared runtime (provider, sessions, scheduling,
    //    coordinator, tool/skill registries, …).
    let options = args.common.to_runtime_options();
    let full = assemble_runtime(options, true).await?;
    let workspace = full.runtime.workspace.clone();

    // 2. Build TTS and ASR services from environment.
    let tts_service = Arc::new(echobot_tts::factory::build_default_tts_service(Some(&workspace)));
    let asr_service = Arc::new(echobot_asr::factory::build_default_asr_service(&workspace));

    // 3. Build the AppRuntime. The TTS/ASR services are best-effort:
    //    if either fails to construct at the provider level, AppRuntime
    //    will still start (its constructors return Option). Move
    //    `full.runtime` into AppRuntime (it's an owned `RuntimeContext`,
    //    not cloneable); clone the Arc-shared coordinator / role
    //    registry so the rest of the function can keep them.
    let coordinator = full.coordinator.clone();
    let role_registry = full.role_registry.clone();
    let mut app_runtime = AppRuntime::new(
        full.runtime,
        Some(coordinator),
        Some(role_registry),
        Some(tts_service),
        Some(asr_service),
    );
    app_runtime.start().await;

    // 4. Build the axum router and bind to host:port.
    let runtime_arc = Arc::new(app_runtime);

    let bind_addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --host/--port: {e}"))?;

    let listener = TcpListener::bind(bind_addr).await.map_err(|e| {
        anyhow::anyhow!("failed to bind {bind_addr}: {e} (is another process using the port?)")
    })?;

    let local_addr = listener.local_addr().unwrap_or(bind_addr);

    info!(addr = %local_addr, "EchoBot HTTP server starting");
    println!("EchoBot API listening on http://{local_addr}/web");
    println!("(API: http://{local_addr}/api/health · Ctrl+C to stop)");

    // 5. Serve with graceful shutdown on Ctrl+C.
    let shutdown = async {
        match signal::ctrl_c().await {
            Ok(()) => info!("Ctrl+C received, shutting down"),
            Err(e) => error!(error = %e, "failed to install Ctrl+C handler"),
        }
    };

    let router = create_app(runtime_arc);
    if let Err(e) = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
    {
        warn!(error = %e, "axum::serve returned an error");
    }

    info!("EchoBot HTTP server stopped");
    Ok(())
}
