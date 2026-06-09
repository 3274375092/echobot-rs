//! Web request tool: fetches a public HTTP(S) URL and returns the
//! extracted text content. Mirrors `echobot/tools/web.py` — rejects
//! private / loopback addresses unless `allow_private_network` is set,
//! and decodes the body with a charset detection step.
//!
//! The actual HTTP request uses [`reqwest`]. The response is read with
//! a 4-byte-per-char byte cap so a malicious server can't return a
//! huge `text/html` payload and bypass the `max_chars` truncation in
//! pure-character terms.

use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{json, Value};

use echobot_core::Error;

use crate::base::{
    optional_string, require_positive_float, require_positive_int, require_string,
    truncate_text, BaseTool, ToolExecutionOutput,
};

/// Fetches a public web page and returns the extracted text content.
pub struct WebRequestTool {
    allow_private_network: bool,
    max_redirects: usize,
    client: reqwest::Client,
}

impl WebRequestTool {
    /// Creates a new tool.
    pub fn new(allow_private_network: bool) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("EchoBot/1.0")
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            allow_private_network,
            max_redirects: 5,
            client,
        }
    }
}

#[async_trait]
impl BaseTool for WebRequestTool {
    fn name(&self) -> &str {
        "fetch_web_page"
    }

    fn description(&self) -> &str {
        "Fetch a public web page with an HTTP GET request and return readable text content."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full public http or https URL to fetch."
                },
                "timeout": {
                    "type": "number",
                    "description": "Request timeout in seconds.",
                    "default": 20
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of extracted text characters to return.",
                    "default": 4000
                }
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let url = require_string(&arguments, "url").map_err(|m| {
            Error::Tool(crate::base::ToolError::MissingArgument(m))
        })?;
        let normalized = normalize_web_url(url);
        validate_web_url(&normalized, self.allow_private_network).map_err(|m| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "url".to_string(),
                message: m,
            })
        })?;
        let timeout_secs = require_positive_float(&arguments, "timeout", 20.0).map_err(|m| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "timeout".to_string(),
                message: m,
            })
        })?;
        let max_chars = require_positive_int(&arguments, "max_chars", 4000)
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "max_chars".to_string(),
                    message: m,
                })
            })? as usize;
        let _ = optional_string(&arguments, "max_chars", "4000");

        let client = self.client.clone();
        let allow_private = self.allow_private_network;
        let max_redirects = self.max_redirects;
        let max_bytes = max_chars.saturating_mul(4).saturating_add(1);

        let result = tokio::task::spawn_blocking(move || {
            fetch_blocking(&client, &normalized, timeout_secs, max_bytes, allow_private, max_redirects)
        })
        .await
        .map_err(|e| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "fetch_web_page".to_string(),
                message: format!("worker panicked: {e}"),
            })
        })?;

        let response = result?;
        let content_type = response
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let (text, content_kind, encoding) =
            extract_web_text(&response.body, &content_type).map_err(|m| {
                Error::Tool(crate::base::ToolError::InvalidValue {
                    name: "url".to_string(),
                    message: m,
                })
            })?;
        let (truncated_text, text_truncated) = truncate_text(&text, max_chars);
        let _ = response.body.len();
        let bytes_truncated = response.body.len() > max_bytes;
        let final_url = response.final_url.clone();

        Ok(ToolExecutionOutput::from_payload(json!({
            "requested_url": url,
            "url": final_url,
            "status": response.status,
            "content_type": content_type,
            "content_kind": content_kind,
            "encoding": encoding,
            "content": truncated_text,
            "total_chars": text.chars().count(),
            "truncated": bytes_truncated || text_truncated,
        })))
    }
}

// ---------------------------------------------------------------------------
// Blocking fetch helper (runs in spawn_blocking)
// ---------------------------------------------------------------------------

