//! Live2D model catalog — verbatim port of
//! `echobot/app/services/web_console/live2d/catalog.py`.

use std::env;
use std::path::{Path, PathBuf};

use crate::error::AppError;

use super::constants::{
    DEFAULT_LIP_SYNC_PARAMETER_IDS, LIVE2D_SOURCE_BUILTIN, LIVE2D_SOURCE_WORKSPACE,
};
use super::models::Live2DModelCandidate;

/// Discovers and indexes Live2D models under workspace and builtin roots.
pub struct Live2DModelCatalog {
    workspace_root: PathBuf,
    builtin_root: PathBuf,
}

impl Live2DModelCatalog {
    pub fn new(workspace_root: PathBuf, builtin_root: PathBuf) -> Self {
        Self {
            workspace_root,
            builtin_root,
        }
    }

    pub fn empty_config(&self) -> serde_json::Value {
        serde_json::json!({
            "available": false,
            "source": "",
            "selection_key": "",
            "model_name": "",
            "model_url": "",
            "directory_name": "",
            "lip_sync_parameter_ids": DEFAULT_LIP_SYNC_PARAMETER_IDS,
            "mouth_form_parameter_id": null,
            "expressions": [],
            "motions": [],
            "hotkeys": [],
            "annotations_writable": false,
            "models": [],
        })
    }

    pub fn discover_model_candidates(&self) -> Vec<Live2DModelCandidate> {
        let mut candidates = Vec::new();
        for (source, root) in self.roots() {
            if !root.exists() {
                continue;
            }
            let mut paths: Vec<PathBuf> = Vec::new();
            collect_model3_files(root, root, &mut paths);
            // Sort by depth, then by posix path.
            paths.sort_by(|a, b| {
                let da = a.components().count();
                let db = b.components().count();
                da.cmp(&db).then_with(|| {
                    a.to_string_lossy()
                        .replace('\\', "/")
                        .cmp(&b.to_string_lossy().replace('\\', "/"))
                })
            });
            for model_path in paths {
                if let Some(c) = self.candidate_from_path(source, root, &model_path) {
                    candidates.push(c);
                }
            }
        }
        candidates
    }

    pub fn select_default_candidate(
        &self,
        candidates: &[Live2DModelCandidate],
    ) -> Live2DModelCandidate {
        let preferred = env::var("ECHOBOT_WEB_LIVE2D_MODEL")
            .unwrap_or_default()
            .trim()
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_lowercase();
        if !preferred.is_empty() {
            for c in candidates {
                if self.matches_preferred_model(c, &preferred) {
                    return c.clone();
                }
            }
        }
        candidates[0].clone()
    }

    pub fn resolve_asset(&self, asset_path: &str) -> Result<PathBuf, AppError> {
        let (source, relative_path) = self.parse_asset_path(asset_path)?;
        let root = self.root_for(&source);
        let resolved = Self::resolve_under_root(root, &relative_path).ok_or_else(|| {
            AppError::BadRequest(format!("Invalid live2d asset path: {asset_path}"))
        })?;
        if !resolved.is_file() {
            return Err(AppError::NotFound(asset_path.to_string()));
        }
        Ok(resolved)
    }

