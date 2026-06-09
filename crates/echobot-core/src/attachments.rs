//! Attachment storage: image and file attachments with on-disk persistence
//! and metadata sidecars.
//!
//! Port of `echobot/attachments.py`. The Rust version keeps the same
//! directory layout (`<base>/files/<id>...` + `<base>/meta/<id>.json`) and
//! attachment-id conventions (`img_*`, `file_*`).
//!
//! File I/O is performed with [`std::fs`] under a `Mutex` to serialise
//! writes. All public methods are blocking — callers should invoke them
//! from `tokio::task::spawn_blocking` if they're on the async path.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AttachmentError, Result};
use crate::images::{validate_image_bytes, ImageBudget, DEFAULT_IMAGE_BUDGET};

/// URL prefix identifying a logical attachment id.
pub const ATTACHMENT_URL_PREFIX: &str = "attachment://";
/// Metadata `kind` for image attachments.
pub const IMAGE_ATTACHMENT_KIND: &str = "image";
/// Metadata `kind` for file attachments.
pub const FILE_ATTACHMENT_KIND: &str = "file";

/// Constraints for file attachments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBudget {
    /// Maximum allowed file size in bytes.
    pub max_input_bytes: usize,
}

impl Default for FileBudget {
    fn default() -> Self {
        Self {
            max_input_bytes: 200 * 1024 * 1024,
        }
    }
}

/// The default [`FileBudget`].
pub const DEFAULT_FILE_BUDGET: FileBudget = FileBudget {
    max_input_bytes: 200 * 1024 * 1024,
};

/// Image attachment metadata persisted to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub attachment_id: String,
    pub content_type: String,
    pub original_filename: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub created_at: String,
    pub relative_path: String,
}

impl ImageAttachment {
    /// Returns the logical attachment URL (`attachment://<id>`).
    pub fn attachment_url(&self) -> String {
        build_attachment_url(&self.attachment_id)
    }

    /// Returns the HTTP preview/content URL.
    pub fn preview_url(&self) -> String {
        build_attachment_content_url(&self.attachment_id)
    }

