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

#[test]
fn web_route_without_trailing_slash_serves_spa() {
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Browser hits `http://host:port/web` with no trailing slash.
    // The Python `create_app.py` serves the SPA shell here; the
    // Rust port must do the same or the user gets a 404.
    for uri in ["/web", "/web/"] {
        let response = rt.block_on(async {
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{uri} returned {}",
            response.status()
        );
        let body = rt.block_on(async {
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
        });
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("<title>"),
            "{uri} should serve index.html, got: {}",
            &html[..html.len().min(200)]
        );
    }
}

#[test]
fn tts_endpoint_reachable_at_api_web_tts() {
    // The frontend calls POST /api/web/tts. Verify the route is
    // mounted at that exact path so the browser's fetch() doesn't
    // 404 (which surfaces as "Failed to fetch" in JS).
    use axum::http::header;
    let app = create_app(build_test_runtime());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = rt.block_on(async {
        app.oneshot(
            Request::builder()
                .uri("/api/web/tts")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"text":"hi","provider":"edge"}"#))
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
    let body_str = String::from_utf8_lossy(&body);
    // We don't care whether the TTS provider itself succeeds — the
    // browser does. We only care that the route exists (so the
    // request isn't a 404 / network error).
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "/api/web/tts returned 404 (route missing or nested under wrong prefix); body: {body_str}"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "/api/web/tts returned 405 (route registered but for a different method); body: {body_str}"
    );
    // Print the actual status for visibility in test output.
    eprintln!("/api/web/tts POST -> {status} body[0..200]={}", &body_str[..body_str.len().min(200)]);
}

#[test]
fn tts_endpoint_returns_audio_with_stub_provider() {
    // End-to-end TTS flow: build a real AppRuntime with a stub
    // TTS provider, POST to /api/web/tts, and verify the audio
    // bytes come back. This catches the "Failed to fetch" case
    // end-to-end (the CORS layer, the timeout, the router, the
    // web_console, and the provider all wired up correctly).
    use axum::http::header;
    use echobot_app::runtime::AppRuntime;
    use echobot_tts::{
        base::{TtsAudio, TtsError, TtsProvider},
        service::TtsService,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use async_trait::async_trait;

    struct StubProvider;
    #[async_trait]
    impl TtsProvider for StubProvider {
        fn name(&self) -> &str { "stub" }
        fn label(&self) -> &str { "Stub TTS" }
        fn default_voice(&self) -> &str { "alloy" }
        async fn synthesize(
            &self,
            _text: &str,
            _options: Option<&echobot_tts::base::TtsSynthesisOptions>,
        ) -> Result<TtsAudio, TtsError> {
            Ok(TtsAudio {
                audio_bytes: b"FAKE_WAV_BYTES".to_vec(),
                content_type: "audio/wav".to_string(),
                file_extension: "wav".to_string(),
                provider: "stub".to_string(),
                voice: "alloy".to_string(),
            })
        }
    }

    let mut providers: BTreeMap<String, Arc<dyn TtsProvider>> = BTreeMap::new();
    providers.insert("stub".to_string(), Arc::new(StubProvider));
    let tts_service = Arc::new(TtsService::new(providers, "stub").expect("service builds"));

    let rt_handle = tokio::runtime::Handle::try_current();
    drop(rt_handle);
    let ctx = futures::executor::block_on(echobot_runtime::bootstrap::build_runtime_context(
        echobot_runtime::bootstrap::RuntimeOptions::default(),
        false,
    ))
    .expect("runtime context");
    let runtime = Arc::new(AppRuntime::new(
        ctx,
        None,
        None,
        Some(tts_service),
        None,
    ));

    let app = create_app(runtime);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let response = rt.block_on(async {
        app.oneshot(
            Request::builder()
                .uri("/api/web/tts")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"text":"hi","provider":"stub"}"#))
                .unwrap(),
        )
        .await
        .unwrap()
    });
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = rt.block_on(async {
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
    });
    let body_str = String::from_utf8_lossy(&body);
    eprintln!("/api/web/tts (stub provider) -> {status} body[0..200]={}", &body_str[..body_str.len().min(200)]);
    assert_eq!(
        status,
        StatusCode::OK,
        "TTS endpoint with stub provider returned {status}: {body_str}"
    );
    assert_eq!(
        &body[..],
        b"FAKE_WAV_BYTES",
        "TTS endpoint should return the audio bytes from the stub provider"
    );
    assert_eq!(
        content_type.as_deref(),
        Some("audio/wav"),
        "TTS endpoint should advertise audio/wav content type"
    );
}
