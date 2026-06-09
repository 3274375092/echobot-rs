//! Runtime configuration and per-user overrides.
//!
//! Mirrors `echobot/runtime/settings.py`. The settings are split into three
//! concerns:
//!
//! 1. [`RuntimeConfigSnapshot`] — a frozen, in-memory view of the effective
//!    values (resolved from overrides + defaults).
//! 2. [`RuntimeControls`] — a mutable container that the agent/tools layer
//!    consults at request time (e.g. `file_write_enabled`).
//! 3. [`RuntimeSettings`] + [`RuntimeSettingsStore`] — the on-disk override
//!    layer the user can edit. [`RuntimeSettingsManager`] glues all three
//!    together and is the API the CLI / HTTP surface uses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{Error, Result};

/// Default shell-safety mode applied when no override exists.
pub const DEFAULT_SHELL_SAFETY_MODE: &str = "danger-full-access";

/// Description of a single runtime setting — used by `/runtime` introspection
/// and by documentation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSettingDefinition {
    /// The setting's name (e.g. `"delegated_ack_enabled"`).
    pub name: String,
    /// Human hint for the value shape (e.g. `"on|off"`).
    pub value_hint: String,
    /// One-line description of what the setting controls.
    pub description: String,
}

/// Snapshot of the effective runtime configuration. Frozen and cheap to copy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfigSnapshot {
    /// Show the task-start tip before background work.
    pub delegated_ack_enabled: bool,
    /// Shell safety mode (validated via [`normalize_shell_safety_mode`]).
    pub shell_safety_mode: String,
    /// Allow write tools.
    pub file_write_enabled: bool,
    /// Allow cron-mutating tools.
    pub cron_mutation_enabled: bool,
    /// Allow web tools to access localhost and private network hosts.
    pub web_private_network_enabled: bool,
}

impl Default for RuntimeConfigSnapshot {
    fn default() -> Self {
        Self {
            delegated_ack_enabled: true,
            shell_safety_mode: DEFAULT_SHELL_SAFETY_MODE.to_string(),
            file_write_enabled: true,
            cron_mutation_enabled: true,
            web_private_network_enabled: false,
        }
    }
}

impl RuntimeConfigSnapshot {
    /// Validates the shell-safety mode field. Returns
    /// [`Error::InvalidRuntimeSettingValue`] if the value is unknown.
    pub fn normalize(&mut self) -> Result<()> {
        self.shell_safety_mode = normalize_shell_safety_mode(&self.shell_safety_mode)?;
        Ok(())
    }

    /// Serializes the snapshot to a JSON value (matches Python's `to_dict`).
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "delegated_ack_enabled": self.delegated_ack_enabled,
            "shell_safety_mode": self.shell_safety_mode,
            "file_write_enabled": self.file_write_enabled,
            "cron_mutation_enabled": self.cron_mutation_enabled,
            "web_private_network_enabled": self.web_private_network_enabled,
        })
    }
}

/// Catalog of all runtime settings the runtime understands.
pub fn runtime_setting_definitions() -> HashMap<String, RuntimeSettingDefinition> {
    let mut defs = HashMap::new();
    defs.insert(
        "delegated_ack_enabled".into(),
        RuntimeSettingDefinition {
            name: "delegated_ack_enabled".into(),
            value_hint: "on|off".into(),
            description: "Show the task-start tip before background work".into(),
        },
    );
    defs.insert(
        "shell_safety_mode".into(),
        RuntimeSettingDefinition {
            name: "shell_safety_mode".into(),
            value_hint: "read-only|workspace-write|danger-full-access".into(),
            description: "Control which shell commands the agent may run".into(),
        },
    );
    defs.insert(
        "file_write_enabled".into(),
        RuntimeSettingDefinition {
            name: "file_write_enabled".into(),
            value_hint: "on|off".into(),
            description: "Allow write_text_file and edit_text_file".into(),
        },
    );
    defs.insert(
        "cron_mutation_enabled".into(),
        RuntimeSettingDefinition {
            name: "cron_mutation_enabled".into(),
            value_hint: "on|off".into(),
            description: "Allow the agent to add, remove, run, enable, or disable cron jobs"
                .into(),
        },
    );
    defs.insert(
        "web_private_network_enabled".into(),
        RuntimeSettingDefinition {
            name: "web_private_network_enabled".into(),
            value_hint: "on|off".into(),
            description: "Allow fetch_web_page to access localhost and private network hosts"
                .into(),
        },
    );
    defs
}

