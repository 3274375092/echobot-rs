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
| TTS | `echobot-tts` | done | Trait-based provider abstraction; `edge`, `openai_compatible`, and `kokoro` (stub) providers; `TtsService` facade; env-driven factory. |
| ASR | `echobot-asr` | done | `sherpa-onnx` (STUB — see RUST_PORT.md) and `openai_compatible` providers; `AsrService` facade; WAV + symphonia audio decoders; VAD trait surface. |
| App | `echobot-app` | done | axum-based HTTP server: health, sessions, chat, cron, heartbeat, roles, channels, attachments, web console. Mirrors the Python `echobot.app` package 1:1. |
| CLI | `echobot-cli` | done | `chat` is fully functional end-to-end. `app` wires the full HTTP server (TTS + ASR + axum router). `gateway` remains a v1 stub. |
| Desktop | `echobot-desktop` | done | Tauri 2.x desktop shell. Bundles a 31MB single `.exe` (`target/release/echobot-desktop.exe`) that starts the axum server in-process and opens a native webview window. First run copies `.env.example` → `.env`. |

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

Clippy must be clean under `-D warnings`. The only `#[allow(...)]`
attributes that remain are:

* `#[allow(clippy::too_many_arguments)]` on a handful of trait and
  helper methods (`AgentCoreLike`, `run_agent_turn`,
  `OpenAICompatibleProvider::build_payload`, `RoleplayLlm`, the
  `RoleplayEngine` `run` / `run_stream` helpers, and
  `ConversationJobStore::create`). All of these mirror Python method
  signatures 1:1, so the long argument lists are intentional.
* `#[allow(dead_code)]` on the unused `live2d` / `stage_background`
  asset fetchers in `crates/echobot-app/src/routers/web.rs` and the
  `classify_role_error` helper in `crates/echobot-app/src/routers/roles.rs`,
  which are part of the established public surface for v2.

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

### App

```bash
cd D:/code/重构/echobot-rs
cargo run -- app
```

The `app` subcommand now boots the full EchoBot HTTP server on top of
axum. It:

1. Assembles the shared runtime via `runtime_assembly::assemble_runtime`.
2. Builds a `TtsService` from env via
   `echobot_tts::factory::build_default_tts_service`.
3. Builds an `AsrService` from env via
   `echobot_asr::factory::build_default_asr_service`.
4. Wraps the runtime + services in an `echobot_app::runtime::AppRuntime`.
5. Builds the axum `Router` with `echobot_app::create_app` and binds
   to `--host:--port` (defaults: `127.0.0.1:8000`).
6. Shuts down gracefully on `Ctrl+C` via `tokio::signal::ctrl_c`.

On startup it prints `EchoBot API listening on http://<host>:<port>/web`.

Flags: `--host`, `--port`, plus the shared runtime flags
(`--env-file`, `--workspace`, `--temperature`, `--max-tokens`,
`--no-tools`, `--no-skills`, `--no-memory`, `--no-heartbeat`,
`--heartbeat-interval`). `--channel-config` is accepted for surface
stability but is unused in v1.

### Desktop (Tauri)

```bash
cd D:/code/重构/echobot-rs
cargo build --release -p echobot-desktop
./target/release/echobot-desktop.exe
```

`echobot-desktop` is a Tauri 2.x shell that bundles the entire
EchoBot stack into a single 31MB Windows executable. On launch it:

1. Copies `.env.example` → `.env` on first run (no overwrite).
2. Loads `.env` via `dotenvy`.
3. Assembles the same `FullRuntimeContext` the CLI uses, plus
   the `TtsService` and `AsrService`.
4. Starts the axum HTTP server in a background tokio task on
   `127.0.0.1:8765` (in-process — no separate server binary).
5. Opens a Tauri webview window pointing at `http://127.0.0.1:8765/web`.
6. Aborts the server task when the window closes.

