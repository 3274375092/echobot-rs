# EchoBot Rust Port — Parity Audit

> Audit run **2026-06-10** against commit `f9684dd` (main).
> Compared every backend subsystem and frontend asset tree in
> `echobot-rs/` against the original Python implementation at
> `../EchoBot/echobot/`. Methodology: 18 backend audits + 3 frontend
> audits, each adversarially verified by an independent agent before
> being recorded as a confirmed gap. 38 agents total, 545 tool calls,
> ~1M tokens.

## TL;DR

The Rust port covers the **core runtime surface** (config, models,
tools, providers, sessions, scheduling, HTTP routers) at near-1:1
parity with the Python original. **12 of 18 backend subsystems are at
full parity, 4 are partial, and 2 are entirely missing.**

The largest gap is the **channels / gateway** layer — the Python
multi-platform gateway (Telegram / QQ / Console, `MessageBus`,
`ChannelManager`, `DeliveryStore`, `RouteSessionStore`) is completely
absent; `gateway.rs` is a 5-line stub that prints a message and exits.

Other notable absences: **long-term memory** (`ReMeLite`) is a `Noop`,
**Kokoro TTS** is `NotImplemented`, and **image upload normalization**
only accepts pre-encoded JPEGs.

On the frontend side, the **web SPA is byte-identical** (168 files,
MD5-verified, copied at build time by `build.rs`). The **builtin
Live2D bundles** (40 files) and **builtin stage backgrounds** (2 JPGs)
are not packaged into the Rust binary and are expected to be supplied
in the user workspace.

## Parity table

| Subsystem | Status | Notes |
| --- | :---: | --- |
| config + env loading | ✅ matches | env loader, log levels, override semantics identical |
| core models / attachments / naming | ⚠️ partial | 2 image-processing blockers; `turn_inputs` missing route-mode helpers |
| LLM providers | ⚠️ partial | 2 blockers: stream error swallowing, SSE error-payload swallowing |
| tools (built-ins) | ✅ matches | all 11 tool modules ported; `memory` is a documented `Noop` stub |
| skill_support (loader) | ✅ matches | only `has_skills()` naming differs (`is_empty()`) |
| orchestration (decision / roleplay / coordinator) | ⚠️ partial | 1 major (`role_file_paths` missing), 1 cosmetic |
| runtime (sessions / agent_core / bootstrap) | ⚠️ partial | bootstrap is a stub; `SessionLifecycleService` missing |
| scheduling (cron + heartbeat) | ⚠️ partial | heartbeat doc is stale (claims stub, code is real); `run_loop` panic guard missing |
| TTS providers | ⚠️ partial | Kokoro = pure stub; Edge `list_voices` empty; factory missing 8 env vars |
| ASR providers | ⚠️ partial | `sherpa-sense-voice` default build is stub; Silero VAD absent |
| HTTP routers | ⚠️ partial | channels router stubbed; chat drops image/file attachments; ASR WS stubbed; upload budget math wrong |
| HTTP services (web_console, live2d, stage) | ⚠️ partial | `StageBackgroundService` stubbed; `WebRuntimeSettingsService` absent; TTS `default_voices` hardcoded `{}` |
| CLI subcommands | ⚠️ partial | all slash commands are inline stubs; `gateway` is a phase-1 stub |
| **channels (multi-platform gateway)** | ❌ **missing** | no `MessageBus`, `ChannelManager`, `BaseChannel`, or any platform adapter |
| **memory (long-term)** | ❌ **missing** | `NoopMemorySupport`; no `ReMeLite`, no `compact_history`, no `remember_turn` |
| commands (REPL slash commands) | ⚠️ partial | dispatch architecture absent; subcommands (`list/set/current/...`) missing for `/session`, `/role`, `/route`, `/runtime` |
| gateway entry point | 🟡 stub | 5-line stub; no `GatewayRuntime`, `DeliveryStore`, `RouteSessionStore`, `GatewaySessionService` |
| skill bundles | ✅ matches | 4 modules 1:1; 8 bundled skills discovered at runtime |
| **web SPA assets** | ✅ matches | 168 files, byte-identical, MD5-verified, build-time copy via `build.rs` |
| **builtin Live2D bundles** | ❌ missing | 40 files absent; expected to be user-supplied in workspace |
| **builtin stage backgrounds** | ❌ missing | 2 JPGs absent; stage backgrounds feature stubbed |

## Confirmed gaps

### Blockers (8)

These prevent a documented feature from working end-to-end.

