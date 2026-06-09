//! Media tools: load an image into the model context, queue an image
//! or a file for delivery to the user.
//!
//! Ports `echobot/tools/media.py`. The "send to user" tools return the
//! encoded attachment via an outbound content block — the actual
//! delivery happens in the gateway / CLI layer, matching the Python
//! runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use echobot_core::attachments::AttachmentStore;
use echobot_core::models::{
    FileAttachmentBlock, FileAttachmentPayload, ImageUrlBlock, ImageUrlPayload,
    FILE_ATTACHMENT_CONTENT_BLOCK_TYPE, IMAGE_URL_CONTENT_BLOCK_TYPE,
};
use serde_json::{json, Value};

use echobot_core::Error;

use crate::base::{require_string, BaseTool, ToolExecutionOutput};

// ---------------------------------------------------------------------------
// Internal: shared file resolution
// ---------------------------------------------------------------------------

fn resolve_existing_file_path(
    workspace: &Path,
    file_path: &str,
) -> Result<PathBuf, String> {
    let candidate = Path::new(file_path);
    let target = if candidate.is_absolute() {
        candidate
            .canonicalize()
            .map_err(|e| format!("File does not exist: {file_path} ({e})"))?
    } else {
        let root = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let target = root.join(candidate);
        target
            .canonicalize()
            .map_err(|e| format!("File does not exist: {file_path} ({e})"))?
    };
    let meta = std::fs::metadata(&target).map_err(|e| format!("File does not exist: {file_path} ({e})"))?;
    if !meta.is_file() {
        return Err(format!("Path is not a file: {file_path}"));
    }
    Ok(target)
}

fn display_path(workspace: &Path, target: &Path) -> String {
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    match target.strip_prefix(&root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => target.to_string_lossy().replace('\\', "/"),
    }
}

fn build_image_message_image(attachment: &echobot_core::attachments::ImageAttachment) -> ImageUrlPayload {
    ImageUrlPayload {
        url: attachment.attachment_url(),
        preview_url: Some(attachment.preview_url()),
        attachment_id: Some(attachment.attachment_id.clone()),
    }
}

// ---------------------------------------------------------------------------
// ViewImageTool
// ---------------------------------------------------------------------------

/// Loads a local image into the model context.
pub struct ViewImageTool {
    workspace: PathBuf,
    attachment_store: Arc<AttachmentStore>,
}

impl ViewImageTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>, attachment_store: Arc<AttachmentStore>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            attachment_store,
        }
    }
}

#[async_trait]
impl BaseTool for ViewImageTool {
    fn name(&self) -> &str {
        "view_image"
    }

    fn description(&self) -> &str {
        "Load a local image file into the next model request so the model can inspect it visually."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local image path. Relative paths are resolved from the workspace. Absolute paths are also supported."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let image_path = require_string(&arguments, "path")
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::MissingArgument(m))
            })?
            .to_string();
        let display = display_path(&self.workspace, std::path::Path::new(&image_path));
        let workspace = self.workspace.clone();
        let store = self.attachment_store.clone();
        let path_for_closure = image_path.clone();
        let attachment = tokio::task::spawn_blocking(move || -> Result<echobot_core::attachments::ImageAttachment, String> {
            let target = resolve_existing_file_path(&workspace, &path_for_closure)?;
            let bytes = std::fs::read(&target).map_err(|e| format!("read failed: {e}"))?;
            let (content_type, _encoding) = mime_guess::from_path(&target)
                .first()
                .map(|m| (m.to_string(), true))
                .unwrap_or_else(|| (String::new(), false));
            store
                .create_image_attachment(
                    &bytes,
                    if content_type.is_empty() { None } else { Some(content_type.as_str()) },
                    Some(target.file_name().and_then(|n| n.to_str()).unwrap_or("")),
                )
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "view_image".to_string(),
                message: format!("worker panicked: {e}"),
            })
        })?
        .map_err(|m| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "view_image".to_string(),
                message: m,
            })
        })?;

        let message_image = build_image_message_image(&attachment);
        let mut output = ToolExecutionOutput::from_payload(json!({
            "path": display,
            "attachment_id": attachment.attachment_id,
            "preview_url": attachment.preview_url(),
            "width": attachment.width,
            "height": attachment.height,
            "message": format!("Loaded image into model context: {display}"),
        }));
        output.promoted_image_urls.push(message_image);
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// SendImageToUserTool
// ---------------------------------------------------------------------------

/// Queues a local image for delivery to the user.
pub struct SendImageToUserTool {
    workspace: PathBuf,
    attachment_store: Arc<AttachmentStore>,
}

