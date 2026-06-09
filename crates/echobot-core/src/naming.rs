//! Helpers for normalizing human-readable name tokens into slugs.
//!
//! Port of `echobot/naming.py`. The Python version only had a single
//! function — we keep the public surface tiny.

const ALLOWED_NAME_PUNCTUATION: &[char] = &['-', '_'];

/// Normalizes a free-form name token into a lowercase, hyphen-joined slug.
///
/// Whitespace separates parts; only alphanumerics and `-_` are kept inside
/// each part. The result is suitable for filesystem or session names.
pub fn normalize_name_token(value: &str) -> String {
    let lowered: String = value
        .split_whitespace()
        .map(|part| part.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    lowered
        .chars()
        .filter(|c| c.is_alphanumeric() || ALLOWED_NAME_PUNCTUATION.contains(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_whitespace_and_case() {
        assert_eq!(normalize_name_token("Hello World"), "hello-world");
    }

    #[test]
    fn strips_disallowed_punctuation() {
        assert_eq!(
            normalize_name_token("Project #1 (alpha!)"),
            "project-1-alpha"
        );
    }

    #[test]
    fn preserves_dash_and_underscore() {
        assert_eq!(
            normalize_name_token("my-cool_session"),
            "my-cool_session"
        );
    }
}