    pub fn candidate_from_selection_key(
        &self,
        selection_key: &str,
    ) -> Option<Live2DModelCandidate> {
        let key = selection_key.trim();
        if key.is_empty() {
            return None;
        }
        let (source, model_path_text) = key.split_once(':').unwrap_or((LIVE2D_SOURCE_WORKSPACE, key));
        if source != LIVE2D_SOURCE_WORKSPACE && source != LIVE2D_SOURCE_BUILTIN {
            return None;
        }
        let relative_path = Self::normalize_relative_path(model_path_text)?;
        let root = self.root_for(source);
        let resolved = Self::resolve_under_root(root, &relative_path)?;
        if !resolved.is_file()
            || !resolved
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".model3.json"))
                .unwrap_or(false)
        {
            return None;
        }
        self.candidate_from_path(source, root, &resolved)
    }

    pub fn candidate_for_model_asset(&self, asset_path: &str) -> Option<Live2DModelCandidate> {
        let (source, relative_path) = self.parse_asset_path(asset_path).ok()?;
        if !relative_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".model3.json"))
            .unwrap_or(false)
        {
            return None;
        }
        let root = self.root_for(&source);
        let resolved = Self::resolve_under_root(root, &relative_path)?;
        if !resolved.is_file() {
            return None;
        }
        self.candidate_from_path(&source, root, &resolved)
    }

    pub fn selection_key_for(&self, candidate: &Live2DModelCandidate) -> String {
        format!(
            "{}:{}",
            candidate.source,
            candidate.model_relative_path().to_string_lossy().replace('\\', "/")
        )
    }

    pub fn asset_url_for(&self, candidate: &Live2DModelCandidate, relative_path: &str) -> String {
        let clean = relative_path.replace('\\', "/");
        let encoded = clean
            .split('/')
            .map(|seg| {
                let mut s = String::new();
                for b in seg.bytes() {
                    if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
                        s.push(b as char);
                    } else {
                        s.push_str(&format!("%{:02X}", b));
                    }
                }
                s
            })
            .collect::<Vec<_>>()
            .join("/");
        format!("/api/web/live2d/{}/{}", candidate.source, encoded)
    }

    pub fn directory_name_for(&self, candidate: &Live2DModelCandidate) -> String {
        let rrp = candidate.runtime_relative_path();
        let parts: Vec<_> = rrp.components().collect();
        if parts.len() > 1 {
            return parts[0].as_os_str().to_string_lossy().to_string();
        }
        if parts.len() == 1 {
            return rrp
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
        }
        candidate.model_name()
    }

    pub fn parse_asset_path(&self, asset_path: &str) -> Result<(String, PathBuf), AppError> {
        let raw = asset_path.replace('\\', "/").trim().to_string();
        if raw.is_empty() {
            return Err(AppError::BadRequest(
                "Live2D asset path must not be empty".to_string(),
            ));
        }
        let normalized = Self::normalize_relative_path(&raw).ok_or_else(|| {
            AppError::BadRequest(format!("Invalid live2d asset path: {asset_path}"))
        })?;
        let parts: Vec<_> = normalized.components().collect();
        let source = parts[0].as_os_str().to_string_lossy().to_string();
        if source == LIVE2D_SOURCE_WORKSPACE || source == LIVE2D_SOURCE_BUILTIN {
            let relative: PathBuf = parts[1..].iter().collect();
            if relative.components().next().is_none() {
                return Err(AppError::BadRequest(format!(
                    "Invalid live2d asset path: {asset_path}"
                )));
            }
            Ok((source, relative))
        } else {
            Ok((LIVE2D_SOURCE_WORKSPACE.to_string(), normalized))
        }
    }

    // --- private ---

    fn roots(&self) -> Vec<(&'static str, &Path)> {
        vec![
            (LIVE2D_SOURCE_WORKSPACE, &self.workspace_root),
            (LIVE2D_SOURCE_BUILTIN, &self.builtin_root),
        ]
    }

    fn root_for(&self, source: &str) -> &Path {
        if source == LIVE2D_SOURCE_BUILTIN {
            &self.builtin_root
        } else {
            &self.workspace_root
        }
    }

    fn candidate_from_path(
        &self,
        source: &str,
        root: &Path,
        model_path: &Path,
    ) -> Option<Live2DModelCandidate> {
        let resolved_root = std::fs::canonicalize(root).ok()?;
        let resolved_model_path = std::fs::canonicalize(model_path).ok()?;
        if resolved_model_path != resolved_root
            && !resolved_model_path.starts_with(&resolved_root)
        {
            return None;
        }
        if !resolved_model_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".model3.json"))
            .unwrap_or(false)
        {
            return None;
        }
        let runtime_root = resolved_model_path.parent()?.to_path_buf();
        Some(Live2DModelCandidate {
            source: source.to_string(),
            source_root: resolved_root,
            model_path: resolved_model_path,
            runtime_root,
        })
    }

    fn normalize_relative_path(path_text: &str) -> Option<PathBuf> {
        let normalized = path_text.replace('\\', "/").trim().to_string();
        if normalized.is_empty() || normalized.starts_with('/') {
            return None;
        }
        let mut parts: Vec<PathBuf> = Vec::new();
        for seg in normalized.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." || seg.contains(':') {
                return None;
            }
            parts.push(PathBuf::from(seg));
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

    fn resolve_under_root(root: &Path, relative_path: &Path) -> Option<PathBuf> {
        let resolved_root = std::fs::canonicalize(root).ok()?;
        let resolved = std::fs::canonicalize(root.join(relative_path)).ok()?;
        if resolved != resolved_root && !resolved.starts_with(&resolved_root) {
            return None;
        }
        Some(resolved)
    }

    fn matches_preferred_model(
        &self,
        candidate: &Live2DModelCandidate,
        normalized_preference: &str,
    ) -> bool {
        let model_rel = candidate.model_relative_path().to_string_lossy().replace('\\', "/").to_lowercase();
        let model_parent = candidate
            .model_relative_path()
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/").to_lowercase())
            .unwrap_or_default();
        let runtime_rel = candidate.runtime_relative_path().to_string_lossy().replace('\\', "/").to_lowercase();
        let dir_name = self.directory_name_for(candidate).to_lowercase();
        let model_name = candidate.model_name().to_lowercase();
        let s = &candidate.source;

        [
            model_rel.clone(),
            model_parent,
            runtime_rel.clone(),
            dir_name.clone(),
            model_name.clone(),
            format!("{s}:{model_rel}"),
            format!("{s}:{runtime_rel}"),
            format!("{s}:{dir_name}"),
            format!("{s}/{model_rel}"),
            format!("{s}/{runtime_rel}"),
            format!("{s}/{dir_name}"),
        ]
        .contains(&normalized_preference.to_string())
    }
}

// --- helpers ---

fn collect_model3_files(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_model3_files(base, &path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".model3.json"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}
