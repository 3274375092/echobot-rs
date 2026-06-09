//! Live2D service — verbatim port of
//! `echobot/app/services/web_console/live2d/service.py`.

pub mod annotations;
pub mod catalog;
pub mod constants;
pub mod metadata;
pub mod models;
pub mod uploads;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::error::AppError;

use self::annotations::Live2DAnnotationsRepository;
use self::catalog::Live2DModelCatalog;
use self::constants::{
    DEFAULT_LIP_SYNC_PARAMETER_IDS, DEFAULT_MOUTH_FORM_PARAMETER_IDS,
    LIVE2D_SOURCE_WORKSPACE,
};
use self::metadata::Live2DMetadataService;
use self::models::{Live2DDiscoveredHotkey, Live2DModelCandidate, Live2DUploadFile};
use self::uploads::Live2DUploadManager;

pub use self::models::Live2DUploadFile as UploadFile;

/// The main Live2D service coordinating catalog, metadata, annotations, and uploads.
#[derive(Clone)]
pub struct Live2DService {
    catalog: Arc<Live2DModelCatalog>,
    annotations: Arc<Live2DAnnotationsRepository>,
    metadata: Arc<Live2DMetadataService>,
    uploads: Arc<Live2DUploadManager>,
}

impl std::fmt::Debug for Live2DService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live2DService").finish()
    }
}

impl Live2DService {
    pub fn new(workspace_root: PathBuf, builtin_root: PathBuf) -> Self {
        let annotations = Arc::new(Live2DAnnotationsRepository::new(None));
        let catalog = Arc::new(Live2DModelCatalog::new(
            workspace_root.clone(),
            builtin_root,
        ));
        let metadata = Arc::new(Live2DMetadataService::new(
            (*annotations).clone(),
        ));
        // uploads needs the workspace_root where .echobot/live2d lives,
        // not the workspace root itself
        let uploads = Arc::new(Live2DUploadManager::new(workspace_root));
        Self {
            catalog,
            annotations,
            metadata,
            uploads,
        }
    }

    /// Build the Live2D config payload for the web frontend.
    pub fn build_config(&self) -> Option<Value> {
        let candidates = self.catalog.discover_model_candidates();
        if candidates.is_empty() {
            return None;
        }
        let selected = self.catalog.select_default_candidate(&candidates);
        let selected_key = self.catalog.selection_key_for(&selected);
        let model_options: Vec<Value> = candidates
            .iter()
            .map(|c| self.build_model_option(c))
            .collect();
        let selected_option = model_options
            .iter()
            .find(|o| {
                o.get("selection_key")
                    .and_then(|v| v.as_str())
                    == Some(&selected_key)
            })
            .cloned()
            .unwrap_or_else(|| model_options[0].clone());

        let mut config = selected_option;
        if let Some(obj) = config.as_object_mut() {
            obj.insert("available".to_string(), Value::Bool(true));
            obj.insert("models".to_string(), Value::Array(model_options));
        }
        Some(config)
    }

