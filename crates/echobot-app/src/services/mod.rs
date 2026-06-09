//! Service layer for the HTTP front-end.
//!
//! These mirror the `echobot.app.services` Python package:
//!
//! * [`session_service`]   — gateway-shaped session lifecycle (loads the
//!   current session, switches / renames, etc.). v1 thin wrapper around
//!   the underlying `SessionStore`.
//! * [`route_sessions`]    — persistent per-channel route-session map.
//!   v1 in-memory stub.
//! * [`delivery`]          — gateway delivery store (delivery attempts
//!   for outbound messages). v1 in-memory stub.
//! * [`web_console`]       — Live2D model + stage background + web-config
//!   helpers. v1 returns empty configuration.

pub mod delivery;
pub mod route_sessions;
pub mod session_service;
pub mod web_console;

pub use delivery::DeliveryStore;
pub use route_sessions::RouteSessionStore;
pub use session_service::SessionService;
pub use web_console::{Live2DUploadFile, WebConsoleService};