The build uses the workspace's `[profile.release]` settings
(`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`,
`panic = "abort"`), which gets the binary down to ~31MB. The
embedded web assets (21MB of frontend bundles) are baked into
`echobot-app` via `include_dir!` and pulled into the desktop
binary by transitively linking the `echobot-app` crate.

To produce a Windows installer (`.msi`/`.exe`), install the
Tauri CLI (`cargo install tauri-cli --version "^2"`) and run
`cargo tauri build` from `crates/echobot-desktop/`.

### Gateway (stub)

```bash
cd D:/code/重构/echobot-rs
cargo run -- gateway
```

Prints a "out of scope for v1" message and exits. Accepts
`--channel-config` so the flag surface is locked in for phase 2.

## TTS providers in v1

The `echobot-tts` crate ships a trait-based provider abstraction
(`TtsProvider`) plus a `TtsService` facade that dispatches to the
active provider. v1 supports:

| Provider | Name | Status | Notes |
|---|---|---|---|
| Microsoft Edge "read aloud" | `edge` | done | Free, no auth; default provider. WebSocket API. |
| OpenAI-compatible HTTP | `openai_compatible` | done | Any `/audio/speech`-compatible endpoint. |
| Kokoro (local) | `kokoro` | STUB | Provider type compiles and is registered when the `kokoro` cargo feature is enabled, but it does not produce audio in v1. Wiring `sherpa-rs` for the local model is phase 3. |

Select the active provider with `ECHOBOT_TTS_PROVIDER` (defaults to
`edge`). Voice and per-provider configuration is documented in
`crates/echobot-tts/src/factory.rs`.

## ASR providers in v1

The `echobot-asr` crate ships an `AsrProvider` trait, an `AsrService`
facade, audio decode utilities (WAV via `hound`, anything else via
`symphonia`), and a VAD trait surface (no concrete VAD provider in
v1). v1 supports:

| Provider | Name | Status | Notes |
|---|---|---|---|
| OpenAI Transcriptions | `openai_compatible` | done | Any `/audio/transcriptions`-compatible endpoint. |
| sherpa-onnx (SenseVoice) | `sherpa-onnx` | done (feature-gated) | Real `sherpa_rs::sense_voice::SenseVoiceRecognizer` provider; opt in with `--features sherpa-rs` (pulls in the sherpa-onnx C library). Default build keeps the stub behaviour. |

Select the active ASR provider with `ECHOBOT_ASR_PROVIDER` (defaults
to `sherpa-sense-voice`; the default build will surface
`AsrError::NotImplemented` if it is actually invoked — opt into the real
implementation with `cargo build --features sherpa-rs` or
`cargo build -p echobot-asr --features sherpa-rs`). Use the
OpenAI-compatible provider by setting
`ECHOBOT_ASR_PROVIDER=openai-transcriptions`.

## Web frontend

The `echobot-app` crate serves the Python EchoBot web console's
frontend assets under `/web/`. Assets are embedded at compile time
via the `include_dir!` macro from
`D:/code/重构/EchoBot/echobot/app/web/` (the workspace's sibling
directory), so no runtime file serving is needed. A future task may
copy them into the `echobot-app` crate itself to remove the sibling
dependency, but for v1 the `include_dir!` approach is the simplest way
to share the same assets the Python app already uses.

Routes:

* `/` and `/favicon.ico` are explicit (index + favicon.svg).
* `/api/*` is the JSON API (health, sessions, chat, cron, heartbeat,
  roles, channels, attachments).
* `/web/*` falls through to the embedded assets, with an SPA-style
  `index.html` fallback for unknown paths.

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
  ReMeLight (or a real memory back-end) lands in phase 3.
* **Provider is OpenAI-compatible only in v1.** Anthropic and the other
  back-ends are behind a single `OpenAICompatibleSettings` (different
  base URL + auth header). A first-class Anthropic client is phase 3.
* **The `gateway` subcommand is a stub.** QQ / Telegram are still out
  of scope for v1. The `app` subcommand is now fully functional.

## Known TODOs and v1 Limitations

* Memory subsystem: only the `NoopMemorySupport` placeholder is
  wired. ReMeLight / long-term memory is phase 3.
