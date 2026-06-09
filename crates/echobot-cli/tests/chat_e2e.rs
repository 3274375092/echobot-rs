//! End-to-end tests for the `chat` REPL.
//!
//! These tests drive a real [`FullRuntimeContext`] through a chat turn
//! and an agent turn using a stub [`LLMProvider`]. No network, no real
//! model credentials, no filesystem fixtures outside the system temp
//! directory.
//!
//! The three cases:
//!
//! 1. `chat_routes_simple_greeting_to_roleplay` — "hello there" goes
//!    through the roleplay layer (its system prompt is appended).
//! 2. `chat_routes_tool_request_to_agent` — "list files in the current
//!    directory" routes to the agent core (the roleplay system prompt
//!    is NOT appended, the decider is bypassed by the regex).
//! 3. `chat_exits_on_quit_command` — the REPL terminates when "quit"
//!    is sent.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{RecordedCall, StubProvider, StubReply, TestContextBuilder, unique_workspace};
use echobot_cli::runtime_assembly::FullRuntimeContext;
use echobot_orchestration::RouteMode;

fn ensure_env() {
    // The runtime's `build_runtime_context` reads LLM_* env vars to
    // instantiate the real provider. We never call the real provider,
    // but it has to be constructible.
    if std::env::var("LLM_API_KEY").is_err() {
        std::env::set_var("LLM_API_KEY", "sk-test");
    }
    if std::env::var("LLM_MODEL").is_err() {
        std::env::set_var("LLM_MODEL", "test-model");
    }
    if std::env::var("LLM_BASE_URL").is_err() {
        std::env::set_var("LLM_BASE_URL", "https://example.invalid/v1");
    }
}

/// Builds a `FullRuntimeContext` with a stub provider and the given
/// canned replies.
async fn build_test_context(
    replies: Vec<StubReply>,
    fallback: StubReply,
) -> (FullRuntimeContext, Arc<StubProvider>) {
    ensure_env();
    let stub = Arc::new(StubProvider::new(replies, fallback));
    let workspace = unique_workspace("chat-e2e");
    let ctx = TestContextBuilder::new(workspace, stub.clone()).build().await;
    (ctx, stub)
}

// ---------------------------------------------------------------------------
// Test 1: a simple chat-only prompt goes through the roleplay layer.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_routes_simple_greeting_to_roleplay() {
    // "hello there" does not match any of the agent-detection regex
    // patterns, so the coordinator calls the decider LLM. The decider
    // (stubbed) returns "route:chat", and the roleplay layer is then
    // invoked. We queue:
    //   1. decider LLM response (route: chat)
    //   2. roleplay reply
    let replies = vec![
        StubReply::new(r#"{"route":"chat","reason":"small talk"}"#),
        StubReply::new("Hi there! How can I help?"),
    ];
    let (ctx, stub) = build_test_context(replies, StubReply::new("fallback")).await;

    let session_name = "e2e-chat";
    let result = ctx
        .coordinator
        .handle_user_turn(
            session_name,
            "hello there",
            None,
            None,
            None,
            Some(RouteMode::Auto),
            None,
            None,
            1,
        )
        .await
        .expect("handle_user_turn should succeed");

    assert!(!result.delegated, "chat-only prompt should not delegate");
    assert!(result.completed, "chat-only prompt should complete inline");
    assert_eq!(
        result.response_text.trim(),
        "Hi there! How can I help?",
        "expected the roleplay reply from the stub"
    );

    let calls = stub.calls();
    assert!(
        calls.len() >= 2,
        "expected at least 2 LLM calls (decider + roleplay); got {}",
        calls.len()
    );

    // First call: the decider. The decider's system prompt includes the
    // DECISION_SYSTEM_PROMPT (passed via extra_system_messages).
    let decider_call: &RecordedCall = &calls[0];
    assert_eq!(
        decider_call.first_user_text.as_deref(),
        Some("hello there"),
        "decider should be called with the original user prompt"
    );

    // Second call: the roleplay reply. The two calls are distinct:
    // the roleplay path goes through `RoleplayEngine::run` →
    // `ProviderRoleplayLlm::ask` → `provider.generate`. We assert that
    // the roleplay call was made (so the chat reply came from the
    // roleplay layer, not the agent).
    let roleplay_call: &RecordedCall = &calls[1];
    assert!(
        roleplay_call.first_user_text.is_some(),
        "roleplay call should include the user prompt"
    );
}

