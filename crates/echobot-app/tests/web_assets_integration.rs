//! Integration test that verifies the embedded frontend assets serve
//! correctly under the real paths the index.html references.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use echobot_app::{create_app, runtime::AppRuntime};

fn build_test_runtime() -> Arc<AppRuntime> {
    std::env::set_var("LLM_API_KEY", "sk-test");
    std::env::set_var("LLM_MODEL", "gpt-4o-mini");
    std::env::set_var("LLM_BASE_URL", "https://api.openai.com/v1");
    let ctx = futures::executor::block_on(echobot_runtime::bootstrap::build_runtime_context(
        echobot_runtime::bootstrap::RuntimeOptions::default(),
        false,
    ))
    .expect("runtime context");
    Arc::new(AppRuntime::new(ctx, None, None, None, None))
}

#[test]
fn embedded_web_assets_directory_is_populated() {
    // Recursively count files in the embedded bundle. `Dir::files()`
    // only returns the top-level files, so walk into each subdirectory.
    fn count_recursive(dir: &include_dir::Dir) -> usize {
        let mut total = dir.files().count();
        for entry in dir.entries() {
            if let include_dir::DirEntry::Dir(d) = entry {
                total += count_recursive(d);
            }
        }
        total
    }
    let count = count_recursive(&echobot_app::create_app::WEB_ASSETS);
    assert!(
        count > 100,
        "expected the embedded web bundle to contain >100 files recursively, got {count}"
    );
}

#[test]
fn index_html_serves_at_root() {
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = rt.block_on(async {
        app.oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });
    assert_eq!(response.status(), StatusCode::OK);
    let body = rt.block_on(async {
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
    });
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("<title>"),
        "index.html should contain a <title> tag; got: {}",
        &html[..html.len().min(200)]
    );
}

#[test]
fn web_assets_serve_under_web_prefix() {
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // The real path the index.html references for CSS.
    let css_uri = "/web/assets/styles/index.css";
    let response = rt.block_on(async {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(css_uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    let status = response.status();
    let body = rt.block_on(async {
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
    });
    assert_eq!(
        status,
        StatusCode::OK,
        "{css_uri} returned {status} with body: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        !body.is_empty(),
        "{css_uri} returned an empty body"
    );
}

#[test]
fn web_assets_serve_vendor_js() {
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // pixi.min.js is the biggest vendored asset; if this serves we
    // know the whole vendor/ tree made it into the binary.
    let uri = "/web/assets/vendor/pixi.min.js";
    let response = rt.block_on(async {
        app.oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });
    assert_eq!(response.status(), StatusCode::OK);
    let body = rt.block_on(async {
        axum::body::to_bytes(response.into_body(), 200 * 1024 * 1024)
            .await
            .unwrap()
    });
    assert!(
        body.len() > 100_000,
        "pixi.min.js should be >100KB, got {} bytes",
        body.len()
    );
}

#[test]
fn web_assets_serve_main_app_js_module() {
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let uri = "/web/assets/app.js";
    let response = rt.block_on(async {
        app.oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });
    assert_eq!(response.status(), StatusCode::OK);
    let body = rt.block_on(async {
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
    });
    let js = String::from_utf8_lossy(&body);
    assert!(
        js.contains("import") || js.contains("export"),
        "app.js should be an ES module; got: {}",
        &js[..js.len().min(200)]
    );
}

#[test]
fn spa_fallback_serves_index_for_unknown_web_routes() {
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // /web/features/some/random/path should fall through to index.html.
    let response = rt.block_on(async {
        app.oneshot(
            Request::builder()
                .uri("/web/features/some/random/path")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });
    assert_eq!(response.status(), StatusCode::OK);
    let body = rt.block_on(async {
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
    });
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("<title>"), "SPA fallback should return index.html");
}