/// Mutable runtime control values. The agent + tools read from this struct
/// at request time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeControls {
    /// Shell-safety mode.
    pub shell_safety_mode: String,
    /// Whether write tools are allowed.
    pub file_write_enabled: bool,
    /// Whether cron-mutating tools are allowed.
    pub cron_mutation_enabled: bool,
    /// Whether web tools may hit private network hosts.
    pub web_private_network_enabled: bool,
}

impl Default for RuntimeControls {
    fn default() -> Self {
        Self {
            shell_safety_mode: DEFAULT_SHELL_SAFETY_MODE.to_string(),
            file_write_enabled: true,
            cron_mutation_enabled: true,
            web_private_network_enabled: false,
        }
    }
}

impl RuntimeControls {
    /// Normalizes the shell-safety mode.
    pub fn normalize(&mut self) -> Result<()> {
        self.shell_safety_mode = normalize_shell_safety_mode(&self.shell_safety_mode)?;
        Ok(())
    }

    /// Sets the shell-safety mode and validates the value.
    pub fn set_shell_safety_mode(&mut self, value: &str) -> Result<()> {
        self.shell_safety_mode = normalize_shell_safety_mode(value)?;
        Ok(())
    }

    /// Sets the file-write toggle.
    pub fn set_file_write_enabled(&mut self, value: bool) {
        self.file_write_enabled = value;
    }

    /// Sets the cron-mutation toggle.
    pub fn set_cron_mutation_enabled(&mut self, value: bool) {
        self.cron_mutation_enabled = value;
    }

    /// Sets the private-network toggle.
    pub fn set_web_private_network_enabled(&mut self, value: bool) {
        self.web_private_network_enabled = value;
    }
}

/// Trait for any object that exposes a `delegated_ack_enabled` toggle.
/// `RuntimeControls`-like objects can also be added by implementing
/// `set_delegated_ack_enabled` so the manager can push values back.
pub trait RuntimeSettingsCoordinator: Send + Sync {
    /// Returns the current `delegated_ack_enabled` value.
    fn delegated_ack_enabled(&self) -> bool;
    /// Sets the current `delegated_ack_enabled` value.
    fn set_delegated_ack_enabled(&mut self, enabled: bool);
}

/// In-memory override layer that mirrors the on-disk `runtime_settings.json`.
///
/// `None` for a field means "no override; use the default".
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeSettings {
    /// Override for `delegated_ack_enabled`.
    pub delegated_ack_enabled: Option<bool>,
    /// Override for `selected_asr_provider`.
    pub selected_asr_provider: Option<String>,
    /// Override for `shell_safety_mode`.
    pub shell_safety_mode: Option<String>,
    /// Override for `file_write_enabled`.
    pub file_write_enabled: Option<bool>,
    /// Override for `cron_mutation_enabled`.
    pub cron_mutation_enabled: Option<bool>,
    /// Override for `web_private_network_enabled`.
    pub web_private_network_enabled: Option<bool>,
    /// Any extra keys the file may contain (preserved across load/save).
    #[serde(default)]
    pub extra_values: HashMap<String, Value>,
}