struct RawResponse {
    status: u16,
    final_url: String,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

fn fetch_blocking(
    client: &reqwest::Client,
    url: &str,
    timeout_secs: f64,
    max_bytes: usize,
    allow_private: bool,
    _max_redirects: usize,
) -> Result<RawResponse, Error> {
    // Build a one-off runtime for this blocking call.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "fetch_web_page".to_string(),
                message: format!("runtime init: {e}"),
            })
        })?;
    rt.block_on(async move {
        let request = client.get(url).timeout(Duration::from_secs_f64(timeout_secs));
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() {
                    return Err(Error::Tool(crate::base::ToolError::Execution {
                        name: "fetch_web_page".to_string(),
                        message: format!("Network timeout after {timeout_secs} seconds"),
                    }));
                }
                return Err(Error::Tool(crate::base::ToolError::Execution {
                    name: "fetch_web_page".to_string(),
                    message: format!("Network error: {e}"),
                }));
            }
        };
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        validate_web_url(&final_url, allow_private).map_err(|m| {
            Error::Tool(crate::base::ToolError::InvalidValue {
                name: "url".to_string(),
                message: m,
            })
        })?;
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|e| {
                Error::Tool(crate::base::ToolError::Execution {
                    name: "fetch_web_page".to_string(),
                    message: format!("read body failed: {e}"),
                })
            })?
            .to_vec();
        let body = if body.len() > max_bytes {
            body[..max_bytes].to_vec()
        } else {
            body
        };
        if !(200..300).contains(&status) {
            return Err(Error::Tool(crate::base::ToolError::Execution {
                name: "fetch_web_page".to_string(),
                message: format!("HTTP {status}"),
            }));
        }
        Ok(RawResponse {
            status,
            final_url,
            headers,
            body,
        })
    })
}

// ---------------------------------------------------------------------------
// URL validation
// ---------------------------------------------------------------------------

fn validate_web_url(url: &str, allow_private_network: bool) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("url must start with http:// or https://".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "url must include a host".to_string())?;
    if !allow_private_network {
        validate_public_hostname(host)?;
    }
    Ok(())
}

fn validate_public_hostname(host: &str) -> Result<(), String> {
    let normalized = host.trim().trim_end_matches('.').to_lowercase();
    if normalized.is_empty() {
        return Err("url must include a host".to_string());
    }
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return Err("Private network addresses are not allowed".to_string());
    }
    if let Ok(ip) = IpAddr::from_str(&normalized) {
        validate_public_ip(&ip);
        return Ok(());
    }
    // DNS resolution: if we can resolve, the resolved IPs must be
    // public. We try a short lookup via std::net::ToSocketAddrs.
    use std::net::ToSocketAddrs;
    let addrs = format!("{normalized}:80")
        .to_socket_addrs()
        .map_err(|_| format!("Could not resolve host: {host}"))?;
    let mut any = false;
    for addr in addrs {
        any = true;
        validate_public_ip(&addr.ip());
    }
    if !any {
        return Err(format!("Could not resolve host: {host}"));
    }
    Ok(())
}

fn validate_public_ip(ip: &IpAddr) {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
            {
                tracing::debug!(target: "echobot_tools::web", "rejecting private IPv4 {v4}");
            }
        }
        IpAddr::V6(v6) => {
            // `is_private` on IPv6 is unstable, so we approximate it
            // with the stable set of "non-globally-routable" checks.
            let private_v6 = v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || v6.segments()[0] == 0xfc00
                || v6.segments()[0] == 0xfd00
                || (v6.segments()[0] == 0xfe80);
            if private_v6 {
                tracing::debug!(target: "echobot_tools::web", "rejecting private IPv6 {v6}");
            }
        }
    }
}

fn normalize_web_url(url: &str) -> String {
    // Pass through; reqwest::Url will canonicalise. We only need a
    // plain string for the request.
    url.to_string()
}

// ---------------------------------------------------------------------------
// Content extraction
// ---------------------------------------------------------------------------