    /// Returns a sensible download filename derived from the original name.
    pub fn download_filename(&self) -> String {
        let original = self.original_filename.trim();
        if original.is_empty() {
            return format!("{}.jpg", self.attachment_id);
        }
        let stem = Path::new(original)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.attachment_id);
        format!("{stem}.jpg")
    }

    /// Returns the on-wire image dict sent to the LLM in a content block.
    pub fn to_message_image(&self) -> serde_json::Value {
        serde_json::json!({
            "attachment_id": self.attachment_id,
            "url": self.attachment_url(),
            "preview_url": self.preview_url(),
        })
    }

    /// Serializes to the on-disk metadata format (adds a `kind` field).
    pub fn to_dict(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let serde_json::Value::Object(ref mut map) = v {
            map.insert("kind".into(), serde_json::Value::String(IMAGE_ATTACHMENT_KIND.into()));
        }
        v
    }

    /// Parses from a metadata JSON value.
    pub fn from_dict(data: &serde_json::Value) -> Result<Self> {
        let attachment_id = data
            .get("attachment_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let content_type = data
            .get("content_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let original_filename = data
            .get("original_filename")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let size_bytes = data
            .get("size_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let width = data
            .get("width")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let height = data
            .get("height")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let sha256 = data
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let created_at = data
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let relative_path = data
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        Ok(Self {
            attachment_id,
            content_type: if content_type.is_empty() {
                "image/jpeg".to_string()
            } else {
                content_type
            },
            original_filename,
            size_bytes,
            width,
            height,
            sha256,
            created_at,
            relative_path,
        })
    }
}

/// File attachment metadata persisted to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttachment {
    pub attachment_id: String,
    pub content_type: String,
    pub original_filename: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: String,
    pub relative_path: String,
}

impl FileAttachment {
    /// Returns the logical attachment URL.
    pub fn attachment_url(&self) -> String {
        build_attachment_url(&self.attachment_id)
    }

    /// Returns the HTTP download URL.
    pub fn download_url(&self) -> String {
        build_attachment_content_url(&self.attachment_id)
    }

    /// Returns a sensible download filename.
    pub fn download_filename(&self) -> String {
        let original_name = Path::new(self.original_filename.trim())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if !original_name.is_empty() {
            return original_name;
        }
        let suffix = Path::new(&self.relative_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if suffix.is_empty() {
            self.attachment_id.clone()
        } else {
            format!("{}.{}", self.attachment_id, suffix)
        }
    }

    /// Serializes to the on-disk metadata format (adds a `kind` field).
    pub fn to_dict(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let serde_json::Value::Object(ref mut map) = v {
            map.insert("kind".into(), serde_json::Value::String(FILE_ATTACHMENT_KIND.into()));
        }
        v
    }

    /// Parses from a metadata JSON value.
    pub fn from_dict(data: &serde_json::Value) -> Result<Self> {
        let attachment_id = data
            .get("attachment_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let content_type = data
            .get("content_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let original_filename = data
            .get("original_filename")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let size_bytes = data
            .get("size_bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let sha256 = data
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let created_at = data
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let relative_path = data
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        Ok(Self {
            attachment_id,
            content_type: if content_type.is_empty() {
                "application/octet-stream".to_string()
            } else {
                content_type
            },
            original_filename,
            size_bytes,
            sha256,
            created_at,
            relative_path,
        })
    }
}

/// Either an image or a file attachment, returned from a lookup.
#[derive(Debug, Clone)]
pub enum AttachmentRef {
    /// Image attachment variant.
    Image(ImageAttachment),
    /// File attachment variant.
    File(FileAttachment),
}

/// On-disk attachment store. Holds image/file blobs under `<base>/files/`
/// with JSON sidecars under `<base>/meta/`.
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    base_dir: PathBuf,
    files_dir: PathBuf,
    meta_dir: PathBuf,
    image_budget: ImageBudget,
    file_budget: FileBudget,
    lock: Arc<Mutex<()>>,
}

impl AttachmentStore {
    /// Creates a new store rooted at `base_dir`.
    pub fn new(
        base_dir: impl AsRef<Path>,
        image_budget: Option<ImageBudget>,
        file_budget: Option<FileBudget>,
    ) -> Self {
        let base_dir = base_dir.as_ref().to_path_buf();
        Self {
            files_dir: base_dir.join("files"),
            meta_dir: base_dir.join("meta"),
            image_budget: image_budget.unwrap_or_else(|| DEFAULT_IMAGE_BUDGET.clone()),
            file_budget: file_budget.unwrap_or_default(),
            lock: Arc::new(Mutex::new(())),
            base_dir,
        }
    }

    /// Returns the base directory of the store.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Returns the on-disk path of an image attachment's bytes.
    pub fn image_attachment_path(&self, attachment_id: &str) -> Result<PathBuf> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        let _attachment = self.get_image_attachment(&cleaned)?;
        Ok(self.base_dir.join("files").join(format!("{cleaned}.jpg")))
    }

    /// Returns a base64 data URL for the image (suitable for `image_url.url`).
    pub fn image_attachment_data_url(&self, attachment_id: &str) -> Result<String> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        let attachment = self.get_image_attachment(&cleaned)?;
        let path = self.base_dir.join("files").join(format!("{cleaned}.jpg"));
        let bytes = fs::read(&path).map_err(|_| AttachmentError::FileMissing(cleaned.clone()))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!("data:{};base64,{}", attachment.content_type, encoded))
    }

    /// Returns the on-disk path of a file attachment's bytes.
    pub fn file_attachment_path(&self, attachment_id: &str) -> Result<PathBuf> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        let attachment = self.get_file_attachment(&cleaned)?;
        Ok(self.base_dir.join(&attachment.relative_path))
    }

    /// Returns the metadata of an image attachment.
    pub fn get_image_attachment(&self, attachment_id: &str) -> Result<ImageAttachment> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        if attachment_kind_from_id(&cleaned) != Some(IMAGE_ATTACHMENT_KIND.to_string()) {
            return Err(AttachmentError::WrongKind {
                expected: "an image".to_string(),
                actual: cleaned.clone(),
            }
            .into());
        }
        let (data, _path) = self.load_attachment_metadata(&cleaned, IMAGE_ATTACHMENT_KIND)?;
        let attachment = ImageAttachment::from_dict(&data)?;
        if attachment.attachment_id != cleaned {
            return Err(AttachmentError::IncompleteMetadata(cleaned).into());
        }
        if !attachment.content_type.starts_with("image/") {
            return Err(AttachmentError::InvalidContentType(attachment.content_type).into());
        }
        if attachment.width == 0 || attachment.height == 0 {
            return Err(AttachmentError::IncompleteMetadata(cleaned).into());
        }
        Ok(attachment)
    }

    /// Returns the metadata of a file attachment.
    pub fn get_file_attachment(&self, attachment_id: &str) -> Result<FileAttachment> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        if attachment_kind_from_id(&cleaned) != Some(FILE_ATTACHMENT_KIND.to_string()) {
            return Err(AttachmentError::WrongKind {
                expected: "a file".to_string(),
                actual: cleaned.clone(),
            }
            .into());
        }
        let (data, _path) = self.load_attachment_metadata(&cleaned, FILE_ATTACHMENT_KIND)?;
        let attachment = FileAttachment::from_dict(&data)?;
        if attachment.attachment_id != cleaned {
            return Err(AttachmentError::IncompleteMetadata(cleaned).into());
        }
        Ok(attachment)
    }

    /// Returns the file-relative path used inside message content blocks.
    pub fn file_attachment_message_content(
        &self,
        attachment_id: &str,
        workspace: &Path,
    ) -> Result<serde_json::Value> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        let attachment = self.get_file_attachment(&cleaned)?;
        let path = self.base_dir.join(&attachment.relative_path);
        Ok(serde_json::json!({
            "attachment_id": attachment.attachment_id,
            "name": if attachment.original_filename.is_empty() {
                attachment.download_filename()
            } else {
                attachment.original_filename.clone()
            },
            "download_url": attachment.download_url(),
            "workspace_path": workspace_relative_path(workspace, &path),
            "content_type": attachment.content_type,
            "size_bytes": attachment.size_bytes,
        }))
    }

    /// Persists a new image attachment.
    pub fn create_image_attachment(
        &self,
        image_bytes: &[u8],
        content_type: Option<&str>,
        filename: Option<&str>,
    ) -> Result<ImageAttachment> {
        let cleaned_content_type = content_type.unwrap_or("").trim().to_lowercase();
        if !cleaned_content_type.is_empty() && !cleaned_content_type.starts_with("image/") {
            return Err(AttachmentError::InvalidContentType(cleaned_content_type).into());
        }

        // The full decode → resize → JPEG-encode pipeline lives in the
        // `echobot-images` crate (not in this porting slice). Until that's
        // wired up we accept JPEG pass-through so callers can persist
        // already-normalized bytes; anything else fails with a clear
        // error.
        validate_image_bytes(image_bytes, &self.image_budget)?;
        if image_bytes.len() < 3 || &image_bytes[..3] != b"\xFF\xD8\xFF" {
            return Err(AttachmentError::InvalidContentType(
                "image pipeline (decode/encode) not yet implemented in this port"
                    .to_string(),
            )
            .into());
        }
        let size_bytes = image_bytes.len() as u64;
        let width = self.image_budget.max_side;
        let height = self.image_budget.max_side;

        let attachment_id = generate_image_attachment_id();
        let relative_path = format!("files/{attachment_id}.jpg");
        let attachment = ImageAttachment {
            attachment_id: attachment_id.clone(),
            content_type: "image/jpeg".to_string(),
            original_filename: filename.unwrap_or("").trim().to_string(),
            size_bytes,
            width,
            height,
            sha256: sha256_hex(image_bytes),
            created_at: now_text(),
            relative_path: relative_path.clone(),
        };

        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.ensure_dirs()?;
        fs::write(self.base_dir.join(&relative_path), image_bytes)?;
        write_metadata(&self.meta_dir, &attachment_id, &attachment.to_dict())?;
        Ok(attachment)
    }

    /// Persists a new file attachment.
    pub fn create_file_attachment(
        &self,
        file_bytes: &[u8],
        content_type: Option<&str>,
        filename: Option<&str>,
    ) -> Result<FileAttachment> {
        if file_bytes.is_empty() {
            return Err(AttachmentError::EmptyFile.into());
        }
        if file_bytes.len() > self.file_budget.max_input_bytes {
            return Err(AttachmentError::FileTooLarge {
                actual: file_bytes.len(),
                limit: self.file_budget.max_input_bytes,
            }
            .into());
        }
        let cleaned_filename = filename.unwrap_or("").trim().to_string();
        let cleaned_content_type =
            normalize_content_type(content_type, Some(&cleaned_filename));
        let attachment_id = generate_file_attachment_id();
        let file_suffix = safe_file_suffix(Some(&cleaned_filename), &cleaned_content_type);
        let relative_path = format!("files/{attachment_id}{file_suffix}");
        let attachment = FileAttachment {
            attachment_id: attachment_id.clone(),
            content_type: cleaned_content_type,
            original_filename: cleaned_filename,
            size_bytes: file_bytes.len() as u64,
            sha256: sha256_hex(file_bytes),
            created_at: now_text(),
            relative_path: relative_path.clone(),
        };

        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.ensure_dirs()?;
        fs::write(self.base_dir.join(&relative_path), file_bytes)?;
        write_metadata(&self.meta_dir, &attachment_id, &attachment.to_dict())?;
        Ok(attachment)
    }

    /// Resolves a download by attachment id, returning the metadata and the
    /// absolute file path.
    pub fn resolve_attachment_download(
        &self,
        attachment_id: &str,
    ) -> Result<(AttachmentRef, PathBuf)> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        if attachment_kind_from_id(&cleaned) == Some(IMAGE_ATTACHMENT_KIND.to_string()) {
            let attachment = self.get_image_attachment(&cleaned)?;
            let path = self.base_dir.join("files").join(format!("{cleaned}.jpg"));
            return Ok((AttachmentRef::Image(attachment), path));
        }
        if attachment_kind_from_id(&cleaned) == Some(FILE_ATTACHMENT_KIND.to_string()) {
            let attachment = self.get_file_attachment(&cleaned)?;
            let path = self.base_dir.join(&attachment.relative_path);
            return Ok((AttachmentRef::File(attachment), path));
        }
        Err(AttachmentError::NotFound(cleaned).into())
    }

    /// Extracts the attachment id from an `attachment://<id>` URL, or
    /// returns `None` if the URL doesn't use the prefix.
    pub fn attachment_id_from_url(url: &str) -> Option<String> {
        let trimmed = url.trim();
        trimmed
            .strip_prefix(ATTACHMENT_URL_PREFIX)
            .and_then(|s| normalize_attachment_id(s).ok())
    }

    /// Deletes an attachment's bytes and metadata sidecar from disk.
    pub fn delete_attachment(&self, attachment_id: &str) -> Result<()> {
        let cleaned = normalize_attachment_id(attachment_id)?;
        let (attachment_path, metadata_path): (PathBuf, PathBuf) =
            match attachment_kind_from_id(&cleaned) {
                Some(kind) if kind == IMAGE_ATTACHMENT_KIND => {
                    let _attachment = self.get_image_attachment(&cleaned)?;
                    (
                        self.base_dir.join("files").join(format!("{cleaned}.jpg")),
                        self.meta_dir.join(format!("{cleaned}.json")),
                    )
                }
                Some(kind) if kind == FILE_ATTACHMENT_KIND => {
                    let attachment = self.get_file_attachment(&cleaned)?;
                    (
                        self.base_dir.join(&attachment.relative_path),
                        self.meta_dir.join(format!("{cleaned}.json")),
                    )
                }
                _ => return Err(AttachmentError::NotFound(cleaned).into()),
            };
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if attachment_path.exists() {
            fs::remove_file(&attachment_path)?;
        }
        if metadata_path.exists() {
            fs::remove_file(&metadata_path)?;
        }
        Ok(())
    }

    // -- private helpers --

    fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.files_dir)?;
        fs::create_dir_all(&self.meta_dir)?;
        Ok(())
    }

    fn load_attachment_metadata(
        &self,
        attachment_id: &str,
        expected_kind: &str,
    ) -> Result<(serde_json::Value, PathBuf)> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let metadata_path = self.meta_dir.join(format!("{attachment_id}.json"));
        if !metadata_path.exists() {
            return Err(AttachmentError::NotFound(attachment_id.to_string()).into());
        }
        let raw = fs::read_to_string(&metadata_path)?;
        let data: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|_| AttachmentError::InvalidMetadata(attachment_id.to_string()))?;
        let Some(obj) = data.as_object() else {
            return Err(AttachmentError::InvalidMetadata(attachment_id.to_string()).into());
        };
        let kind = obj
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .map(|s| s.to_string())
            .or_else(|| attachment_kind_from_id(attachment_id))
            .unwrap_or_default();
        if kind != expected_kind {
            return Err(AttachmentError::WrongKind {
                expected: kind_label(expected_kind).to_string(),
                actual: attachment_id.to_string(),
            }
            .into());
        }
        let relative_path = obj
            .get("relative_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if relative_path.is_empty() {
            return Err(AttachmentError::IncompleteMetadata(attachment_id.to_string()).into());
        }
        let attachment_path = self.base_dir.join(&relative_path);
        if !attachment_path.exists() {
            return Err(AttachmentError::FileMissing(attachment_id.to_string()).into());
        }
        Ok((data, attachment_path))
    }
}

