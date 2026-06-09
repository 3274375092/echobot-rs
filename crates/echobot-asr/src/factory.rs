//! Factory helpers for building a default ASR service.
//!
//! Mirrors `echobot/asr/factory.py`. The default service bundles:
//!
//! * `sherpa-sense-voice` ASR provider (stub for v1 — see
//!   `providers::sherpa`).
//! * `openai-transcriptions` ASR provider.
//! * No VAD provider in v1 — only the `VadProvider` trait surface is ported.
//!   `silero` will land in `providers/silero.rs`; until then VAD is disabled
//!   by default. Override with `ECHOBOT_VAD_PROVIDER=<name>` once a provider
//!   is registered.
//!
//! The default names are also exposed as constants so other crates can
//! reference them without hard-coding strings.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::base::AsrServiceBuilder;
use crate::providers::openai::{OpenAITranscriptionsConfig, OpenAITranscriptionsProvider};
use crate::providers::sherpa::{SherpaSenseVoiceConfig, SherpaSenseVoiceProvider};
use crate::service::AsrService;

/// Default ASR provider name.
pub const DEFAULT_ASR_PROVIDER: &str = "sherpa-sense-voice";
/// Default VAD provider name. `"none"` disables VAD — see the module-level
/// docs for why silero isn't wired up in v1.
pub const DEFAULT_VAD_PROVIDER: &str = "none";

/// Read an environment variable as a trimmed `String`, falling back to
/// `default` when unset or empty.
pub fn env_text(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => default.to_string(),
    }
}

/// Read an environment variable as an `i64`, falling back to `default`
/// when unset, empty, or unparseable.
pub fn env_int(name: &str, default: i64) -> i64 {
    match std::env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return default;
            }
            trimmed.parse::<i64>().unwrap_or(default)
        }
        Err(_) => default,
    }
}

/// Read an environment variable as an `f64`, falling back to `default`
/// when unset, empty, or unparseable.
pub fn env_float(name: &str, default: f64) -> f64 {
    match std::env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return default;
            }
            trimmed.parse::<f64>().unwrap_or(default)
        }
        Err(_) => default,
    }
}

/// Read an environment variable as a `bool`, with truthy values
/// `{"1", "true", "yes", "on"}` (case-insensitive). Empty / missing
/// values fall back to `default`.
pub fn env_flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let trimmed = value.trim().to_ascii_lowercase();
            if trimmed.is_empty() {
                default
            } else {
                matches!(trimmed.as_str(), "1" | "true" | "yes" | "on")
            }
        }
        Err(_) => default,
    }
}

/// Resolve an optional path string. Empty values return `None`; relative
/// paths are anchored at `workspace`.
pub fn resolve_optional_path(workspace: &Path, raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() {
        Some(candidate)
    } else {
        Some(workspace.join(candidate))
    }
}

/// Normalize a provider name: empty / "none" / "off" / "disabled" →
/// `None`, everything else → trimmed `String`.
pub fn optional_provider_name(name: &str) -> Option<String> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "none" | "off" | "disabled") {
        None
    } else {
        Some(name.trim().to_string())
    }
}

/// Read an environment variable as an `Option<f32>` — returns `None` when
/// the variable is unset, empty, or unparseable.
pub fn env_optional_float(name: &str) -> Option<f32> {
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<f32>().ok()
}

/// Build a default OpenAI Transcriptions provider from environment
/// variables.
pub fn build_default_openai_provider() -> OpenAITranscriptionsProvider {
    let config = OpenAITranscriptionsConfig {
        sample_rate: env_int("ECHOBOT_ASR_SAMPLE_RATE", 16_000).max(0) as u32,
        api_key: env_text("ECHOBOT_ASR_OPENAI_API_KEY", "EMPTY"),
        model: env_text("ECHOBOT_ASR_OPENAI_MODEL", ""),
        base_url: env_text("ECHOBOT_ASR_OPENAI_BASE_URL", "https://api.openai.com/v1"),
        timeout: Duration::from_secs_f64(env_float("ECHOBOT_ASR_OPENAI_TIMEOUT", 60.0).max(1.0)),
        language: env_text("ECHOBOT_ASR_OPENAI_LANGUAGE", ""),
        prompt: env_text("ECHOBOT_ASR_OPENAI_PROMPT", ""),
        temperature: env_optional_float("ECHOBOT_ASR_OPENAI_TEMPERATURE"),
    };
    OpenAITranscriptionsProvider::new(config).expect("OpenAI provider config is valid")
}

