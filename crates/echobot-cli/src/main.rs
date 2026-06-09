//! `echobot-cli` is the unified entrypoint for the Rust port. It exposes
//! the three subcommands (`chat`, `app`, `gateway`) that mirror the
//! Python CLI.
//!
//! - `chat` is fully functional end-to-end (REPL + cron + heartbeat).
//! - `app` and `gateway` are phase 1 stubs that print a message and exit
//!   cleanly (full HTTP server / QQ / Telegram integration land in
//!   phase 2 / v2).

mod app;
mod bridge;
mod chat;
mod common;
mod gateway;
mod runtime_assembly;

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
    Chat(chat::ChatArgs),
    /// FastAPI-style web console + channels.
    App(app::AppArgs),
    /// Multi-channel gateway only (QQ / Telegram).
    Gateway(gateway::GatewayArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Command::Chat(args) => chat::run(args).await,
        Command::App(args) => app::run(args).await,
        Command::Gateway(args) => gateway::run(args).await,
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
