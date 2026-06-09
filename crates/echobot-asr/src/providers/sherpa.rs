//! Sherpa-onnx SenseVoice ASR provider.
//!
//! This module is the Rust counterpart of the Python
//! `echobot.asr.providers.sherpa_sense_voice.SherpaSenseVoiceASRProvider`.
//! It is split into two halves behind a single cargo feature so the default
//! build does not pull in the sherpa-onnx C library:
//!
//! * **Default (no `sherpa-rs` feature)** — a stub that mirrors the
//!   configuration surface of the real provider but returns
//!   `AsrError::NotImplemented` from every transcription call. This is what
//!   v1 ships with by default.
//! * **`sherpa-rs` feature enabled** — a real
//!   [`sherpa_rs::sense_voice::SenseVoiceRecognizer`] is constructed on
//!   demand inside `tokio::task::spawn_blocking`, lazy-downloading the
//!   SenseVoice model bundle on first use.
//!
//! The on-disk layout matches the Python port:
//!
//! ```text
//! {model_root_dir}/
//!   model.int8.onnx
//!   tokens.txt
//! ```
//!
//! By default `model_root_dir` resolves to
//! `{workspace}/.echobot/models/sherpa-sense-voice`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::base::{AsrError, AsrProvider, Result};
use crate::models::{AsrConfig, AsrResult, ProviderStatusSnapshot, TranscriptionResult};

/// Default URL for the SenseVoice model bundle used by the Python port.
///
/// Same archive the Python `SenseVoiceModelManager` downloads — it contains
/// `model.int8.onnx` and `tokens.txt` at the top level of the extracted
/// directory.
pub const DEFAULT_SENSE_VOICE_MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/\
     sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2";

/// Sub-directory (relative to the workspace) where the SenseVoice model
/// bundle is cached when the user does not pin `ECHOBOT_ASR_SHERPA_MODEL_DIR`.
pub const DEFAULT_SENSE_VOICE_MODEL_SUBDIR: &str = ".echobot/models/sherpa-sense-voice";

/// Name of the on-disk SenseVoice model weights file inside the model root.
pub const SENSE_VOICE_MODEL_FILE: &str = "model.int8.onnx";

/// Name of the SenseVoice tokens file inside the model root.
pub const SENSE_VOICE_TOKENS_FILE: &str = "tokens.txt";

/// Configuration for the SenseVoice provider.
///
/// The same struct is used for both the stub (default) and the real
/// `sherpa-rs`-backed implementation, so the public surface of `echobot-asr`
/// does not change between feature states.
#[derive(Debug, Clone)]
pub struct SherpaSenseVoiceConfig {
    /// Target sample rate in Hz. SenseVoice is trained on 16 kHz mono.
    pub sample_rate: u32,
    /// If true, the provider auto-downloads the model bundle on first use.
    pub auto_download: bool,
    /// Optional override for the on-disk model root directory.
    pub model_root_dir: Option<PathBuf>,
    /// ONNX execution provider (`"cpu"`, `"cuda"`, `"coreml"`, …).
    pub execution_provider: String,
    /// Number of inference threads. Must be ≥ 1.
    pub num_threads: u32,
    /// Language hint (`"auto"`, `"zh"`, `"en"`, …).
    pub language: String,
    /// Whether to apply inverse text normalization.
    pub use_itn: bool,
    /// URL to download the model from when `auto_download` is true.
    pub model_url: String,
    /// Timeout in seconds for the (optional) model download.
    pub download_timeout_seconds: f64,
}

impl Default for SherpaSenseVoiceConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            auto_download: true,
            model_root_dir: None,
            execution_provider: "cpu".to_string(),
            num_threads: 2,
            language: "auto".to_string(),
            use_itn: false,
            model_url: DEFAULT_SENSE_VOICE_MODEL_URL.to_string(),
            download_timeout_seconds: 600.0,
        }
    }
}

impl SherpaSenseVoiceConfig {
    /// Resolve the on-disk model root directory, anchoring relative paths at
    /// `workspace` when necessary. Always returns an absolute path.
    pub fn resolved_model_root(&self, workspace: &std::path::Path) -> PathBuf {
        match &self.model_root_dir {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => workspace.join(path),
            None => workspace.join(DEFAULT_SENSE_VOICE_MODEL_SUBDIR),
        }
    }
}

