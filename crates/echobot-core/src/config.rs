//! Environment loading and application configuration.
//!
//! Mirrors `echobot/config.py`. The Python version also configures the
//! `loguru` / stdlib loggers — in Rust that responsibility lives with
//! `tracing-subscriber` (see `echobot-cli`), so this module only provides
//! `.env` loading and the typed [`AppConfig`] struct.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Result};

/// Valid log-level names accepted by the env loader.
pub const VALID_LOG_LEVELS: &[&str] = &["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"];

/// Loads environment variables from `path` into the process environment.
///
/// Lines starting with `#` and blank lines are ignored. A line must be of
/// the form `KEY=value`. Values wrapped in matching single or double quotes
/// have the quotes stripped. If `override` is false (the default), existing
/// environment variables take precedence over values in the file.
pub fn load_env_file(path: impl AsRef<Path>, override_existing: bool) -> Result<()> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(());
    }

    let contents = std::fs::read_to_string(path)?;
    for (line_number, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| ConfigError::InvalidEnvLine {
                path: path.display().to_string(),
                line_number: line_number + 1,
                message: format!("missing '=' separator in {line:?}"),
            })?;

        let key = key.trim();
        if key.is_empty() {
            return Err(ConfigError::InvalidEnvLine {
                path: path.display().to_string(),
                line_number: line_number + 1,
                message: "missing env key".to_string(),
            }
            .into());
        }

        let mut value = value.trim().to_string();
        if value.len() >= 2 && value.starts_with(value.chars().next().unwrap()) && value.ends_with(value.chars().next().unwrap()) {
            let first = value.chars().next().unwrap();
            if first == '"' || first == '\'' {
                value = value[1..value.len() - 1].to_string();
            }
        }

        if override_existing || std::env::var_os(key).is_none() {
            // SAFETY: this is the same pattern `dotenvy` uses.
            unsafe {
                std::env::set_var(key, &value);
            }
        }
    }
    Ok(())
}

/// Resolves a log level string. Returns `Ok(None)` if `value` is empty.
pub fn parse_log_level(name: &str, value: &str) -> Result<Option<String>> {
    let cleaned = value.trim().to_uppercase();
    if cleaned.is_empty() {
        return Ok(None);
    }
    if !VALID_LOG_LEVELS.contains(&cleaned.as_str()) {
        return Err(ConfigError::InvalidLogLevel {
            name: name.to_string(),
            value: cleaned.clone(),
            valid: VALID_LOG_LEVELS.join(", "),
        }
        .into());
    }
    Ok(Some(cleaned))
}

/// Top-level typed application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Workspace directory used for sessions, memory, attachments, etc.
    pub workspace: PathBuf,

    /// Path to the `.env` file (loaded at startup if it exists).
    pub env_file: Option<PathBuf>,

    /// LLM provider base URL (e.g. `https://api.openai.com/v1`).
    pub llm_base_url: String,

    /// LLM model name.
    pub llm_model: String,

    /// LLM API key.
    pub llm_api_key: String,

    /// Request timeout for the LLM provider.
    pub llm_timeout: Duration,

    /// Optional sampling temperature override.
    pub temperature: Option<f32>,

    /// Optional max-tokens override.
    pub max_tokens: Option<u32>,

    /// Extra body fields merged into every LLM request.
    #[serde(default)]
    pub llm_extra_body: HashMap<String, serde_json::Value>,

    /// Extra HTTP headers sent with every LLM request.
    #[serde(default)]
    pub llm_extra_headers: HashMap<String, String>,

    /// Whether built-in tools are enabled.
    pub enable_tools: bool,

    /// Whether skills are enabled.
    pub enable_skills: bool,

    /// Whether long-term memory is enabled.
    pub enable_memory: bool,

    /// Whether the heartbeat service is enabled.
    pub enable_heartbeat: bool,

    /// Heartbeat check interval.
    pub heartbeat_interval: Duration,

    /// Log level for the `echobot` namespace.
    pub log_level: String,
}

impl AppConfig {
    /// Loads an [`AppConfig`] from the process environment. Reads `LLM_*`,
    /// `ECHOBOT_*` variables. Returns an error if required variables are
    /// missing.
    pub fn from_env() -> Result<Self> {
        Self::from_env_map(&env_to_map())
    }

