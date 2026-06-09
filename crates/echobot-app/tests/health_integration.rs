//! Integration test for `GET /api/health`.
//!
//! Mirrors the smoke test the Python app ships: build a minimal
//! `AppRuntime` (stub TTS / ASR, no coordinator), build the router,
//! and hit `/api/health` with a request body. Verifies the response
//! is 200 and the JSON body has the expected shape.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use echobot_app::{create_app, AppRuntime};

/// Build a minimal `AppRuntime` for the integration test. We use
/// `build_minimal_tts_service` and a no-network ASR service so the
/// runtime constructor doesn't try to reach out to a VAD provider.
async fn build_test_runtime() -> Arc<AppRuntime> {
    std::env::set_var("LLM_API_KEY", "sk-test");
    std::env::set_var("LLM_MODEL", "gpt-4o-mini");
    std::env::set_var("LLM_BASE_URL", "https://api.openai.com/v1");
    // Build the runtime with no TTS / ASR services so the test does
    // not need any network or model files. The runtime constructor
    // treats `None` as a stub, which is exactly what the integration
    // test wants for `/api/health`.
    let runtime_context = echobot_runtime::bootstrap::build_runtime_context(
        echobot_runtime::bootstrap::RuntimeOptions::default(),
        false,
    )
    .await
    .expect("runtime context");
    Arc::new(AppRuntime::new(runtime_context, None, None, None, None))
}

#[tokio::test]
async fn health_endpoint_returns_200() {
    let runtime = build_test_runtime().await;
    let app = create_app(runtime);

    let request = Request::builder()
        .uri("/api/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "ok");
    assert!(value["workspace"].is_string());
    assert!(value["current_session"].is_string());
    assert!(value["current_role"].is_string());
    assert!(value["channels"].is_object());
    assert!(value["bus"].is_object());
    assert!(value["jobs"].is_object());
}
