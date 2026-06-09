//! Live2D metadata service — verbatim port of
//! `echobot/app/services/web_console/live2d/metadata.py`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use super::annotations::Live2DAnnotationsRepository;
use super::constants::{
    display_hotkey_token, hotkey_token_map, supported_hotkey_actions,
    LIVE2D_AUTO_MOTION_GROUP, LIVE2D_IDLE_MOTION_GROUP, LIVE2D_SOURCE_WORKSPACE,
};
use super::models::{
    Live2DDiscoveredExpression, Live2DDiscoveredHotkey, Live2DDiscoveredMetadata,
    Live2DDiscoveredMotion, Live2DModelCandidate, Live2DVTubeConfig,
};

static TOKEN_MAP: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(hotkey_token_map);
static SUPPORTED_ACTIONS: Lazy<HashSet<&'static str>> = Lazy::new(supported_hotkey_actions);

pub struct Live2DMetadataService {
    annotations: Live2DAnnotationsRepository,
}

impl Live2DMetadataService {
    pub fn new(annotations: Live2DAnnotationsRepository) -> Self {
        Self { annotations }
    }

    /// Load and parse the `.model3.json` file.
    pub fn load_model_data(&self, candidate: &Live2DModelCandidate) -> Result<Value, String> {
        load_json_file(&candidate.model_path)
    }