    /// Loads an [`AppConfig`] from a custom env map (handy for tests).
    pub fn from_env_map(env: &HashMap<String, String>) -> Result<Self> {
        let api_key = require(env, "LLM_API_KEY")?;
        let model = require(env, "LLM_MODEL")?;
        let base_url = optional(env, "LLM_BASE_URL")
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let timeout_secs: f64 = optional(env, "LLM_TIMEOUT")
            .and_then(|s| s.parse().ok())
            .unwrap_or(60.0);
        if timeout_secs <= 0.0 {
            return Err(ConfigError::InvalidEnvValue {
                name: "LLM_TIMEOUT".to_string(),
                message: "must be a positive number".to_string(),
            }
            .into());
        }

        let extra_body = match optional(env, "LLM_EXTRA_BODY") {
            None => HashMap::new(),
            Some(text) => {
                let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                    ConfigError::InvalidEnvValue {
                        name: "LLM_EXTRA_BODY".to_string(),
                        message: format!("must be valid JSON: {e}"),
                    }
                })?;
                match parsed {
                    serde_json::Value::Object(map) => map.into_iter().collect(),
                    _ => {
                        return Err(ConfigError::InvalidEnvValue {
                            name: "LLM_EXTRA_BODY".to_string(),
                            message: "must be a JSON object".to_string(),
                        }
                        .into());
                    }
                }
            }
        };

        let temperature = optional(env, "ECHOBOT_TEMPERATURE").and_then(|s| s.parse().ok());
        let max_tokens = optional(env, "ECHOBOT_MAX_TOKENS").and_then(|s| s.parse().ok());

        let workspace = PathBuf::from(
            optional(env, "ECHOBOT_WORKSPACE").unwrap_or_else(|| ".echobot".to_string()),
        );
        let env_file = optional(env, "ECHOBOT_ENV_FILE").map(PathBuf::from);

        let enable_tools = parse_bool(optional(env, "ECHOBOT_ENABLE_TOOLS").as_deref(), true);
        let enable_skills = parse_bool(optional(env, "ECHOBOT_ENABLE_SKILLS").as_deref(), true);
        let enable_memory = parse_bool(optional(env, "ECHOBOT_ENABLE_MEMORY").as_deref(), true);
        let enable_heartbeat =
            parse_bool(optional(env, "ECHOBOT_ENABLE_HEARTBEAT").as_deref(), true);

        let heartbeat_secs: f64 = optional(env, "ECHOBOT_HEARTBEAT_INTERVAL_SECONDS")
            .and_then(|s| s.parse().ok())
            .unwrap_or(30.0 * 60.0);
        if heartbeat_secs <= 0.0 {
            return Err(ConfigError::InvalidEnvValue {
                name: "ECHOBOT_HEARTBEAT_INTERVAL_SECONDS".to_string(),
                message: "must be a positive number".to_string(),
            }
            .into());
        }

        let log_level = match optional(env, "ECHOBOT_LOG_LEVEL") {
            None => "INFO".to_string(),
            Some(value) => {
                if let Some(level) = parse_log_level("ECHOBOT_LOG_LEVEL", &value)? {
                    level
                } else {
                    "INFO".to_string()
                }
            }
        };

        Ok(Self {
            workspace,
            env_file,
            llm_base_url: base_url,
            llm_model: model,
            llm_api_key: api_key,
            llm_timeout: Duration::from_secs_f64(timeout_secs),
            temperature,
            max_tokens,
            llm_extra_body: extra_body,
            llm_extra_headers: HashMap::new(),
            enable_tools,
            enable_skills,
            enable_memory,
            enable_heartbeat,
            heartbeat_interval: Duration::from_secs_f64(heartbeat_secs),
            log_level,
        })
    }
}

fn env_to_map() -> HashMap<String, String> {
    std::env::vars().collect()
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

fn parse_bool(value: Option<&str>, default: bool) -> bool {
    match value {
        None => default,
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(keys: &[(&str, &str)]) -> HashMap<String, String> {
        keys.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn loads_minimal_required_env() {
        let map = env(&[("LLM_API_KEY", "sk-test"), ("LLM_MODEL", "gpt-x")]);
        let cfg = AppConfig::from_env_map(&map).unwrap();
        assert_eq!(cfg.llm_api_key, "sk-test");
        assert_eq!(cfg.llm_model, "gpt-x");
        assert_eq!(cfg.llm_base_url, "https://api.openai.com/v1");
        assert!((cfg.llm_timeout.as_secs_f64() - 60.0).abs() < 1e-6);
    }

    #[test]
    fn missing_api_key_errors() {
        let map = env(&[("LLM_MODEL", "gpt-x")]);
        let err = AppConfig::from_env_map(&map).unwrap_err();
        assert!(format!("{err}").contains("LLM_API_KEY"));
    }

    #[test]
    fn parses_extra_body() {
        let map = env(&[
            ("LLM_API_KEY", "k"),
            ("LLM_MODEL", "m"),
            ("LLM_EXTRA_BODY", "{\"x\": 1}"),
        ]);
        let cfg = AppConfig::from_env_map(&map).unwrap();
        assert_eq!(
            cfg.llm_extra_body.get("x").unwrap(),
            &serde_json::json!(1)
        );
    }

    #[test]
    fn parses_booleans() {
        assert!(parse_bool(Some("true"), false));
        assert!(!parse_bool(Some("FALSE"), true));
        assert_eq!(parse_bool(None, true), true);
        assert_eq!(parse_bool(None, false), false);
    }

    #[test]
    fn parses_log_level() {
        assert_eq!(parse_log_level("X", "debug").unwrap(), Some("DEBUG".into()));
        assert!(parse_log_level("X", "BOGUS").is_err());
        assert_eq!(parse_log_level("X", "").unwrap(), None);
    }
}
