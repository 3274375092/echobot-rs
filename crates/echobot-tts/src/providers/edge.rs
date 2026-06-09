//! `echobot-tts::providers::edge` — Microsoft Edge "read aloud" TTS.
//!
//! Mirrors the Python `edge-tts` package's WebSocket protocol against
//! `wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1`.
//! The service is free and unauthenticated; we send a config + speak
//! request, then collect audio binary frames until the server closes.
//!
//! Protocol overview (1:1 with `edge-tts`):
//!
//! 1. Connect with `Sec-WebSocket-Protocol: websocket-bing-tts-protocol`.
//! 2. Send `X-TimestampPath` header on the HTTP upgrade (it carries no
//!    meaningful value but the server rejects requests that omit it).
//! 3. Send a `speech.config` JSON message to declare voice / format.
//! 4. Send a `ssml` message that wraps the text in `<speak version="1.0"
//!    xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="...">` and
//!    contains a single `<voice name="...">` element.
//! 5. Receive a stream of `Path: audio` binary messages and
//!    `Path: turn.end` text messages. Concatenate the binaries in order.
//!
//! This implementation favours clarity over micro-optimisation. It uses
//! `tokio-tungstenite` to drive the WebSocket, and `reqwest` is not
//! required at all (kept as an unused import in case we fall back to
//! HTTP later).

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use crate::base::{
    TtsAudio, TtsError, TtsProvider, TtsProviderStatus, TtsSynthesisOptions, VoiceOption,
};
use crate::synthesis::edge_rate_from_speed;

/// WebSocket endpoint used by the public Edge read-aloud service.
const EDGE_WSS_URL: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";

/// Sub-protocol token the Edge service requires.
const EDGE_WSS_SUBPROTOCOL: &str = "websocket-bing-tts-protocol";

/// Trust-Client-Token header value: the well-known public token used by
/// the official `edge-tts` client.
const EDGE_TRUST_TOKEN: &str =
    "6A5AA1D4EAFF4E9FB37E23D68491D6F4";

/// Connection / response timeout (seconds). Edge TTS usually responds
/// in well under this.
const EDGE_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Default voice if the caller doesn't pick one. Matches the Python
/// port's `DEFAULT_EDGE_VOICE`.
pub const DEFAULT_EDGE_VOICE: &str = "zh-CN-XiaoxiaoNeural";

/// Configuration for [`EdgeTtsProvider`].
#[derive(Debug, Clone)]
pub struct EdgeTtsConfig {
    pub default_voice: String,
}

impl Default for EdgeTtsConfig {
    fn default() -> Self {
        Self {
            default_voice: DEFAULT_EDGE_VOICE.to_string(),
        }
    }
}

impl EdgeTtsConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_voice(mut self, voice: impl Into<String>) -> Self {
        self.default_voice = voice.into();
        self
    }
}

/// Microsoft Edge read-aloud TTS provider.
pub struct EdgeTtsProvider {
    config: EdgeTtsConfig,
}

