# Rust Port Migration Plan

This document tracks the Rust port of EchoBot. The Rust port aims to be a
1:1 port of the Python implementation (`echobot/`) with the same
features, CLI surface, and provider / orchestration semantics, but
implemented in idiomatic Rust on top of `tokio`, `clap`, and `reqwest`.

## Crate Status

| Work Stream | Crate | Status | Notes |
|---|---|---|---|
| Core types | `echobot-core` | done | Attachments, config, error, images, models, naming, turn_inputs. |
| Providers | `echobot-providers` | done | OpenAI-compatible provider (chat + streaming), settings, request body builder. |
| Tools | `echobot-tools` | done | Base registry, basic built-ins (shell, fs, web, git, media, memory, planning, cron), plus the `ToolRegistry` aggregate. |
| Skill | `echobot-skill` | done | Skill registry, model, parsing, tool helpers. |
| Runtime | `echobot-runtime` | done | `build_runtime_context` wires the provider, sessions, scheduling, agent core, session runner, settings, trace store, and the (optional) heartbeat service. The cross-crate assembly (role registry, decider, roleplay, coordinator) lives in the CLI crate. |
| Orchestration | `echobot-orchestration` | done | Decision engine, roleplay engine, route modes, role cards, background jobs, and the `ConversationCoordinator`. |
| CLI | `echobot-cli` | done | `chat` is fully functional end-to-end. `app` and `gateway` are phase 1 stubs that print a message and exit cleanly. |

## Smoke Tests

Smoke tests live next to the code they exercise:

| Crate | File | Coverage |
|---|---|---|
| `echobot-orchestration` | `src/decision.rs` | Rule-based decision classifies agent-style inputs as `Agent` and chat-style inputs as `Chat`. |
| `echobot-orchestration` | `src/roleplay.rs` | Default system prompt is non-empty and contains key phrases. |
| `echobot-runtime` | `src/sessions.rs` | Round-trips a session through JSON. |
| `echobot-tools` | `src/base.rs` | `ToolRegistry` registers and invokes a simple custom tool. |
| `echobot-providers` | `src/openai_compatible.rs` | Request body building (no real HTTP calls). |

## Running

### Build

```bash
cd D:/code/重构/echobot-rs
cargo build --workspace
```

### Tests

```bash
cd D:/code/重构/echobot-rs
cargo test --workspace
```

### Clippy

```bash
cd D:/code/重构/echobot-rs
cargo clippy --workspace --all-targets -- -D warnings
```

Clippy is allowed to emit warnings (the `--all-targets` flag turns up
some "too many arguments" warnings on the runtime trait surfaces, which
mirror the Python method signatures 1:1 and are intentional). Errors
are not allowed.

### Chat REPL

```bash
cd D:/code/重构/echobot-rs
cargo run -- chat
```

The `chat` subcommand:

1. Calls `build_runtime_context` to assemble the runtime.
2. Starts the cron service and (optionally) the heartbeat service.
3. Enters a REPL with the prompt `You[<session>]> `.
4. Dispatches the built-in commands: `exit`, `quit`, `clear`, `/help`,
   `/route`, `/role`, `/runtime`, `/session`.
5. For non-command input, calls
   `ConversationCoordinator::handle_user_turn_stream(...)` and prints
   the streamed response.
6. On EOF or `KeyboardInterrupt`, stops the services and exits cleanly.

Shared flags come from `common.rs`:
`--env-file`, `--workspace`, `--temperature`, `--max-tokens`,
`--no-tools`, `--no-skills`, `--no-memory`, `--no-heartbeat`,
`--heartbeat-interval`. The `chat` subcommand additionally takes
`--session`, `--new-session`, and `--verbose`.

### App (stub)

```bash
cd D:/code/重构/echobot-rs
cargo run -- app
```

Prints a phase 2 stub message and exits. Accepts `--channel-config`,
`--host`, and `--port` so the flag surface is locked in for phase 2.

### Gateway (stub)

```bash
cd D:/code/重构/echobot-rs
cargo run -- gateway
```

Prints a "out of scope for v1" message and exits. Accepts
`--channel-config` so the flag surface is locked in for phase 2.

## Notable Differences From the Python Implementation

* **Trait objects, not dataclasses.** Python dataclasses become Rust
  structs. The Python mixin pattern (e.g. `CoordinatorLike`) becomes
  trait objects (`Arc<dyn CoordinatorLike>`). The runtime crate
  declares placeholder traits so it does not have to depend on the
  orchestration crate directly.
* **Runtime assembly is split.** The Python `runtime/bootstrap.py`
  builds the full runtime in one go. The Rust port splits that into
  two layers: `echobot-runtime::bootstrap::build_runtime_context`
  builds the runtime-only pieces (provider, sessions, scheduling,
  session runner, settings, traces); the CLI crate's
  `runtime_assembly::assemble_runtime` layers the orchestration
  pieces (role registry, decider, roleplay, coordinator) on top. This
  keeps the dependency graph acyclic.
* **Adapters live in the CLI crate.** The runtime defines the
  `ToolRegistryLike` / `SkillRegistryLike` / `CronService` traits
  minimally. The concrete `echobot_tools::ToolRegistry` /
  `echobot_skill::SkillRegistry` are wrapped in
  `RuntimeToolAdapter` / `RuntimeSkillAdapter` / `RuntimeCronAdapter`
  in `crates/echobot-cli/src/bridge.rs`.
* **Async REPL.** The Python `input()` becomes a `tokio::io::BufReader`
  on stdin with `AsyncBufReadExt::lines`. Streaming output is plain
  `println!` / `print!` with explicit `std::io::Write::flush()` after
  each chunk so the user sees the LLM tokens as they arrive.
* **Memory support is a noop in v1.** The runtime accepts a
  `MemorySupport` trait object. v1 ships with `NoopMemorySupport`;
  ReMeLight (or a real memory back-end) lands in phase 2.
* **Provider is OpenAI-compatible only in v1.** Anthropic and the other
  back-ends are behind a single `OpenAICompatibleSettings` (different
  base URL + auth header). A first-class Anthropic client is phase 2.
* **The `app` and `gateway` subcommands are stubs.** The flag surface
  is locked in but the implementation lands in phase 2.

## Known TODOs and v1 Limitations

* Memory subsystem: only the `NoopMemorySupport` placeholder is
  wired. ReMeLight / long-term memory is phase 2.
* QQ / Telegram channels: out of scope for v1. The `gateway`
  subcommand accepts the flags but exits cleanly.
* Full HTTP API: out of scope for v1. The `app` subcommand accepts
  `--host` / `--port` / `--channel-config` but exits cleanly.
* Auto-generated skill scripts: the `SkillRegistry` parses and
  indexes skill directories but does not generate skill scripts
  from conversational data.
* The runtime still uses a few `Arc::get_mut` calls to wire the cron
  and heartbeat executors; a cleaner builder API is on the roadmap.

## Next Steps (Phase 2)

1. Replace `NoopMemorySupport` with a real memory back-end (ReMeLight
   or a simpler in-process store).
2. Land the FastAPI-equivalent HTTP server in the `app` subcommand,
   backed by `axum` or `warp`.
3. Land the QQ / Telegram channels in the `gateway` subcommand.
4. Add per-tool smoke tests in the `echobot-tools` crate, plus an
   integration test that drives the `chat` REPL end-to-end through a
   stub provider.
5. Wire a first-class Anthropic client in `echobot-providers`.
6. Auto-generated skill scripts.
