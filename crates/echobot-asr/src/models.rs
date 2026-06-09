//! Typed models for the ASR subsystem.
//!
//! Mirrors the Python `echobot/asr/models.py` dataclasses. Everything in this
//! module is a small `Clone + Debug` struct; the heavy lifting lives in the
//! provider implementations.

use serde::{Deserialize, Serialize};

/// Status snapshot for a single provider (ASR or VAD).
///
/// Kind/name/label identify the provider; `selected` / `available` / `state`
/// describe its place in the runtime. `detail` is a human-readable string the
/// UI can show ("model missing", "downloading", "ready"…). `resource_directory`
/// is the on-disk directory the provider uses for its weights.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatusSnapshot {
    /// `"asr"` or `"vad"`.
    pub kind: String,
    /// Stable machine name (e.g. `"sherpa-sense-voice"`, `"silero"`).
    pub name: String,
    /// Human-readable label.
    pub label: String,
    /// True if this provider is the currently selected one.
    pub selected: bool,
    /// True if the provider is ready to be used.
    pub available: bool,
    /// Coarse state: `"ready"`, `"missing"`, `"downloading"`, `"error"`,
    /// `"unavailable"`, etc.
    pub state: String,
    /// Human-readable detail.
    pub detail: String,
    /// Where this provider keeps its model files on disk (or base URL, for
    /// the OpenAI provider which has no on-disk assets).
    pub resource_directory: String,
}

impl ProviderStatusSnapshot {
    /// Construct a snapshot — primarily useful for tests.
    pub fn new(
        kind: impl Into<String>,
        name: impl Into<String>,
        label: impl Into<String>,
        available: bool,
        state: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
            label: label.into(),
            selected: false,
            available,
            state: state.into(),
            detail: detail.into(),
            resource_directory: String::new(),
        }
    }
}

/// Combined status snapshot for the whole ASR service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsrStatusSnapshot {
    /// Whether the active ASR provider is ready.
    pub available: bool,
    /// Coarse state for the active ASR provider.
    pub state: String,
    /// Human-readable detail.
    pub detail: String,
    /// Sample rate the service is configured for.
    pub sample_rate: u32,
    /// Name of the currently selected ASR provider.
    pub selected_asr_provider: String,
    /// Name of the currently selected VAD provider (empty if disabled).
    pub selected_vad_provider: String,
    /// Whether "always listen" mode is supported (i.e. a VAD provider is
    /// configured and ready).
    pub always_listen_supported: bool,
    /// All registered ASR providers and their statuses.
    pub asr_providers: Vec<ProviderStatusSnapshot>,
    /// All registered VAD providers and their statuses.
    pub vad_providers: Vec<ProviderStatusSnapshot>,
}

/// Result of transcribing an audio buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionResult {
    /// The transcript text. Empty when the input was silent / blank.
    pub text: String,
    /// Detected language code (BCP-47 / ISO-639-1 style), or empty if
    /// unknown.
    pub language: String,
}

impl TranscriptionResult {
    /// Construct a result with the given text and an empty language.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            language: String::new(),
        }
    }
}

/// Richer transcription result carrying optional per-segment timing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AsrResult {
    /// Full transcript text.
    pub text: String,
    /// Detected language code, or empty if unknown.
    pub language: String,
    /// Per-segment breakdown, in source order. Empty for providers that
    /// only return a single blob of text.
    pub segments: Vec<AsrSegment>,
}

impl AsrResult {
    /// Build an `AsrResult` from a `TranscriptionResult` (no segments).
    pub fn from_transcription(result: TranscriptionResult) -> Self {
        Self {
            text: result.text,
            language: result.language,
            segments: Vec::new(),
        }
    }
}

impl From<TranscriptionResult> for AsrResult {
    fn from(value: TranscriptionResult) -> Self {
        Self::from_transcription(value)
    }
}

/// A single word/segment of a transcription, with timing information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrWord {
    /// The text of the word (or token).
    pub text: String,
    /// Start time in milliseconds.
    pub start_ms: u64,
    /// End time in milliseconds.
    pub end_ms: u64,
    /// Confidence score in `[0.0, 1.0]` if the provider reports one.
    pub confidence: Option<f32>,
}

/// A contiguous segment of a transcription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsrSegment {
    /// Zero-based segment index in the source audio.
    pub index: u32,
    /// Segment start time in milliseconds.
    pub start_ms: u64,
    /// Segment end time in milliseconds.
    pub end_ms: u64,
    /// Transcribed text for this segment.
    pub text: String,
    /// Language detected for this segment, if reported.
    pub language: String,
    /// Optional per-word breakdown.
    pub words: Vec<AsrWord>,
}

/// Configuration passed alongside a transcribe request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AsrConfig {
    /// Optional BCP-47 language hint, e.g. `"en"`, `"zh"`. Empty = auto.
    pub language: String,
    /// Optional initial prompt (used by OpenAI-style providers).
    pub prompt: String,
    /// Optional sampling temperature (used by OpenAI-style providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Number of inference threads, where the provider supports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_threads: Option<u32>,
    /// Whether to apply inverse text normalization (e.g. digits/date
    /// normalization) when supported.
    pub use_itn: bool,
}

impl AsrConfig {
    /// Construct a config with the given language hint.
    pub fn with_language(language: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            ..Self::default()
        }
    }
}
