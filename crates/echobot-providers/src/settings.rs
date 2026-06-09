//! Settings for the OpenAI-compatible provider.
//!
//! Mirrors `OpenAICompatibleSettings` in `echobot/providers/openai_compatible.py`.

use std::collections::HashMap;
use std::env::VarError;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use echobot_core::error::{ConfigError, Result};

/// Default `base_url` for OpenAI-compatible providers.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default request timeout.
pub const DEFAULT_TIMEOUT_SECS: f64 = 60.0;

/// Settings used to construct an [`OpenAICompatibleProvider`](crate::openai_compatible::OpenAICompatibleProvider).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAICompatibleSettings {
    /// API key (sent as `Authorization: Bearer <key>`).
    pub api_key: String,
    /// Model name.
    pub model: String,
    /// Base URL of the chat completions endpoint.
    pub base_url: String,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Extra HTTP headers sent with every request.
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    /// Extra body fields merged into every request.
    #[serde(default)]
    pub extra_body: HashMap<String, Value>,
}

impl OpenAICompatibleSettings {
    /// Loads settings from the process environment, using the
    /// `LLM_API_KEY` / `LLM_MODEL` / `LLM_BASE_URL` / `LLM_TIMEOUT` /
    /// `LLM_EXTRA_BODY` variables (configurable via `prefix`).
    pub fn from_env(prefix: Option<&str>) -> Result<Self> {
        Self::from_env_map(&env_to_map(), prefix.unwrap_or("LLM_"))
    }

    /// Loads settings from a custom env map (for tests).
    pub fn from_env_map(env: &HashMap<String, String>, prefix: &str) -> Result<Self> {
        let api_key_name = format!("{prefix}API_KEY");
        let model_name = format!("{prefix}MODEL");
        let base_url_name = format!("{prefix}BASE_URL");
        let timeout_name = format!("{prefix}TIMEOUT");
        let extra_body_name = format!("{prefix}EXTRA_BODY");

        let api_key = require(env, &api_key_name)?;
        let model = require(env, &model_name)?;
        let base_url = optional(env, &base_url_name).unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let timeout_text = optional(env, &timeout_name).unwrap_or_else(|| DEFAULT_TIMEOUT_SECS.to_string());
        let timeout_secs: f64 = timeout_text.parse().map_err(|e: std::num::ParseFloatError| {
            ConfigError::InvalidEnvValue {
                name: timeout_name.clone(),
                message: format!("must be a number: {e}"),
            }
        })?;
        if timeout_secs <= 0.0 {
            return Err(ConfigError::InvalidEnvValue {
                name: timeout_name,
                message: "must be a positive number".to_string(),
            }
            .into());
        }

        let extra_body = match optional(env, &extra_body_name) {
            None => HashMap::new(),
            Some(text) => {
                let parsed: Value = serde_json::from_str(&text).map_err(|e| {
                    ConfigError::InvalidEnvValue {
                        name: extra_body_name.clone(),
                        message: format!("must be valid JSON: {e}"),
                    }
                })?;
                match parsed {
                    Value::Object(map) => map.into_iter().collect(),
                    _ => {
                        return Err(ConfigError::InvalidEnvValue {
                            name: extra_body_name,
                            message: "must be a JSON object".to_string(),
                        }
                        .into());
                    }
                }
            }
        };

        Ok(Self {
            api_key,
            model,
            base_url,
            timeout: Duration::from_secs_f64(timeout_secs),
            extra_headers: HashMap::new(),
            extra_body,
        })
    }
}

fn env_to_map() -> HashMap<String, String> {
    // `VarError` shouldn't fire here, but we still convert via Result to
    // keep the API uniform.
    std::env::vars().collect::<HashMap<_, _>>()
}

fn require(env: &HashMap<String, String>, name: &str) -> Result<String> {
    env.get(name)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ConfigError::MissingEnvVar(name.to_string()).into())
}

fn optional(env: &HashMap<String, String>, name: &str) -> Option<String> {
    env.get(name)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[allow(dead_code)]
fn _ensure_var_error_in_scope() -> Option<VarError> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(items: &[(&str, &str)]) -> HashMap<String, String> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn loads_required_fields() {
        let map = env(&[("LLM_API_KEY", "k"), ("LLM_MODEL", "m")]);
        let s = OpenAICompatibleSettings::from_env_map(&map, "LLM_").unwrap();
        assert_eq!(s.api_key, "k");
        assert_eq!(s.model, "m");
        assert_eq!(s.base_url, DEFAULT_BASE_URL);
        assert!((s.timeout.as_secs_f64() - DEFAULT_TIMEOUT_SECS).abs() < 1e-6);
    }

    #[test]
    fn honors_prefix_override() {
        let map = env(&[("FOO_API_KEY", "k"), ("FOO_MODEL", "m")]);
        let s = OpenAICompatibleSettings::from_env_map(&map, "FOO_").unwrap();
        assert_eq!(s.api_key, "k");
        assert_eq!(s.model, "m");
    }

    #[test]
    fn parses_extra_body() {
        let map = env(&[
            ("LLM_API_KEY", "k"),
            ("LLM_MODEL", "m"),
            ("LLM_EXTRA_BODY", "{\"x\":1}"),
        ]);
        let s = OpenAICompatibleSettings::from_env_map(&map, "LLM_").unwrap();
        assert_eq!(s.extra_body.get("x").unwrap(), &serde_json::json!(1));
    }

    #[test]
    fn rejects_non_object_extra_body() {
        let map = env(&[
            ("LLM_API_KEY", "k"),
            ("LLM_MODEL", "m"),
            ("LLM_EXTRA_BODY", "[1, 2]"),
        ]);
        assert!(OpenAICompatibleSettings::from_env_map(&map, "LLM_").is_err());
    }

    #[test]
    fn missing_api_key_errors() {
        let map = env(&[("LLM_MODEL", "m")]);
        assert!(OpenAICompatibleSettings::from_env_map(&map, "LLM_").is_err());
    }
}