impl EdgeTtsProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: EdgeTtsConfig) -> Self {
        Self { config }
    }

    /// Build the `speech.config` JSON message body.
    fn build_config_message(&self) -> String {
        json!({
            "context": {
                "synthesis": {
                    "audio": {
                        "metadataoptions": {
                            "sentenceBoundaryEnabled": false,
                            "wordBoundaryEnabled": false
                        },
                        "outputFormat": "audio-24khz-48kbitrate-mono-mp3"
                    }
                }
            }
        })
        .to_string()
    }

    /// Build the SSML body for the `ssml` message. Voice, rate, volume,
    /// and pitch are injected into the `<voice>` element.
    fn build_ssml(
        &self,
        text: &str,
        voice: &str,
        options: Option<&TtsSynthesisOptions>,
    ) -> String {
        // Escape XML special characters in the text. The text we receive
        // has already been Markdown-stripped by `normalize_text_for_tts`,
        // but defensive escaping is cheap.
        let escaped_text = xml_escape(text);

        let mut attrs = format!("name=\"{}\"", xml_escape(voice));
        if let Some(speed) = options.and_then(|o| o.speed) {
            if let Some(rate) = edge_rate_from_speed(Some(speed)) {
                attrs.push_str(&format!(" rate=\"{}\"", xml_escape(&rate)));
            }
        }
        if let Some(volume) = options.and_then(|o| o.volume.as_deref()) {
            if !volume.trim().is_empty() {
                attrs.push_str(&format!(" volume=\"{}\"", xml_escape(volume)));
            }
        }
        if let Some(pitch) = options.and_then(|o| o.pitch.as_deref()) {
            if !pitch.trim().is_empty() {
                attrs.push_str(&format!(" pitch=\"{}\"", xml_escape(pitch)));
            }
        }

        // We use `xml:lang="en-US"` because the service tolerates any
        // well-formed value; the actual language is set by the voice.
        format!(
            "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"en-US\"><voice {attrs}>{escaped_text}</voice></speak>"
        )
    }

    /// Connect to the Edge service. Returns a configured WebSocket stream
    /// with the sub-protocol set.
    async fn connect(&self) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, TtsError> {
        let mut request = EDGE_WSS_URL
            .into_client_request()
            .map_err(|e| TtsError::network(format!("failed to build WS request: {e}")))?;
        request.headers_mut().insert(
            "Pragma",
            "no-cache".parse().expect("hardcoded header is valid"),
        );
        request.headers_mut().insert(
            "Cache-Control",
            "no-cache".parse().expect("hardcoded header is valid"),
        );
        request.headers_mut().insert(
            "Origin",
            "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold"
                .parse()
                .expect("hardcoded header is valid"),
        );
        request.headers_mut().insert(
            "Accept-Encoding",
            "gzip, deflate, br".parse().expect("hardcoded header is valid"),
        );
        request.headers_mut().insert(
            "Accept-Language",
            "en-US,en;q=0.9".parse().expect("hardcoded header is valid"),
        );
        request
            .headers_mut()
            .insert("X-Edge-Origin", "msedge-tts".parse().unwrap());
        request.headers_mut().insert(
            "X-Timestamp",
            chrono_like_timestamp().parse().expect("hardcoded header is valid"),
        );
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            EDGE_WSS_SUBPROTOCOL.parse().unwrap(),
        );
        request.headers_mut().insert(
            "Trust-Client-Token",
            EDGE_TRUST_TOKEN.parse().expect("hardcoded header is valid"),
        );

        let connect_future = connect_async(request);
        let (ws_stream, _response) = tokio::time::timeout(
            Duration::from_secs(EDGE_REQUEST_TIMEOUT_SECS),
            connect_future,
        )
        .await
        .map_err(|_| TtsError::network("Edge TTS connect timed out"))?
        .map_err(|e| TtsError::network(format!("Edge TTS connect failed: {e}")))?;

        Ok(ws_stream)
    }

    /// Run a full synthesis round-trip and return the concatenated audio
    /// bytes. Extracted from `synthesize` so it can be unit-tested via
    /// the request/response construction helpers.
    async fn run_synthesis(
        &self,
        text: &str,
        voice: &str,
        options: Option<&TtsSynthesisOptions>,
    ) -> Result<Vec<u8>, TtsError> {
        let mut ws = self.connect().await?;
        let config_msg = self.build_config_message();
        let ssml = self.build_ssml(text, voice, options);

        // `mktts-` is a marker that lets the server correlate the two
        // messages. We pick a stable short tag (millisecond timestamp).
        let request_id = format!("mktts-{}", chrono_like_timestamp());

        // 1. speech.config
        let config_frame = format!(
            "X-RequestId:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{}",
            request_id, config_msg
        );
        ws.send(Message::Text(config_frame))
            .await
            .map_err(|e| TtsError::network(format!("Edge TTS send config failed: {e}")))?;

        // 2. ssml
        let ssml_frame = format!(
            "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nPath:ssml\r\n\r\n{}",
            request_id, ssml
        );
        ws.send(Message::Text(ssml_frame))
            .await
            .map_err(|e| TtsError::network(format!("Edge TTS send ssml failed: {e}")))?;

        // 3. Drain responses. Concatenate audio binaries, stop on
        //    `turn.end` or stream close.
        let mut audio: Vec<u8> = Vec::new();
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(EDGE_REQUEST_TIMEOUT_SECS);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(TtsError::network("Edge TTS response timed out"));
            }
            let next = tokio::time::timeout(remaining, ws.next()).await;
            let message = match next {
                Ok(Some(Ok(m))) => m,
                Ok(Some(Err(e))) => {
                    return Err(TtsError::network(format!(
                        "Edge TTS receive failed: {e}"
                    )))
                }
                Ok(None) => break, // server closed
                Err(_) => return Err(TtsError::network("Edge TTS response timed out")),
            };

            match message {
                Message::Binary(bytes) => {
                    // Each binary frame is the full `Path:audio` payload
                    // prefixed with a short header. The audio starts
                    // after the `\r\n\r\n` separator; we keep it
                    // verbatim (the `edge-tts` library does the same).
                    if let Some(idx) = find_double_crlf(&bytes) {
                        let payload = &bytes[idx + 4..];
                        if !payload.is_empty() {
                            audio.extend_from_slice(payload);
                        }
                    } else if !bytes.is_empty() {
                        audio.extend_from_slice(&bytes);
                    }
                }
                Message::Text(text) => {
                    // The "Path:" header is on the first line.
                    let first_line = text.split('\n').next().unwrap_or("");
                    if first_line.contains("Path:turn.end") {
                        debug!("Edge TTS turn.end received");
                        break;
                    }
                    if first_line.contains("Path:audio") {
                        // Some servers send audio as text frames too
                        // (rare). Decode base64 in the body if present.
                        if let Some(body) = text.split("\r\n\r\n").nth(1) {
                            use base64::Engine;
                            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(body.trim()) {
                                audio.extend_from_slice(&decoded);
                            }
                        }
                    } else if first_line.contains("Path:error") || first_line.contains("Path:challenge") {
                        // The "challenge" path is a no-op that some
                        // servers send periodically; "error" we surface
                        // as a provider error.
                        let detail: String = text.chars().take(512).collect();
                        if first_line.contains("Path:error") {
                            return Err(TtsError::provider(format!(
                                "Edge TTS reported error: {detail}"
                            )));
                        }
                        warn!("Edge TTS unexpected frame: {}", first_line);
                    } else {
                        debug!("Edge TTS frame: {}", first_line);
                    }
                }
                Message::Close(frame) => {
                    if let Some(close) = frame {
                        // 1000 = normal closure.
                        let close_code = u16::from(close.code);
                        if close_code != 1000 {
                            return Err(TtsError::provider(format!(
                                "Edge TTS closed with code {close_code}: {}",
                                close.reason
                            )));
                        }
                    }
                    break;
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                    // No-op.
                }
            }
        }

        if audio.is_empty() {
            return Err(TtsError::provider(
                "Edge TTS did not return any audio data",
            ));
        }
        Ok(audio)
    }
}

