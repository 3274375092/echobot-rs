//! `echobot-cli` is the unified entrypoint for the Rust port. It exposes the
//! three subcommands (`chat`, `app`, `gateway`) that mirror the Python CLI.
//! Each subcommand is a stub during scaffold phase and will be filled in as
//! the runtime and orchestration crates are ported.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "echobot", version, about = "EchoBot Rust port")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Interactive terminal chat.
    Chat,
    /// FastAPI-style web console + channels.
    App,
    /// Multi-channel gateway only (QQ / Telegram).
    Gateway,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Chat => {
            println!("echobot chat: not yet implemented");
            Ok(())
        }
        Command::App => {
            println!("echobot app: not yet implemented");
            Ok(())
        }
        Command::Gateway => {
            println!("echobot gateway: not yet implemented");
            Ok(())
        }
    }
}