// ===========================================================================
// Stub provider — used when the `sherpa-rs` feature is OFF.
// ===========================================================================

#[cfg(not(feature = "sherpa-rs"))]
mod stub {
    use super::*;

    /// SenseVoice provider **stub** — used when the `sherpa-rs` feature is
    /// off.
    ///
    /// Mirrors the configuration shape of the real provider so the v1
    /// binary compiles and runs, but every transcription call returns
    /// `AsrError::NotImplemented` and the status snapshot reports
    /// `unavailable`.
    #[derive(Debug)]
    pub struct SherpaSenseVoiceProvider {
        config: SherpaSenseVoiceConfig,
        /// Tracks whether `on_startup` has been called.
        startup_called: Arc<Mutex<bool>>,
    }

    impl SherpaSenseVoiceProvider {
        /// Build a stub provider with the given configuration.
        pub fn new(config: SherpaSenseVoiceConfig) -> Result<Self> {
            if config.sample_rate == 0 {
                return Err(AsrError::Config(
                    "ASR sample_rate must be positive".to_string(),
                ));
            }
            Ok(Self {
                config,
                startup_called: Arc::new(Mutex::new(false)),
            })
        }

        /// Stable human-readable detail string for the not-implemented state.
        pub fn not_implemented_detail(&self) -> String {
            "sherpa-onnx SenseVoice is not wired in v1 — rebuild with --features sherpa-rs"
                .to_string()
        }
    }

    #[async_trait]
    impl AsrProvider for SherpaSenseVoiceProvider {
        fn name(&self) -> &str {
            "sherpa-sense-voice"
        }

        fn label(&self) -> &str {
            "Sherpa SenseVoice"
        }

        async fn on_startup(&self) -> Result<()> {
            *self.startup_called.lock() = true;
            Ok(())
        }

        async fn close(&self) -> Result<()> {
            Ok(())
        }

        async fn status_snapshot(&self) -> Result<ProviderStatusSnapshot> {
            let startup_called = *self.startup_called.lock();
            let detail = if startup_called {
                self.not_implemented_detail()
            } else {
                format!(
                    "{} (provider not yet started; call on_startup() to see the not-implemented detail)",
                    self.not_implemented_detail()
                )
            };
            Ok(ProviderStatusSnapshot {
                kind: "asr".to_string(),
                name: self.name().to_string(),
                label: self.label().to_string(),
                selected: false,
                available: false,
                state: "unavailable".to_string(),
                detail,
                resource_directory: self
                    .config
                    .model_root_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            })
        }

        async fn transcribe_samples(&self, _samples: &[f32]) -> Result<TranscriptionResult> {
            Err(AsrError::NotImplemented(self.not_implemented_detail()))
        }

        async fn transcribe_with_config(
            &self,
            _samples: &[f32],
            _config: &AsrConfig,
        ) -> Result<AsrResult> {
            Err(AsrError::NotImplemented(self.not_implemented_detail()))
        }
    }
}

// ===========================================================================
// Real sherpa-rs-backed provider — used when the `sherpa-rs` feature is ON.
// ===========================================================================

#[cfg(feature = "sherpa-rs")]
mod real {
    use super::*;

    use std::path::{Path, PathBuf};

    use sherpa_rs::sense_voice::{SenseVoiceConfig, SenseVoiceRecognizer};
    use tokio::sync::Mutex as AsyncMutex;

