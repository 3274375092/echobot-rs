//! Helpers for resolving turn inputs into LLM message content.
//!
//! Mirrors `echobot/turn_inputs.py`. The Python version also depends on
//! `echobot/orchestration/route_modes.py` for file-routing logic; that
//! orchestration lives in a separate crate so this module only ships the
//! attachment-id resolution helpers that the core crate owns.

use std::path::Path;

use serde_json::Value;

use crate::attachments::AttachmentStore;
use crate::error::Result;
use crate::models::{
    build_message_content, ImageUrlPayload, MessageContent, MessageContentBlock,
    FILE_ATTACHMENT_CONTENT_BLOCK_TYPE, IMAGE_URL_CONTENT_BLOCK_TYPE,
};

/// Resolves the supplied attachment inputs (any iterable of attachment ids or
/// objects carrying an `attachment_id` field) into deduplicated, ordered
/// image-payload dicts.
pub fn resolve_attachment_images<I, T>(store: &AttachmentStore, inputs: I) -> Result<Vec<Value>>
where
    I: IntoIterator<Item = T>,
    T: AsRef<str>,
{
    let mut images: Vec<Value> = Vec::new();
    for attachment_id in attachment_ids_from_inputs(inputs) {
        let attachment = store.get_image_attachment(&attachment_id)?;
        images.push(attachment.to_message_image());
    }
    Ok(images)
}

/// Resolves the supplied attachment inputs into file-relative path dicts,
/// scoped to `workspace`.
pub fn resolve_attachment_files<I, T>(
    store: &AttachmentStore,
    workspace: &Path,
    inputs: I,
) -> Result<Vec<Value>>
where
    I: IntoIterator<Item = T>,
    T: AsRef<str>,
{
    let mut files: Vec<Value> = Vec::new();
    for attachment_id in attachment_ids_from_inputs(inputs) {
        files.push(store.file_attachment_message_content(&attachment_id, workspace)?);
    }
    Ok(files)
}

/// Extracts and de-duplicates attachment ids from any iterable of values.
///
/// Each item is interpreted as:
/// * a plain string (the attachment id), or
/// * anything implementing `AsRef<str>` whose `as_ref()` yields the id.
///
/// Strings are trimmed; empty strings are dropped.
pub fn attachment_ids_from_inputs<I, T>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = T>,
    T: AsRef<str>,
{
    let mut unique: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for value in values {
        let cleaned = value.as_ref().trim().to_string();
        if cleaned.is_empty() || seen.contains(&cleaned) {
            continue;
        }
        seen.insert(cleaned.clone());
        unique.push(cleaned);
    }
    unique
}

/// Builds a user message [`MessageContent`] from text, image payloads and
/// file payloads. This is the convenience entry point most call sites
/// should use.
pub fn build_user_message_content(
    text: &str,
    image_urls: Option<&[Value]>,
    file_attachments: Option<&[Value]>,
) -> MessageContent {
    build_message_content(text, image_urls, file_attachments)
}

/// Resolves attachment inputs directly into [`MessageContentBlock`]s (image
/// + file). Useful when the caller has already collected image/file dicts
/// and just wants the typed blocks.
pub fn attachment_inputs_to_blocks(
    store: &AttachmentStore,
    workspace: &Path,
    inputs: &[String],
) -> Result<Vec<MessageContentBlock>> {
    let mut blocks: Vec<MessageContentBlock> = Vec::new();
    for id in attachment_ids_from_inputs(inputs) {
        // Images first (per the Python helper that resolves images
        // separately); then files.
        if let Ok(attachment) = store.get_image_attachment(&id) {
            let value = attachment.to_message_image();
            if let Some(payload) = crate::models::normalize_image_input(&value) {
                blocks.push(MessageContentBlock::ImageUrl(crate::models::ImageUrlBlock {
                    kind: IMAGE_URL_CONTENT_BLOCK_TYPE.to_string(),
                    image_url: payload,
                }));
            }
            continue;
        }
        if let Ok(value) = store.file_attachment_message_content(&id, workspace) {
            if let Some(payload) = crate::models::normalize_file_attachment_input(&value) {
                blocks.push(MessageContentBlock::FileAttachment(
                    crate::models::FileAttachmentBlock {
                        kind: FILE_ATTACHMENT_CONTENT_BLOCK_TYPE.to_string(),
                        file_attachment: payload,
                    },
                ));
            }
        }
    }
    Ok(blocks)
}

/// Convenience for callers that already have an [`ImageUrlPayload`] list.
pub fn image_payloads_to_blocks(payloads: Vec<ImageUrlPayload>) -> Vec<MessageContentBlock> {
    payloads
        .into_iter()
        .map(|payload| {
            MessageContentBlock::ImageUrl(crate::models::ImageUrlBlock {
                kind: IMAGE_URL_CONTENT_BLOCK_TYPE.to_string(),
                image_url: payload,
            })
        })
        .collect()
}

/// Text content block helper (re-exported for ergonomics).
pub fn text_block(text: impl Into<String>) -> Option<MessageContentBlock> {
    crate::models::TextContentBlock::new(text).map(MessageContentBlock::Text)
}

/// Re-export of the text content block type constant.
pub use crate::models::TEXT_CONTENT_BLOCK_TYPE as TEXT_TYPE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_ids_dedup_and_trim() {
        let inputs = vec!["img_a", "  img_a  ", "img_b", ""];
        let ids = attachment_ids_from_inputs(inputs);
        assert_eq!(ids, vec!["img_a".to_string(), "img_b".to_string()]);
    }

    #[test]
    fn build_user_message_content_shortcuts_to_text() {
        let c = build_user_message_content("hello", None, None);
        assert!(matches!(c, MessageContent::Text(t) if t == "hello"));
    }
}