impl RuntimeSettings {
    /// Parses a settings value from its on-disk JSON shape.
    pub fn from_value(data: &Value) -> Result<Self> {
        let obj = data
            .as_object()
            .ok_or_else(|| Error::InvalidRuntimeSettingValue {
                name: "runtime_settings.json".into(),
                message: "must be a JSON object".into(),
            })?;
        let mut settings = RuntimeSettings::default();
        let mut extra: HashMap<String, Value> = HashMap::new();
        for (key, value) in obj {
            match key.as_str() {
                "delegated_ack_enabled" => {
                    settings.delegated_ack_enabled = optional_bool(value, "delegated_ack_enabled")?;
                }
                "selected_asr_provider" => {
                    settings.selected_asr_provider = optional_string(value, "selected_asr_provider")?;
                }
                "shell_safety_mode" => {
                    settings.shell_safety_mode = match value {
                        Value::Null => None,
                        Value::String(s) => Some(normalize_shell_safety_mode(s)?),
                        _ => {
                            return Err(Error::InvalidRuntimeSettingValue {
                                name: "shell_safety_mode".into(),
                                message: "must be a string".into(),
                            });
                        }
                    };
                }
                "file_write_enabled" => {
                    settings.file_write_enabled = optional_bool(value, "file_write_enabled")?;
                }
                "cron_mutation_enabled" => {
                    settings.cron_mutation_enabled = optional_bool(value, "cron_mutation_enabled")?;
                }
                "web_private_network_enabled" => {
                    settings.web_private_network_enabled =
                        optional_bool(value, "web_private_network_enabled")?;
                }
                _ => {
                    extra.insert(key.clone(), value.clone());
                }
            }
        }
        settings.extra_values = extra;
        Ok(settings)
    }

    /// Serializes to its on-disk JSON shape. Unknown keys are not emitted.
    pub fn to_value(&self) -> Value {
        let mut map = serde_json::Map::new();
        for (k, v) in &self.extra_values {
            map.insert(k.clone(), v.clone());
        }
        if let Some(b) = self.delegated_ack_enabled {
            map.insert("delegated_ack_enabled".into(), Value::Bool(b));
        }
        if let Some(s) = &self.selected_asr_provider {
            map.insert("selected_asr_provider".into(), Value::String(s.clone()));
        }
        if let Some(s) = &self.shell_safety_mode {
            map.insert("shell_safety_mode".into(), Value::String(s.clone()));
        }
        if let Some(b) = self.file_write_enabled {
            map.insert("file_write_enabled".into(), Value::Bool(b));
        }
        if let Some(b) = self.cron_mutation_enabled {
            map.insert("cron_mutation_enabled".into(), Value::Bool(b));
        }
        if let Some(b) = self.web_private_network_enabled {
            map.insert("web_private_network_enabled".into(), Value::Bool(b));
        }
        Value::Object(map)
    }

    /// Returns a named value or `None` if the name is unknown.
    pub fn get_named_value(&self, name: &str) -> Option<Value> {
        match name {
            "delegated_ack_enabled" => self.delegated_ack_enabled.map(Value::Bool),
            "selected_asr_provider" => self.selected_asr_provider.clone().map(Value::String),
            "shell_safety_mode" => self.shell_safety_mode.clone().map(Value::String),
            "file_write_enabled" => self.file_write_enabled.map(Value::Bool),
            "cron_mutation_enabled" => self.cron_mutation_enabled.map(Value::Bool),
            "web_private_network_enabled" => self.web_private_network_enabled.map(Value::Bool),
            other => self.extra_values.get(other).cloned(),
        }
    }