* QQ / Telegram channels: out of scope for v1. The `gateway`
  subcommand accepts the flags but exits cleanly.
* Auto-generated skill scripts: the `SkillRegistry` parses and
  indexes skill directories but does not generate skill scripts
  from conversational data.
* The runtime still uses a few `Arc::get_mut` calls to wire the cron
  and heartbeat executors; a cleaner builder API is on the roadmap.
* **Web asset catch-all uses `Router::fallback`.** matchit 0.7 (the
  router crate bundled with axum 0.7.9) rejects the brace-prefix
  catch-all syntax `{*name}` once any sibling route is registered, so
  the asset fallthrough lives in `create_app::fallback(serve_static)`
  rather than as a router entry. The brace syntax is the only
  documented one in axum 0.7; bumping to matchit 0.8 (and axum
  0.8) would let us bring the catch-all back into the web router.
* **ASR sherpa-onnx is feature-gated.** The default build ships the
  stub provider; opt into the real `sherpa-rs`-backed SenseVoice
  implementation with `cargo build --features sherpa-rs` (or `--features
  'echobot-asr/sherpa-rs'` from a workspace member). The first
  `--features sherpa-rs` build downloads the sherpa-onnx native
  libraries from GitHub via the `sherpa-rs-sys` `download-binaries`
  feature.

## Phase 2 metrics

| Crate | LoC (src/) | Tests |
|---|---|---|
| `echobot-tts` | 2,538 | 42 |
| `echobot-asr` | 2,450 | 25 |
| `echobot-app` | 3,416 | 1 (integration) |
| **Phase 2 added** | **8,404** | **68** |
| Workspace total | 29,154 | 171 |

Crate count grew from 7 (phase 1) to 10 (phase 2). All phase 1 tests
still pass — the wire-up was strictly additive.

## Phase 3 metrics

| Crate | LoC (src/) | Tests | Δ vs phase 2 |
|---|---|---|---|
| `echobot-asr` | 3,140 | 29 | +690 LoC, +4 tests |
| `echobot-app` | 3,425 | 1 (integration) | +9 LoC, +0 tests |
| `echobot-cli` | 1,119 | 4 (e2e + smoke) | +78 LoC, +3 tests |
| `echobot-tools` | 6,247 | 71 | +0 LoC, +58 tests (per-tool coverage) |
| **Phase 3 added** | **+777** | **+65** | per-tool tests + e2e + sherpa-rs wiring |
| **Workspace total** | **~30,000** | **243** | +72 tests over phase 2 |

Phase 3 also adds the `sherpa-rs` feature flag (default: off). When
enabled, the `sherpa-onnx` ASR provider is built against
`sherpa_rs::sense_voice::SenseVoiceRecognizer`; the default build
keeps the stub provider so CI does not need to download the native
binaries. Enable with:

```bash
cargo build --features sherpa-rs
# or, from a workspace member:
cargo build -p echobot-asr --features sherpa-rs
```

**Clippy status:** `cargo clippy --workspace --all-targets -- -D
warnings` is clean — the workspace builds with zero warnings.

## Phase 4 Audit (2026-06-10)

Before scoping phase 4 work, the entire Rust port was audited 1:1
against the Python original. The full report — including per-finding
`file:line` evidence and the 8 false positives the verify stage
caught — lives in [`AUDIT.md`](AUDIT.md).

**Headline result:** 12 of 18 backend subsystems at full parity, 4
partial, 2 entirely missing. The web SPA is byte-identical
(168 files, MD5-verified). Total confirmed gaps: ~50
(8 blockers, ~25 major, ~12 minor, ~5 cosmetic).

