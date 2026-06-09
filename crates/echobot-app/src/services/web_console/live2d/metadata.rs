//! Live2D metadata service stub.
//!
//! v1: returns empty metadata. The full port will replicate the Python
//! `metadata.py` logic (model3.json discovery, vtube.json matching,
//! hotkey normalization, etc.).

use serde_json::Value;

use crate::error::AppError;

#[derive(Debug, Default)]
pub struct Live2DMetadataService;

impl Live2DMetadataService {
    pub fn new() -> Self {
        Self
    }

    pub fn load_model_data(&self, _model_path: &std::path::Path) -> Result<Value, AppError> {
        Ok(Value::Null)
    }
}