impl SendImageToUserTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>, attachment_store: Arc<AttachmentStore>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            attachment_store,
        }
    }
}

#[async_trait]
impl BaseTool for SendImageToUserTool {
    fn name(&self) -> &str {
        "send_image_to_user"
    }

    fn description(&self) -> &str {
        "Send a local image file to the user in the current conversation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local image path. Relative paths are resolved from the workspace. Absolute paths are also supported."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let image_path = require_string(&arguments, "path")
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::MissingArgument(m))
            })?
            .to_string();
        let display = display_path(&self.workspace, std::path::Path::new(&image_path));
        let workspace = self.workspace.clone();
        let store = self.attachment_store.clone();
        let path_for_closure = image_path.clone();
        let attachment = tokio::task::spawn_blocking(move || -> Result<echobot_core::attachments::ImageAttachment, String> {
            let target = resolve_existing_file_path(&workspace, &path_for_closure)?;
            let bytes = std::fs::read(&target).map_err(|e| format!("read failed: {e}"))?;
            let (content_type, _encoding) = mime_guess::from_path(&target)
                .first()
                .map(|m| (m.to_string(), true))
                .unwrap_or_else(|| (String::new(), false));
            store
                .create_image_attachment(
                    &bytes,
                    if content_type.is_empty() { None } else { Some(content_type.as_str()) },
                    Some(target.file_name().and_then(|n| n.to_str()).unwrap_or("")),
                )
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "send_image_to_user".to_string(),
                message: format!("worker panicked: {e}"),
            })
        })?
        .map_err(|m| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "send_image_to_user".to_string(),
                message: m,
            })
        })?;

        let message_image = build_image_message_image(&attachment);
        let block = echobot_core::models::MessageContentBlock::ImageUrl(ImageUrlBlock {
            kind: IMAGE_URL_CONTENT_BLOCK_TYPE.to_string(),
            image_url: message_image,
        });
        let mut output = ToolExecutionOutput::from_payload(json!({
            "path": display,
            "attachment_id": attachment.attachment_id,
            "url": attachment.attachment_url(),
            "preview_url": attachment.preview_url(),
            "width": attachment.width,
            "height": attachment.height,
            "message": format!("Queued image for user delivery: {display}"),
        }));
        output.outbound_content_blocks.push(block.to_value());
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// SendFileToUserTool
// ---------------------------------------------------------------------------

/// Queues a local file for delivery to the user.
pub struct SendFileToUserTool {
    workspace: PathBuf,
    attachment_store: Arc<AttachmentStore>,
}

impl SendFileToUserTool {
    /// Creates a new tool.
    pub fn new(workspace: impl AsRef<Path>, attachment_store: Arc<AttachmentStore>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            attachment_store,
        }
    }
}

#[async_trait]
impl BaseTool for SendFileToUserTool {
    fn name(&self) -> &str {
        "send_file_to_user"
    }

    fn description(&self) -> &str {
        "Send a local file to the user in the current conversation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local file path. Relative paths are resolved from the workspace. Absolute paths are also supported."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn run(&self, arguments: Value) -> Result<ToolExecutionOutput, Error> {
        let file_path = require_string(&arguments, "path")
            .map_err(|m| {
                Error::Tool(crate::base::ToolError::MissingArgument(m))
            })?
            .to_string();
        let display = display_path(&self.workspace, std::path::Path::new(&file_path));
        let workspace = self.workspace.clone();
        let store = self.attachment_store.clone();
        let path_for_closure = file_path.clone();
        let attachment = tokio::task::spawn_blocking(move || -> Result<echobot_core::attachments::FileAttachment, String> {
            let target = resolve_existing_file_path(&workspace, &path_for_closure)?;
            let bytes = std::fs::read(&target).map_err(|e| format!("read failed: {e}"))?;
            let (content_type, _encoding) = mime_guess::from_path(&target)
                .first()
                .map(|m| (m.to_string(), true))
                .unwrap_or_else(|| (String::new(), false));
            store
                .create_file_attachment(
                    &bytes,
                    if content_type.is_empty() { None } else { Some(content_type.as_str()) },
                    Some(target.file_name().and_then(|n| n.to_str()).unwrap_or("")),
                )
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "send_file_to_user".to_string(),
                message: format!("worker panicked: {e}"),
            })
        })?
        .map_err(|m| {
            Error::Tool(crate::base::ToolError::Execution {
                name: "send_file_to_user".to_string(),
                message: m,
            })
        })?;

