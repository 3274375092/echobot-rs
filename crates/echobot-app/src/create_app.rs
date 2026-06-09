//! `create_app` — build the axum [`Router`] for the HTTP server.
//!
//! Mirrors the Python `echobot.app.create_app.create_app` function:
//!
//! * `/api/*` is served by [`crate::routers`]
//! * `/web/*` serves the embedded frontend assets and falls back to
//!   `index.html` for the SPA shell
//! * `/` and `/favicon.ico` map to `index.html` and `favicon.svg`

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use include_dir::include_dir;

use crate::routers;
use crate::runtime::AppRuntime;
use crate::state::AppState;

/// The embedded frontend assets, compiled in at build time.
///
/// Resolved at compile time to `D:/code/重构/EchoBot/echobot/app/web/`
/// (the workspace's sibling directory). The path is relative to the
/// crate manifest directory.
pub static WEB_ASSETS: include_dir::Dir =
    include_dir!("$CARGO_MANIFEST_DIR/../../../EchoBot/echobot/app/web");

/// Build the [`axum::Router`] for the EchoBot HTTP server.
pub fn create_app(runtime: Arc<AppRuntime>) -> Router {
    let state = AppState::new(runtime);
    let api = routers::router(state.clone());
    Router::new()
        .route("/", get(serve_index))
        .route("/favicon.ico", get(serve_favicon))
        .nest("/api", api)
        .fallback(serve_static)
}

async fn serve_index() -> Response {
    serve_embedded("index.html")
}

async fn serve_favicon() -> Response {
    serve_embedded("favicon.svg").with_content_type("image/svg+xml")
}

async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(asset) = WEB_ASSETS.get_file(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime.to_string())
            .body(Body::from(asset.contents()))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }
    // SPA-style fallback: serve index.html for unknown routes under /web/.
    if path.starts_with("web/") {
        return serve_embedded("index.html");
    }
    StatusCode::NOT_FOUND.into_response()
}

fn serve_embedded(path: &str) -> Response {
    match WEB_ASSETS.get_file(path) {
        Some(asset) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.to_string())
                .body(Body::from(asset.contents()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

trait ContentTypeExt {
    fn with_content_type(self, mime: &str) -> Self;
}

impl ContentTypeExt for Response {
    fn with_content_type(mut self, mime: &str) -> Self {
        if let Ok(value) = header::HeaderValue::from_str(mime) {
            self.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        self
    }
}