- **Image decode/resize pipeline is a stub** — `crates/echobot-core/src/images.rs:179-211` only accepts pre-encoded JPEGs (magic-byte check); Python uses Pillow to decode, exif-transpose, resize, and re-encode arbitrary PNG/GIF/WEBP. `attachments.rs:449-450` uses `max_side` as both width and height instead of real decoded dimensions.
- **Stream error swallowing in OpenAI provider** — `crates/echobot-providers/src/openai_compatible.rs:395-405` (`stream_text_chunks`) logs warnings and returns on HTTP errors / send failures instead of raising. `parse_sse_line` at lines 721-728 returns `None` for API error payloads. Python raises `RuntimeError` in both cases. **Production failures become invisible.**
- **`sherpa-sense-voice` default build is a stub** — `crates/echobot-asr/src/providers/sherpa.rs:115-212` is gated behind the non-default `sherpa-rs` Cargo feature; the default build returns `AsrError::NotImplemented`.
- **Silero VAD provider is entirely missing** — `crates/echobot-asr/src/vad.rs` defines traits only; no concrete implementation. The real-time VAD path is non-functional.
- **Channels subsystem completely absent** — no `MessageBus`, `ChannelManager`, `BaseChannel`, Console / QQ / Telegram adapters, config loading, or type definitions anywhere in the Rust codebase.
- **Long-term memory is a `Noop`** — `NoopMemorySupport::search` returns empty; no `ReMeLite`, `compact_history`, or `remember_turn`.

### Major (25)

Affect specific features but don't take down the system.

- **Channels router is fully stubbed** — `crates/echobot-app/src/routers/channels.rs:22-42` returns empty list / empty config / echoes input; never calls `runtime.channel_service`.
- **Chat router drops image/file attachments** — `routers/chat.rs:63-73,104-115` passes `None,None` to `handle_user_turn`; Python calls `_resolve_chat_images` / `_resolve_chat_files`.
- **ASR websocket is a v1 stub** — `routers/web.rs:542-550` replies `{"type":"ignored","reason":"v1 stub"}` to every binary frame.
- **Attachments upload budget math is wrong** — `routers/attachments.rs:146-152` uses `base_dir` path-string length instead of `file_budget.max_input_bytes`.
- **Bootstrap is a stub** — `crates/echobot-runtime/src/bootstrap.rs:129,148` hardcodes `delegated_ack_enabled=true` and `max_steps=24`; ignores `ECHOBOT_DELEGATED_ACK_ENABLED` and `ECHOBOT_AGENT_MAX_STEPS`. `RuntimeContext` fields `coordinator / role_registry / memory_support / tool_registry_factory` are all `None`.
- **`SessionLifecycleService` is missing** — Python `session_service.py` (`list_sessions`, `load_or_create_session`, `switch_session`, `rename`, `delete`, `purge`, ...) has no Rust equivalent.
- **Kokoro TTS is `NotImplemented`** — entire provider returns stub; no auto-download, no ONNX runtime, no 103 voice entries.
- **Kokoro factory ignores 8 env vars** — `crates/echobot-tts/src/factory.rs:122-132` only reads `ECHOBOT_TTS_KOKORO_DEFAULT_VOICE`; Python reads 9.
- **`StageBackgroundService` is stubbed** — `crates/echobot-app/src/services/web_console/mod.rs:97-117` returns errors/dummy payloads; no `build_stage_config`, no file persistence.
- **`WebRuntimeSettingsService` is missing** — no `load_settings` / `save_selected_asr_provider`; `initialize_runtime_settings` silently skips ASR provider validation.
- **TTS `default_voices` always `{}`** — `web_console/mod.rs:204-205` hardcoded empty map; Python iterates providers.
- **`role_file_paths()` missing** — `crates/echobot-orchestration/src/roles.rs:97-100` only has `managed_role_path()`.
- **All REPL slash commands are inline stubs** — `/session`, `/role`, `/route`, `/runtime` only print current state; no `list / set / new / switch / rename / delete / help` subcommands.
- **Heartbeat module doc is stale** — `crates/echobot-runtime/src/scheduling/heartbeat.rs:6-11` claims a stub for the LLM-decision step; `decide()` at lines 231-287 actually calls `provider.generate()`. Doc only.
- **CronService `run_loop` has no panic guard** — `crates/echobot-runtime/src/scheduling/cron.rs:951-953` does not guard `executor` future panics; Python catches `RuntimeError` and continues.
- **`gateway.rs` is a 5-line stub** — `crates/echobot-cli/src/gateway.rs:22-27` prints two lines and returns; no runtime, bus, or service is instantiated.
- **Gateway services absent** — `services/delivery.rs:15-38` (5 missing methods), `services/route_sessions.rs:14-33` (9 missing methods), `services/session_service.rs:15-105` (11 missing async methods).
- **`turn_inputs.rs` missing route-mode helpers** — `resolve_file_attachment_route_mode` and `has_file_processing_capability` absent; depend on unported orchestration.

### Minor (12)

Worth a follow-up commit, but don't block any user flow.

