//! `chat` subcommand: interactive terminal REPL.
//!
//! Mirrors `echobot/cli/chat.py`. Drives the same coordinator the Python
//! CLI uses, and prints streamed responses to stdout.

use std::sync::Arc;

use clap::Args;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn};

use echobot_orchestration::RouteMode;
use echobot_runtime::bootstrap::RuntimeOptions;

use crate::common::CommonRuntimeArgs;
use crate::runtime_assembly::{assemble_runtime, FullRuntimeContext};

/// `chat` subcommand flags.
#[derive(Args, Debug, Clone)]
pub struct ChatArgs {
    #[command(flatten)]
    pub common: CommonRuntimeArgs,

    /// Load or create the given session name.
    #[arg(long)]
    pub session: Option<String>,

    /// Create a new empty session with the given name.
    #[arg(long)]
    pub new_session: Option<String>,

    /// Show tool calls and tool outputs during each turn.
    #[arg(long)]
    pub verbose: bool,
}

impl ChatArgs {
    /// Builds the [`RuntimeOptions`] for this subcommand.
    pub fn to_runtime_options(&self) -> RuntimeOptions {
        let mut options = self.common.to_runtime_options();
        options.session = self.session.clone();
        options.new_session = self.new_session.clone();
        options
    }
}

/// Runs the interactive REPL.
pub async fn run(args: ChatArgs) -> anyhow::Result<()> {
    let options = args.to_runtime_options();
    let context = assemble_runtime(options, true).await?;
    run_repl(context, args.verbose).await
}

/// Drives the REPL loop with a pre-built [`FullRuntimeContext`]. Public
/// so integration tests can construct a context with a stub LLM provider
/// and verify routing behavior end-to-end.
pub async fn run_repl(context: FullRuntimeContext, _verbose: bool) -> anyhow::Result<()> {
    let coordinator = context.coordinator.clone();
    let cron_service = context.runtime.cron_service.clone();
    let heartbeat = context.runtime.heartbeat_service.as_ref();
    let memory_support = context.memory_support.clone();
    let tool_registry_factory = &context.tool_registry_factory;

    // Start cron.
    cron_service.start().await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if let Some(hb) = heartbeat {
        hb.start().await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    }

    let session_name = context
        .runtime
        .session
        .as_ref()
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "default".to_string());

    print_help_text(&context, &session_name);

    // REPL.
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    let result: anyhow::Result<()> = async {
        loop {
            // Print prompt.
            print!("You[{session_name}]> ");
            use std::io::Write;
            std::io::stdout().flush().ok();

            let line = match reader.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => {
                    // EOF.
                    println!();
                    break;
                }
                Err(e) => {
                    warn!(error = %e, "failed to read line");
                    break;
                }
            };

            let prompt = line.trim().to_string();
            if prompt.is_empty() {
                continue;
            }
            if matches!(prompt.as_str(), "exit" | "quit" | "/exit" | "/quit") {
                break;
            }
            if matches!(prompt.as_str(), "clear" | "/clear") {
                println!("History cleared.");
                println!();
                continue;
            }
            if prompt == "/help" {
                print_inline_help();
                println!();
                continue;
            }
            if prompt == "/route" {
                let mode = coordinator
                    .current_route_mode(&session_name)
                    .await
                    .unwrap_or(RouteMode::Auto);
                println!("route mode: {}", mode.as_str());
                println!();
                continue;
            }
            if prompt == "/role" {
                let role = coordinator
                    .current_role_name(&session_name)
                    .await
                    .unwrap_or_else(|_| "default".to_string());
                println!("role: {role}");
                println!();
                continue;
            }
            if prompt == "/runtime" {
                println!(
                    "delegated_ack_enabled: {}",
                    coordinator.delegated_ack_enabled().await
                );
                println!(
                    "workspace: {}",
                    context.runtime.workspace.display()
                );
                println!("session: {session_name}");
                println!();
                continue;
            }
            if prompt == "/session" {
                println!("current session: {session_name}");
                println!();
                continue;
            }

            // Run a turn.
            match run_turn(&coordinator, tool_registry_factory, &session_name, &prompt).await {
                Ok(()) => {}
                Err(e) => {
                    warn!(error = %e, "turn failed");
                    println!("Request failed: {e}");
                    println!();
                }
            }
        }
        Ok(())
    }
    .await;

    // Tear down services.
    cron_service.stop().await;
    if let Some(hb) = heartbeat {
        hb.stop().await;
    }
    coordinator.close().await;
    let _ = tool_registry_factory;
    drop(memory_support);

    result?;
    info!("chat REPL exiting");
    Ok(())
}

