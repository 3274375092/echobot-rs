//! `echobot-tts::base` — core TTS types: the [`TtsProvider`] trait, the
//! request/response DTOs, the error type, and voice + status types.
//!
//! These types are provider-agnostic. They model the smallest reasonable
//! surface area for a TTS subsystem: hand the provider a normalized text
//! string plus a [`TtsSynthesisOptions`], get back a [`TtsAudio`] (or an
//! error).
//!
//! See `echobot/tts/base.py` for the Python equivalent.

use async_trait::async_trait;
use thiserror::Error;

/// A single voice the provider knows about.
///
/// Edge TTS returns rich metadata (locale, gender, friendly name). The
/// openai-compatible provider returns short names only. We carry both
/// shapes here so providers can fill in what they know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceOption {
    /// Long name (e.g. "Microsoft Server Speech Text to Speech Voice
    /// (zh-CN, XiaoxiaoNeural)").
    pub name: String,
    /// Short name (e.g. "zh-CN-XiaoxiaoNeural", "alloy"). This is the value
    /// passed back to providers in `voice`.
    pub short_name: String,
    /// BCP-47 locale tag, e.g. "zh-CN". Empty string if unknown.
    pub locale: String,
    /// "Female" / "Male" / "Neutral" / "" (unknown).
    pub gender: String,
    /// Human-friendly display name.
    pub display_name: String,
}

impl VoiceOption {
    /// Build a voice entry with just a short name. The other fields are
    /// blank / mirror the short name. Used for the openai-compatible
    /// provider, which only knows voice names.
    pub fn from_name(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            name: n.clone(),
            short_name: n.clone(),
            locale: String::new(),
            gender: String::new(),
            display_name: n,
        }
    }
}

/// The synthesized audio result.
#[derive(Debug, Clone)]
pub struct TtsAudio {
    /// Raw audio bytes in the format advertised by `content_type`.
    pub audio_bytes: Vec<u8>,
    /// MIME type, e.g. `"audio/mpeg"`, `"audio/wav"`, `"audio/opus"`.
    pub content_type: String,
    /// File extension (no dot), e.g. `"mp3"`, `"wav"`, `"opus"`.
    pub file_extension: String,
    /// Provider name that produced the audio (e.g. "edge",
    /// "openai-compatible"). Mirrors the Python `provider` field.
    pub provider: String,
    /// Voice short name that was actually used.
    pub voice: String,
}

impl TtsAudio {
    /// Convenience: combine the format / extension lookup tables used by
    /// the openai-compatible provider into a single helper.
    pub fn mime_for_format(format: &str) -> &'static str {
        match format.to_ascii_lowercase().as_str() {
            "mp3" => "audio/mpeg",
            "opus" => "audio/opus",
            "aac" => "audio/aac",
            "flac" => "audio/flac",
            "wav" => "audio/wav",
            "pcm" => "audio/pcm",
            _ => "application/octet-stream",
        }
    }
}

/// Provider status, suitable for surfacing in `/tts/status` endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtsProviderStatus {
    /// Stable provider name (e.g. "edge").
    pub name: String,
    /// Human-readable label (e.g. "Edge TTS").
    pub label: String,
    /// `true` if the provider can synthesize right now.
    pub available: bool,
    /// Coarse state: "ready" | "missing" | "unavailable" | "downloading" |
    /// "error". Defaults to "ready" for providers that don't model their
    /// own state.
    pub state: String,
    /// Optional human-readable detail about why the provider is not
    /// available. Empty when `available` is `true`.
    pub detail: String,
}

impl TtsProviderStatus {
    /// Construct a "ready" status.
    pub fn ready(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            available: true,
            state: "ready".to_string(),
            detail: String::new(),
        }
    }
}

/// Synthesis options. All fields are optional. Providers fall back to
/// their built-in defaults for anything left as `None`.
///
/// Note: there is intentionally no `format` / `response_format` field —
/// each provider exposes its own format knob (e.g. `TtsFormat` for the
/// openai-compatible provider), since the supported format sets are not
/// the same across providers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TtsSynthesisOptions {
    /// Voice short name. `None` means "use the provider default".
    pub voice: Option<String>,
    /// Linear speed multiplier (1.0 = normal). `None` means "use the
    /// provider default". Edge TTS interprets this as a percent delta
    /// (e.g. 1.25 -> "+25%"); openai-compatible sends it as a `speed`
    /// field; kokoro applies it to the underlying sherpa-onnx call.
    pub speed: Option<f32>,
    /// Provider-specific volume hint (e.g. "+0%" / "-10%" for Edge).
    /// The openai-compatible provider ignores this.
    pub volume: Option<String>,
    /// Provider-specific pitch hint (e.g. "+0Hz" for Edge).
    pub pitch: Option<String>,
}

