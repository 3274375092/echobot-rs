//! `app` subcommand: FastAPI-style web console + channels.
//!
//! Phase 1 stub. The full HTTP server lands in phase 2.

use clap::Args;

use crate::common::CommonRuntimeArgs;

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

/// Runs the app subcommand. Phase 1 prints a stub message and exits cleanly.
pub async fn run(args: AppArgs) -> anyhow::Result<()> {
    let _ = args;
    println!("echobot app: HTTP server / channel router is a phase 2 deliverable.");
    println!("(subcommand accepted the flags; exiting cleanly)");
    Ok(())
}
