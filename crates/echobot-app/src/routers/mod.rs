//! Routers — axum handler functions grouped by Python router module.

pub mod attachments;
pub mod channels;
pub mod chat;
pub mod cron;
pub mod health;
pub mod heartbeat;
pub mod roles;
pub mod sessions;
pub mod web;

use axum::Router;
use axum::routing::get;

use crate::state::AppState;

/// Build the API sub-router (everything under `/api`).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::get_health))
        .merge(sessions::router())
        .merge(chat::router())
        .merge(cron::router())
        .merge(heartbeat::router())
        .merge(roles::router())
        .merge(channels::router())
        .merge(attachments::router())
        .nest("/web", web::router())
        .with_state(state)
}