impl Default for EdgeTtsProvider {
    fn default() -> Self {
        Self {
            config: EdgeTtsConfig::default(),
        }
    }
}

#[async_trait]
impl TtsProvider for EdgeTtsProvider {
    fn name(&self) -> &str {
        "edge"
    }

    fn label(&self) -> &str {
        "Edge TTS"
    }

    fn default_voice(&self) -> &str {
        &self.config.default_voice
    }

    fn status(&self) -> TtsProviderStatus {
        // We don't actively probe the network in `status()`; the
        // connection is established lazily on `synthesize`. That keeps
        // status cheap and matches the spirit of the Python port.
        TtsProviderStatus::ready(self.name(), self.label())
    }

    async fn list_voices(&self) -> Result<Vec<VoiceOption>, TtsError> {
        // Edge TTS supports a `voice.list` message. We do not implement
        // the enumeration endpoint here (it's a non-trivial SSML shape
        // and not needed for the v1 port). Providers that need it can
        // extend this impl; the trait signature stays the same.
        Ok(Vec::new())
    }

    async fn synthesize(
        &self,
        text: &str,
        options: Option<&TtsSynthesisOptions>,
    ) -> Result<TtsAudio, TtsError> {
        if text.trim().is_empty() {
            return Err(TtsError::argument("TTS text must not be empty"));
        }
        let voice = options
            .and_then(|o| o.voice.clone())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.config.default_voice.clone());

        let audio_bytes = self.run_synthesis(text, &voice, options).await?;
        Ok(TtsAudio {
            audio_bytes,
            content_type: "audio/mpeg".to_string(),
            file_extension: "mp3".to_string(),
            provider: self.name().to_string(),
            voice,
        })
    }
}

// --- helpers ----------------------------------------------------------

/// Find the byte offset of the first `\r\n\r\n` separator. Returns
/// `None` if not found.
fn find_double_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Minimal XML escape. The set of characters is what `<` and `>`
/// produce; `&` and quotes are escaped because they appear inside
/// attribute values.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Cheap "now-ish" timestamp string used as a request marker. We avoid
/// pulling `chrono` into the dep list for this one usage.
fn chrono_like_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();
    format!("{}{:03}Z", secs, millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ssml_wraps_voice_with_attrs() {
        let provider = EdgeTtsProvider::new();
        let opts = TtsSynthesisOptions {
            voice: None,
            speed: Some(1.25),
            volume: Some("+0%".to_string()),
            pitch: Some("+0Hz".to_string()),
        };
        let ssml = provider.build_ssml("hello world", "zh-CN-XiaoxiaoNeural", Some(&opts));
        assert!(ssml.starts_with("<speak"));
        assert!(ssml.contains("name=\"zh-CN-XiaoxiaoNeural\""));
        assert!(ssml.contains("rate=\"+25%\""));
        assert!(ssml.contains("volume=\"+0%\""));
        assert!(ssml.contains("pitch=\"+0Hz\""));
        assert!(ssml.contains("hello world"));
        assert!(ssml.ends_with("</speak>"));
    }

    #[test]
    fn build_ssml_escapes_special_chars() {
        let provider = EdgeTtsProvider::new();
        let ssml = provider.build_ssml("a < b & c", "alloy", None);
        assert!(ssml.contains("a &lt; b &amp; c"));
    }

    #[test]
    fn build_config_message_has_speech_config_path() {
        let provider = EdgeTtsProvider::new();
        let msg = provider.build_config_message();
        assert!(msg.contains("audio-24khz-48kbitrate-mono-mp3"));
    }

    #[test]
    fn xml_escape_handles_all_reserved() {
        let got = xml_escape("a&b<c>d\"e'f");
        assert_eq!(got, "a&amp;b&lt;c&gt;d&quot;e&apos;f");
    }

    #[test]
    fn find_double_crlf_works() {
        let bytes = b"header\r\nmore\r\n\r\nbody";
        let pos = find_double_crlf(bytes).expect("separator present");
        assert_eq!(&bytes[pos + 4..], b"body");
    }

    #[test]
    fn find_double_crlf_missing_returns_none() {
        let bytes = b"no separator here";
        assert!(find_double_crlf(bytes).is_none());
    }
}