    /// Sets a named value. Pass `Value::Null` (or a matching typed value) to
    /// clear the override.
    pub fn set_named_value(&mut self, name: &str, value: Value) -> Result<()> {
        match name {
            "delegated_ack_enabled" => {
                self.delegated_ack_enabled = Some(required_bool(&value, "delegated_ack_enabled")?);
            }
            "selected_asr_provider" => match value {
                Value::Null => self.selected_asr_provider = None,
                Value::String(s) => {
                    let cleaned = s.trim();
                    self.selected_asr_provider =
                        if cleaned.is_empty() { None } else { Some(cleaned.to_string()) };
                }
                _ => {
                    return Err(Error::InvalidRuntimeSettingValue {
                        name: "selected_asr_provider".into(),
                        message: "must be a string".into(),
                    });
                }
            },
            "shell_safety_mode" => match value {
                Value::Null => self.shell_safety_mode = None,
                Value::String(s) => {
                    self.shell_safety_mode = Some(normalize_shell_safety_mode(&s)?);
                }
                _ => {
                    return Err(Error::InvalidRuntimeSettingValue {
                        name: "shell_safety_mode".into(),
                        message: "must be a string".into(),
                    });
                }
            },
            "file_write_enabled" => {
                self.file_write_enabled = Some(required_bool(&value, "file_write_enabled")?);
            }
            "cron_mutation_enabled" => {
                self.cron_mutation_enabled = Some(required_bool(&value, "cron_mutation_enabled")?);
            }
            "web_private_network_enabled" => {
                self.web_private_network_enabled =
                    Some(required_bool(&value, "web_private_network_enabled")?);
            }
            other => {
                if value.is_null() {
                    self.extra_values.remove(other);
                } else {
                    self.extra_values.insert(other.to_string(), value);
                }
            }
        }
        Ok(())
    }

    /// Clears the override for a named setting.
    pub fn clear_named_value(&mut self, name: &str) -> Result<()> {
        match name {
            "delegated_ack_enabled" => self.delegated_ack_enabled = None,
            "selected_asr_provider" => self.selected_asr_provider = None,
            "shell_safety_mode" => self.shell_safety_mode = None,
            "file_write_enabled" => self.file_write_enabled = None,
            "cron_mutation_enabled" => self.cron_mutation_enabled = None,
            "web_private_network_enabled" => self.web_private_network_enabled = None,
            other => {
                self.extra_values.remove(other);
            }
        }
        Ok(())
    }
}

/// Thread-safe, on-disk store for [`RuntimeSettings`].
#[derive(Debug)]
pub struct RuntimeSettingsStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl RuntimeSettingsStore {
    /// Creates a new store pointing at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Returns the on-disk path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the settings from disk, returning an empty [`RuntimeSettings`]
    /// if the file doesn't exist yet.
    pub async fn load(&self) -> Result<RuntimeSettings> {
        let _g = self.lock.lock().await;
        Self::load_unlocked(&self.path).await
    }

    /// Saves `settings` to disk, replacing the existing file.
    pub async fn save(&self, settings: RuntimeSettings) -> Result<RuntimeSettings> {
        let _g = self.lock.lock().await;
        Self::save_unlocked(&self.path, &settings).await?;
        Ok(settings)
    }

    /// Loads, applies `updater`, then saves the result.
    pub async fn update(
        &self,
        updater: impl FnOnce(&mut RuntimeSettings),
    ) -> Result<RuntimeSettings> {
        let _g = self.lock.lock().await;
        let mut settings = Self::load_unlocked(&self.path).await?;
        updater(&mut settings);
        Self::save_unlocked(&self.path, &settings).await?;
        Ok(settings)
    }

    /// Convenience: sets a single named value and saves.
    pub async fn update_named_value(
        &self,
        name: &str,
        value: Value,
    ) -> Result<RuntimeSettings> {
        self.update(|s| {
            let _ = s.set_named_value(name, value);
        })
        .await
    }

    async fn load_unlocked(path: &Path) -> Result<RuntimeSettings> {
        if !path.exists() {
            return Ok(RuntimeSettings::default());
        }
        let path_clone = path.to_path_buf();
        let raw = tokio::task::spawn_blocking(move || std::fs::read_to_string(path_clone))
            .await
            .map_err(|e| Error::Wiring(format!("settings read task failed: {e}")))??;
        let value: Value = serde_json::from_str(&raw).map_err(|e| {
            Error::InvalidRuntimeSettingValue {
                name: path.display().to_string(),
                message: e.to_string(),
            }
        })?;
        RuntimeSettings::from_value(&value)
    }

    async fn save_unlocked(path: &Path, settings: &RuntimeSettings) -> Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let path_clone = path.to_path_buf();
        let value = settings.to_value();
        let parent_clone = parent.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<()> {
            std::fs::create_dir_all(&parent_clone)?;
            let text = serde_json::to_string_pretty(&value)?;
            std::fs::write(&path_clone, format!("{text}\n"))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Wiring(format!("settings write task failed: {e}")))??;
        Ok(())
    }
}

