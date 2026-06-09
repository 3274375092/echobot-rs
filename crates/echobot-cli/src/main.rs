//! `echobot-cli` is the unified entrypoint for the Rust port. It exposes
//! the three subcommands (`chat`, `app`, `gateway`) that mirror the
//! Python CLI.
//!
//! - `chat` is fully functional end-to-end (REPL + cron + heartbeat).
//! - `app` and `gateway` are phase 1 stubs that print a message and exit
//!   cleanly (full HTTP server / QQ / Telegram integration land in
//!   phase 2 / v2).
//!
//! The implementation lives in the `echobot_cli` library crate (see
//! `src/lib.rs`); this binary is a thin wrapper that maps the parsed
//! subcommand to the right module function.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "echobot", version, about = "EchoBot Rust port")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Interactive terminal chat.
    Chat(echobot_cli::chat::ChatArgs),
    /// FastAPI-style web console + channels.
    App(echobot_cli::app::AppArgs),
    /// Multi-channel gateway only (QQ / Telegram).
    Gateway(echobot_cli::gateway::GatewayArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls aws-lc-rs crypto provider");
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Chat(args) => echobot_cli::chat::run(args).await,
        Command::App(args) => echobot_cli::app::run(args).await,
        Command::Gateway(args) => echobot_cli::gateway::run(args).await,
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("echobot=info,warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