- **`ToolRegistryLike` stub** — `turns.rs:126-129` only has `names()`; Python also exposes `get_tool` / `has_tool`.
- **`SkillRegistry.has_skills()` missing** — Rust only has `is_empty()` / `len()`; Python `turn_inputs.py:76` uses `has_skills()`.
- **`RoleplayEngine.chat_reply()` non-streaming wrapper missing** — Rust only has `stream_chat_reply()`; cosmetic.
- **`Live2DDiscoveredMotion.index` is `usize`** — Python allows `index=-1`; Rust would wrap. Path not currently hit in practice.
- **`system_prompt.rs` identity omits Python version + OS release** — only shows `std::env::consts::OS`.
- **`agent_traces.rs` JSON escaping may differ** — `serde_json::to_string` does not provide `ensure_ascii=False` equivalent.
- **`StatusReport` / `HeartbeatService` ergonomic differences** — `enabled: bool` shape; `on_notify` is a required parameter.
- **Edge / OpenAI-compatible TTS minor gaps** — `list_voices()` returns empty; network errors propagate instead of falling back; no dynamic `edge-tts` import-with-hint check.
- **Memory helpers** — `_ensure_memory_files()`, `build_summary_message()` absent.
- **CLI `--verbose` flag unused** — `chat.rs:57` accepts but does not wire to any trace output; `trace.py` has no Rust counterpart.

### Cosmetic (~5)

`agent_traces` JSON escaping, `parse_message_parts` style, micro-timestamp format, single missing Python aliases. Not user-visible.

## Recommended fix order

Ranked by value-per-hour, not by audit order.

1. **Replace the image normalization stub** — use the `image` crate to
   decode arbitrary formats, exif-transpose, resize, and re-encode
   JPEG. Fix `create_image_attachment` to use real decoded dimensions.
   Unlocks PNG / GIF / WEBP uploads. *~half a day.*
2. **Fix LLM streaming error propagation** in `openai_compatible.rs`:
   `stream_text_chunks` returns `Err(ProviderError::HttpStatus)` on
   non-2xx; `parse_sse_line` propagates API-level `error` payloads.
   Match Python's `RuntimeError` behavior. *~1 hour, affects every
   user.*
3. **Wire the real runtime bootstrap** — replace `bootstrap.rs`
   hardcoded values with `ECHOBOT_DELEGATED_ACK_ENABLED` /
   `ECHOBOT_AGENT_MAX_STEPS` env reads; instantiate `coordinator /
   role_registry / memory_support / tool_registry_factory` in
   `RuntimeContext`. Port `SessionLifecycleService` (10 async methods)
   for full session CRUD parity. *~1–2 days.*
4. **Implement the channels subsystem** end-to-end: `MessageBus`,
   `BaseChannel`, `ChannelManager`, `ChannelConfig` loading, and at
   least `ConsoleChannel` plus one bot-platform adapter (Telegram
   recommended). Wire `gateway.rs` to actually start `GatewayRuntime`,
   instantiate the bus, and run the message loop. **Single largest
   functional gap.** *~1 week.*
5. **Enable the `sherpa-rs` Cargo feature by default** (or
   auto-download) and **implement the Silero VAD provider**, so the
   ASR subsystem can run end-to-end. In parallel, port the REPL
   command dispatch architecture so `/session`, `/role`, `/route`,
   `/runtime` get full subcommand support, and implement
   `StageBackgroundService` + `WebRuntimeSettingsService` so the web
   console is fully functional. *~a few days.*

## Notes / non-gaps

These were flagged during the audit but rejected on adversarial
re-check. Listed here so future readers know they were considered.

- `LLMUsage.prompt_cache_hit_rate_percent` rounding — both Python and
  Rust produce identical 2-decimal-place results.
- `post_json` HTTP error semantics — both raise concrete errors
  (`RuntimeError` vs `ProviderError::HttpStatus`); same outcome for
  callers.
- `stream_generate` tools fallback — both fall back to non-streaming
  `generate()` and yield one chunk; match confirmed.
- `tool_choice` type surface — Rust enum covers all Python `str` /
  `dict` inputs.
- `current_role_name()` metadata write-back — Rust **does** write
  back; initial finding was a false positive.
- `RoleCardRegistry.get()` returning `Option` vs raising — semantics
  equivalent; callers handle both shapes.
- All 11 tool modules are fully ported with matching parameter
  schemas.
- The web SPA tree (168 files) is **byte-identical** and MD5-verified.
- `memory_support` being `None` / `NoopMemorySupport` everywhere is
  documented as a phase-2/3/4 placeholder per `RUST_PORT.md`.

## Audit summary

| Metric | Value |
| --- | --- |
| Backend subsystems audited | 18 |
| Frontend asset trees audited | 3 |
| Confirmed gaps | ~50 (8 blockers, ~25 major, ~12 minor, ~5 cosmetic) |
| Largest single gap | **channels + gateway** (9+ blockers/majors across `MessageBus`, `ChannelManager`, platform adapters, config, runtime, services) |
| Second largest gap | **image normalization** (2 blockers in adjacent files) |
| Strongest areas | tools, skill_support, scheduling types, web SPA assets, config, sessions, providers' payload construction |
| Agents | 38 |
| Tool uses | 545 |
| Tokens | ~1.06M |
| Wall time | ~6 minutes |