impl TtsSynthesisOptions {
    /// Construct options with just a voice.
    pub fn with_voice(voice: impl Into<String>) -> Self {
        Self {
            voice: Some(voice.into()),
            ..Self::default()
        }
    }
}

/// Crate-wide TTS error type.
#[derive(Debug, Error)]
pub enum TtsError {
    /// Feature is not implemented yet (e.g. kokoro in v1).
    #[error("not implemented: {0}")]
    NotImplemented(String),

    /// Provider returned an error (HTTP 4xx/5xx, WS close with error, ...).
    /// The inner string is the human-readable detail.
    #[error("provider error: {0}")]
    ProviderError(String),

    /// Network / transport error (DNS, TCP, TLS, timeout, ...).
    #[error("network error: {0}")]
    NetworkError(String),

    /// Configuration is invalid (missing API key, empty base URL, ...).
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// Caller passed an invalid argument (empty text after normalization,
    /// unknown voice, ...).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Response from the provider could not be parsed.
    #[error("invalid response: {0}")]
    InvalidResponse(String),

    /// Wrapped I/O error (file read/write, etc).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Catch-all for unexpected errors. Prefer a more specific variant
    /// when you can.
    #[error("internal error: {0}")]
    Internal(String),
}

impl TtsError {
    /// Helper to build a provider error from any display-able value.
    pub fn provider<S: Into<String>>(msg: S) -> Self {
        Self::ProviderError(msg.into())
    }
    /// Helper to build an invalid-config error.
    pub fn config<S: Into<String>>(msg: S) -> Self {
        Self::InvalidConfig(msg.into())
    }
    /// Helper to build an invalid-argument error.
    pub fn argument<S: Into<String>>(msg: S) -> Self {
        Self::InvalidArgument(msg.into())
    }
    /// Helper to build a network error.
    pub fn network<S: Into<String>>(msg: S) -> Self {
        Self::NetworkError(msg.into())
    }
}

/// The provider trait every TTS backend implements.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Stable identifier, e.g. "edge", "openai-compatible", "kokoro".
    fn name(&self) -> &str;

    /// Human-readable label for UI surfaces.
    fn label(&self) -> &str;

    /// Default voice short name used when the caller doesn't pick one.
    fn default_voice(&self) -> &str;

    /// Status snapshot. Default impl returns "ready" with `name` / `label`.
    fn status(&self) -> TtsProviderStatus {
        TtsProviderStatus::ready(self.name(), self.label())
    }

    /// Return the list of voices the provider knows about. Default is an
    /// empty list (e.g. when the provider doesn't support enumeration).
    async fn list_voices(&self) -> Result<Vec<VoiceOption>, TtsError> {
        Ok(Vec::new())
    }

    /// Synthesize speech. Implementations are expected to:
    ///
    /// 1. Apply provider-specific default voice / format if `options` is
    ///    `None` or has empty fields.
    /// 2. Perform the network / on-device work without blocking the
    ///    runtime (use `tokio::task::spawn_blocking` for heavy CPU).
    /// 3. Return audio bytes wrapped in a [`TtsAudio`].
    async fn synthesize(
        &self,
        text: &str,
        options: Option<&TtsSynthesisOptions>,
    ) -> Result<TtsAudio, TtsError>;

    /// Release any owned resources (HTTP clients, persistent connections).
    /// Default impl is a no-op.
    async fn close(&self) -> Result<(), TtsError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_from_name_mirrors_short_name() {
        let v = VoiceOption::from_name("alloy");
        assert_eq!(v.short_name, "alloy");
        assert_eq!(v.name, "alloy");
        assert_eq!(v.display_name, "alloy");
    }

    #[test]
    fn mime_for_format_maps_known_values() {
        assert_eq!(TtsAudio::mime_for_format("mp3"), "audio/mpeg");
        assert_eq!(TtsAudio::mime_for_format("WAV"), "audio/wav");
        assert_eq!(TtsAudio::mime_for_format("opus"), "audio/opus");
        assert_eq!(
            TtsAudio::mime_for_format("garbage"),
            "application/octet-stream"
        );
    }

    #[test]
    fn ready_status_is_available() {
        let s = TtsProviderStatus::ready("edge", "Edge TTS");
        assert!(s.available);
        assert_eq!(s.state, "ready");
        assert!(s.detail.is_empty());
    }
}