/// Glues [`RuntimeSettingsStore`] to a [`RuntimeSettingsCoordinator`] (for
/// `delegated_ack_enabled`) and a [`RuntimeControls`] (for everything else).
pub struct RuntimeSettingsManager<C: RuntimeSettingsCoordinator> {
    coordinator: Arc<Mutex<C>>,
    runtime_controls: Arc<Mutex<RuntimeControls>>,
    store: RuntimeSettingsStore,
}

impl<C: RuntimeSettingsCoordinator> RuntimeSettingsManager<C> {
    /// Creates a new manager rooted at `<workspace>/.echobot/runtime_settings.json`.
    pub fn new(
        workspace: impl AsRef<Path>,
        coordinator: Arc<Mutex<C>>,
        runtime_controls: Arc<Mutex<RuntimeControls>>,
    ) -> Self {
        let path = workspace.as_ref().join(".echobot").join("runtime_settings.json");
        Self {
            coordinator,
            runtime_controls,
            store: RuntimeSettingsStore::new(path),
        }
    }

    /// Creates a manager pointing at an explicit store path.
    pub fn with_store(
        store: RuntimeSettingsStore,
        coordinator: Arc<Mutex<C>>,
        runtime_controls: Arc<Mutex<RuntimeControls>>,
    ) -> Self {
        Self {
            coordinator,
            runtime_controls,
            store,
        }
    }

    /// Returns the catalog of known setting definitions.
    pub fn definitions(&self) -> HashMap<String, RuntimeSettingDefinition> {
        runtime_setting_definitions()
    }

    /// Returns a JSON snapshot of the current effective values.
    pub async fn snapshot(&self) -> Result<Value> {
        let coordinator = self.coordinator.lock().await;
        let controls = self.runtime_controls.lock().await;
        Ok(runtime_settings_snapshot(&*coordinator, &controls))
    }

    /// Looks up a single setting by name.
    pub async fn get(&self, name: &str) -> Result<Value> {
        let normalized = normalize_runtime_setting_name(name)?;
        let snap = self.snapshot().await?;
        snap.get(&normalized)
            .cloned()
            .ok_or(Error::UnknownRuntimeSetting(normalized))
    }

    /// Applies a single override.
    pub async fn apply_named_value(&self, name: &str, value: Value) -> Result<Value> {
        let normalized = normalize_runtime_setting_name(name)?;
        let mut updates = HashMap::new();
        updates.insert(normalized, value);
        self.apply_updates(updates).await
    }

    /// Applies a batch of overrides. `null` values clear the override.
    pub async fn apply_updates(
        &self,
        updates: HashMap<String, Value>,
    ) -> Result<Value> {
        let normalized = normalize_runtime_updates(updates);
        if normalized.is_empty() {
            return Err(Error::InvalidRuntimeSettingValue {
                name: "updates".into(),
                message: "At least one runtime setting must be provided".into(),
            });
        }
        let mut copy = normalized.clone();
        self.store
            .update(|settings| {
                for (name, value) in &normalized {
                    let _ = settings.set_named_value(name, value.clone());
                }
            })
            .await?;
        for (name, value) in &normalized {
            apply_runtime_setting(&self.coordinator, &self.runtime_controls, name, value.clone())
                .await?;
            // consume the value so we can use it as a "check" later if needed
            copy.remove(name);
        }
        self.snapshot().await
    }

    /// Clears every override and applies `defaults` to the live state.
    pub async fn reset_overrides(
        &self,
        defaults: HashMap<String, Value>,
    ) -> Result<Value> {
        let normalized = normalize_runtime_defaults(defaults)?;
        self.store
            .update(|settings| {
                let names: Vec<String> = runtime_setting_definitions()
                    .keys()
                    .cloned()
                    .collect();
                for name in names {
                    let _ = settings.clear_named_value(&name);
                }
            })
            .await?;
        for (name, value) in &normalized {
            apply_runtime_setting(&self.coordinator, &self.runtime_controls, name, value.clone())
                .await?;
        }
        self.snapshot().await
    }
}