    /// Coarse provider state machine — mirrors the Python one.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ProviderState {
        /// Model files are on disk and the recognizer is built.
        Ready,
        /// Model files are missing; download in progress or not started.
        Missing,
        /// Download / extract is in progress.
        Downloading,
        /// A previous attempt failed; `detail` carries the reason.
        Error,
        /// Provider is being shut down.
        Unavailable,
    }

    /// Real SenseVoice provider backed by `sherpa-rs`.
    #[derive(Debug)]
    pub struct SherpaSenseVoiceProvider {
        config: SherpaSenseVoiceConfig,
        /// Coarse state — protected by a sync mutex.
        state: Arc<parking_lot::Mutex<ProviderState>>,
        /// Detailed status message shown to the user.
        detail: Arc<parking_lot::Mutex<String>>,
        /// The recognizer once it has been built. `None` until first use.
        runtime: Arc<AsyncMutex<Option<SenseVoiceRecognizer>>>,
        /// Background download task (if any).
        prepare_task: Arc<AsyncMutex<Option<tokio::task::JoinHandle<()>>>>,
    }

    impl SherpaSenseVoiceProvider {
        /// Build a real provider with the given configuration. The
        /// recognizer is *not* constructed here — it is built lazily on
        /// first transcribe, after the model files are on disk.
        pub fn new(config: SherpaSenseVoiceConfig) -> Result<Self> {
            if config.sample_rate == 0 {
                return Err(AsrError::Config(
                    "ASR sample_rate must be positive".to_string(),
                ));
            }
            if config.num_threads == 0 {
                return Err(AsrError::Config(
                    "ASR num_threads must be >= 1".to_string(),
                ));
            }
            Ok(Self {
                config,
                state: Arc::new(parking_lot::Mutex::new(ProviderState::Missing)),
                detail: Arc::new(parking_lot::Mutex::new(String::new())),
                runtime: Arc::new(AsyncMutex::new(None)),
                prepare_task: Arc::new(AsyncMutex::new(None)),
            })
        }

        fn set_state(&self, state: ProviderState, detail: impl Into<String>) {
            *self.state.lock() = state;
            *self.detail.lock() = detail.into();
        }

        fn snapshot_state(&self) -> (ProviderState, String) {
            (self.state.lock().clone(), self.detail.lock().clone())
        }

        /// Check whether the model files are present on disk and update the
        /// state accordingly.
        fn refresh_state_from_disk(&self, workspace: &Path) {
            let root_dir = self.config.resolved_model_root(workspace);
            let model_file = root_dir.join(SENSE_VOICE_MODEL_FILE);
            let tokens_file = root_dir.join(SENSE_VOICE_TOKENS_FILE);
            if model_file.is_file() && tokens_file.is_file() {
                self.set_state(ProviderState::Ready, "SenseVoice 已就绪。".to_string());
            } else {
                let mut missing = Vec::new();
                if !model_file.is_file() {
                    missing.push(SENSE_VOICE_MODEL_FILE);
                }
                if !tokens_file.is_file() {
                    missing.push(SENSE_VOICE_TOKENS_FILE);
                }
                let detail = if self.config.auto_download {
                    format!(
                        "SenseVoice 模型文件缺失，等待自动下载: {}",
                        missing.join(", ")
                    )
                } else {
                    format!("缺少 SenseVoice 模型文件: {}", missing.join(", "))
                };
                self.set_state(ProviderState::Missing, detail);
            }
        }

        /// Kick off the model download (if needed). The download runs on a
        /// blocking thread because the Python implementation uses
        /// `asyncio.to_thread` for the same reason — fetching a ~hundreds
        /// of MB tarball is too heavy for the async runtime.
        async fn maybe_start_prepare(&self, workspace: PathBuf) {
            let mut guard = self.prepare_task.lock().await;
            // Re-check under the lock: another task may have finished.
            self.refresh_state_from_disk(&workspace);
            if *self.state.lock() == ProviderState::Ready {
                return;
            }
            if !self.config.auto_download {
                return;
            }
            if let Some(handle) = guard.as_ref() {
                if !handle.is_finished() {
                    return;
                }
            }

            let config = self.config.clone();
            let state = self.state.clone();
            let detail = self.detail.clone();
            let runtime = self.runtime.clone();
            let root_dir = config.resolved_model_root(&workspace);
            *state.lock() = ProviderState::Downloading;
            *detail.lock() = "正在自动下载 SenseVoice 模型，请稍候。".to_string();
            let handle = tokio::task::spawn_blocking(move || {
                if let Err(e) = download_and_extract_blocking(&config, &root_dir) {
                    *state.lock() = ProviderState::Error;
                    *detail.lock() = format!("SenseVoice 模型下载失败: {e}");
                    return;
                }
                // Try to build the recognizer eagerly so the first
                // transcribe is fast.
                let build = build_recognizer_blocking(&config, &root_dir);
                let mut runtime_lock = runtime.blocking_lock();
                match build {
                    Ok(recognizer) => {
                        *runtime_lock = Some(recognizer);
                        *state.lock() = ProviderState::Ready;
                        *detail.lock() = "SenseVoice 已就绪。".to_string();
                    }
                    Err(e) => {
                        *state.lock() = ProviderState::Error;
                        *detail.lock() = format!("SenseVoice 初始化失败: {e}");
                    }
                }
            });
            *guard = Some(handle);
            // The handle is held by `guard`; dropping the guard here would
            // race with concurrent calls. We intentionally keep the lock
            // for the rest of this function — `maybe_start_prepare` is
            // short and idempotent.
            drop(guard);
        }

        async fn ensure_runtime_loaded(&self, workspace: &Path) -> std::result::Result<(), AsrError> {
            // Step 1: trigger the download if we are still missing.
            self.maybe_start_prepare(workspace.to_path_buf()).await;
            // Step 2: if a prepare task exists, wait for it to finish.
            {
                let mut guard = self.prepare_task.lock().await;
                if let Some(handle) = guard.as_mut() {
                    if !handle.is_finished() {
                        let _ = (&mut *handle).await;
                    }
                    *guard = None;
                }
            }
            // Step 3: refresh from disk in case the task changed state.
            self.refresh_state_from_disk(workspace);
            match *self.state.lock() {
                ProviderState::Ready => {}
                ProviderState::Downloading => {
                    return Err(AsrError::NotReady(
                        "SenseVoice model is still downloading".to_string(),
                    ));
                }
                ProviderState::Missing => {
                    return Err(AsrError::NotReady(
                        "SenseVoice model files are missing (auto-download disabled?)".to_string(),
                    ));
                }
                ProviderState::Error => {
                    let detail = self.detail.lock().clone();
                    return Err(AsrError::Provider(detail));
                }
                ProviderState::Unavailable => {
                    return Err(AsrError::NotReady("SenseVoice unavailable".to_string()));
                }
            }
            // Step 4: build the recognizer if we have not yet done so.
            let mut guard = self.runtime.lock().await;
            if guard.is_none() {
                let config = self.config.clone();
                let root_dir = config.resolved_model_root(workspace);
                let runtime = tokio::task::spawn_blocking(move || {
                    build_recognizer_blocking(&config, &root_dir)
                })
                .await
                .map_err(|e| AsrError::Internal(format!("join error: {e}")))?;
                *guard = Some(runtime?);
            }
            Ok(())
        }

        async fn transcribe_blocking(
            &self,
            samples: Vec<f32>,
        ) -> std::result::Result<TranscriptionResult, AsrError> {
            let runtime = self.runtime.clone();
            let sample_rate = self.config.sample_rate;
            tokio::task::spawn_blocking(move || {
                let mut guard = runtime.blocking_lock();
                let runtime = guard
                    .as_mut()
                    .ok_or_else(|| AsrError::NotReady("recognizer not loaded".to_string()))?;
                let r = runtime.transcribe(sample_rate, &samples);
                Ok::<_, AsrError>(TranscriptionResult {
                    text: r.text,
                    language: r.lang,
                })
            })
            .await
            .map_err(|e| AsrError::Internal(format!("join error: {e}")))?
        }
    }

    #[async_trait]
    impl AsrProvider for SherpaSenseVoiceProvider {
        fn name(&self) -> &str {
            "sherpa-sense-voice"
        }

        fn label(&self) -> &str {
            "Sherpa SenseVoice"
        }

        async fn on_startup(&self) -> Result<()> {
            // Refresh from disk in case files are already present.
            let workspace = std::env::current_dir()
                .map_err(|e| AsrError::Internal(format!("current_dir: {e}")))?;
            self.refresh_state_from_disk(&workspace);
            self.maybe_start_prepare(workspace).await;
            Ok(())
        }

        async fn close(&self) -> Result<()> {
            let mut guard = self.prepare_task.lock().await;
            if let Some(handle) = guard.as_mut() {
                if !handle.is_finished() {
                    handle.abort();
                    let _ = (&mut *handle).await;
                }
            }
            *guard = None;
            *self.runtime.lock().await = None;
            self.set_state(ProviderState::Unavailable, "closed".to_string());
            Ok(())
        }

        async fn status_snapshot(&self) -> Result<ProviderStatusSnapshot> {
            let workspace = std::env::current_dir()
                .map_err(|e| AsrError::Internal(format!("current_dir: {e}")))?;
            self.refresh_state_from_disk(&workspace);
            let (state, detail) = self.snapshot_state();
            let (state_str, available) = match state {
                ProviderState::Ready => ("ready", true),
                ProviderState::Downloading => ("downloading", false),
                ProviderState::Missing => ("missing", false),
                ProviderState::Error => ("error", false),
                ProviderState::Unavailable => ("unavailable", false),
            };
            Ok(ProviderStatusSnapshot {
                kind: "asr".to_string(),
                name: self.name().to_string(),
                label: self.label().to_string(),
                selected: false,
                available,
                state: state_str.to_string(),
                detail,
                resource_directory: self
                    .config
                    .resolved_model_root(&workspace)
                    .display()
                    .to_string(),
            })
        }

        async fn transcribe_samples(&self, samples: &[f32]) -> Result<TranscriptionResult> {
            if samples.is_empty() {
                return Ok(TranscriptionResult::default());
            }
            let workspace = std::env::current_dir()
                .map_err(|e| AsrError::Internal(format!("current_dir: {e}")))?;
            self.ensure_runtime_loaded(&workspace).await?;
            self.transcribe_blocking(samples.to_vec()).await
        }

        async fn transcribe_with_config(
            &self,
            samples: &[f32],
            _config: &AsrConfig,
        ) -> Result<AsrResult> {
            let result = self.transcribe_samples(samples).await?;
            Ok(AsrResult::from_transcription(result))
        }
    }

    /// Build a `SenseVoiceRecognizer` from the model files under
    /// `root_dir`. Blocking; meant to be called from
    /// `tokio::task::spawn_blocking`.
    fn build_recognizer_blocking(
        config: &SherpaSenseVoiceConfig,
        root_dir: &Path,
    ) -> std::result::Result<SenseVoiceRecognizer, AsrError> {
        let model_file = root_dir.join(SENSE_VOICE_MODEL_FILE);
        let tokens_file = root_dir.join(SENSE_VOICE_TOKENS_FILE);
        if !model_file.is_file() {
            return Err(AsrError::Config(format!(
                "SenseVoice model file not found: {}",
                model_file.display()
            )));
        }
        if !tokens_file.is_file() {
            return Err(AsrError::Config(format!(
                "SenseVoice tokens file not found: {}",
                tokens_file.display()
            )));
        }
        let sv_config = SenseVoiceConfig {
            model: model_file.to_string_lossy().into_owned(),
            tokens: tokens_file.to_string_lossy().into_owned(),
            language: if config.language.is_empty() {
                "auto".to_string()
            } else {
                config.language.clone()
            },
            use_itn: config.use_itn,
            provider: Some(config.execution_provider.clone()),
            num_threads: Some(config.num_threads.max(1) as i32),
            debug: false,
        };
        SenseVoiceRecognizer::new(sv_config)
            .map_err(|e| AsrError::Provider(format!("SenseVoice init failed: {e}")))
    }

    /// Download the SenseVoice model bundle (a `.tar.bz2` archive) and
    /// extract `model.int8.onnx` + `tokens.txt` into `root_dir`. Blocking;
    /// meant to be called from `tokio::task::spawn_blocking`.
    fn download_and_extract_blocking(
        config: &SherpaSenseVoiceConfig,
        root_dir: &Path,
    ) -> std::result::Result<(), AsrError> {
        if root_dir.exists() {
            let model = root_dir.join(SENSE_VOICE_MODEL_FILE);
            let tokens = root_dir.join(SENSE_VOICE_TOKENS_FILE);
            if model.is_file() && tokens.is_file() {
                return Ok(());
            }
        }
        if let Some(parent) = root_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AsrError::Io(std::io::Error::other(format!(
                    "create_dir_all({}): {e}",
                    parent.display()
                )))
            })?;
        }
        let temp_dir = std::env::temp_dir().join(format!(
            "echobot_sense_voice_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "create_dir_all({}): {e}",
                temp_dir.display()
            )))
        })?;
        let archive_name = config
            .model_url
            .rsplit('/')
            .next()
            .unwrap_or("sherpa-sense-voice.tar.bz2")
            .to_string();
        let archive_path = temp_dir.join(&archive_name);
        let extract_dir = temp_dir.join("extract");
        std::fs::create_dir_all(&extract_dir).map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "create_dir_all({}): {e}",
                extract_dir.display()
            )))
        })?;
        // Download via reqwest (blocking) so we share the workspace's
        // reqwest version and inherit the same proxy/CA configuration.
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs_f64(
                config.download_timeout_seconds.max(30.0),
            ))
            .build()
            .map_err(|e| AsrError::Network(format!("build http client: {e}")))?;
        let mut response = client
            .get(&config.model_url)
            .send()
            .map_err(|e| AsrError::Network(format!("GET {}: {e}", config.model_url)))?;
        if !response.status().is_success() {
            return Err(AsrError::HttpStatus {
                status: response.status().as_u16(),
                detail: format!("failed to download {}", config.model_url),
            });
        }
        use std::io::Read as _;
        use std::io::Write as _;
        let mut file = std::fs::File::create(&archive_path)
            .map_err(|e| AsrError::Io(std::io::Error::other(format!("create archive: {e}"))))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = response
                .read(&mut buf)
                .map_err(|e| AsrError::Network(format!("read body: {e}")))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| AsrError::Io(std::io::Error::other(format!("write: {e}"))))?;
        }
        drop(file);
        // Extract — support both `.tar.bz2` and `.tar.gz` shapes via `tar -xaf`.
        let archive_path_str = archive_path.to_string_lossy().into_owned();
        let status = std::process::Command::new("tar")
            .arg("-xaf")
            .arg(&archive_path_str)
            .arg("-C")
            .arg(&extract_dir)
            .status()
            .map_err(|e| AsrError::Io(std::io::Error::other(format!("spawn tar: {e}"))))?;
        if !status.success() {
            return Err(AsrError::Provider(format!(
                "`tar -xaf` failed with status {status}"
            )));
        }
        // Locate the directory that contains the model + tokens files.
        let source_dir = find_dir_with_model_files(&extract_dir)?;
        // Move the two files into root_dir via a staging directory so a
        // crash mid-extract does not leave the model in a half-state.
        let staging = root_dir.with_file_name(format!(
            "{}.tmp",
            root_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("sherpa-sense-voice")
        ));
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        std::fs::create_dir_all(&staging).map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "create_dir_all({}): {e}",
                staging.display()
            )))
        })?;
        std::fs::copy(
            source_dir.join(SENSE_VOICE_MODEL_FILE),
            staging.join(SENSE_VOICE_MODEL_FILE),
        )
        .map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "copy {}: {e}",
                SENSE_VOICE_MODEL_FILE
            )))
        })?;
        std::fs::copy(
            source_dir.join(SENSE_VOICE_TOKENS_FILE),
            staging.join(SENSE_VOICE_TOKENS_FILE),
        )
        .map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "copy {}: {e}",
                SENSE_VOICE_TOKENS_FILE
            )))
        })?;
        if root_dir.exists() {
            let _ = std::fs::remove_dir_all(root_dir);
        }
        std::fs::rename(&staging, root_dir).map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "rename {} -> {}: {e}",
                staging.display(),
                root_dir.display()
            )))
        })?;
        // Best-effort cleanup of the temp dir.
        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    fn find_dir_with_model_files(root: &Path) -> std::result::Result<PathBuf, AsrError> {
        let model = SENSE_VOICE_MODEL_FILE;
        let tokens = SENSE_VOICE_TOKENS_FILE;
        if root.join(model).is_file() && root.join(tokens).is_file() {
            return Ok(root.to_path_buf());
        }
        let entries = std::fs::read_dir(root).map_err(|e| {
            AsrError::Io(std::io::Error::other(format!(
                "read_dir({}): {e}",
                root.display()
            )))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                AsrError::Io(std::io::Error::other(format!("read_dir entry: {e}")))
            })?;
            let path = entry.path();
            if path.is_dir() {
                if let Ok(found) = find_dir_with_model_files(&path) {
                    return Ok(found);
                }
            }
        }
        Err(AsrError::Provider(format!(
            "could not find SenseVoice model files under {}",
            root.display()
        )))
    }
}

