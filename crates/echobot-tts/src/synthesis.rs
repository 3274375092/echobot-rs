//! `echobot-tts::synthesis` — synthesis-option helpers.
//!
//! These are pure functions that translate the loose "rate / speed /
//! volume / pitch" strings the user passes (e.g. `"+25%"`, `"1.25"`,
//! `"-10%"`) into typed values the providers can use.
//!
//! Mirrors `echobot/tts/synthesis.py`. Notably:
//!
//! * `parse_tts_speed` accepts a percent (with or without sign), a raw
//!   multiplier, and clamps to `[MIN_TTS_SPEED, MAX_TTS_SPEED]`.
//! * `edge_rate_from_speed` converts a linear multiplier back to Edge
//!   TTS's "percent delta" format (e.g. `1.25` -> `"+25%"`).

use crate::base::{TtsError, TtsSynthesisOptions};

/// Minimum allowed TTS speed multiplier. Below this most providers start
/// producing nonsensical audio.
pub const MIN_TTS_SPEED: f32 = 0.25;

/// Maximum allowed TTS speed multiplier.
pub const MAX_TTS_SPEED: f32 = 4.0;

/// Build a [`TtsSynthesisOptions`] from loosely-typed caller input. All
/// fields are optional and may be `None` or empty.
pub fn build_tts_synthesis_options(
    voice: Option<&str>,
    rate: Option<&str>,
    volume: Option<&str>,
    pitch: Option<&str>,
) -> TtsSynthesisOptions {
    TtsSynthesisOptions {
        voice: clean_optional_text(voice),
        speed: parse_tts_speed(rate).ok().flatten(),
        volume: clean_optional_text(volume),
        pitch: clean_optional_text(pitch),
    }
}

/// Parse a "rate" string into a linear speed multiplier.
///
/// * `"+25%"` and `"-10%"` -> `1.25` and `0.9` respectively.
/// * `"25%"`              -> `0.25` (no sign = literal multiplier).
/// * `"1.25"`             -> `1.25` (raw multiplier).
///
/// Returns:
/// * `Ok(None)` if the input is empty / whitespace.
/// * `Err(TtsError::InvalidArgument)` on parse failure or non-positive
///   multipliers.
pub fn parse_tts_speed(rate: Option<&str>) -> Result<Option<f32>, TtsError> {
    let raw = match clean_optional_text(rate) {
        Some(s) => s,
        None => return Ok(None),
    };

    let speed = if let Some(percent_text) = raw.strip_suffix('%') {
        let percent_text = percent_text.trim();
        let percent_value: f32 = percent_text.parse().map_err(|_| {
            TtsError::argument(format!("invalid TTS rate: {}", rate.unwrap_or("")))
        })?;
        if raw.starts_with('+') || raw.starts_with('-') {
            1.0 + (percent_value / 100.0)
        } else {
            percent_value / 100.0
        }
    } else {
        raw.parse::<f32>().map_err(|_| {
            TtsError::argument(format!("invalid TTS rate: {}", rate.unwrap_or("")))
        })?
    };

    if speed <= 0.0 {
        return Err(TtsError::argument("TTS rate must be greater than zero"));
    }

    Ok(Some(speed.clamp(MIN_TTS_SPEED, MAX_TTS_SPEED)))
}

/// Convert a linear speed multiplier into the percent-delta form Edge
/// TTS uses. `None` when the input is `None` or essentially 1.0 (the
/// provider's own default).
pub fn edge_rate_from_speed(speed: Option<f32>) -> Option<String> {
    let speed = speed?;
    let percent_delta = (speed - 1.0) * 100.0;
    if percent_delta.abs() < 1e-9 {
        return None;
    }
    let sign = if percent_delta > 0.0 { "+" } else { "" };
    Some(format!("{sign}{}%", format_number(percent_delta)))
}

fn clean_optional_text(value: Option<&str>) -> Option<String> {
    let v = value?;
    let trimmed = v.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Render a float with up to 2 fractional digits, stripping trailing
/// zeros and a trailing dot. Mirrors the Python helper.
fn format_number(value: f32) -> String {
    // Use a fixed precision then trim.
    let formatted = format!("{value:.2}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_speed_handles_percent_with_sign() {
        assert_eq!(parse_tts_speed(Some("+25%")).unwrap(), Some(1.25));
        assert_eq!(parse_tts_speed(Some("-10%")).unwrap(), Some(0.9));
    }

    #[test]
    fn parse_speed_handles_percent_without_sign() {
        // "25%" is interpreted as a literal multiplier (0.25), not as
        // "+25%". This matches the Python implementation.
        assert_eq!(parse_tts_speed(Some("25%")).unwrap(), Some(0.25));
    }

    #[test]
    fn parse_speed_handles_raw_multiplier() {
        assert_eq!(parse_tts_speed(Some("1.5")).unwrap(), Some(1.5));
    }

    #[test]
    fn parse_speed_clamps_to_range() {
        assert_eq!(parse_tts_speed(Some("0.01")).unwrap(), Some(MIN_TTS_SPEED));
        assert_eq!(parse_tts_speed(Some("100")).unwrap(), Some(MAX_TTS_SPEED));
    }

    #[test]
    fn parse_speed_rejects_zero_and_negative() {
        assert!(parse_tts_speed(Some("0")).is_err());
        assert!(parse_tts_speed(Some("-1.0")).is_err());
        assert!(parse_tts_speed(Some("0%")).is_err());
    }

    #[test]
    fn parse_speed_rejects_garbage() {
        assert!(parse_tts_speed(Some("nope")).is_err());
    }

    #[test]
    fn parse_speed_empty_returns_none() {
        assert_eq!(parse_tts_speed(None).unwrap(), None);
        assert_eq!(parse_tts_speed(Some("")).unwrap(), None);
        assert_eq!(parse_tts_speed(Some("   ")).unwrap(), None);
    }

    #[test]
    fn edge_rate_round_trip() {
        assert_eq!(edge_rate_from_speed(Some(1.0)), None);
        assert_eq!(edge_rate_from_speed(Some(1.25)).as_deref(), Some("+25%"));
        assert_eq!(edge_rate_from_speed(Some(0.9)).as_deref(), Some("-10%"));
        assert_eq!(edge_rate_from_speed(None), None);
    }

    #[test]
    fn build_options_collects_everything() {
        let opts = build_tts_synthesis_options(
            Some("  alloy  "),
            Some("+25%"),
            Some(" +0% "),
            Some("+5Hz"),
        );
        assert_eq!(opts.voice.as_deref(), Some("alloy"));
        assert_eq!(opts.speed, Some(1.25));
        assert_eq!(opts.volume.as_deref(), Some("+0%"));
        assert_eq!(opts.pitch.as_deref(), Some("+5Hz"));
    }
}
