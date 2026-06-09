//! `RouteMode` enum and per-session metadata helpers.
//!
//! Mirrors `echobot/orchestration/route_modes.py`. The three route modes
//! are: auto (default — let the decision layer choose), chat_only (always
//! chat), and force_agent (always delegate to the full agent).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// The default route mode used when no value is supplied in metadata.
pub const DEFAULT_ROUTE_MODE: RouteMode = RouteMode::Auto;

/// All valid route mode string values (lowercase).
pub const ROUTE_MODE_VALUES: [&str; 3] = ["auto", "chat_only", "force_agent"];

/// Per-session route mode. Determines whether a turn is handled by the
/// lightweight chat layer, the full agent, or decided automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RouteMode {
    /// Let the decision layer route the turn (regex rules + optional LLM).
    #[default]
    Auto,
    /// Force the lightweight chat reply (skip the agent entirely).
    ChatOnly,
    /// Force delegation to the full agent.
    ForceAgent,
}

impl RouteMode {
    /// The lowercase string form used in session metadata and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            RouteMode::Auto => "auto",
            RouteMode::ChatOnly => "chat_only",
            RouteMode::ForceAgent => "force_agent",
        }
    }

    /// Parses a `RouteMode` from the lowercase string form.
    pub fn parse(value: &str) -> Self {
        normalize_route_mode(Some(value))
    }
}

impl std::fmt::Display for RouteMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Normalizes a free-form value into a [`RouteMode`]. Returns
/// [`DEFAULT_ROUTE_MODE`] for `None`, empty, or unknown inputs.
pub fn normalize_route_mode(value: Option<&str>) -> RouteMode {
    let cleaned = value.unwrap_or("").trim().to_lowercase();
    match cleaned.as_str() {
        "auto" => RouteMode::Auto,
        "chat_only" => RouteMode::ChatOnly,
        "force_agent" => RouteMode::ForceAgent,
        _ => DEFAULT_ROUTE_MODE,
    }
}

/// Reads the route mode from a session metadata map. Missing or invalid
/// values fall back to [`DEFAULT_ROUTE_MODE`].
pub fn route_mode_from_metadata(metadata: Option<&HashMap<String, Value>>) -> RouteMode {
    let Some(metadata) = metadata else {
        return DEFAULT_ROUTE_MODE;
    };
    let value = metadata.get("route_mode");
    match value {
        Some(Value::String(s)) => normalize_route_mode(Some(s.as_str())),
        _ => DEFAULT_ROUTE_MODE,
    }
}

/// Returns a new metadata map with `route_mode` set to the string form of
/// `route_mode`. The input map is not mutated.
pub fn set_route_mode(
    metadata: &HashMap<String, Value>,
    route_mode: RouteMode,
) -> HashMap<String, Value> {
    let mut next = metadata.clone();
    next.insert(
        "route_mode".to_string(),
        Value::String(route_mode.as_str().to_string()),
    );
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto() {
        assert_eq!(DEFAULT_ROUTE_MODE, RouteMode::Auto);
        assert_eq!(RouteMode::default(), RouteMode::Auto);
    }

    #[test]
    fn round_trip_string() {
        for mode in [RouteMode::Auto, RouteMode::ChatOnly, RouteMode::ForceAgent] {
            assert_eq!(RouteMode::parse(mode.as_str()), mode);
        }
    }

    #[test]
    fn normalize_handles_invalid_values() {
        assert_eq!(normalize_route_mode(None), RouteMode::Auto);
        assert_eq!(normalize_route_mode(Some("")), RouteMode::Auto);
        assert_eq!(normalize_route_mode(Some("  ")), RouteMode::Auto);
        assert_eq!(normalize_route_mode(Some("nope")), RouteMode::Auto);
        assert_eq!(normalize_route_mode(Some("CHAT_ONLY")), RouteMode::ChatOnly);
    }

    #[test]
    fn metadata_helpers_round_trip() {
        let mut meta: HashMap<String, Value> = HashMap::new();
        meta.insert("role_name".to_string(), Value::String("default".to_string()));
        let next = set_route_mode(&meta, RouteMode::ChatOnly);
        assert_eq!(
            next.get("route_mode").and_then(Value::as_str),
            Some("chat_only")
        );
        assert_eq!(
            route_mode_from_metadata(Some(&next)),
            RouteMode::ChatOnly
        );
    }

    #[test]
    fn metadata_handles_missing_or_invalid() {
        let meta: HashMap<String, Value> = HashMap::new();
        assert_eq!(route_mode_from_metadata(Some(&meta)), RouteMode::Auto);
        assert_eq!(route_mode_from_metadata(None), RouteMode::Auto);
    }
}
