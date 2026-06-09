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
/// `build.rs` snapshots the Python EchoBot web console's
/// `app/web/` directory into `web-assets/` inside this crate at
/// build time. `include_dir!` then embeds the local copy. This
/// avoids the `..`-escape path that `include_dir!` 0.7 mishandles
/// (the macro silently produces an empty bundle when the path
/// walks up out of the crate's manifest dir).
pub static WEB_ASSETS: include_dir::Dir =
    include_dir!("$CARGO_MANIFEST_DIR/web-assets");

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
    // The Python `create_app.py` mounts the static dir at
    // `/web/assets/`, so HTTP paths look like `/web/assets/X` while
    // the actual files in the bundle live at the top level (or under
    // `vendor/`, `styles/`, etc.) without the `assets/` segment.
    // Strip both prefixes before looking the file up in the embedded
    // `Dir`. The `include_dir!` macro stores paths relative to the
    // bundle root (`web-assets/`), with no prefix.
    let lookup = path
        .strip_prefix("web/assets/")
        .or_else(|| path.strip_prefix("web/"))
        .unwrap_or(path);
    if let Some(asset) = WEB_ASSETS.get_file(lookup) {
        let mime = mime_guess::from_path(lookup).first_or_octet_stream();
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