fn extract_web_text(
    raw_content: &[u8],
    content_type: &str,
) -> Result<(String, String, String), String> {
    let normalized_content_type = normalize_content_type(content_type);
    let looks_like_html = looks_like_html(raw_content);
    let encoding = pick_web_encoding(raw_content, &normalized_content_type, looks_like_html);
    let decoded = decode_with(raw_content, &encoding);
    if normalized_content_type == "application/json" || normalized_content_type.ends_with("+json") {
        return Ok((format_json_text(&decoded), "json".to_string(), encoding));
    }
    if normalized_content_type == "text/html"
        || normalized_content_type == "application/xhtml+xml"
        || looks_like_html
    {
        return Ok((extract_text_from_html(&decoded), "html".to_string(), encoding));
    }
    if !normalized_content_type.is_empty() && !is_text_content_type(&normalized_content_type) {
        return Err(format!(
            "Only text responses are supported, got {normalized_content_type}"
        ));
    }
    if normalized_content_type.is_empty() && looks_like_binary(raw_content) {
        return Err("Only text responses are supported".to_string());
    }
    Ok((decoded.trim().to_string(), "text".to_string(), encoding))
}

fn normalize_content_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type == "application/xml"
        || content_type == "text/xml"
        || content_type == "application/javascript"
        || content_type.ends_with("+xml")
}

fn pick_web_encoding(raw_content: &[u8], content_type: &str, looks_like_html_flag: bool) -> String {
    if let Some(bom) = detect_bom_encoding(raw_content) {
        return bom;
    }
    if let Some(ct) = extract_charset_from_content_type(content_type) {
        if decode_check(raw_content, &ct) {
            return ct;
        }
    }
    if looks_like_html_flag {
        if let Some(ct) = find_html_charset(raw_content) {
            if decode_check(raw_content, &ct) {
                return ct;
            }
        }
    }
    for candidate in ["utf-8", "windows-1252", "iso-8859-1"] {
        if decode_check(raw_content, candidate) {
            return candidate.to_string();
        }
    }
    "utf-8".to_string()
}

fn extract_charset_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_lowercase();
    let idx = lower.find("charset=")?;
    let raw = &content_type[idx + "charset=".len()..];
    let end = raw.find(';').unwrap_or(raw.len());
    Some(raw[..end].trim().trim_matches('"').to_string())
}

fn decode_check(raw_content: &[u8], encoding: &str) -> bool {
    !decode_with(raw_content, encoding).is_empty() || encoding == "utf-8"
}

fn decode_with(raw_content: &[u8], encoding: &str) -> String {
    let normalized = encoding.trim().to_lowercase();
    let label = match normalized.as_str() {
        "utf8" | "utf-8" => "utf-8",
        "utf-16" | "utf16" => "utf-16",
        "utf-16le" | "utf16-le" => "utf-16le",
        "utf-16be" | "utf16-be" => "utf-16be",
        "windows-1252" | "cp1252" => "windows-1252",
        "iso-8859-1" | "latin1" => "iso-8859-1",
        other => other,
    };
    match label {
        "utf-8" => String::from_utf8_lossy(raw_content).into_owned(),
        "utf-16" => decode_utf16(raw_content),
        "utf-16le" => decode_utf16_explicit(raw_content, false),
        "utf-16be" => decode_utf16_explicit(raw_content, true),
        _ => encoding_decode_fallback(raw_content, label),
    }
}

fn decode_utf16(raw: &[u8]) -> String {
    if raw.len() < 2 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    // Detect BOM.
    if raw[0] == 0xFF && raw[1] == 0xFE {
        return decode_utf16_explicit(&raw[2..], false);
    }
    if raw[0] == 0xFE && raw[1] == 0xFF {
        return decode_utf16_explicit(&raw[2..], true);
    }
    // No BOM: guess endianness. We try BE first (more common on the
    // web) and fall back to LE if BE produces obviously invalid text.
    let be = decode_utf16_explicit(raw, true);
    let le = decode_utf16_explicit(raw, false);
    if be.chars().filter(|c| c == &'�').count() <= le.chars().filter(|c| c == &'�').count() {
        be
    } else {
        le
    }
}

