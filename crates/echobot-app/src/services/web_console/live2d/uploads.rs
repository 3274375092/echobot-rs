//! Live2D upload manager — verbatim port of `echobot/app/services/web_console/live2d/uploads.py`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;

use super::constants::{
    allowed_live2d_upload_suffixes, MAX_LIVE2D_UPLOAD_FILES, MAX_LIVE2D_UPLOAD_TOTAL_BYTES,
};
use super::models::Live2DUploadFile;

static ALLOWED_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(allowed_live2d_upload_suffixes);

pub struct Live2DUploadManager {
    workspace_root: PathBuf,
}

impl Live2DUploadManager {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    /// Validate and save uploaded files. Returns the target directory path.
    pub fn save_directory(
        &self,
        uploaded_files: &[Live2DUploadFile],
    ) -> Result<PathBuf, String> {
        let (root_directory_name, files_to_save) = self.normalize_upload_files(uploaded_files)?;
        let target_directory = self.prepare_upload_directory(&root_directory_name);

        // Save under target_directory
        for (relative_path, file_bytes) in &files_to_save {
            // Skip the first component (the root dir name)
            let sub_parts: Vec<_> = relative_path.components().collect();
            let inner: PathBuf = if sub_parts.len() > 1 {
                sub_parts[1..].iter().collect()
            } else {
                PathBuf::new()
            };
            let target_file = target_directory.join(&inner);
            if let Some(parent) = target_file.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
            }
            std::fs::write(&target_file, file_bytes).map_err(|e| format!("write failed: {e}"))?;
        }
        Ok(target_directory)
    }

    // --- private ---

    fn normalize_upload_files(
        &self,
        uploaded_files: &[Live2DUploadFile],
    ) -> Result<(String, Vec<(PathBuf, Vec<u8>)>), String> {
        if uploaded_files.is_empty() {
            return Err("Please choose a Live2D folder to upload".to_string());
        }
        if uploaded_files.len() > MAX_LIVE2D_UPLOAD_FILES {
            return Err("Too many files in Live2D folder. Keep it under 512 files.".to_string());
        }

        let mut normalized: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut total_bytes: usize = 0;
        let mut root_names: HashSet<String> = HashSet::new();
        let mut has_model3 = false;

        for uf in uploaded_files {
            let relative_path = Self::clean_upload_relative_path(&uf.relative_path)?;
            if !Self::is_supported_upload_path(&relative_path) {
                continue;
            }
            if uf.file_bytes.is_empty() {
                return Err(format!(
                    "Live2D file must not be empty: {}",
                    relative_path.display()
                ));
            }
            total_bytes += uf.file_bytes.len();
            if total_bytes > MAX_LIVE2D_UPLOAD_TOTAL_BYTES {
                return Err(
                    "Live2D folder is too large. Keep it under 200 MB.".to_string()
                );
            }

            if relative_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".model3.json"))
                .unwrap_or(false)
            {
                has_model3 = true;
            }

            let first_part = relative_path
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            root_names.insert(first_part);
            normalized.push((relative_path, uf.file_bytes.clone()));
        }

        if normalized.is_empty() {
            return Err(
                "The selected folder does not contain supported Live2D runtime files"
                    .to_string(),
            );
        }
        if root_names.len() != 1 {
            return Err("Please upload exactly one Live2D folder at a time".to_string());
        }
        if !has_model3 {
            return Err(
                "The selected folder must include at least one .model3.json file"
                    .to_string(),
            );
        }
        let root_name = root_names.into_iter().next().unwrap();
        Ok((root_name, normalized))
    }

    fn clean_upload_relative_path(relative_path: &str) -> Result<PathBuf, String> {
        let raw = relative_path.replace('\\', "/").trim().to_string();
        if raw.is_empty() {
            return Err("Live2D file path must not be empty".to_string());
        }
        if raw.starts_with('/') {
            return Err(format!("Invalid Live2D file path: {relative_path}"));
        }
        let mut parts = Vec::new();
        for seg in raw.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." || seg.contains(':') {
                return Err(format!("Invalid Live2D file path: {relative_path}"));
            }
            parts.push(seg.to_string());
        }
        if parts.len() < 2 {
            return Err(
                "Please upload a Live2D folder instead of individual files".to_string()
            );
        }
        let mut out = PathBuf::new();
        for p in parts {
            out.push(p);
        }
        Ok(out)
    }

    fn is_supported_upload_path(relative_path: &Path) -> bool {
        relative_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| {
                let ext_lower = format!(".{}", ext.to_lowercase());
                ALLOWED_SUFFIXES.contains(ext_lower.as_str())
            })
            .unwrap_or(false)
    }

    fn prepare_upload_directory(&self, directory_name: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(&self.workspace_root);

        let cleaned = Self::clean_upload_directory_name(directory_name);
        let mut candidate = self.workspace_root.join(&cleaned);
        let mut index = 2u32;
        while candidate.exists() {
            candidate = self
                .workspace_root
                .join(format!("{cleaned}-{index}"));
            index += 1;
        }
        std::fs::create_dir_all(&candidate).expect("mkdir upload dir");
        candidate
    }

    fn clean_upload_directory_name(directory_name: &str) -> String {
        let raw = Path::new(directory_name)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| directory_name.to_string());
        let cleaned: String = raw
            .chars()
            .map(|c| match c {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\x00'..='\x1f' => '_',
                other => other,
            })
            .collect();
        let trimmed = cleaned.trim_matches(|c: char| c == ' ' || c == '.');
        if trimmed.is_empty() {
            "live2d-model".to_string()
        } else {
            trimmed.to_string()
        }
    }
}
