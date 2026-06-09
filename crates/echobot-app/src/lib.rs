//! `echobot-app` provides the HTTP front-end for the EchoBot Rust port.
//!
//! It exposes an `axum`-based HTTP server that mirrors the FastAPI
//! `echobot.app` package from the Python reference implementation: chat,
//! sessions, cron, heartbeat, roles, channels, attachments, the web
//! console, plus a health endpoint. Static frontend assets are embedded
//! at compile time via the `include_dir!` macro and served under `/web/`.

pub mod create_app;
pub mod error;
pub mod router;
pub mod routers;
pub mod runtime;
pub mod schemas;
pub mod services;
pub mod state;

pub use create_app::create_app;
pub use error::AppError;
pub use router::router as build_router;
pub use runtime::AppRuntime;
pub use state::AppState;