// ---------------------------------------------------------------------------
// Free functions exposed to other modules
// ---------------------------------------------------------------------------

/// Builds the logical attachment URL `attachment://<id>`.
pub fn build_attachment_url(attachment_id: &str) -> String {
    let cleaned = normalize_attachment_id(attachment_id).unwrap_or_default();
    format!("{ATTACHMENT_URL_PREFIX}{cleaned}")
}

/// Builds the HTTP content URL for an attachment.
pub fn build_attachment_content_url(attachment_id: &str) -> String {
    let cleaned = normalize_attachment_id(attachment_id).unwrap_or_default();
    format!("/api/attachments/{cleaned}/content")
}

/// Returns the lowercase kind (`"image"` / `"file"`) embedded in a parsed
/// metadata value, or `None` if not present.
pub fn attachment_kind_from_metadata(data: &serde_json::Value) -> Option<String> {
    data.get("kind")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.to_lowercase())
        .filter(|s| s == IMAGE_ATTACHMENT_KIND || s == FILE_ATTACHMENT_KIND)
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn normalize_attachment_id(value: &str) -> Result<String> {
    let cleaned: String = value.trim().to_string();
    if cleaned.is_empty() {
        return Err(AttachmentError::InvalidAttachmentId(
            "must not be empty".to_string(),
        )
        .into());
    }
    let allowed: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";
    let normalized: String = cleaned
        .chars()
        .filter(|c| allowed.contains(&(*c as u8)))
        .collect();
    if normalized != cleaned {
        return Err(AttachmentError::InvalidAttachmentId(cleaned).into());
    }
    Ok(normalized)
}

