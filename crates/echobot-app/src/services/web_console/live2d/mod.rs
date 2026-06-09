//! Live2D sub-module stub.

pub mod metadata;

#[derive(Debug, Clone)]
pub struct Live2DUploadFile {
    pub relative_path: String,
    pub file_bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Live2DService;

impl Live2DService {
    pub fn new() -> Self {
        Self
    }
}
