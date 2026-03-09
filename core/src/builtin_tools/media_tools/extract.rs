//! `document_extract` tool — text and table extraction from documents.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::error::Result;
use crate::media::detect::detect_from_path;
use crate::media::{MediaInput, MediaOutput, MediaPipeline};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocumentExtractArgs {
    /// Path to a local document file (txt, md, html, pdf, docx, xlsx).
    pub file_path: String,

    /// Optional extraction prompt (e.g., "Extract all tables", "Summarize").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentExtractOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl DocumentExtractOutput {
    fn ok(text: String) -> Self {
        Self {
            success: true,
            text: Some(text),
            message: None,
            data: None,
        }
    }

    fn ok_data(text: String, data: Value) -> Self {
        Self {
            success: true,
            text: Some(text),
            message: None,
            data: Some(data),
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            text: None,
            message: Some(msg.into()),
            data: None,
        }
    }
}

#[derive(Clone)]
pub struct DocumentExtractTool {
    pipeline: Arc<MediaPipeline>,
}

impl DocumentExtractTool {
    pub fn new(pipeline: Arc<MediaPipeline>) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl AlephTool for DocumentExtractTool {
    const NAME: &'static str = "document_extract";
    const DESCRIPTION: &'static str = r#"Extract text and structured data from documents.

Supports: txt, md, html (native), pdf, docx, xlsx (via plugins).

Parameters:
- file_path: Path to a local document file
- prompt: Optional extraction instruction (e.g., "Extract all tables")

Example:
{"file_path":"/tmp/report.pdf"}
{"file_path":"/tmp/data.xlsx","prompt":"Extract all tables"}"#;

    type Args = DocumentExtractArgs;
    type Output = DocumentExtractOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let path = PathBuf::from(&args.file_path);
        let mt = match detect_from_path(&path) {
            Ok(mt) => mt,
            Err(e) => {
                return Ok(DocumentExtractOutput::err(format!(
                    "Format detection failed: {}",
                    e
                )))
            }
        };

        if mt.category() != "document" {
            return Ok(DocumentExtractOutput::err(format!(
                "Expected document file, got {} format",
                mt.category()
            )));
        }

        let input = MediaInput::FilePath { path };
        match self
            .pipeline
            .process(&input, &mt, args.prompt.as_deref())
            .await
        {
            Ok(output) => {
                let data = serde_json::to_value(&output).unwrap_or_default();
                match &output {
                    MediaOutput::Text { text } => Ok(DocumentExtractOutput::ok(text.clone())),
                    _ => Ok(DocumentExtractOutput::ok_data(
                        "Document extracted".into(),
                        data,
                    )),
                }
            }
            Err(e) => Ok(DocumentExtractOutput::err(format!(
                "Document extraction failed: {}",
                e
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::processors::TextDocumentProvider;

    fn make_tool() -> DocumentExtractTool {
        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(TextDocumentProvider));
        DocumentExtractTool::new(Arc::new(mp))
    }

    #[tokio::test]
    async fn extract_text_file() {
        let tool = make_tool();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "Hello, world!").unwrap();

        let args = DocumentExtractArgs {
            file_path: path.to_string_lossy().to_string(),
            prompt: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.text.as_deref(), Some("Hello, world!"));
    }

    #[tokio::test]
    async fn rejects_non_document() {
        let tool = make_tool();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        std::fs::write(&path, &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();

        let args = DocumentExtractArgs {
            file_path: path.to_string_lossy().to_string(),
            prompt: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("document"));
    }

    #[test]
    fn tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "document_extract");
    }
}