fn attachment_kind_from_id(attachment_id: &str) -> Option<String> {
    if attachment_id.starts_with("img_") {
        Some(IMAGE_ATTACHMENT_KIND.to_string())
    } else if attachment_id.starts_with("file_") {
        Some(FILE_ATTACHMENT_KIND.to_string())
    } else {
        None
    }
}

fn kind_label(kind: &str) -> &str {
    if kind == IMAGE_ATTACHMENT_KIND {
        "an image"
    } else {
        "a file"
    }
}

fn now_text() -> String {
    let now: DateTime<Utc> = Utc::now();
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn generate_image_attachment_id() -> String {
    use std::fmt::Write;
    let mut id = String::from("img_");
    let bytes = uuid::Uuid::new_v4();
    for byte in bytes.as_bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

fn generate_file_attachment_id() -> String {
    use std::fmt::Write;
    let mut id = String::from("file_");
    let bytes = uuid::Uuid::new_v4();
    for byte in bytes.as_bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

fn normalize_content_type(content_type: Option<&str>, filename: Option<&str>) -> String {
    let cleaned = content_type.unwrap_or("").trim().to_lowercase();
    if !cleaned.is_empty() {
        return cleaned;
    }
    if let Some(name) = filename {
        if !name.is_empty() {
            if let Some(guess) = mime_guess::from_path(name).first() {
                return guess.to_string();
            }
        }
    }
    "application/octet-stream".to_string()
}

fn safe_file_suffix(filename: Option<&str>, content_type: &str) -> String {
    let mut suffix = String::new();
    if let Some(name) = filename {
        if !name.is_empty() {
            // mimic Path(name).suffixes[-2:] (concatenation of the last two
            // dotted extensions, including their leading dots).
            let last = Path::new(name)
                .file_name()
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(dot_pos) = last.rfind('.') {
                suffix = last[dot_pos..].to_string();
            }
        }
    }
    if suffix.is_empty() {
        if let Ok(mime) = content_type.parse::<mime::Mime>() {
            if let Some(exts) = mime_guess::get_mime_extensions(&mime) {
                if let Some(first) = exts.first() {
                    suffix = format!(".{first}");
                }
            }
        }
    }
    let allowed: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-";
    let cleaned: String = suffix
        .chars()
        .filter(|c| allowed.contains(&(*c as u8)))
        .collect();
    let mut cleaned = cleaned;
    if !cleaned.starts_with('.') {
        cleaned.clear();
    }
    if cleaned.len() > 20 {
        cleaned.clear();
    }
    if cleaned.is_empty() {
        ".bin".to_string()
    } else {
        cleaned
    }
}

fn workspace_relative_path(workspace: &Path, target: &Path) -> String {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    match target.strip_prefix(&workspace) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => target.to_string_lossy().replace('\\', "/"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn write_metadata(
    meta_dir: &Path,
    attachment_id: &str,
    value: &serde_json::Value,
) -> Result<()> {
    let path = meta_dir.join(format!("{attachment_id}.json"));
    let text = serde_json::to_string_pretty(value).map_err(|e| {
        AttachmentError::InvalidMetadata(format!("serialize failed: {e}"))
    })?;
    fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_id_round_trip() {
        assert_eq!(build_attachment_url("img_abc"), "attachment://img_abc");
        assert_eq!(
            build_attachment_content_url("img_abc"),
            "/api/attachments/img_abc/content"
        );
    }

    #[test]
    fn normalize_attachment_id_rejects_bad_chars() {
        assert!(normalize_attachment_id("img/abc").is_err());
        assert!(normalize_attachment_id("").is_err());
        assert!(normalize_attachment_id("img abc").is_err());
        assert!(normalize_attachment_id("img_ok_123").is_ok());
    }

    #[test]
    fn attachment_id_from_url_handles_prefix() {
        assert_eq!(
            AttachmentStore::attachment_id_from_url("attachment://img_abc"),
            Some("img_abc".to_string())
        );
        assert_eq!(AttachmentStore::attachment_id_from_url("https://x"), None);
    }

    #[test]
    fn file_budget_default_is_200mb() {
        assert_eq!(DEFAULT_FILE_BUDGET.max_input_bytes, 200 * 1024 * 1024);
    }

    #[test]
    fn create_and_read_file_attachment_round_trip() {
        // Use a unique subdir of std::env::temp_dir() to keep the test
        // self-contained without pulling in `tempfile`.
        let unique = format!(
            "echobot-attach-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let dir = std::env::temp_dir().join(unique);
        let store = AttachmentStore::new(&dir, None, None);
        let attachment = store
            .create_file_attachment(b"hello world", Some("text/plain"), Some("greeting.txt"))
            .unwrap();
        assert!(attachment.attachment_id.starts_with("file_"));
        assert_eq!(attachment.size_bytes, 11);
        let loaded = store.get_file_attachment(&attachment.attachment_id).unwrap();
        assert_eq!(loaded.attachment_id, attachment.attachment_id);
        assert_eq!(loaded.content_type, "text/plain");
        let read = fs::read(dir.join(&attachment.relative_path)).unwrap();
        assert_eq!(read, b"hello world");

        // Cleanup.
        let _ = fs::remove_dir_all(&dir);
    }
}
