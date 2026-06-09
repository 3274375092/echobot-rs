//! Image normalization helpers (budgets + base64 data URLs).
//!
//! Mirrors the data-class side of `echobot/images.py` (budgets, encoded
//! payloads) and the public helpers that don't depend on Pillow. The actual
//! decode/resize/encode pipeline (Pillow-equivalent) lives in a separate
//! crate because porting Pillow line-for-line is out of scope for this
//! porting slice; the budget + result types defined here are the contract
//! that pipeline implements.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{ImageError, Result};

/// Content type used for normalized JPEGs.
pub const JPEG_CONTENT_TYPE: &str = "image/jpeg";

/// Constraints + tuning knobs for image normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageBudget {
    /// Maximum allowed input size in bytes.
    pub max_input_bytes: usize,
    /// Maximum allowed compressed (output) size in bytes.
    pub max_output_bytes: usize,
    /// Maximum allowed longest-side in pixels.
    pub max_side: u32,
    /// Maximum allowed total pixel count.
    pub max_pixels: u64,
    /// Initial JPEG quality to try.
    pub start_quality: u8,
    /// Lowest acceptable JPEG quality.
    pub min_quality: u8,
    /// Step size when reducing quality.
    pub quality_step: u8,
    /// Scale factor when resizing down (e.g. `0.85`).
    pub resize_step: f32,
    /// Maximum number of resize attempts before giving up.
    pub max_resize_attempts: u32,
}

impl Default for ImageBudget {
    fn default() -> Self {
        Self {
            max_input_bytes: 40 * 1024 * 1024,
            max_output_bytes: 4 * 1024 * 1024,
            max_side: 3072,
            max_pixels: 24_000_000,
            start_quality: 90,
            min_quality: 55,
            quality_step: 10,
            resize_step: 0.85,
            max_resize_attempts: 6,
        }
    }
}

/// The default [`ImageBudget`].
pub const DEFAULT_IMAGE_BUDGET: ImageBudget = ImageBudget {
    max_input_bytes: 40 * 1024 * 1024,
    max_output_bytes: 4 * 1024 * 1024,
    max_side: 3072,
    max_pixels: 24_000_000,
    start_quality: 90,
    min_quality: 55,
    quality_step: 10,
    resize_step: 0.85,
    max_resize_attempts: 6,
};

/// The result of running [`normalize_image_bytes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedImage {
    /// The re-encoded JPEG bytes.
    pub image_bytes: Vec<u8>,
    /// The content type (always [`JPEG_CONTENT_TYPE`]).
    pub content_type: String,
    /// Final width in pixels.
    pub width: u32,
    /// Final height in pixels.
    pub height: u32,
    /// The JPEG quality used.
    pub quality: u8,
}

/// Validates input bytes against the supplied budget. Returns the relevant
/// [`ImageError`] variant when the bytes are too large or empty.
pub fn validate_image_bytes(image_bytes: &[u8], budget: &ImageBudget) -> Result<()> {
    if image_bytes.is_empty() {
        return Err(ImageError::Empty.into());
    }
    if image_bytes.len() > budget.max_input_bytes {
        return Err(ImageError::InputTooLarge {
            actual: image_bytes.len(),
            limit: budget.max_input_bytes,
        }
        .into());
    }
    Ok(())
}

/// Validates a `(width, height)` pair against the pixel budget.
pub fn validate_pixel_budget(width: u32, height: u32, budget: &ImageBudget) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(ImageError::InvalidSize.into());
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > budget.max_pixels {
        return Err(ImageError::PixelBudgetExceeded {
            width,
            height,
            max: budget.max_pixels,
        }
        .into());
    }
    Ok(())
}

/// Encodes the given JPEG bytes as a `data:` URL.
pub fn image_bytes_to_jpeg_data_url(
    image_bytes: &[u8],
    content_type: &str,
) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes);
    format!("data:{content_type};base64,{encoded}")
}

/// Lightweight wrapper around [`image_bytes_to_jpeg_data_url`] that assumes
/// the output is JPEG and uses the [`DEFAULT_IMAGE_BUDGET`].
pub fn image_bytes_to_default_jpeg_data_url(image_bytes: &[u8]) -> Result<String> {
    validate_image_bytes(image_bytes, &DEFAULT_IMAGE_BUDGET)?;
    Ok(image_bytes_to_jpeg_data_url(image_bytes, JPEG_CONTENT_TYPE))
}