/// Build the default `AsrService` for the given workspace.
///
/// The `sherpa-sense-voice` provider is always registered, but the
/// **concrete type** it resolves to depends on the `sherpa-rs` cargo
/// feature:
///
/// * **Default (no `sherpa-rs` feature)** — a stub provider that returns
///   `AsrError::NotImplemented` from every transcription call. v1 falls
///   back to the OpenAI-compatible provider; the stub keeps the
///   registration name stable so the rest of the crate (config, env
///   parsing, factory wiring) does not have to special-case it.
/// * **`sherpa-rs` feature enabled** — a real
///   `sherpa_rs::sense_voice::SenseVoiceRecognizer`-backed provider that
///   lazy-downloads the model bundle on first use.
///
/// Either way, the registration name is `"sherpa-sense-voice"` and the
/// OpenAI-compatible provider is also registered as
/// `"openai-transcriptions"`.
pub fn build_default_asr_service(workspace: &Path) -> AsrService {
    let sample_rate = env_int("ECHOBOT_ASR_SAMPLE_RATE", 16_000).max(0) as u32;

    let sherpa = SherpaSenseVoiceProvider::new(SherpaSenseVoiceConfig {
        sample_rate,
        auto_download: env_flag("ECHOBOT_ASR_SHERPA_AUTO_DOWNLOAD", true),
        model_root_dir: resolve_optional_path(
            workspace,
            &env_text("ECHOBOT_ASR_SHERPA_MODEL_DIR", ""),
        ),
        execution_provider: env_text("ECHOBOT_ASR_SHERPA_EXECUTION_PROVIDER", "cpu"),
        num_threads: env_int("ECHOBOT_ASR_SHERPA_NUM_THREADS", 2).max(1) as u32,
        language: env_text("ECHOBOT_ASR_SHERPA_LANGUAGE", "auto"),
        use_itn: env_flag("ECHOBOT_ASR_SHERPA_USE_ITN", false),
        model_url: env_text(
            "ECHOBOT_ASR_SHERPA_MODEL_URL",
            crate::providers::sherpa::DEFAULT_SENSE_VOICE_MODEL_URL,
        ),
        download_timeout_seconds: env_float("ECHOBOT_ASR_SHERPA_DOWNLOAD_TIMEOUT_SECONDS", 600.0)
            .max(30.0),
    })
    .expect("Sherpa config is valid");

    let openai = build_default_openai_provider();

    let selected_asr = env_text("ECHOBOT_ASR_PROVIDER", DEFAULT_ASR_PROVIDER);
    let selected_vad = optional_provider_name(&env_text(
        "ECHOBOT_VAD_PROVIDER",
        DEFAULT_VAD_PROVIDER,
    ));

    AsrServiceBuilder::new()
        .sample_rate(sample_rate)
        .with_asr_provider("sherpa-sense-voice", Arc::new(sherpa) as Arc<dyn crate::base::AsrProvider>)
        .with_asr_provider("openai-transcriptions", Arc::new(openai) as Arc<dyn crate::base::AsrProvider>)
        .selected_asr(selected_asr)
        .selected_vad(selected_vad)
        .build()
        .expect("ASR service should build with default providers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_text_falls_back_to_default() {
        // We can't easily unset env vars in a safe way, but we can read
        // an obviously-unset name and verify the default wins.
        let value = env_text("__ECHOBOT_ASR_TEST_UNSET__", "fallback");
        assert_eq!(value, "fallback");
    }

    #[test]
    fn env_int_handles_bad_input() {
        // Setting then reading back is the simplest portable test.
        std::env::set_var("__ECHOBOT_ASR_TEST_INT__", "not-a-number");
        let value = env_int("__ECHOBOT_ASR_TEST_INT__", 7);
        assert_eq!(value, 7);
        std::env::remove_var("__ECHOBOT_ASR_TEST_INT__");
    }

    #[test]
    fn env_flag_recognizes_truthy_values() {
        for (raw, expected) in [
            ("1", true),
            ("true", true),
            ("YES", true),
            ("on", true),
            ("0", false),
            ("false", false),
            ("", true), // empty => default true
        ] {
            std::env::set_var("__ECHOBOT_ASR_TEST_FLAG__", raw);
            let actual = env_flag("__ECHOBOT_ASR_TEST_FLAG__", true);
            assert_eq!(actual, expected, "raw={raw}");
        }
        std::env::set_var("__ECHOBOT_ASR_TEST_FLAG__", "off");
        let actual = env_flag("__ECHOBOT_ASR_TEST_FLAG__", true);
        assert!(!actual);
        std::env::remove_var("__ECHOBOT_ASR_TEST_FLAG__");
    }

    #[test]
    fn resolve_optional_path_anchors_relative() {
        let workspace = Path::new("/tmp/echobot");
        assert!(resolve_optional_path(workspace, "").is_none());
        assert!(resolve_optional_path(workspace, "   ").is_none());
        let abs = resolve_optional_path(workspace, "/var/models").unwrap();
        assert_eq!(abs, PathBuf::from("/var/models"));
        let rel = resolve_optional_path(workspace, "models/asr").unwrap();
        assert_eq!(rel, PathBuf::from("/tmp/echobot/models/asr"));
    }

    #[test]
    fn optional_provider_name_handles_disabled() {
        assert!(optional_provider_name("").is_none());
        assert!(optional_provider_name("none").is_none());
        assert!(optional_provider_name("OFF").is_none());
        assert!(optional_provider_name("disabled").is_none());
        assert_eq!(optional_provider_name("silero"), Some("silero".to_string()));
        assert_eq!(optional_provider_name("  silero  "), Some("silero".to_string()));
    }
}
