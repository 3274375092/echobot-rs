//! `attachments` router — upload / download / delete images and files.

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use echobot_core::attachments::{
    AttachmentStore, FileAttachment, ImageAttachment, DEFAULT_FILE_BUDGET,
};

use crate::error::AppError;
use crate::schemas::{FileAttachmentResponse, ImageAttachmentResponse};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/attachments/images", post(upload_image))
        .route("/attachments/files", post(upload_file))
        .route(
            "/attachments/{attachment_id}",
            delete(delete_attachment).get(download_attachment),
        )
        .route(
            "/attachments/{attachment_id}/content",
            get(download_attachment),
        )
}

fn store(state: &AppState) -> Result<std::sync::Arc<AttachmentStore>, AppError> {
    Ok(state.runtime().context.attachment_store.clone())
}

const UPLOAD_READ_CHUNK_BYTES: usize = 1024 * 1024;

async fn read_multipart_bytes(
    mut multipart: Multipart,
    max_bytes: usize,
    label: &str,
) -> Result<(Vec<u8>, Option<String>, Option<String>), AppError> {
    let mut bytes = Vec::new();
    let mut content_type: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut field_found = false;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        if field.name() != Some("file") {
            // Drain non-file fields so the body is consumed.
            continue;
        }
        field_found = true;
        if let Some(ctype) = field.content_type() {
            content_type = Some(ctype.to_string());
        }
        if let Some(name) = field.file_name() {
            filename = Some(name.to_string());
        }
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > max_bytes {
                return Err(AppError::BadRequest(format!(
                    "{label} exceeds the upload size limit ({} > {max_bytes} bytes)",
                    bytes.len()
                )));
            }
        }
        break;
    }
    if !field_found {
        return Err(AppError::BadRequest("missing 'file' field".to_string()));
    }
    let _ = UPLOAD_READ_CHUNK_BYTES;
    Ok((bytes, content_type, filename))
}

fn image_response(a: &ImageAttachment) -> ImageAttachmentResponse {
    ImageAttachmentResponse {
        attachment_id: a.attachment_id.clone(),
        url: a.attachment_url(),
        preview_url: a.preview_url(),
        content_type: a.content_type.clone(),
        size_bytes: a.size_bytes,
        width: a.width,
        height: a.height,
        original_filename: a.original_filename.clone(),
    }
}

fn file_response(a: &FileAttachment, workspace: &std::path::Path) -> FileAttachmentResponse {
    let base = std::path::Path::new(&a.relative_path);
    let stored_path: PathBuf = workspace.join(".echobot").join("attachments").join(base);
    let workspace_path = stored_path
        .strip_prefix(workspace)
        .map(|p| p.display().to_string().replace("\\", "/"))
        .unwrap_or_else(|_| a.relative_path.clone());
    FileAttachmentResponse {
        attachment_id: a.attachment_id.clone(),
        url: a.attachment_url(),
        download_url: a.download_url(),
        content_type: a.content_type.clone(),
        size_bytes: a.size_bytes,
        original_filename: a.original_filename.clone(),
        workspace_path,
    }
}

async fn upload_image(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<ImageAttachmentResponse>, AppError> {
    let _runtime = state.runtime();
    // image budget isn't exposed publicly; use the default image cap.
    let max_bytes = DEFAULT_FILE_BUDGET.max_input_bytes;
    let (bytes, content_type, filename) =
        read_multipart_bytes(multipart, max_bytes, "Chat image").await?;
    let store = store(&state)?;
    let attachment = tokio::task::spawn_blocking(move || {
        store.create_image_attachment(
            &bytes,
            content_type.as_deref(),
            filename.as_deref(),
        )
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(image_response(&attachment)))
}

async fn upload_file(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<FileAttachmentResponse>, AppError> {
    let runtime = state.runtime();
    let max_bytes = runtime
        .context
        .attachment_store
        .base_dir()
        .as_os_str()
        .len()
        .max(DEFAULT_FILE_BUDGET.max_input_bytes);
    let (bytes, content_type, filename) =
        read_multipart_bytes(multipart, max_bytes, "Attachment file").await?;
    let store = store(&state)?;
    let attachment = tokio::task::spawn_blocking(move || {
        store.create_file_attachment(
            &bytes,
            content_type.as_deref(),
            filename.as_deref(),
        )
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(file_response(&attachment, runtime.workspace_path())))
}

async fn delete_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let store = store(&state)?;
    tokio::task::spawn_blocking(move || store.delete_attachment(&attachment_id))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn download_attachment(
    State(state): State<AppState>,
    Path(attachment_id): Path<String>,
) -> Result<Response, AppError> {
    let store = store(&state)?;
    let resolved = tokio::task::spawn_blocking(move || {
        store.resolve_attachment_download(&attachment_id)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| AppError::NotFound(e.to_string()))?;
    let (attachment_ref, path) = resolved;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;
    let (content_type, download_filename) = match attachment_ref {
        echobot_core::attachments::AttachmentRef::Image(img) => {
            (img.content_type.clone(), img.download_filename())
        }
        echobot_core::attachments::AttachmentRef::File(file) => {
            (file.content_type.clone(), file.download_filename())
        }
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", download_filename),
        )
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(response)
}

// silence unused imports
#[allow(dead_code)]
fn _unused(_: impl IntoResponse) {}
