# echobot-rs

Rust port of [EchoBot](../EchoBot) — a Live2D anime-style AI assistant that
combines a persona-driven chat layer with a full tool-using background
agent. Same features as the Python original, native Windows binary, no
runtime dependencies.

> **Status — phase 3 complete.** All 11 crates ported. `chat` REPL fully
> functional end-to-end. `app` boots the full HTTP server (TTS + ASR +
> Live2D web console). `desktop` bundles everything into a single ~31 MB
> `.exe`. See [`RUST_PORT.md`](RUST_PORT.md) for the per-crate migration
> log.

## What's in the box

| Subcommand | What it does |
|---|---|
| `echobot chat` | Terminal REPL. Streams persona replies; routes tool-using prompts to a background agent loop. Built-in slash commands (`/route`, `/role`, `/session`, `/runtime`, ...). |
| `echobot app` | Full HTTP server on top of `axum` — chat, sessions, cron, heartbeat, roles, channels, attachments, embedded Live2D web console, OpenAI-compatible TTS / Whisper-ASR endpoints. Default `http://127.0.0.1:8000/web`. |
| `echobot-desktop` (Tauri) | Single ~31 MB Windows `.exe` that starts the same HTTP server in-process and opens a native webview window pointing at it. |
| `echobot gateway` | Reserved for phase-4 multi-channel gateway; v1 stub. |

## Workspace layout

```
crates/
├── echobot-core             — attachments, config, error, models, naming, turn inputs
├── echobot-providers        — OpenAI-compatible LLM provider (chat + streaming)
├── echobot-tools            — shell / fs / web / git / media / memory / planning / cron
├── echobot-skill            — file-based skill registry + parser
├── echobot-runtime          — provider + sessions + scheduling + trace store wiring
├── echobot-orchestration    — Decision → Roleplay → Agent coordinator, role cards, jobs
├── echobot-tts              — `edge` (default), `openai_compatible`, `kokoro` (stub)
├── echobot-asr              — `openai_compatible` (Whisper), `sherpa-sense-voice`
├── echobot-app              — axum HTTP server + embedded web console
├── echobot-cli              — `chat` / `app` / `gateway` CLI front-end
└── echobot-desktop          — Tauri 2.x shell, bundles everything into one .exe
```

## Quick start

### 1. Configure the LLM provider

Copy the template and fill in your API key + model:

```shell
cp .env.example .env
# edit .env: at minimum set LLM_API_KEY and LLM_MODEL
# (LLM_BASE_URL too, if you're not on OpenAI)
```

Provider presets for **DeepSeek**, **SiliconFlow**, **Ollama**, **vLLM /
LM Studio** are pre-written in `.env.example` — uncomment one.

### 2. Build

```shell
cargo build --workspace --release
```

### 3. Run

```shell
# Terminal REPL
cargo run --release -- chat

# Full HTTP server + web console
cargo run --release -- app
# → open http://127.0.0.1:8000/web

# Native desktop window (Windows)
cargo build --release -p echobot-desktop
./target/release/echobot-desktop.exe
```

## Voice (TTS / ASR)

Both are off the critical path — `chat` works without either. To enable:

| Feature | Default provider | Env switch |
|---|---|---|
| **TTS** | `edge` (Microsoft "read aloud", free, no auth) | `ECHOBOT_TTS_PROVIDER=openai_compatible` for any `/audio/speech` endpoint |
| **ASR** | OpenAI-compatible `/audio/transcriptions` | `ECHOBOT_ASR_PROVIDER=sherpa-sense-voice` for local SenseVoice via `sherpa-onnx` (requires `--features sherpa-rs` on `echobot-asr`) |
| **VAD** | disabled in v1 | Silero VAD landing once `providers/silero.rs` is filled in |

See `.env.example` for the complete `ECHOBOT_TTS_*` / `ECHOBOT_ASR_*` key
list and per-provider knobs.

## Development

```shell
# Type-check
cargo check --workspace

# Tests (unit + integration + e2e chat REPL)
cargo test --workspace

# Clippy — must be clean under -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

The codebase tracks the Python implementation 1:1 — most modules carry a
`//! Verbatim port of echobot/<path>.py` header so changes can be diffed
across languages.

## Project layout against the original Python project

Each Rust crate maps onto a Python package:

```
echobot-rs/crates/echobot-<x>      ↔   EchoBot/echobot/<x>/
```

When in doubt about behaviour, the Python source is the spec. See
[`RUST_PORT.md`](RUST_PORT.md) for the migration plan, per-crate
checklist, and the list of allowed deviations.

## License

Same as the upstream Python project.