/// Computes the effective settings snapshot from the coordinator + controls.
pub fn runtime_settings_snapshot(
    coordinator: &dyn RuntimeSettingsCoordinator,
    runtime_controls: &RuntimeControls,
) -> Value {
    serde_json::json!({
        "delegated_ack_enabled": coordinator.delegated_ack_enabled(),
        "shell_safety_mode": runtime_controls.shell_safety_mode,
        "file_write_enabled": runtime_controls.file_write_enabled,
        "cron_mutation_enabled": runtime_controls.cron_mutation_enabled,
        "web_private_network_enabled": runtime_controls.web_private_network_enabled,
    })
}

/// Parses a textual setting value (CLI input) into its typed form.
pub fn parse_text_runtime_setting_value(name: &str, raw: &str) -> Result<Value> {
    let normalized = normalize_runtime_setting_name(name)?;
    let cleaned = raw.trim().to_lowercase();
    match normalized.as_str() {
        "delegated_ack_enabled" => Ok(Value::Bool(parse_on_off(&cleaned, "delegated_ack_enabled")?)),
        "shell_safety_mode" => Ok(Value::String(normalize_shell_safety_mode(&cleaned)?)),
        "file_write_enabled" | "cron_mutation_enabled" | "web_private_network_enabled" => {
            Ok(Value::Bool(parse_on_off(&cleaned, &normalized)?))
        }
        _ => Err(Error::UnknownRuntimeSetting(normalized)),
    }
}

/// Formats a setting value back into a user-facing string.
pub fn format_runtime_setting_value(name: &str, value: &Value) -> Result<String> {
    let normalized = normalize_runtime_setting_name(name)?;
    match normalized.as_str() {
        "shell_safety_mode" => Ok(value.as_str().unwrap_or("").to_string()),
        _ => {
            let on = match value {
                Value::Bool(b) => *b,
                _ => false,
            };
            Ok(if on { "on".to_string() } else { "off".to_string() })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Allowed values for `shell_safety_mode`.
pub const SHELL_SAFETY_MODES: &[&str] = &["read-only", "workspace-write", "danger-full-access"];

/// Validates a shell-safety mode value.
pub fn normalize_shell_safety_mode(value: &str) -> Result<String> {
    let cleaned = value.trim().to_lowercase();
    if SHELL_SAFETY_MODES.contains(&cleaned.as_str()) {
        Ok(cleaned)
    } else {
        Err(Error::InvalidRuntimeSettingValue {
            name: "shell_safety_mode".into(),
            message: format!(
                "must be one of: {}",
                SHELL_SAFETY_MODES.join(", ")
            ),
        })
    }
}

fn normalize_runtime_setting_name(name: &str) -> Result<String> {
    let cleaned = name.trim().to_lowercase();
    if runtime_setting_definitions().contains_key(&cleaned) {
        Ok(cleaned)
    } else {
        Err(Error::UnknownRuntimeSetting(cleaned))
    }
}

fn normalize_runtime_updates(updates: HashMap<String, Value>) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for (raw_name, value) in updates {
        if value.is_null() {
            continue;
        }
        if let Ok(name) = normalize_runtime_setting_name(&raw_name) {
            out.insert(name, value);
        }
    }
    out
}

fn normalize_runtime_defaults(
    defaults: HashMap<String, Value>,
) -> Result<HashMap<String, Value>> {
    let defs = runtime_setting_definitions();
    let mut out = HashMap::new();
    for name in defs.keys() {
        let value = defaults.get(name).cloned().ok_or_else(|| {
            Error::InvalidRuntimeSettingValue {
                name: name.clone(),
                message: "missing default".into(),
            }
        })?;
        if name == "shell_safety_mode" {
            let s = value
                .as_str()
                .ok_or_else(|| Error::InvalidRuntimeSettingValue {
                    name: name.clone(),
                    message: "must be a string".into(),
                })?;
            out.insert(name.clone(), Value::String(normalize_shell_safety_mode(s)?));
        } else {
            if !value.is_boolean() {
                return Err(Error::InvalidRuntimeSettingValue {
                    name: name.clone(),
                    message: "must be a boolean".into(),
                });
            }
            out.insert(name.clone(), value);
        }
    }
    Ok(out)
}

fn optional_bool(value: &Value, name: &str) -> Result<Option<bool>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(required_bool(value, name)?))
}

