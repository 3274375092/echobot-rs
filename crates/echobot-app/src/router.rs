//! Top-level router builder. Re-exports [`routers::router`] under a
//! stable name so the lib crate's public surface stays small.

use axum::Router;

use crate::routers;
use crate::state::AppState;

/// Build the combined API router for a given [`AppState`].
pub fn router(state: AppState) -> Router {
    routers::router(state)
}