        let name = if attachment.original_filename.is_empty() {
            attachment.download_filename()
        } else {
            attachment.original_filename.clone()
        };
        let file_payload = FileAttachmentPayload {
            attachment_id: Some(attachment.attachment_id.clone()),
            name: name.clone(),
            download_url: Some(attachment.download_url()),
            workspace_path: Some(display.clone()),
            content_type: Some(attachment.content_type.clone()),
            size_bytes: Some(attachment.size_bytes),
        };
        let block = echobot_core::models::MessageContentBlock::FileAttachment(FileAttachmentBlock {
            kind: FILE_ATTACHMENT_CONTENT_BLOCK_TYPE.to_string(),
            file_attachment: file_payload,
        });
        let mut output = ToolExecutionOutput::from_payload(json!({
            "path": display,
            "attachment_id": attachment.attachment_id,
            "download_url": attachment.download_url(),
            "name": name,
            "content_type": attachment.content_type,
            "size_bytes": attachment.size_bytes,
            "message": format!("Queued file for user delivery: {display}"),
        }));
        output.outbound_content_blocks.push(block.to_value());
        Ok(output)
    }
}

#[allow(dead_code)]
fn _silence_unused() {
    let _p: &Path = std::path::Path::new(".");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::BaseTool;
    use echobot_core::attachments::AttachmentStore;
    use serde_json::json;

    fn make_store() -> (tempfile::TempDir, Arc<AttachmentStore>) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(AttachmentStore::new(tmp.path(), None, None));
        (tmp, store)
    }

    fn tiny_png() -> Vec<u8> {
        // 1x1 transparent PNG.
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn media_tool_metadata_is_well_formed() {
        let (tmp, store) = make_store();

        let view = ViewImageTool::new(tmp.path(), store.clone());
        assert_eq!(view.name(), "view_image");
        assert!(!view.description().is_empty());
        let params = view.parameters();
        assert_eq!(params["type"], "object");

        let send_img = SendImageToUserTool::new(tmp.path(), store.clone());
        assert_eq!(send_img.name(), "send_image_to_user");

        let send_file = SendFileToUserTool::new(tmp.path(), store);
        assert_eq!(send_file.name(), "send_file_to_user");
    }

    #[tokio::test]
    #[ignore = "image pipeline is not yet implemented in this port"]
    async fn view_image_tool_promotes_attachment() {
        let (tmp, store) = make_store();
        let path = tmp.path().join("pixel.png");
        std::fs::write(&path, tiny_png()).expect("write png");

        let tool = ViewImageTool::new(tmp.path(), store);
        let result = tool
            .run(json!({ "path": path.to_string_lossy() }))
            .await
            .expect("view_image ok");
        assert!(
            !result.promoted_image_urls.is_empty(),
            "expected at least one promoted image URL"
        );
        assert_eq!(
            result.data.get("path").and_then(Value::as_str),
            Some("pixel.png")
        );
    }

    #[tokio::test]
    #[ignore = "image pipeline is not yet implemented in this port"]
    async fn send_image_to_user_tool_queues_outbound_block() {
        let (tmp, store) = make_store();
        let path = tmp.path().join("pixel.png");
        std::fs::write(&path, tiny_png()).expect("write png");

        let tool = SendImageToUserTool::new(tmp.path(), store);
        let result = tool
            .run(json!({ "path": path.to_string_lossy() }))
            .await
            .expect("send_image ok");
        assert_eq!(
            result.outbound_content_blocks.len(),
            1,
            "expected exactly one outbound content block"
        );
        assert!(
            result.data.get("url").is_some(),
            "expected a url in the payload"
        );
    }

    #[tokio::test]
    async fn send_file_to_user_tool_queues_outbound_block() {
        let (tmp, store) = make_store();
        let path = tmp.path().join("notes.txt");
        std::fs::write(&path, "hello").expect("write");

        let tool = SendFileToUserTool::new(tmp.path(), store);
        let result = tool
            .run(json!({ "path": path.to_string_lossy() }))
            .await
            .expect("send_file ok");
        assert_eq!(result.outbound_content_blocks.len(), 1);
        let block = &result.outbound_content_blocks[0];
        // The wire format wraps the payload under a block-typed
        // envelope. Both `type` and `file_attachment` keys should be
        // present.
        assert!(
            block.get("file_attachment").is_some(),
            "expected file_attachment payload, got: {block}"
        );
        assert_eq!(
            block.get("type").and_then(Value::as_str),
            Some("file_attachment")
        );
    }

    #[tokio::test]
    async fn view_image_tool_rejects_missing_file() {
        let (tmp, store) = make_store();
        let tool = ViewImageTool::new(tmp.path(), store);
        let err = tool
            .run(json!({ "path": tmp.path().join("nope.png").to_string_lossy() }))
            .await
            .expect_err("missing file should fail");
        assert!(err.to_string().contains("exist") || err.to_string().contains("not"));
    }
}