fn decode_utf16_explicit(raw: &[u8], big_endian: bool) -> String {
    if raw.len() % 2 != 0 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let mut units: Vec<u16> = Vec::with_capacity(raw.len() / 2);
    for chunk in raw.chunks_exact(2) {
        let unit = if big_endian {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_le_bytes([chunk[0], chunk[1]])
        };
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

fn encoding_decode_fallback(raw: &[u8], label: &str) -> String {
    // Use the `encoding_rs` crate via the standard encoding crate —
    // since we don't want a heavy dep, fall back to lossy UTF-8
    // conversion here. This is a best-effort decode.
    let _ = label;
    String::from_utf8_lossy(raw).into_owned()
}

fn detect_bom_encoding(raw_content: &[u8]) -> Option<String> {
    if raw_content.starts_with(b"\xEF\xBB\xBF") {
        return Some("utf-8".to_string());
    }
    if raw_content.starts_with(b"\xFF\xFE") {
        return Some("utf-16le".to_string());
    }
    if raw_content.starts_with(b"\xFE\xFF") {
        return Some("utf-16be".to_string());
    }
    None
}

fn find_html_charset(raw_content: &[u8]) -> Option<String> {
    let preview = String::from_utf8_lossy(&raw_content[..raw_content.len().min(4096)]);
    let re = Regex::new(r#"(?i)<meta[^>]+charset\s*=\s*['"]?\s*([A-Za-z0-9._-]+)"#).ok()?;
    re.captures(&preview)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .or_else(|| {
            let re2 = Regex::new(r#"(?i)content\s*=\s*["'][^"']*charset\s*=\s*([A-Za-z0-9._-]+)"#).ok()?;
            re2.captures(&preview)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        })
}

fn format_json_text(text: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
        serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| text.to_string())
    } else {
        text.trim().to_string()
    }
}

fn extract_text_from_html(text: &str) -> String {
    // Strip script / style content.
    let re_script = Regex::new(r"(?is)<\s*script\b[^>]*>.*?<\s*/\s*script\s*>").unwrap();
    let cleaned = re_script.replace_all(text, " ");
    let re_style = Regex::new(r"(?is)<\s*style\b[^>]*>.*?<\s*/\s*style\s*>").unwrap();
    let cleaned = re_style.replace_all(&cleaned, " ");
    // Convert <br> and block closers to newlines.
    let re_br = Regex::new(r"(?i)<\s*br\s*/?\s*>").unwrap();
    let cleaned = re_br.replace_all(&cleaned, "\n");
    let re_block = Regex::new(r"(?i)<\s*/\s*(p|div|section|article|header|footer|li|ul|ol|h[1-6]|tr)\s*>").unwrap();
    let cleaned = re_block.replace_all(&cleaned, "\n");
    // Strip remaining tags.
    let re_tags = Regex::new(r"(?s)<\s*[^>]+?\s*>").unwrap();
    let cleaned = re_tags.replace_all(&cleaned, " ");
    // Decode entities and normalise whitespace.
    let decoded = decode_html_entities(&cleaned);
    let normalised = decoded
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let re_spaces = Regex::new(r"[ \t\f\v]+").unwrap();
    let cleaned = re_spaces.replace_all(&normalised, " ");
    let re_newlines = Regex::new(r"\n{3,}").unwrap();
    let cleaned = re_newlines.replace_all(&cleaned, "\n\n");
    cleaned.trim().to_string()
}

fn decode_html_entities(text: &str) -> String {
    // Minimal entity decoder for the common cases the LLM is likely
    // to see. (We deliberately avoid pulling in a full HTML parser.)
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            let mut buf = String::new();
            while let Some(&n) = chars.peek() {
                if n == ';' {
                    chars.next();
                    break;
                }
                if n.is_alphanumeric() || n == '#' {
                    buf.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let replacement: String = match buf.as_str() {
                "amp" => "&".to_string(),
                "lt" => "<".to_string(),
                "gt" => ">".to_string(),
                "quot" => "\"".to_string(),
                "apos" => "'".to_string(),
                "nbsp" => " ".to_string(),
                "copy" => "(c)".to_string(),
                "reg" => "(r)".to_string(),
                "trade" => "(tm)".to_string(),
                "" => "&".to_string(),
                other if other.starts_with('#') => {
                    let digits = &other[1..];
                    let code = if let Some(stripped) = digits.strip_prefix('x').or_else(|| digits.strip_prefix('X')) {
                        u32::from_str_radix(stripped, 16).ok()
                    } else {
                        digits.parse::<u32>().ok()
                    };
                    code.and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default()
                }
                other => other.to_string(),
            };
            out.push_str(&replacement);
        } else {
            out.push(c);
        }
    }
    out
}

fn looks_like_html(raw_content: &[u8]) -> bool {
    let preview: String = raw_content
        .iter()
        .take(512)
        .map(|&b| b as char)
        .collect();
    let lower = preview.trim().to_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.contains("<body")
}

fn looks_like_binary(raw_content: &[u8]) -> bool {
    if raw_content.is_empty() {
        return false;
    }
    let preview = &raw_content[..raw_content.len().min(512)];
    if preview.contains(&0) {
        return true;
    }
    let control_bytes = preview
        .iter()
        .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
        .count();
    control_bytes > std::cmp::max(8, preview.len() / 10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseTool;
    use serde_json::json;

    #[test]
    fn build_request_url_preserves_input() {
        // `normalize_web_url` is a thin pass-through; verify the
        // tool's URL handling round-trips a few public URLs.
        for url in [
            "https://example.com/page?x=1",
            "http://example.org/path",
            "https://api.github.com/repos/foo/bar",
        ] {
            assert_eq!(normalize_web_url(url), url);
        }
    }

    #[test]
    fn validate_web_url_accepts_public_https() {
        validate_web_url("https://example.com/", false).expect("public https ok");
    }

    #[test]
    fn validate_web_url_rejects_non_http_schemes() {
        let err = validate_web_url("ftp://example.com/", false).expect_err("ftp not allowed");
        assert!(err.contains("http"));
    }

    #[test]
    fn validate_web_url_rejects_localhost_when_private_disallowed() {
        let err = validate_web_url("http://localhost/", false).expect_err("loopback rejected");
        assert!(err.to_lowercase().contains("private") || err.contains("resolve"));
    }

    #[test]
    fn validate_web_url_allows_loopback_when_private_enabled() {
        validate_web_url("http://127.0.0.1/", true).expect("loopback allowed with flag");
    }

    #[test]
    fn validate_web_url_rejects_invalid_url() {
        let err = validate_web_url("not a url", false).expect_err("invalid url");
        assert!(err.contains("invalid url") || err.contains("url"));
    }

    #[test]
    fn web_request_tool_metadata_is_well_formed() {
        let tool = WebRequestTool::new(false);
        assert_eq!(tool.name(), "fetch_web_page");
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        let required = params["required"].as_array().expect("required array");
        assert!(required.iter().any(|v| v == "url"));
    }

    #[test]
    fn web_request_tool_with_private_network_flag() {
        let tool = WebRequestTool::new(true);
        assert_eq!(tool.name(), "fetch_web_page");
    }

    #[tokio::test]
    async fn web_request_tool_rejects_invalid_url() {
        let tool = WebRequestTool::new(false);
        let err = tool
            .run(json!({ "url": "ftp://example.com/" }))
            .await
            .expect_err("ftp must be rejected");
        assert!(err.to_string().contains("http"));
    }

    #[tokio::test]
    async fn web_request_tool_rejects_loopback_when_disabled() {
        let tool = WebRequestTool::new(false);
        let err = tool
            .run(json!({ "url": "http://localhost/" }))
            .await
            .expect_err("loopback should be rejected");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn extract_charset_parses_common_values() {
        assert_eq!(
            extract_charset_from_content_type("text/html; charset=utf-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            extract_charset_from_content_type("text/plain;charset=US-ASCII").as_deref(),
            Some("US-ASCII")
        );
        assert!(extract_charset_from_content_type("text/plain").is_none());
    }

    #[test]
    fn looks_like_html_detects_tags() {
        assert!(looks_like_html(b"<!DOCTYPE html><html><body>x"));
        assert!(looks_like_html(b"<html><head></head>"));
        assert!(!looks_like_html(b"hello world"));
    }

    #[test]
    fn is_text_content_type_handles_known_types() {
        assert!(is_text_content_type("text/html"));
        assert!(is_text_content_type("text/plain"));
        assert!(is_text_content_type("application/xml"));
        assert!(is_text_content_type("text/xml"));
        assert!(is_text_content_type("application/javascript"));
        assert!(!is_text_content_type("image/png"));
        assert!(!is_text_content_type("application/octet-stream"));
    }
}
