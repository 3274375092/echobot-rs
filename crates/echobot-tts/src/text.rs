//! `echobot-tts::text` — text normalization for TTS input.
//!
//! The Python implementation lives in `echobot/tts/text.py`. It strips
//! Markdown markers, replaces emoji with spaces (they cannot be spoken),
//! normalizes line endings, and collapses whitespace so the TTS provider
//! receives clean, speakable text.
//!
//! We re-implement the same pipeline in pure Rust using the `regex` crate
//! and a small emoji-detection table. The behaviour is intentionally a
//! port — including the strip-everything-between-``` ``` ``` fences rule,
//! which is what lets users paste a fenced code block without the TTS
//! engine reading "backtick backtick backtick new line ...".

// Standard library first.
use std::ops::RangeInclusive;

// --- Emoji detection ----------------------------------------------------
//
// Mirrors the Python `_EMOJI_CODEPOINTS` and `_EMOJI_RANGES` tables. We
// keep them as small static slices so emoji detection is allocation-free.

const EMOJI_CODEPOINTS: &[u32] = &[
    0x200D, // zero width joiner
    0x20E3, // combining enclosing keycap
    0xFE0E, // variation selector-15
    0xFE0F, // variation selector-16
];

const EMOJI_RANGES: &[RangeInclusive<u32>] = &[
    (0x1F1E6..=0x1F1FF), // flags
    (0x1F3FB..=0x1F3FF), // skin tone modifiers
    (0x1F300..=0x1F5FF), // symbols and pictographs
    (0x1F600..=0x1F64F), // emoticons
    (0x1F680..=0x1F6FF), // transport and map
    (0x1F700..=0x1F77F), // alchemical symbols
    (0x1F780..=0x1F7FF), // geometric shapes extended
    (0x1F800..=0x1F8FF), // supplemental arrows-c
    (0x1F900..=0x1F9FF), // supplemental symbols and pictographs
    (0x1FA70..=0x1FAFF), // symbols and pictographs extended-a
    (0x2600..=0x26FF),   // miscellaneous symbols
    (0x2700..=0x27BF),   // dingbats
];

fn is_emoji_codepoint(cp: u32) -> bool {
    if EMOJI_CODEPOINTS.contains(&cp) {
        return true;
    }
    EMOJI_RANGES.iter().any(|range| range.contains(&cp))
}

// --- Compiled regexes --------------------------------------------------

use once_cell::sync::Lazy;
use regex::Regex;

// `^\s*(```|~~~)[^\n]*$` with MULTILINE — matches a fence line.
static FENCE_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:`{3}|~{3})[^\n]*$").expect("static regex is valid")
});

// ` `text` ` -> `text`
static INLINE_CODE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"`([^`]+)`").expect("static regex is valid"));

// `[label](url)` -> `label`
static LINK_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("static regex is valid"));

// `^\s{0,3}#{1,6}\s+` with MULTILINE — strip heading markers.
static HEADING_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s{0,3}#{1,6}\s+").expect("static regex is valid"));

// `^\s*>\s?` with MULTILINE — strip quote markers.
static QUOTE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*>\s?").expect("static regex is valid"));

// `^\s*[-*+]\s+` with MULTILINE — strip unordered list markers.
static UNORDERED_LIST_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*[-*+]\s+").expect("static regex is valid"));

// `^\s*\d+[.)]\s+` with MULTILINE — strip ordered list markers.
static ORDERED_LIST_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*\d+[.)]\s+").expect("static regex is valid"));

// Strip stray emphasis markers (`*`, `_`, `~`).
static MARKDOWN_MARKER_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[*_~]").expect("static regex is valid"));

/// Normalize a chunk of text for TTS. See module docs for the full
/// pipeline. The result is always speakable text (or empty, if the input
/// was nothing but markdown / emoji).
pub fn normalize_text_for_tts(text: &str) -> String {
    // Treat None-ish inputs as empty strings.
    let mut working = text.replace("\r\n", "\n").replace('\r', "\n");

    // Markdown stripping. Order matters: fences first, then inline code,
    // links, headings, quotes, list markers, stray markers.
    working = FENCE_PATTERN.replace_all(&working, "").into_owned();
    working = INLINE_CODE_PATTERN
        .replace_all(&working, "$1")
        .into_owned();
    working = LINK_PATTERN.replace_all(&working, "$1").into_owned();
    working = HEADING_PATTERN.replace_all(&working, "").into_owned();
    working = QUOTE_PATTERN.replace_all(&working, "").into_owned();
    working = UNORDERED_LIST_PATTERN
        .replace_all(&working, "")
        .into_owned();
    working = ORDERED_LIST_PATTERN
        .replace_all(&working, "")
        .into_owned();
    working = MARKDOWN_MARKER_PATTERN
        .replace_all(&working, "")
        .into_owned();

    // Replace each emoji with a single space (they can't be spoken).
    let mut cleaned = String::with_capacity(working.len());
    for ch in working.chars() {
        if is_emoji_codepoint(ch as u32) {
            cleaned.push(' ');
        } else {
            cleaned.push(ch);
        }
    }

    // Collapse all whitespace (including newlines) to single spaces.
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(normalize_text_for_tts(""), "");
    }

    #[test]
    fn collapses_whitespace() {
        let got = normalize_text_for_tts("hello   world\n\nfoo\tbar");
        assert_eq!(got, "hello world foo bar");
    }

    #[test]
    fn strips_markdown_fences() {
        let input = "intro\n```python\nprint('hi')\n```\noutro";
        let got = normalize_text_for_tts(input);
        // Fence lines and their content are removed; intro and outro remain.
        assert_eq!(got, "intro print('hi') outro");
    }

    #[test]
    fn strips_inline_code_and_links() {
        let input = "use `fmt.Println` and see [docs](https://example.com)";
        let got = normalize_text_for_tts(input);
        assert_eq!(got, "use fmt.Println and see docs");
    }

    #[test]
    fn strips_headings_quotes_and_lists() {
        let input = "# Title\n> a quote\n- item one\n* item two\n1. first\n2. second";
        let got = normalize_text_for_tts(input);
        assert_eq!(got, "Title a quote item one item two first second");
    }

    #[test]
    fn replaces_emoji_with_space() {
        // 🎉 is U+1F389, in the 0x1F300..=0x1F5FF range.
        let input = "hello 🎉 world";
        let got = normalize_text_for_tts(input);
        assert_eq!(got, "hello world");
    }

    #[test]
    fn handles_crlf_input() {
        let got = normalize_text_for_tts("line1\r\nline2\rline3");
        assert_eq!(got, "line1 line2 line3");
    }
}