// ---------------------------------------------------------------------------
// Re-exports: the rest of the crate always uses one stable name.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "sherpa-rs"))]
pub use stub::SherpaSenseVoiceProvider;

#[cfg(feature = "sherpa-rs")]
pub use real::SherpaSenseVoiceProvider;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_resolved_model_root_anchors_relative() {
        let cfg = SherpaSenseVoiceConfig {
            model_root_dir: Some(PathBuf::from("models/sv")),
            ..SherpaSenseVoiceConfig::default()
        };
        let workspace = std::path::Path::new("/tmp/ws");
        assert_eq!(
            cfg.resolved_model_root(workspace),
            PathBuf::from("/tmp/ws/models/sv")
        );
    }

    #[test]
    fn config_resolved_model_root_keeps_absolute() {
        let cfg = SherpaSenseVoiceConfig {
            model_root_dir: Some(PathBuf::from("/var/models/sv")),
            ..SherpaSenseVoiceConfig::default()
        };
        assert_eq!(
            cfg.resolved_model_root(std::path::Path::new("/tmp/ws")),
            PathBuf::from("/var/models/sv")
        );
    }

    #[test]
    fn config_resolved_model_root_defaults_to_workspace() {
        let cfg = SherpaSenseVoiceConfig::default();
        let workspace = std::path::Path::new("/tmp/ws");
        assert_eq!(
            cfg.resolved_model_root(workspace),
            PathBuf::from("/tmp/ws/.echobot/models/sherpa-sense-voice")
        );
    }

    #[test]
    fn new_rejects_zero_sample_rate() {
        let cfg = SherpaSenseVoiceConfig {
            sample_rate: 0,
            ..SherpaSenseVoiceConfig::default()
        };
        let err = SherpaSenseVoiceProvider::new(cfg).unwrap_err();
        assert!(matches!(err, AsrError::Config(_)));
    }

    #[test]
    fn provider_public_surface_is_stable() {
        // Construct a provider from a fake config. `name()` and `label()`
        // are part of the public surface and must not require the model
        // to be on disk.
        let provider =
            SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build");
        assert_eq!(provider.name(), "sherpa-sense-voice");
        assert_eq!(provider.label(), "Sherpa SenseVoice");
    }

    #[cfg(not(feature = "sherpa-rs"))]
    #[test]
    fn stub_status_is_unavailable() {
        let provider =
            SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let snapshot = rt.block_on(provider.status_snapshot()).expect("snapshot");
        assert_eq!(snapshot.name, "sherpa-sense-voice");
        assert_eq!(snapshot.state, "unavailable");
        assert!(!snapshot.available);
    }

    #[cfg(not(feature = "sherpa-rs"))]
    #[test]
    fn stub_transcribe_returns_not_implemented() {
        let provider =
            SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig::default()).expect("build");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(provider.transcribe_samples(&[0.0_f32; 16]));
        assert!(matches!(result, Err(AsrError::NotImplemented(_))));
    }

    // Integration test: real inference against a small fixture audio file.
    // The fixture is expected to live at
    // `crates/echobot-asr/tests/fixtures/sherpa-sense-voice.wav` (16 kHz,
    // mono, 16-bit PCM). Run with:
    //   cargo test -p echobot-asr --features sherpa-rs -- --ignored
    #[cfg(feature = "sherpa-rs")]
    #[test]
    #[ignore = "requires a downloaded SenseVoice model; run with --features sherpa-rs -- --ignored"]
    fn real_inference_against_fixture() {
        let workspace = std::env::current_dir().expect("cwd");
        let fixture = workspace
            .join("tests")
            .join("fixtures")
            .join("sherpa-sense-voice.wav");
        if !fixture.is_file() {
            eprintln!("skipping: fixture {} not present", fixture.display());
            return;
        }
        let provider = SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig {
            auto_download: true,
            ..SherpaSenseVoiceConfig::default()
        })
        .expect("build provider");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(provider.on_startup()).expect("startup");
        let mut reader = hound::WavReader::open(&fixture).expect("open wav");
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| (s.unwrap() as f32) / (i16::MAX as f32))
            .collect();
        let result = rt
            .block_on(provider.transcribe_samples(&samples))
            .expect("transcribe");
        eprintln!(
            "transcribed {} samples -> text={:?} lang={:?}",
            samples.len(),
            result.text,
            result.language
        );
    }
}