| Severity | Where it hurts | Examples |
|---|---|---|
| **Blocker** | A documented feature can't run end-to-end. | Image upload only accepts pre-encoded JPEG; LLM streaming silently swallows API errors; Silero VAD is trait-only; channels subsystem is entirely absent. |
| **Major** | A specific code path is wrong or returns dummy data. | Chat router drops image/file attachments; ASR websocket is a v1 stub; CronService run loop has no panic guard; REPL slash commands have no subcommand dispatch. |
| **Minor** | Worth a follow-up commit, no user impact today. | `ToolRegistryLike` only exposes `names()`; `SkillRegistry.has_skills()` missing; `--verbose` flag accepted but unwired. |
| **Cosmetic** | Doc / format / naming drift, no functional impact. | Stale module docstrings; JSON ASCII-escape differences; missing Python aliases. |

### Re-running the audit

The audit is reproducible — same script + same args returns the same
findings (each pipeline stage is cached on the prompt hash). The
methodology:

1. **Fan out per subsystem.** 18 backend audits + 3 frontend audits
   running independently as `Explore` agents. Each is told to compare
   one Python module against its Rust counterpart and return a
   structured `FINDING` object (gaps, severity, parity table).
2. **Adversarially verify each finding.** Every flagged gap is passed
   to a second agent that tries to *refute* it. Default to
   `rejected` unless concrete file:line evidence confirms the gap.
   This caught 8 false positives — see the "Notes / non-gaps"
   section of `AUDIT.md`.
3. **Synthesize one report.** A final agent consumes the verified
   verdicts and produces the parity table + sorted blocker list.

Stats from the 2026-06-10 run: 38 agents, 545 tool calls, ~1.06M
tokens, ~6 minutes wall time. Re-run with the same Workflow script
(saved under
`.claude/projects/D--code----echobot-rs/<session>/workflows/scripts/audit-rust-port-vs-python-*.js`)
or recreate from scratch using the same fan-out → verify → synthesize
shape.

## Next Steps (Phase 4 / v2)

The v1 feature set is locked. Phase 4 priorities below are derived
from the 2026-06-10 audit and re-ordered by **value per hour** rather
than by audit appearance order. See `AUDIT.md` for the full gap list
with file:line evidence; the tiers below collect the items that
actually move the needle.

### Tier 1 — Correctness & safety (1 day total)

These are small, surgical fixes that prevent invisible production
failures or unlock common user flows.

1. **Replace the image normalization stub.** `crates/echobot-core/src/images.rs:179-211`
   only accepts pre-encoded JPEGs. Wire the `image` crate to decode
   arbitrary PNG / GIF / WEBP, exif-transpose, resize, and re-encode
   JPEG. Fix `create_image_attachment` (`attachments.rs:449-450`) to
   use real decoded dimensions instead of `max_side` for both
   width and height. *~half a day.*
2. **Fix LLM streaming error propagation.** `crates/echobot-providers/src/openai_compatible.rs:395-405`
   (`stream_text_chunks`) currently logs and returns on HTTP errors /
   send failures. `parse_sse_line` at lines 721-728 returns `None` for
   API error payloads. Both should propagate `Err(ProviderError::HttpStatus)`
   to match Python's `RuntimeError` behaviour. *~1 hour, affects every
   user.*
3. **Wire the real runtime bootstrap.** `crates/echobot-runtime/src/bootstrap.rs:129,148`
   hardcodes `delegated_ack_enabled = true` and `max_steps = 24`.
   Replace with reads of `ECHOBOT_DELEGATED_ACK_ENABLED` /
   `ECHOBOT_AGENT_MAX_STEPS`. Instantiate `coordinator /
   role_registry / memory_support / tool_registry_factory` in
   `RuntimeContext` instead of leaving them `None`. *~2 hours.*
4. **Add panic guard to `CronService::run_loop`.** `crates/echobot-runtime/src/scheduling/cron.rs:951-953`
   crashes the scheduler on any executor panic; Python catches and
   continues. Wrap with `catch_unwind` (or per-task `tokio::spawn` +
   `JoinError` handling) so a single bad job doesn't take down the
   service. *~30 minutes.*

### Tier 2 — Largest missing functionality

Multi-day projects that close the biggest gaps.

