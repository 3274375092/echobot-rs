# echobot-rs

Rust port of EchoBot — a Live2D anime-style AI assistant that combines a persona-driven chat layer with a full tool-using background agent.

## Status

Phase 1 in progress. The workspace and crate skeletons are in place; business logic will be ported in subsequent work streams. See `RUST_PORT.md` for the migration plan.

## Build / Run

Build the workspace:

```shell
cargo check --workspace
```

Run the CLI (scaffold only — subcommands are stubs):

```shell
cargo run -p echobot-cli -- chat
cargo run -p echobot-cli -- app
cargo run -p echobot-cli -- gateway
```
