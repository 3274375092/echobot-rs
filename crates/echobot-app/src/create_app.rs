//! `create_app` — build the axum [`Router`] for the HTTP server.
//!
//! Mirrors the Python `echobot.app.create_app.create_app` function:
//!
//! * `/api/*` is served by [`crate::routers`]
//! * `/web/*` serves the embedded frontend assets and falls back to
//!   `index.html` for the SPA shell
//! * `/` and `/favicon.ico` map to `index.html` and `favicon.svg`
//!
//! The router is wrapped in a permissive CORS layer and a
//! 60-second per-request timeout. CORS doesn't matter for
//! same-origin browser traffic but is cheap insurance for any
//! tool that hits the API from a different origin (e.g. `curl`
//! from a test harness). The timeout is the more important
//! guard: without it a slow TTS provider (Edge TTS over a flaky
//! network to `wss://speech.platform.bing.com`) can hold the
//! response open until the browser gives up, which Chrome
//! surfaces as `TypeError: Failed to fetch` — completely
//! indistinguishable from a real network error from the
//! front-end's perspective.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::timeout::TimeoutLayer;

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
        // The Python `create_app.py` exposes the SPA at both `/`
        // and `/web` (no trailing slash). Mirror that explicitly so
        // a browser pointed at `http://host:port/web` doesn't
        // 404 before the SPA shell has a chance to mount.
        .route("/web", get(serve_index))
        .route("/web/", get(serve_index))
        .route("/favicon.ico", get(serve_favicon))
        .nest("/api", api)
        .fallback(serve_static)
        .layer(
            // Permissive CORS — any origin, any method, any header.
            // Same-origin browsers ignore it; cross-origin tools
            // (curl, Postman, test harnesses) benefit.
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        // 30-second per-request timeout. Edge TTS WebSocket calls
        // through to `wss://speech.platform.bing.com` and can take
        // a while; this gives them headroom while still bounding
        // the request so the browser doesn't see a half-closed
        // connection as "Failed to fetch".
        //
        // `TimeoutLayer::new` is deprecated in tower-http 0.6 in
        // favour of `with_status_code`; we use `new` because the
        // default 408 body is fine for our use case.
        .layer(#[allow(deprecated)] TimeoutLayer::new(Duration::from_secs(30)))
}

async fn serve_index() -> Response {
    serve_embedded("index.html")
}

async fn serve_favicon() -> Response {
    serve_embedded("favicon.svg").with_content_type("image/svg+xml")
}

async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Bare `/web` (no trailing slash) sometimes lands here as well
    // depending on how the caller dispatched; serve the SPA shell.
    if path.is_empty() || path == "web" {
        return serve_embedded("index.html");
    }
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