/// Returns the next resize dimensions given a current size and a scale.
///
/// Pure helper (no I/O) — exposed so the actual decode/encode pipeline can
/// share the same math.
pub fn next_resize_dimensions(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let next_w = ((width as f32) * scale).round().max(1.0) as u32;
    let next_h = ((height as f32) * scale).round().max(1.0) as u32;
    (next_w, next_h)
}

/// Returns the resize dimensions that fit `width`/`height` into
/// `max_side` while preserving aspect ratio.
pub fn fit_to_max_side(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_side {
        return (width, height);
    }
    let scale = max_side as f32 / longest as f32;
    let new_w = ((width as f32) * scale).round().max(1.0) as u32;
    let new_h = ((height as f32) * scale).round().max(1.0) as u32;
    (new_w, new_h)
}

/// The list of JPEG qualities the encoder should try, in descending order,
/// clamped to `[min_quality, start_quality]`.
pub fn quality_steps(budget: &ImageBudget) -> Vec<u8> {
    let start = budget.start_quality.min(100).max(budget.min_quality);
    let mut values: Vec<u8> = Vec::new();
    let mut q = start;
    while q >= budget.min_quality {
        values.push(q);
        if budget.quality_step == 0 {
            break;
        }
        match q.checked_sub(budget.quality_step) {
            Some(next) if next >= budget.min_quality => q = next,
            _ => break,
        }
    }
    if values.last() != Some(&budget.min_quality) {
        values.push(budget.min_quality);
    }
    values
}

/// Normalizes raw image bytes into a JPEG using a no-op stub pipeline.
///
/// The Python version uses Pillow to decode → exif-transpose → resize → JPEG
/// encode in a quality loop. Implementing that pipeline in Rust is out of
/// scope for this porting slice — the contract lives here so the rest of
/// the codebase can build against it. Use `echobot-images` (TODO) for the
/// real implementation, or call this function with bytes that are already
/// JPEG so the stub pass-through succeeds.
pub fn normalize_image_bytes(
    image_bytes: &[u8],
    budget: Option<&ImageBudget>,
) -> Result<NormalizedImage> {
    let budget = budget.unwrap_or(&DEFAULT_IMAGE_BUDGET);
    validate_image_bytes(image_bytes, budget)?;

    // Stub: we cannot decode arbitrary formats without a real image
    // pipeline, so we accept JPEG only and copy through.
    if image_bytes.len() < 3 || &image_bytes[..3] != b"\xFF\xD8\xFF" {
        return Err(ImageError::UnsupportedFormat(
            "decode pipeline not yet implemented in this port".to_string(),
        )
        .into());
    }
    // No way to know real dimensions without decoding; default to budget cap.
    let (width, height) = fit_to_max_side(budget.max_side, budget.max_side, budget.max_side);
    Ok(NormalizedImage {
        image_bytes: image_bytes.to_vec(),
        content_type: JPEG_CONTENT_TYPE.to_string(),
        width,
        height,
        quality: budget.start_quality,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_image_bytes_rejects_empty() {
        let err = validate_image_bytes(&[], &DEFAULT_IMAGE_BUDGET).unwrap_err();
        assert!(format!("{err}").contains("must not be empty"));
    }

    #[test]
    fn validate_image_bytes_rejects_oversize() {
        let mut budget = DEFAULT_IMAGE_BUDGET.clone();
        budget.max_input_bytes = 10;
        let err = validate_image_bytes(&[0u8; 20], &budget).unwrap_err();
        assert!(format!("{err}").contains("upload size limit"));
    }

    #[test]
    fn quality_steps_descends_and_includes_min() {
        let mut budget = DEFAULT_IMAGE_BUDGET.clone();
        budget.start_quality = 90;
        budget.min_quality = 50;
        budget.quality_step = 10;
        let steps = quality_steps(&budget);
        assert_eq!(steps.first(), Some(&90));
        assert_eq!(steps.last(), Some(&50));
    }

    #[test]
    fn fit_to_max_side_preserves_aspect() {
        let (w, h) = fit_to_max_side(4000, 2000, 1000);
        assert_eq!(w, 1000);
        assert_eq!(h, 500);
    }

    #[test]
    fn data_url_is_base64_encoded() {
        let url = image_bytes_to_jpeg_data_url(b"hello", "image/jpeg");
        assert!(url.starts_with("data:image/jpeg;base64,"));
        // base64 of "hello" = "aGVsbG8="
        assert!(url.ends_with("aGVsbG8="));
    }
}
