//! `echobot-desktop` — Tauri-based desktop shell for EchoBot.
//!
//! On launch, this binary:
//! 1. Loads `.env` from the current working directory (or copies
//!    `.env.example` to `.env` on first run).
//! 2. Assembles the shared EchoBot runtime (LLM provider, sessions,
//!    tools, skills, scheduling, coordinator).
//! 3. Starts the axum HTTP server in a background tokio task on
//!    127.0.0.1:8765.
//! 4. Opens a Tauri webview window pointing at the server's
//!    `/web` SPA entrypoint.
//! 5. Closes the server on window-close.
//!
//! Build:
//!   cargo build --release -p echobot-desktop
//!
//! Run:
//!   ./target/release/echobot-desktop.exe
//!
//! If the window doesn't appear, check `.echobot/desktop.log` for
//! the panic message. With `windows_subsystem = "windows"` a
//! release build has no console, so without the log file you'd be
//! flying blind.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

use echobot_app::{create_app, runtime::AppRuntime};

mod first_run;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    install_panic_hook();
    init_rustls();
    init_tracing();

    if let Err(e) = run().await {
        // With `windows_subsystem = "windows"` the release build has
        // no console — anything we `eprintln!` here goes nowhere. Log
        // the error to a file in the workspace so the user can find
        // it, and surface a MessageBox via Win32 if we're on Windows.
        log_fatal(&format!("fatal: {e:?}"));
        show_message_box(&format!("EchoBot failed to start:\n\n{e}\n\nSee .echobot/desktop.log for details."));
        return Err(e);
    }
    Ok(())
}

async fn run() -> Result<()> {
    // Resolve the workspace. When the user double-clicks the
    // `.exe` in File Explorer, the cwd is not the repo root —
    // we walk up from the executable looking for `.env` /
    // `.env.example` to find it.
    let workspace = first_run::resolve_workspace();
    if let Err(e) = std::env::set_current_dir(&workspace) {
        warn!(error = %e, workspace = %workspace.display(), "failed to switch cwd to workspace; continuing from current dir");
    } else {
        info!(workspace = %workspace.display(), "using workspace");
    }

    // First-run setup: copy .env.example to .env if .env is missing.
    if let Err(e) = first_run::ensure_env_file(&workspace).await {
        warn!(error = %e, "first-run .env setup failed (continuing)");
    }

    // Load .env (if present) into the process environment.
    let env_path = workspace.join(".env");
    if env_path.is_file() {
        if let Err(e) = dotenvy::from_path(&env_path) {
            warn!(error = %e, path = %env_path.display(), "failed to load .env");
        } else {
            info!(path = %env_path.display(), "loaded .env");
        }
    } else {
        warn!(path = %env_path.display(), ".env not found; LLM provider may fail to construct");
    }

    // Assemble the shared runtime. The Tauri shell reuses the
    // same FullRuntimeContext the `chat` and `app` CLI subcommands
    // use; it adds the Tauri webview on top.
    let runtime_options = echobot_runtime::bootstrap::RuntimeOptions::default();
    let full = match echobot_cli::runtime_assembly::assemble_runtime(
        runtime_options,
        true,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(error = %e, "failed to assemble runtime; check your .env (LLM_API_KEY, LLM_MODEL, LLM_BASE_URL)");
            return Err(e);
        }
    };
    let workspace = full.runtime.workspace.clone();

    // Build the TTS / ASR services and the AppRuntime.
    let tts_service = Arc::new(echobot_tts::factory::build_default_tts_service(Some(&workspace)));
    let asr_service = Arc::new(echobot_asr::factory::build_default_asr_service(&workspace));
    let coordinator = full.coordinator.clone();
    let role_registry = full.role_registry.clone();
    let mut app_runtime = AppRuntime::new(
        full.runtime,
        Some(coordinator),
        Some(role_registry),
        Some(tts_service),
        Some(asr_service),
    );
    app_runtime.start().await;

    // Bind the in-process HTTP server to 127.0.0.1:8765.
    let bind_addr: std::net::SocketAddr = "127.0.0.1:8765".parse().unwrap();
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("failed to bind {bind_addr} (is another process using the port?)"))?;
    let local_addr = listener.local_addr().unwrap_or(bind_addr);
    let web_url = format!("http://{}/web", local_addr);
    info!(addr = %local_addr, "EchoBot HTTP server starting (in-process)");

    // Spawn the server in a background task. Tauri keeps the
    // process alive; the server is shut down when the Tauri
    // event loop ends.
    let runtime_arc = Arc::new(app_runtime);
    let server_task = tokio::spawn(async move {
        let router = create_app(runtime_arc);
        if let Err(e) = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        {
            error!(error = %e, "axum::serve returned an error");
        }
        info!("EchoBot HTTP server stopped");
    });

    // Build and run the Tauri app. The window loads the running
    // server's SPA URL.
    let url_for_window = web_url.clone();
    let url_for_status = web_url.clone();
    tauri::Builder::<tauri::Wry>::default()
        .setup(move |app| {
            // Open the main window pointing at the running server.
            let _ = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(url_for_window.parse().unwrap()),
            )
            .title("EchoBot")
            .inner_size(1280.0, 800.0)
            .min_inner_size(800.0, 600.0)
            .resizable(true)
            .center()
            .build();
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                if window.label() == "main" {
                    info!("main window closing; shutting down HTTP server");
                    server_task.abort();
                }
            }
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("Tauri runtime error: {e}"))?;

    info!("EchoBot desktop exiting; was serving at {}", url_for_status);
    Ok(())
}

/// Install a panic hook that writes to `.echobot/desktop.log` in
/// addition to the default stderr hook. Without this, a panic in
/// a release build (which has no console) is invisible.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("PANIC: {info}\n");
        log_fatal(&msg);
        default_hook(info);
    }));
}

/// Append a line to `.echobot/desktop.log`, creating the directory
/// if needed. Best-effort: never panics itself.
fn log_fatal(msg: &str) {
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = workspace.join(".echobot");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("desktop.log");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", msg);
    }
}

/// Show a native message box on Windows (no-op elsewhere). Uses
/// the `MessageBoxW` Win32 API so it works even with no console.
#[cfg(target_os = "windows")]
fn show_message_box(msg: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            utype: u32,
        ) -> i32;
    }
    let wide_msg: Vec<u16> = OsStr::new(msg).encode_wide().chain(Some(0)).collect();
    let wide_caption: Vec<u16> =
        OsStr::new("EchoBot").encode_wide().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide_msg.as_ptr(),
            wide_caption.as_ptr(),
            0x10, // MB_ICONERROR
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn show_message_box(_msg: &str) {
    // No-op on non-Windows targets; the panic hook's log file is
    // the only diagnostic on those platforms.
}

fn init_rustls() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("failed to install rustls aws-lc-rs crypto provider");
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("echobot=info,warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// Resolve the workspace root for first-run env setup. Falls back
/// to the current directory if the binary was launched without an
/// argument pointing at a workspace.
pub fn resolve_workspace() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// True if the given path exists as a file.
pub fn path_is_file(path: &Path) -> bool {
    path.is_file()
}