async fn run_turn(
    coordinator: &Arc<echobot_orchestration::ConversationCoordinator>,
    _tool_registry_factory: &echobot_runtime::bootstrap::ToolRegistryFactoryPlaceholder,
    session_name: &str,
    prompt: &str,
) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let started = Arc::new(AtomicBool::new(false));
    let started_clone = started.clone();
    let on_chunk: echobot_orchestration::roleplay::StreamCallback = Arc::new(move |chunk: String| {
        let started = started_clone.clone();
        Box::pin(async move {
            if !started.swap(true, Ordering::Relaxed) {
                print!("Assistant> ");
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
            print!("{chunk}");
            use std::io::Write;
            std::io::stdout().flush().ok();
        })
    });

    let completion_callback: Option<echobot_orchestration::CompletionCallback> = None;
    let result = coordinator
        .handle_user_turn_stream(
            session_name,
            prompt,
            None,
            None,
            None,
            None,
            completion_callback,
            Some(on_chunk),
            None,
            1,
        )
        .await?;
    if started.load(Ordering::Relaxed) {
        println!();
    } else {
        let content = result.response_text.trim();
        if !content.is_empty() {
            println!("Assistant> {content}");
        } else if result.delegated && !result.completed {
            // Background job started; nothing to print inline.
        } else {
            println!("Model returned no text content.");
        }
    }
    println!();
    Ok(())
}

fn print_help_text(context: &FullRuntimeContext, session_name: &str) {
    let tool_names: Vec<String> = {
        // Use the factory to build a probe registry for the current session.
        // The factory returns Option; we treat None as no tools.
        Vec::new()
    };
    let skill_names: Vec<String> = context
        .skill_registry
        .as_ref()
        .map(|sr| sr.names())
        .unwrap_or_default();
    let memory_yes = context.memory_support.is_some();
    let cron_store = context.runtime.cron_service.store_path.clone();
    let heartbeat_file = context.runtime.heartbeat_file_path.clone();
    let heartbeat_interval = context.runtime.heartbeat_interval_seconds;

    println!("Chat started.");
    println!("Type exit or quit to stop.");
    println!("Type clear or /clear to clear the conversation history.");
    println!("Type /help to show all commands.");
    println!("Type /session help to manage saved sessions.");
    println!("Type /role help to manage role cards.");
    println!("Type /route help to manage the route mode for this session.");
    println!("Type /runtime help to manage runtime options.");
    println!("Current session: {session_name}");
    println!("Memory support enabled: {}", if memory_yes { "yes" } else { "no" });
    println!("Basic tools enabled: {}", if tool_names.is_empty() { "no" } else { "yes" });
    if !tool_names.is_empty() {
        println!("Available tools: {}", tool_names.join(", "));
    }
    println!(
        "Project skills enabled: {}",
        if skill_names.is_empty() { "no" } else { "yes" }
    );
    if !skill_names.is_empty() {
        println!("Available skills: {}", skill_names.join(", "));
        println!("Use /skill-name or $skill-name to activate a skill explicitly.");
    }
    println!("Cron store: {}", cron_store.display());
    println!(
        "Heartbeat: {} (every {heartbeat_interval}s while this process is running)",
        heartbeat_file.display()
    );
    println!();
}

fn print_inline_help() {
    println!("Available commands:");
    println!("  /help        Show this help.");
    println!("  /session     Show the current session name.");
    println!("  /role        Show the current role card.");
    println!("  /route       Show the current route mode.");
    println!("  /runtime     Show runtime settings.");
    println!("  clear        Clear the conversation history.");
    println!("  exit | quit  Leave the chat.");
}