fn optional_string(value: &Value, name: &str) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => {
            let cleaned = s.trim();
            Ok(if cleaned.is_empty() {
                None
            } else {
                Some(cleaned.to_string())
            })
        }
        _ => Err(Error::InvalidRuntimeSettingValue {
            name: name.into(),
            message: "must be a string".into(),
        }),
    }
}

fn required_bool(value: &Value, name: &str) -> Result<bool> {
    match value {
        Value::Bool(b) => Ok(*b),
        _ => Err(Error::InvalidRuntimeSettingValue {
            name: name.into(),
            message: "must be a boolean".into(),
        }),
    }
}

fn parse_on_off(cleaned: &str, name: &str) -> Result<bool> {
    match cleaned {
        "on" | "true" | "enable" | "enabled" => Ok(true),
        "off" | "false" | "disable" | "disabled" => Ok(false),
        _ => Err(Error::InvalidRuntimeSettingValue {
            name: name.into(),
            message: "Use on or off".into(),
        }),
    }
}

async fn apply_runtime_setting<C: RuntimeSettingsCoordinator>(
    coordinator: &Arc<Mutex<C>>,
    runtime_controls: &Arc<Mutex<RuntimeControls>>,
    name: &str,
    value: Value,
) -> Result<()> {
    match name {
        "delegated_ack_enabled" => {
            let b = required_bool(&value, "delegated_ack_enabled")?;
            coordinator.lock().await.set_delegated_ack_enabled(b);
        }
        "shell_safety_mode" => {
            let s = value
                .as_str()
                .ok_or_else(|| Error::InvalidRuntimeSettingValue {
                    name: "shell_safety_mode".into(),
                    message: "must be a string".into(),
                })?;
            runtime_controls.lock().await.set_shell_safety_mode(s)?;
        }
        "file_write_enabled" => {
            let b = required_bool(&value, "file_write_enabled")?;
            runtime_controls.lock().await.set_file_write_enabled(b);
        }
        "cron_mutation_enabled" => {
            let b = required_bool(&value, "cron_mutation_enabled")?;
            runtime_controls.lock().await.set_cron_mutation_enabled(b);
        }
        "web_private_network_enabled" => {
            let b = required_bool(&value, "web_private_network_enabled")?;
            runtime_controls.lock().await.set_web_private_network_enabled(b);
        }
        _ => return Err(Error::UnknownRuntimeSetting(name.into())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_shell_safety_mode_is_case_insensitive() {
        assert_eq!(normalize_shell_safety_mode("Read-Only").unwrap(), "read-only");
        assert!(normalize_shell_safety_mode("nope").is_err());
    }

    #[test]
    fn parse_text_runtime_setting_value_handles_on_off() {
        assert_eq!(
            parse_text_runtime_setting_value("file_write_enabled", "on")
                .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            parse_text_runtime_setting_value("file_write_enabled", "OFF")
                .unwrap(),
            Value::Bool(false)
        );
        assert!(parse_text_runtime_setting_value("file_write_enabled", "maybe").is_err());
    }

    #[test]
    fn settings_round_trip() {
        let mut s = RuntimeSettings::default();
        s.set_named_value("file_write_enabled", Value::Bool(false)).unwrap();
        s.set_named_value("shell_safety_mode", Value::String("read-only".into()))
            .unwrap();
        let value = s.to_value();
        let parsed = RuntimeSettings::from_value(&value).unwrap();
        assert_eq!(parsed.file_write_enabled, Some(false));
        assert_eq!(parsed.shell_safety_mode.as_deref(), Some("read-only"));
    }
}