5. **Channels + gateway subsystem.** *Largest single gap.* Implement
   `MessageBus`, `BaseChannel`, `ChannelManager`, `ChannelConfig`
   loading, and at least `ConsoleChannel` plus one bot adapter
   (Telegram recommended). Wire `crates/echobot-cli/src/gateway.rs`
   to actually start a `GatewayRuntime`, instantiate the bus, and run
   the message loop. Port the three gateway services:
   `DeliveryStore` (5 methods), `RouteSessionStore` (9 methods),
   `GatewaySessionService` (11 async methods). *~1 week.*
6. **Long-term memory.** Replace `NoopMemorySupport` with a real
   back-end (ReMeLight via pyo3, or a self-rolled `sled` +
   `sqlite-vec` + `tiktoken-rs` stack). Port `compact_history` and
   `remember_turn`. Wire into the runtime and `MemoryTool`. *~3-4
   days.*
7. **`SessionLifecycleService`.** Port the Python `session_service.py`
   methods (`list_sessions`, `load_or_create_session`,
   `switch_session`, `rename`, `delete`, `purge`, ...) so the web
   console and HTTP API get full session CRUD parity. *~1 day.*
8. **REPL slash command dispatch.** `/session`, `/role`, `/route`,
   `/runtime` currently only print state. Port the Python `commands/`
   dispatch architecture (parse/execute handler pairs) and the full
   `list / set / new / switch / rename / delete / help` subcommand
   surface. *~1-2 days.*
9. **Local TTS — Kokoro.** Wire the Kokoro provider on top of
   `sherpa-rs` (or a similar local TTS engine). The feature gate and
   provider trait surface exist; only the implementation is missing.
   Port the 9 `ECHOBOT_TTS_KOKORO_*` env vars (only 1 is read today
   per `crates/echobot-tts/src/factory.rs:122-132`). *~2-3 days.*
10. **Silero VAD provider.** `crates/echobot-asr/src/vad.rs` defines
    traits only. Port `vad/silero.py` on top of `sherpa-rs` so the
    real-time VAD path works. Default `ECHOBOT_VAD_PROVIDER` back to
    `silero` once it lands. *~2 days.*

### Tier 3 — Polish & v2 stretch

11. **HTTP plumbing fixes** — chat router restoring image/file
    attachment resolution; attachments router using `file_budget.max_input_bytes`
    instead of path-string length; ASR websocket holding a long-lived
    session across the connection instead of stubbing every binary
    frame.
12. **Web console services** — port `StageBackgroundService` (no
    longer stubbed), `WebRuntimeSettingsService`, fix TTS
    `default_voices` to iterate providers instead of returning `{}`.
13. **Bundle builtin assets** — `echobot/app/builtin_live2d/` (40
    files) and `echobot/app/builtin_stage_backgrounds/` (2 JPGs)
    should be embedded via `include_dir!` so the desktop `.exe` is
    self-contained. Today they must be user-supplied in the
    workspace.
14. **First-class Anthropic client.** Add a non-OpenAI-compatible
    provider in `echobot-providers` so the runtime can talk to
    Anthropic without going through an OpenAI-style proxy.
15. **Auto-generated skill scripts.** Extend `SkillRegistry` to
    generate skill scripts from conversational data (mirror the
    Python `SkillRegistry.autogen_*` helpers) and add a CLI command
    to trigger generation.
16. **Minor cleanups from the audit** — `ToolRegistryLike.get_tool`
    / `has_tool`, `SkillRegistry.has_skills()`,
    `RoleplayEngine.chat_reply()` non-streaming wrapper, `--verbose`
    flag wiring, stale module docstrings (notably the heartbeat doc
    that claims a stub when the code is real).

### Phase 4 success criteria

Tier 1 done → no invisible failures, all uploads work, the runtime
respects its env vars. Tier 2 done → multi-platform deployment is
real, the bot has long-term memory, the REPL is feature-complete.
Tier 3 done → parity audit shows zero blockers and ≤ 5 majors. The
audit is re-run after each tier completes so progress is measurable.