    /// Render a model3.json with patches from metadata.
    pub fn render_model_json(&self, asset_path: &str) -> Result<String, AppError> {
        let candidate = self
            .catalog
            .candidate_for_model_asset(asset_path)
            .ok_or_else(|| AppError::NotFound(asset_path.to_string()))?;
        let model_data = self
            .metadata
            .load_model_data(&candidate)
            .map_err(AppError::BadRequest)?;
        let discovered = self.metadata.discover_metadata(&candidate, &model_data);
        let patched = self
            .metadata
            .patch_model_data(&candidate, &model_data, &discovered);
        serde_json::to_string(&patched).map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Save an uploaded Live2D directory.
    pub fn save_directory(
        &self,
        uploaded_files: &[Live2DUploadFile],
    ) -> Result<Value, AppError> {
        let target_directory = self
            .uploads
            .save_directory(uploaded_files)
            .map_err(AppError::BadRequest)?;
        let config = self.build_config().ok_or_else(|| {
            // Clean up on failure (matching Python)
            let _ = std::fs::remove_dir_all(&target_directory);
            AppError::BadRequest("No Live2D model was found after upload".to_string())
        })?;
        Ok(config)
    }

    /// Save an annotation note.
    pub fn save_annotation(
        &self,
        selection_key: &str,
        kind: &str,
        file: &str,
        note: &str,
    ) -> Result<Value, AppError> {
        let candidate = self
            .catalog
            .candidate_from_selection_key(selection_key)
            .ok_or_else(|| {
                AppError::BadRequest(format!("Unknown Live2D model: {selection_key}"))
            })?;
        self.ensure_workspace_model(&candidate)?;

        let normalized_kind = kind.trim().to_lowercase();
        if normalized_kind != "expression" && normalized_kind != "motion" {
            return Err(AppError::BadRequest(
                "Live2D annotation kind must be expression or motion".to_string(),
            ));
        }
        let normalized_file = self
            .metadata
            .normalize_annotation_file(file)
            .map_err(AppError::BadRequest)?;
        let model_data = self
            .metadata
            .load_model_data(&candidate)
            .map_err(AppError::BadRequest)?;
        let discovered = self.metadata.discover_metadata(&candidate, &model_data);
        let available_files: Vec<&str> = if normalized_kind == "expression" {
            discovered.expressions.iter().map(|e| e.file.as_str()).collect()
        } else {
            discovered.motions.iter().map(|m| m.file.as_str()).collect()
        };
        if !available_files.contains(&normalized_file.as_str()) {
            return Err(AppError::BadRequest(format!(
                "Unknown Live2D {normalized_kind}: {normalized_file}"
            )));
        }
        let normalized_note = note.trim();
        self.annotations.save_annotation(
            &candidate.runtime_root,
            &normalized_kind,
            &normalized_file,
            normalized_note,
        );
        Ok(serde_json::json!({
            "selection_key": self.catalog.selection_key_for(&candidate),
            "kind": normalized_kind,
            "file": normalized_file,
            "note": normalized_note,
        }))
    }

    /// Save a hotkey override.
    pub fn save_hotkey(
        &self,
        selection_key: &str,
        hotkey_key: &str,
        shortcut_tokens: &[String],
        restore_default: bool,
    ) -> Result<Value, AppError> {
        let candidate = self
            .catalog
            .candidate_from_selection_key(selection_key)
            .ok_or_else(|| {
                AppError::BadRequest(format!("Unknown Live2D model: {selection_key}"))
            })?;
        self.ensure_workspace_model(&candidate)?;

        let model_data = self
            .metadata
            .load_model_data(&candidate)
            .map_err(AppError::BadRequest)?;
        let discovered = self.metadata.discover_metadata(&candidate, &model_data);
        let normalized_hotkey_key = hotkey_key.trim();
        if !discovered.hotkeys.iter().any(|h| h.hotkey_key == normalized_hotkey_key) {
            return Err(AppError::BadRequest(format!("Unknown Live2D hotkey: {hotkey_key}")));
        }

        let normalized_tokens = self.metadata.normalize_shortcut_tokens(shortcut_tokens);
        self.annotations.save_hotkey(
            &candidate.runtime_root,
            normalized_hotkey_key,
            &normalized_tokens,
            restore_default,
        );

        // Re-discover to get updated hotkey
        let refreshed = self.metadata.discover_metadata(&candidate, &model_data);
        let updated = find_hotkey(&refreshed.hotkeys, normalized_hotkey_key)
            .ok_or_else(|| {
                AppError::BadRequest(format!("Unknown Live2D hotkey: {hotkey_key}"))
            })?;

        Ok(serde_json::json!({
            "selection_key": self.catalog.selection_key_for(&candidate),
            "hotkey_key": updated.hotkey_key,
            "hotkey_id": updated.hotkey_id,
            "name": updated.name,
            "action": updated.action,
            "file": updated.file,
            "shortcut_tokens": updated.shortcut_tokens,
            "shortcut_label": updated.shortcut_label,
            "target_kind": updated.target_kind,
            "supported": updated.supported,
        }))
    }

    /// Returns the empty / no-model config.
    pub fn empty_config(&self) -> Value {
        self.catalog.empty_config()
    }

    /// Resolve an asset path to a filesystem path.
    pub fn resolve_asset(&self, asset_path: &str) -> Result<PathBuf, AppError> {
        self.catalog.resolve_asset(asset_path)
    }

    // --- private ---

    fn build_model_option(&self, candidate: &Live2DModelCandidate) -> Value {
        let model_data = self
            .metadata
            .load_model_data(candidate)
            .unwrap_or(Value::Null);
        let discovered = self.metadata.discover_metadata(candidate, &model_data);
        let parameter_ids = self.metadata.load_parameter_ids(candidate, &model_data);
        let lip_sync_ids =
            self.resolve_lip_sync_parameter_ids(&model_data, &parameter_ids);
        let mouth_form_id = self.resolve_mouth_form_parameter_id(&parameter_ids);

        let expressions: Vec<Value> = discovered
            .expressions
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "file": e.file,
                    "url": self.catalog.asset_url_for(candidate, &e.asset_relative_path),
                    "note": e.note,
                })
            })
            .collect();
        let motions: Vec<Value> = discovered
            .motions
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "file": m.file,
                    "url": self.catalog.asset_url_for(candidate, &m.asset_relative_path),
                    "note": m.note,
                    "group": m.group,
                    "index": m.index,
                })
            })
            .collect();
        let hotkeys: Vec<Value> = discovered
            .hotkeys
            .iter()
            .map(|h| self.metadata.hotkey_payload(h))
            .collect();

        serde_json::json!({
            "source": candidate.source,
            "selection_key": self.catalog.selection_key_for(candidate),
            "model_name": candidate.model_name(),
            "model_url": self.catalog.asset_url_for(candidate, &candidate.model_relative_path().to_string_lossy().replace('\\', "/")),
            "directory_name": self.catalog.directory_name_for(candidate),
            "lip_sync_parameter_ids": lip_sync_ids,
            "mouth_form_parameter_id": mouth_form_id,
            "expressions": expressions,
            "motions": motions,
            "hotkeys": hotkeys,
            "annotations_writable": discovered.annotations_writable,
        })
    }

    fn resolve_lip_sync_parameter_ids(
        &self,
        model_data: &Value,
        parameter_ids: &[String],
    ) -> Vec<String> {
        let group_ids = self
            .metadata
            .load_group_parameter_ids(model_data, "LipSync");
        if !group_ids.is_empty() {
            return group_ids;
        }
        let inferred: Vec<String> = parameter_ids
            .iter()
            .filter(|id| id.contains("MouthOpen"))
            .cloned()
            .collect();
        if !inferred.is_empty() {
            return inferred;
        }
        for fallback in DEFAULT_LIP_SYNC_PARAMETER_IDS {
            if parameter_ids.contains(&(*fallback).to_string()) {
                return vec![(*fallback).to_string()];
            }
        }
        DEFAULT_LIP_SYNC_PARAMETER_IDS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    fn resolve_mouth_form_parameter_id(&self, parameter_ids: &[String]) -> Option<String> {
        for id in parameter_ids {
            if id.contains("MouthForm") {
                return Some(id.clone());
            }
        }
        for fallback in DEFAULT_MOUTH_FORM_PARAMETER_IDS {
            if parameter_ids.contains(&(*fallback).to_string()) {
                return Some((*fallback).to_string());
            }
        }
        None
    }

    fn ensure_workspace_model(&self, candidate: &Live2DModelCandidate) -> Result<(), AppError> {
        if candidate.source != LIVE2D_SOURCE_WORKSPACE {
            return Err(AppError::BadRequest(
                "Built-in Live2D models are read-only".to_string(),
            ));
        }
        Ok(())
    }
}

fn find_hotkey<'a>(
    hotkeys: &'a [Live2DDiscoveredHotkey],
    hotkey_key: &str,
) -> Option<&'a Live2DDiscoveredHotkey> {
    hotkeys.iter().find(|h| h.hotkey_key == hotkey_key)
}
