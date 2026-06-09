//! `gateway` subcommand: multi-channel gateway (QQ / Telegram).
//!
//! Phase 1 stub. QQ / Telegram integration is out of scope for v1.

use clap::Args;

use crate::common::CommonRuntimeArgs;

/// `gateway` subcommand flags.
#[derive(Args, Debug, Clone)]
pub struct GatewayArgs {
    #[command(flatten)]
    pub common: CommonRuntimeArgs,

    /// Path to the channel config file. Default: `.echobot/channels.json`.
    #[arg(long, default_value = ".echobot/channels.json")]
    pub channel_config: String,
}

/// Runs the gateway subcommand. Phase 1 prints a stub message and exits
/// cleanly.
pub async fn run(args: GatewayArgs) -> anyhow::Result<()> {
    let _ = args;
    println!("echobot gateway: QQ / Telegram are out of scope for v1.");
    println!("(subcommand accepted the flags; exiting cleanly)");
    Ok(())
}