    /// Extract parameter IDs from the DisplayInfo file referenced by model_data.
    pub fn load_parameter_ids(
        &self,
        candidate: &Live2DModelCandidate,
        model_data: &Value,
    ) -> Vec<String> {
        let Some(display_info_path) = self.display_info_path(candidate, model_data) else {
            return Vec::new();
        };
        if !display_info_path.exists() {
            return Vec::new();
        }
        let Ok(display_info) = load_json_file(&display_info_path) else {
            return Vec::new();
        };
        let Some(params) = display_info.get("Parameters").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        params
            .iter()
            .filter_map(|p| p.get("Id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect()
    }

    /// Get parameter IDs for a named group (e.g. "LipSync").
    pub fn load_group_parameter_ids(
        &self,
        model_data: &Value,
        group_name: &str,
    ) -> Vec<String> {
        let Some(groups) = model_data.get("Groups").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        for group in groups {
            let Some(obj) = group.as_object() else {
                continue;
            };
            if obj.get("Target").and_then(|v| v.as_str()) != Some("Parameter") {
                continue;
            }
            if obj.get("Name").and_then(|v| v.as_str()) != Some(group_name) {
                continue;
            }
            return obj
                .get("Ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
        }
        Vec::new()
    }

    /// Discover all metadata for a model candidate.
    pub fn discover_metadata(
        &self,
        candidate: &Live2DModelCandidate,
        model_data: &Value,
    ) -> Live2DDiscoveredMetadata {
        let annotations_payload = self.annotations.load(&candidate.runtime_root);
        let vtube_config = self.load_matching_vtube_config(candidate);
        let expressions = self.discover_expressions(candidate, model_data, vtube_config.as_ref(), &annotations_payload);
        let motions = self.discover_motions(candidate, model_data, vtube_config.as_ref(), &annotations_payload);
        let hotkeys = self.discover_hotkeys(
            candidate,
            vtube_config.as_ref(),
            &expressions,
            &motions,
            &annotations_payload,
        );
        Live2DDiscoveredMetadata {
            expressions,
            motions,
            hotkeys,
            annotations_writable: candidate.source == LIVE2D_SOURCE_WORKSPACE,
        }
    }

    /// Patch model3.json with discovered expressions/motions.
    pub fn patch_model_data(
        &self,
        candidate: &Live2DModelCandidate,
        model_data: &Value,
        metadata: &Live2DDiscoveredMetadata,
    ) -> Value {
        let mut patched = model_data.clone();
        let file_refs = patched
            .as_object_mut()
            .and_then(|obj| {
                if !obj.contains_key("FileReferences") {
                    obj.insert("FileReferences".to_string(), serde_json::json!({}));
                }
                obj.get_mut("FileReferences")
            })
            .and_then(|v| v.as_object_mut());

        let Some(file_refs) = file_refs else {
            return patched;
        };

        // Expressions
        let rel_exprs: Vec<Value> = metadata
            .expressions
            .iter()
            .map(|expr| {
                let rel = self.relative_file_to_model_parent(candidate, &expr.asset_relative_path);
                serde_json::json!({"Name": expr.name, "File": rel})
            })
            .collect();
        if !rel_exprs.is_empty() {
            file_refs.insert("Expressions".to_string(), Value::Array(rel_exprs));
        }

        // Motions
        let existing_motions = file_refs
            .get("Motions")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut patched_motions: HashMap<String, Vec<Value>> = HashMap::new();
        for (group_name, entries) in existing_motions.iter() {
            if let Some(arr) = entries.as_array() {
                patched_motions.insert(
                    group_name.clone(),
                    arr.iter()
                        .filter_map(|v| v.as_object().map(|o| {
                            o.iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect::<HashMap<String, Value>>()
                        }))
                        .map(|m| Value::Object(m.into_iter().collect()))
                        .collect(),
                );
            }
        }

        for motion in &metadata.motions {
            let mut motion_entry = motion.definition.clone();
            let rel = self.relative_file_to_model_parent(candidate, &motion.asset_relative_path);
            motion_entry.insert("File".to_string(), Value::String(rel));
            let entries = patched_motions
                .entry(motion.group.clone())
                .or_default();
            while entries.len() <= motion.index {
                entries.push(Value::Object(Default::default()));
            }
            let existing_file = entries[motion.index]
                .get("File")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());
            if existing_file.is_none() {
                entries[motion.index] = Value::Object(motion_entry.into_iter().collect());
            }
        }

        if !patched_motions.is_empty() {
            let obj: serde_json::Map<String, Value> = patched_motions
                .into_iter()
                .map(|(k, v)| (k, Value::Array(v)))
                .collect();
            file_refs.insert("Motions".to_string(), Value::Object(obj));
        }

        patched
    }

    /// Build the hotkey payload for the frontend.
    pub fn hotkey_payload(&self, hotkey: &Live2DDiscoveredHotkey) -> serde_json::Value {
        serde_json::json!({
            "hotkey_key": hotkey.hotkey_key,
            "hotkey_id": hotkey.hotkey_id,
            "name": hotkey.name,
            "action": hotkey.action,
            "file": hotkey.file,
            "shortcut_tokens": hotkey.shortcut_tokens,
            "shortcut_label": hotkey.shortcut_label,
            "target_kind": hotkey.target_kind,
            "supported": hotkey.supported,
        })
    }

    pub fn normalize_annotation_file(&self, file: &str) -> Result<String, String> {
        let normalized = file.replace('\\', "/").trim().trim_start_matches("./").to_string();
        if normalized.is_empty() {
            return Err("Live2D annotation file must not be empty".to_string());
        }
        if normalized.starts_with('/') {
            return Err(format!("Invalid Live2D annotation file: {file}"));
        }
        let pp = Path::new(&normalized);
        for comp in pp.components() {
            let s = comp.as_os_str().to_string_lossy();
            if s.is_empty() || s == "." || s == ".." {
                return Err(format!("Invalid Live2D annotation file: {file}"));
            }
        }
        Ok(normalized)
    }

    pub fn normalize_shortcut_tokens(&self, shortcut_tokens: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for token in shortcut_tokens.iter().take(3) {
            let nt = self.normalize_hotkey_token(token);
            if !nt.is_empty() && !out.contains(&nt) {
                out.push(nt);
            }
        }
        out
    }

    // --- private ---

    fn load_matching_vtube_config(
        &self,
        candidate: &Live2DModelCandidate,
    ) -> Option<Live2DVTubeConfig> {
        let mut vtube_paths: Vec<PathBuf> = Vec::new();
        collect_vtube_files(&candidate.runtime_root, &candidate.runtime_root, &mut vtube_paths);
        if vtube_paths.is_empty() {
            return None;
        }
        vtube_paths.sort_by(|a, b| {
            let da = a.components().count();
            let db = b.components().count();
            da.cmp(&db).then_with(|| a.to_string_lossy().cmp(&b.to_string_lossy()))
        });

        let resolved_model_path = std::fs::canonicalize(&candidate.model_path).ok()?;
        let mut matching: Vec<Live2DVTubeConfig> = Vec::new();
        let mut fallback: Vec<Live2DVTubeConfig> = Vec::new();

        for vtube_path in &vtube_paths {
            let Ok(payload) = load_json_file(vtube_path) else {
                continue;
            };
            let config = Live2DVTubeConfig {
                path: std::fs::canonicalize(vtube_path).unwrap_or_else(|_| vtube_path.clone()),
                payload,
            };
            if let Some(model_ref) = config
                .payload
                .get("FileReferences")
                .and_then(|v| v.get("Model"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                let reference_path =
                    std::fs::canonicalize(vtube_path.parent().unwrap_or(&candidate.runtime_root).join(model_ref));
                if let Ok(ref rp) = reference_path {
                    if *rp == resolved_model_path {
                        matching.push(config);
                        continue;
                    }
                }
            }
            fallback.push(config);
        }

        if !matching.is_empty() {
            matching.sort_by(|a, b| {
                let da = a
                    .path
                    .strip_prefix(&candidate.runtime_root)
                    .map(|p| p.components().count())
                    .unwrap_or(usize::MAX);
                let db = b
                    .path
                    .strip_prefix(&candidate.runtime_root)
                    .map(|p| p.components().count())
                    .unwrap_or(usize::MAX);
                da.cmp(&db)
            });
            return Some(matching.into_iter().next().unwrap());
        }
        if fallback.len() == 1 {
            return Some(fallback.into_iter().next().unwrap());
        }
        None
    }

    fn discover_expressions(
        &self,
        candidate: &Live2DModelCandidate,
        model_data: &Value,
        vtube_config: Option<&Live2DVTubeConfig>,
        annotations_payload: &HashMap<String, Value>,
    ) -> Vec<Live2DDiscoveredExpression> {
        let mut expression_map: HashMap<String, Live2DDiscoveredExpression> = HashMap::new();
        let note_map = annotations_payload
            .get("expressions")
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect::<HashMap<String, String>>()
            })
            .unwrap_or_default();

        // From model3.json FileReferences.Expressions
        if let Some(exprs) = model_data
            .get("FileReferences")
            .and_then(|v| v.get("Expressions"))
            .and_then(|v| v.as_array())
        {
            for expr in exprs {
                let Some(obj) = expr.as_object() else {
                    continue;
                };
                self.add_expression_reference(
                    &mut expression_map,
                    candidate,
                    &candidate.model_path.parent().unwrap_or(&candidate.runtime_root),
                    obj.get("File").and_then(|v| v.as_str()),
                    obj.get("Name").and_then(|v| v.as_str()),
                    &note_map,
                );
            }
        }

        // From vtube.json Hotkeys
        if let Some(vt) = vtube_config {
            if let Some(hotkeys) = vt.payload.get("Hotkeys").and_then(|v| v.as_array()) {
                for hk in hotkeys {
                    let Some(obj) = hk.as_object() else {
                        continue;
                    };
                    if obj.get("Action").and_then(|v| v.as_str()) != Some("ToggleExpression") {
                        continue;
                    }
                    self.add_expression_reference(
                        &mut expression_map,
                        candidate,
                        vt.path.parent().unwrap_or(&candidate.runtime_root),
                        obj.get("File").and_then(|v| v.as_str()),
                        obj.get("Name").and_then(|v| v.as_str()),
                        &note_map,
                    );
                }
            }
        }

        // Discover *.exp3.json files
        let mut exp3_paths: Vec<PathBuf> = Vec::new();
        collect_glob(&candidate.runtime_root, &candidate.runtime_root, ".exp3.json", &mut exp3_paths);
        exp3_paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        for exp_path in exp3_paths {
            let note_key = exp_path
                .strip_prefix(&candidate.runtime_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if expression_map.contains_key(&note_key) {
                continue;
            }
            expression_map.insert(
                note_key.clone(),
                Live2DDiscoveredExpression {
                    name: asset_name_from_file(
                        exp_path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                        ".exp3.json",
                    ),
                    file: note_key.clone(),
                    asset_relative_path: exp_path
                        .strip_prefix(&candidate.source_root)
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default(),
                    note: note_map.get(&note_key).cloned().unwrap_or_default(),
                },
            );
        }

        expression_map.into_values().collect()
    }

    fn add_expression_reference(
        &self,
        expression_map: &mut HashMap<String, Live2DDiscoveredExpression>,
        candidate: &Live2DModelCandidate,
        base_directory: &Path,
        file_reference: Option<&str>,
        name_hint: Option<&str>,
        note_map: &HashMap<String, String>,
    ) {
        let Some(asset_path) = self.resolve_runtime_reference(
            &candidate.runtime_root,
            base_directory,
            file_reference,
        ) else {
            return;
        };
        let note_key = asset_path
            .strip_prefix(&candidate.runtime_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if expression_map.contains_key(&note_key) {
            return;
        }
        let name = name_hint
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                asset_name_from_file(
                    asset_path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    ".exp3.json",
                )
            });
        expression_map.insert(
            note_key.clone(),
            Live2DDiscoveredExpression {
                name,
                file: note_key.clone(),
                asset_relative_path: asset_path
                    .strip_prefix(&candidate.source_root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default(),
                note: note_map.get(&note_key).cloned().unwrap_or_default(),
            },
        );
    }

    fn discover_motions(
        &self,
        candidate: &Live2DModelCandidate,
        model_data: &Value,
        vtube_config: Option<&Live2DVTubeConfig>,
        annotations_payload: &HashMap<String, Value>,
    ) -> Vec<Live2DDiscoveredMotion> {
        let mut pending: Vec<HashMap<String, Value>> = Vec::new();
        let note_map: HashMap<String, String> = annotations_payload
            .get("motions")
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // From model3.json FileReferences.Motions
        if let Some(motions) = model_data
            .get("FileReferences")
            .and_then(|v| v.get("Motions"))
            .and_then(|v| v.as_object())
        {
            for (group_name, entries) in motions {
                let Some(arr) = entries.as_array() else {
                    continue;
                };
                for (index, entry) in arr.iter().enumerate() {
                    let Some(obj) = entry.as_object() else {
                        continue;
                    };
                    self.append_motion_reference(
                        &mut pending,
                        candidate,
                        &candidate.model_path.parent().unwrap_or(&candidate.runtime_root),
                        obj.get("File").and_then(|v| v.as_str()),
                        group_name,
                        index as i64,
                        obj.get("Name").and_then(|v| v.as_str()),
                        &note_map,
                        obj.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    );
                }
            }
        }

        // From vtube.json
        if let Some(vt) = vtube_config {
            // IdleAnimation
            if let Some(idle) = vt
                .payload
                .get("FileReferences")
                .and_then(|v| v.get("IdleAnimation"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                self.append_motion_reference(
                    &mut pending,
                    candidate,
                    vt.path.parent().unwrap_or(&candidate.runtime_root),
                    Some(idle),
                    LIVE2D_IDLE_MOTION_GROUP,
                    -1,
                    Some("Idle"),
                    &note_map,
                    {
                        let mut m = HashMap::new();
                        m.insert("File".to_string(), Value::String(idle.to_string()));
                        m
                    },
                );
            }
            // Hotkeys with TriggerAnimation
            if let Some(hotkeys) = vt.payload.get("Hotkeys").and_then(|v| v.as_array()) {
                for hk in hotkeys {
                    let Some(obj) = hk.as_object() else {
                        continue;
                    };
                    if obj.get("Action").and_then(|v| v.as_str()) != Some("TriggerAnimation") {
                        continue;
                    }
                    self.append_motion_reference(
                        &mut pending,
                        candidate,
                        vt.path.parent().unwrap_or(&candidate.runtime_root),
                        obj.get("File").and_then(|v| v.as_str()),
                        LIVE2D_AUTO_MOTION_GROUP,
                        -1,
                        obj.get("Name").and_then(|v| v.as_str()),
                        &note_map,
                        {
                            let mut m = HashMap::new();
                            if let Some(f) = obj.get("File").and_then(|v| v.as_str()) {
                                m.insert("File".to_string(), Value::String(f.to_string()));
                            }
                            m
                        },
                    );
                }
            }
        }

        // Discover *.motion3.json files
        let mut motion_paths: Vec<PathBuf> = Vec::new();
        collect_glob(&candidate.runtime_root, &candidate.runtime_root, ".motion3.json", &mut motion_paths);
        motion_paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        for motion_path in motion_paths {
            let note_key = motion_path
                .strip_prefix(&candidate.runtime_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if pending.iter().any(|m| {
                m.get("file").and_then(|v| v.as_str()) == Some(&note_key)
            }) {
                continue;
            }
            let name = asset_name_from_file(
                motion_path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                ".motion3.json",
            );
            let asset_rel = motion_path
                .strip_prefix(&candidate.source_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let mut m = HashMap::new();
            m.insert("name".to_string(), Value::String(name.clone()));
            m.insert("file".to_string(), Value::String(note_key.clone()));
            m.insert("asset_relative_path".to_string(), Value::String(asset_rel));
            m.insert(
                "note".to_string(),
                Value::String(note_map.get(&note_key).cloned().unwrap_or_default()),
            );
            m.insert("group".to_string(), Value::String(LIVE2D_AUTO_MOTION_GROUP.to_string()));
            m.insert("index".to_string(), Value::Number((-1).into()));
            let mut def = HashMap::new();
            def.insert("File".to_string(), Value::String(note_key.clone()));
            m.insert("definition".to_string(), Value::Object(def.into_iter().collect()));
            pending.push(m);
        }

        // Assign indices
        let mut grouped_indexes: HashMap<String, usize> = HashMap::new();
        let mut seen_files: HashSet<String> = HashSet::new();
        let mut discovered: Vec<Live2DDiscoveredMotion> = Vec::new();

        for motion in &pending {
            let Some(file_key) = motion.get("file").and_then(|v| v.as_str()) else {
                continue;
            };
            if seen_files.contains(file_key) {
                continue;
            }
            seen_files.insert(file_key.to_string());

            let group = motion
                .get("group")
                .and_then(|v| v.as_str())
                .unwrap_or(LIVE2D_AUTO_MOTION_GROUP);
            let idx_raw = motion.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
            let index = if idx_raw < 0 {
                let next = grouped_indexes.entry(group.to_string()).or_insert(0);
                let idx = *next;
                *next += 1;
                idx
            } else {
                let idx = idx_raw as usize;
                let cur = grouped_indexes.entry(group.to_string()).or_insert(0);
                *cur = (*cur).max(idx + 1);
                idx
            };

            discovered.push(Live2DDiscoveredMotion {
                name: motion
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                file: file_key.to_string(),
                asset_relative_path: motion
                    .get("asset_relative_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                note: motion
                    .get("note")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                group: group.to_string(),
                index,
                definition: motion
                    .get("definition")
                    .and_then(|v| v.as_object())
                    .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default(),
            });
        }
        discovered
    }

    #[allow(clippy::too_many_arguments)]
    fn append_motion_reference(
        &self,
        pending: &mut Vec<HashMap<String, Value>>,
        candidate: &Live2DModelCandidate,
        base_directory: &Path,
        file_reference: Option<&str>,
        group: &str,
        index: i64,
        name_hint: Option<&str>,
        note_map: &HashMap<String, String>,
        definition: HashMap<String, Value>,
    ) {
        let Some(asset_path) =
            self.resolve_runtime_reference(&candidate.runtime_root, base_directory, file_reference)
        else {
            return;
        };
        let note_key = asset_path
            .strip_prefix(&candidate.runtime_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if pending.iter().any(|m| {
            m.get("file").and_then(|v| v.as_str()) == Some(&note_key)
        }) {
            return;
        }
        let name = name_hint
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                asset_name_from_file(
                    asset_path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    ".motion3.json",
                )
            });
        let mut motion_def = definition;
        motion_def.insert(
            "File".to_string(),
            Value::String(note_key.clone()),
        );
        let mut m = HashMap::new();
        m.insert("name".to_string(), Value::String(name.to_string()));
        m.insert("file".to_string(), Value::String(note_key.clone()));
        m.insert(
            "asset_relative_path".to_string(),
            Value::String(
                asset_path
                    .strip_prefix(&candidate.source_root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default(),
            ),
        );
        m.insert(
            "note".to_string(),
            Value::String(note_map.get(&note_key).cloned().unwrap_or_default()),
        );
        m.insert("group".to_string(), Value::String(group.to_string()));
        m.insert("index".to_string(), Value::Number(index.into()));
        m.insert("definition".to_string(), Value::Object(motion_def.into_iter().collect()));
        pending.push(m);
    }

    fn discover_hotkeys(
        &self,
        candidate: &Live2DModelCandidate,
        vtube_config: Option<&Live2DVTubeConfig>,
        expressions: &[Live2DDiscoveredExpression],
        motions: &[Live2DDiscoveredMotion],
        annotations_payload: &HashMap<String, Value>,
    ) -> Vec<Live2DDiscoveredHotkey> {
        let Some(vt) = vtube_config else {
            return Vec::new();
        };
        let hotkeys = match vt.payload.get("Hotkeys").and_then(|v| v.as_array()) {
            Some(h) => h,
            None => return Vec::new(),
        };
        let expression_files: HashSet<String> =
            expressions.iter().map(|e| e.file.clone()).collect();
        let motion_files: HashSet<String> = motions.iter().map(|m| m.file.clone()).collect();
        let hotkey_overrides = annotations_payload
            .get("hotkeys")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut discovered: Vec<Live2DDiscoveredHotkey> = Vec::new();
        let vt_parent = vt.path.parent().unwrap_or(&candidate.runtime_root);

        for hk in hotkeys {
            let Some(obj) = hk.as_object() else {
                continue;
            };
            let action = obj
                .get("Action")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .unwrap_or("")
                .to_string();
            let normalized_file = self.hotkey_target_file(
                candidate,
                vt_parent,
                obj,
                &expression_files,
                &motion_files,
            );
            let hotkey_id = obj
                .get("HotkeyID")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .unwrap_or("")
                .to_string();
            let hotkey_key = self.hotkey_key_for(&hotkey_id, &action, &normalized_file);

            let shortcut_override = annotation_shortcut_tokens(&hotkey_overrides, &hotkey_key);
            let shortcut_tokens = shortcut_override.unwrap_or_else(|| {
                self.extract_shortcut_tokens(obj)
            });
            let shortcut_label = shortcut_label_for(obj, &shortcut_tokens);

            let target_kind = match action.as_str() {
                "ToggleExpression" => "expression",
                "TriggerAnimation" => "motion",
                "RemoveAllExpressions" => "system",
                _ => "",
            };

            let mut supported = SUPPORTED_ACTIONS.contains(action.as_str());
            if action == "ToggleExpression" {
                supported = supported && expression_files.contains(&normalized_file);
            } else if action == "TriggerAnimation" {
                supported = supported && motion_files.contains(&normalized_file);
            }

            discovered.push(Live2DDiscoveredHotkey {
                hotkey_key: hotkey_key.clone(),
                hotkey_id,
                name: obj
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(if action.is_empty() { "Hotkey" } else { &action })
                    .to_string(),
                action: action.clone(),
                file: normalized_file,
                shortcut_tokens: shortcut_tokens.clone(),
                shortcut_label,
                target_kind: target_kind.to_string(),
                supported,
            });
        }
        discovered
    }

    fn hotkey_target_file(
        &self,
        candidate: &Live2DModelCandidate,
        base_directory: &Path,
        hotkey: &serde_json::Map<String, Value>,
        expression_files: &HashSet<String>,
        motion_files: &HashSet<String>,
    ) -> String {
        let action = hotkey
            .get("Action")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .unwrap_or("")
            .to_string();
        let file_ref = hotkey
            .get("File")
            .and_then(|v| v.as_str())
            .map(|s| s.replace('\\', "/").trim().trim_start_matches("./").to_string())
            .unwrap_or_default();
        if file_ref.is_empty() {
            return String::new();
        }
        if let Some(asset_path) =
            self.resolve_runtime_reference(&candidate.runtime_root, base_directory, Some(&file_ref))
        {
            return asset_path
                .strip_prefix(&candidate.runtime_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
        }
        if action == "ToggleExpression" {
            return match_file_by_suffix(expression_files, &file_ref);
        }
        if action == "TriggerAnimation" {
            return match_file_by_suffix(motion_files, &file_ref);
        }
        file_ref
    }

    fn extract_shortcut_tokens(&self, hotkey: &serde_json::Map<String, Value>) -> Vec<String> {
        let Some(triggers) = hotkey.get("Triggers").and_then(|v| v.as_object()) else {
            return Vec::new();
        };
        let mut tokens: Vec<String> = Vec::new();
        for &name in &["Trigger1", "Trigger2", "Trigger3"] {
            let nt = self.normalize_hotkey_token(
                triggers
                    .get(name)
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            if !nt.is_empty() && !tokens.contains(&nt) {
                tokens.push(nt);
            }
        }
        tokens
    }

    fn normalize_hotkey_token(&self, token: &str) -> String {
        let text = token.trim();
        if text.is_empty() {
            return String::new();
        }
        let lowered = text.to_lowercase();
        if let Some(mapped) = TOKEN_MAP.get(lowered.as_str()) {
            return (*mapped).to_string();
        }
        // N<digit>
        if Regex::new(r"^[Nn]\d$").unwrap().is_match(text) {
            return format!("digit{}", &text[1..]);
        }
        // F<1-2 digits>
        if Regex::new(r"^[Ff]\d{1,2}$").unwrap().is_match(text) {
            return lowered;
        }
        // Single letter
        if Regex::new(r"^[A-Za-z]$").unwrap().is_match(text) {
            return format!("key{lowered}");
        }
        // NumPad<digit>
        if Regex::new(r"^[Nn]um[Pp]ad\d$").unwrap().is_match(text) {
            return format!("numpad{}", &text[6..]);
        }
        // NumPad<operation>
        if let Some(op) = text
            .to_lowercase()
            .strip_prefix("numpad")
        {
            return format!("numpad{op}");
        }
        lowered
    }

    fn display_info_path(
        &self,
        candidate: &Live2DModelCandidate,
        model_data: &Value,
    ) -> Option<PathBuf> {
        let display_info = model_data
            .get("FileReferences")
            .and_then(|v| v.get("DisplayInfo"))
            .and_then(|v| v.as_str())?;
        if display_info.is_empty() {
            return None;
        }
        Some(candidate.model_path.parent()?.join(display_info))
    }

    fn resolve_runtime_reference(
        &self,
        runtime_root: &Path,
        base_directory: &Path,
        file_reference: Option<&str>,
    ) -> Option<PathBuf> {
        let reference_path = normalize_reference_path(file_reference?)?;
        let resolved = std::fs::canonicalize(base_directory.join(&reference_path)).ok()?;
        let resolved_root = std::fs::canonicalize(runtime_root).ok()?;
        if resolved != resolved_root && !resolved.starts_with(&resolved_root) {
            return None;
        }
        if !resolved.is_file() {
            return None;
        }
        Some(resolved)
    }

    fn hotkey_key_for(&self, hotkey_id: &str, action: &str, file: &str) -> String {
        let id = hotkey_id.trim();
        if !id.is_empty() {
            return id.to_string();
        }
        let action = action.trim();
        let file = file.replace('\\', "/").trim().trim_start_matches("./").to_string();
        if !file.is_empty() {
            return format!("{action}:{file}");
        }
        action.to_string()
    }

    fn relative_file_to_model_parent(
        &self,
        candidate: &Live2DModelCandidate,
        asset_relative_path: &str,
    ) -> String {
        let asset_path = &candidate.source_root.join(asset_relative_path);
        asset_path
            .strip_prefix(candidate.model_path.parent().unwrap_or(&candidate.source_root))
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| asset_relative_path.to_string())
    }
}

// --- free functions ---

fn load_json_file(path: &Path) -> Result<Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {path:?}: {e}"))
}

fn normalize_reference_path(file_reference: &str) -> Option<PathBuf> {
    let normalized = file_reference.replace('\\', "/").trim().to_string();
    if normalized.is_empty() {
        return None;
    }
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return None;
    }
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(p) => {
                let s = p.to_string_lossy();
                if s.is_empty() || s.contains(':') {
                    return None;
                }
                parts.push(PathBuf::from(p));
            }
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    Some(out)
}

fn collect_vtube_files(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vtube_files(base, &path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".vtube.json"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

fn collect_glob(base: &Path, dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_glob(base, &path, suffix, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(suffix))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

fn asset_name_from_file(filename: &str, suffix: &str) -> String {
    filename.strip_suffix(suffix).unwrap_or_else(|| {
        Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename)
    }).to_string()
}

fn match_file_by_suffix(available_files: &HashSet<String>, file_reference: &str) -> String {
    let normalized = file_reference.replace('\\', "/").trim().trim_start_matches("./").to_string();
    if normalized.is_empty() {
        return String::new();
    }
    if available_files.contains(&normalized) {
        return normalized;
    }
    let matches: Vec<&String> = available_files
        .iter()
        .filter(|f| *f == &normalized || f.ends_with(&format!("/{normalized}")))
        .collect();
    if matches.len() == 1 {
        return matches[0].clone();
    }
    normalized
}

fn annotation_shortcut_tokens(
    hotkeys_map: &serde_json::Map<String, Value>,
    hotkey_key: &str,
) -> Option<Vec<String>> {
    let payload = hotkeys_map.get(hotkey_key)?.as_object()?;
    let tokens = payload.get("shortcut_tokens")?.as_array()?;
    Some(
        tokens
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
    )
}

fn shortcut_label_for(hotkey: &serde_json::Map<String, Value>, shortcut_tokens: &[String]) -> String {
    if !shortcut_tokens.is_empty() {
        return shortcut_label_from_tokens(shortcut_tokens);
    }
    if let Some(triggers) = hotkey.get("Triggers").and_then(|v| v.as_object()) {
        let screen_button = triggers
            .get("ScreenButton")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if screen_button >= 0 {
            return "Screen Button".to_string();
        }
    }
    "Unassigned".to_string()
}

fn shortcut_label_from_tokens(shortcut_tokens: &[String]) -> String {
    if shortcut_tokens.is_empty() {
        return "Unassigned".to_string();
    }
    shortcut_tokens
        .iter()
        .map(|t| display_hotkey_token(t))
        .collect::<Vec<_>>()
        .join(" + ")
}