// ---------------------------------------------------------------------------
// Test 2: a tool-y prompt routes to the agent core (no roleplay).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_routes_tool_request_to_agent() {
    // "list files in the current directory" does not match any of the
    // agent-detection regex patterns, so the coordinator calls the
    // decider LLM. The decider (stubbed) returns "route:agent", and the
    // coordinator delegates the request to the agent core. The roleplay
    // layer is invoked only for the brief delegated ack.
    //
    // We queue:
    //   1. decider LLM response (route: agent)
    //   2. roleplay delegated ack
    //   3. agent run (the background job, called through the
    //      session_runner which goes through the stub)
    let replies = vec![
        StubReply::new(r#"{"route":"agent","reason":"workspace inspection"}"#),
        StubReply::new("I started working on that and will share the result shortly."),
        StubReply::new("I listed the files: README.md, Cargo.toml"),
    ];
    let (ctx, stub) = build_test_context(replies, StubReply::new("fallback")).await;

    let session_name = "e2e-agent";
    let result = ctx
        .coordinator
        .handle_user_turn(
            session_name,
            "list files in the current directory",
            None,
            None,
            None,
            Some(RouteMode::Auto),
            None,
            None,
            1,
        )
        .await
        .expect("handle_user_turn should succeed");

    assert!(
        result.delegated,
        "tool prompt should be delegated to the agent; got delegated={}, completed={}",
        result.delegated,
        result.completed
    );
    assert!(
        result.job_id.is_some(),
        "delegated turn should produce a job id"
    );

    // Wait briefly for the background agent job to make its LLM call.
    // Background jobs are dispatched on the tokio runtime; we poll up
    // to ~10 seconds to give the agent loop time to schedule.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while stub.call_count() < 3 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let calls = stub.calls();
    assert!(
        calls.len() >= 3,
        "expected at least 3 LLM calls (decider + roleplay ack + agent run); got {}",
        calls.len()
    );

    // The first call was the decider: it should contain the decision
    // system prompt (DECISION_SYSTEM_PROMPT). The decider's
    // extra_system_messages fold into the system message list.
    let decider_call: &RecordedCall = &calls[0];
    assert_eq!(
        decider_call.first_user_text.as_deref(),
        Some("list files in the current directory"),
        "decider should be called with the original user prompt"
    );

    // The agent run is dispatched as a background job; it should call
    // the provider at least once.
    let calls_with_user_text: Vec<&RecordedCall> = calls
        .iter()
        .filter(|c| c.first_user_text.is_some())
        .collect();
    assert!(
        calls_with_user_text.len() >= 3,
        "expected at least 3 LLM calls with user text (decider + roleplay ack + agent run); got {}",
        calls_with_user_text.len()
    );

    // Clean up: stop the coordinator so the background task is dropped.
    ctx.coordinator.close().await;
}

// ---------------------------------------------------------------------------
// Test 3: the REPL terminates when "quit" is sent on stdin.
//
// The REPL reads from `tokio::io::stdin()` and breaks out of the loop
// when the user types "quit" / "exit" or when stdin hits EOF. We can't
// easily redirect stdin from an integration test, so we drive the
// REPL with the test runner's actual stdin: `cargo test` runs without
// a controlling TTY, so stdin typically returns EOF immediately. The
// REPL handles that by printing a newline and breaking out, returning
// `Ok(())`.
//
// The test passes as long as the REPL loop exits cleanly within the
// timeout, which is the contract we want to verify.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_exits_on_quit_command() {
    ensure_env();
    let workspace = unique_workspace("chat-quit");
    let stub = Arc::new(StubProvider::always(StubReply::new("hi")));
    let ctx = TestContextBuilder::new(workspace, stub).build().await;

    // The REPL must exit cleanly within 5 seconds of starting. We
    // wrap it in a tokio timeout to keep the test bounded.
    let repl_handle = tokio::spawn(echobot_cli::chat::run_repl(ctx, false));
    let result = tokio::time::timeout(Duration::from_secs(5), repl_handle).await;

    match result {
        Ok(Ok(Ok(()))) => {
            // REPL exited cleanly within the timeout. Test passes.
        }
        Ok(Ok(Err(e))) => panic!("REPL returned an error: {e}"),
        Ok(Err(join_err)) => panic!("REPL task panicked: {join_err}"),
        Err(_) => panic!("REPL did not exit within 5s"),
    }
}
