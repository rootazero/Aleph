//! Clipboard Read Tool
//!
//! Provides read-only access to clipboard content via the AgentTool trait.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Clipboard content provided by Swift layer
#[derive(Debug, Clone, Default)]
pub struct ClipboardContent {
    /// Text content from clipboard (if any)
    pub text: Option<String>,
    /// Image data as base64 (if any)
    pub image_base64: Option<String>,
    /// File paths (if any)
    pub file_paths: Vec<String>,
    /// Content type indicator: "text", "image", "files", "mixed", "empty"
    pub content_type: String,
}

impl ClipboardContent {
    /// Create empty clipboard content
    pub fn empty() -> Self {
        Self {
            text: None,
            image_base64: None,
            file_paths: Vec::new(),
            content_type: "empty".to_string(),
        }
    }

    /// Create text-only clipboard content
    pub fn text(content: String) -> Self {
        Self {
            text: Some(content),
            image_base64: None,
            file_paths: Vec::new(),
            content_type: "text".to_string(),
        }
    }

    /// Create image clipboard content
    pub fn image(base64_data: String) -> Self {
        Self {
            text: None,
            image_base64: Some(base64_data),
            file_paths: Vec::new(),
            content_type: "image".to_string(),
        }
    }

    /// Create files clipboard content
    pub fn files(paths: Vec<String>) -> Self {
        Self {
            text: None,
            image_base64: None,
            file_paths: paths,
            content_type: "files".to_string(),
        }
    }
}

/// Clipboard tools context
///
/// Provides shared access to clipboard content.
/// Content is updated by Swift layer via callback.
#[derive(Clone)]
pub struct ClipboardContext {
    /// Latest clipboard content
    content: Arc<RwLock<ClipboardContent>>,
}

impl ClipboardContext {
    /// Create a new context with empty content
    pub fn new() -> Self {
        Self {
            content: Arc::new(RwLock::new(ClipboardContent::empty())),
        }
    }

    /// Update clipboard content (called by Swift via UniFFI)
    pub async fn update_content(&self, content: ClipboardContent) {
        let mut guard = self.content.write().await;
        *guard = content;
    }

    /// Get current clipboard content
    pub async fn get_content(&self) -> ClipboardContent {
        self.content.read().await.clone()
    }

    /// Get shared content reference for external updates
    pub fn content_handle(&self) -> Arc<RwLock<ClipboardContent>> {
        Arc::clone(&self.content)
    }
}

impl Default for ClipboardContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters for clipboard_read tool
#[derive(Debug, Deserialize)]
struct ClipboardReadParams {
    /// Type of content to read: "text", "image", "all"
    #[serde(default = "default_format")]
    format: String,
}

fn default_format() -> String {
    "all".to_string()
}

/// Clipboard read tool
///
/// Provides read-only access to clipboard content.
/// Content is provided by Swift layer via callback.
pub struct ClipboardReadTool {
    ctx: ClipboardContext,
}

impl ClipboardReadTool {
    /// Create a new ClipboardReadTool with the given context
    pub fn new(ctx: ClipboardContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for ClipboardReadTool {
    fn name(&self) -> &str {
        "clipboard_read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "clipboard_read",
            "Read current clipboard content (text, images, or files).",
            json!({
                "type": "object",
                "properties": {
                    "format": {
                        "type": "string",
                        "enum": ["text", "image", "all"],
                        "description": "Type of content to read (default: all)",
                        "default": "all"
                    }
                }
            }),
            ToolCategory::Native,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: ClipboardReadParams = serde_json::from_str(args).unwrap_or(ClipboardReadParams {
            format: "all".to_string(),
        });

        let content = self.ctx.get_content().await;

        match params.format.as_str() {
            "text" => {
                let text = content.text.as_deref().unwrap_or("");
                let has_content = content.text.is_some();

                Ok(ToolResult::success_with_data(
                    if has_content {
                        text.to_string()
                    } else {
                        "No text content in clipboard".to_string()
                    },
                    json!({
                        "content_type": content.content_type,
                        "text": text,
                        "has_content": has_content,
                    }),
                ))
            }

            "image" => {
                let has_image = content.image_base64.is_some();

                Ok(ToolResult::success_with_data(
                    if has_image {
                        "Image content available".to_string()
                    } else {
                        "No image content in clipboard".to_string()
                    },
                    json!({
                        "content_type": content.content_type,
                        "has_image": has_image,
                        "image_base64": content.image_base64.as_deref().unwrap_or(""),
                    }),
                ))
            }

            _ => {
                // "all" - return everything
                let summary = match content.content_type.as_str() {
                    "empty" => "Clipboard is empty".to_string(),
                    "text" => format!(
                        "Text content: {} chars",
                        content.text.as_ref().map(|t| t.len()).unwrap_or(0)
                    ),
                    "image" => "Image content available".to_string(),
                    "files" => format!("{} file(s) in clipboard", content.file_paths.len()),
                    "mixed" => "Mixed content (text + files/images)".to_string(),
                    _ => format!("Content type: {}", content.content_type),
                };

                Ok(ToolResult::success_with_data(
                    summary,
                    json!({
                        "content_type": content.content_type,
                        "text": content.text,
                        "has_image": content.image_base64.is_some(),
                        "file_paths": content.file_paths,
                    }),
                ))
            }
        }
    }

    fn requires_confirmation(&self) -> bool {
        false // Read-only operation
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Native
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool() -> (ClipboardReadTool, ClipboardContext) {
        let ctx = ClipboardContext::new();
        let tool = ClipboardReadTool::new(ctx.clone());
        (tool, ctx)
    }

    #[tokio::test]
    async fn test_clipboard_read_empty() {
        let (tool, _ctx) = create_test_tool();

        let args = json!({}).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("empty"));

        if let Some(data) = &result.data {
            assert_eq!(data["content_type"], "empty");
        }
    }

    #[tokio::test]
    async fn test_clipboard_read_text() {
        let (tool, ctx) = create_test_tool();
        ctx.update_content(ClipboardContent::text("Hello World".to_string()))
            .await;

        let args = json!({ "format": "text" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Hello World"));

        if let Some(data) = &result.data {
            assert_eq!(data["text"], "Hello World");
            assert_eq!(data["content_type"], "text");
        }
    }

    #[tokio::test]
    async fn test_clipboard_read_image() {
        let (tool, ctx) = create_test_tool();
        ctx.update_content(ClipboardContent::image("base64data...".to_string()))
            .await;

        let args = json!({ "format": "image" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Image content available"));

        if let Some(data) = &result.data {
            assert_eq!(data["has_image"], true);
            assert_eq!(data["image_base64"], "base64data...");
        }
    }

    #[tokio::test]
    async fn test_clipboard_read_files() {
        let (tool, ctx) = create_test_tool();
        ctx.update_content(ClipboardContent::files(vec![
            "/path/to/file1.txt".to_string(),
            "/path/to/file2.txt".to_string(),
        ]))
        .await;

        let args = json!({ "format": "all" }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("2 file(s)"));

        if let Some(data) = &result.data {
            assert_eq!(data["content_type"], "files");
            assert_eq!(data["file_paths"].as_array().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn test_clipboard_read_all() {
        let (tool, ctx) = create_test_tool();
        ctx.update_content(ClipboardContent {
            text: Some("Test text".to_string()),
            image_base64: None,
            file_paths: vec!["/path/to/file.txt".to_string()],
            content_type: "mixed".to_string(),
        })
        .await;

        let args = json!({}).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);

        if let Some(data) = &result.data {
            assert_eq!(data["content_type"], "mixed");
            assert_eq!(data["text"], "Test text");
        }
    }

    #[test]
    fn test_clipboard_read_metadata() {
        let (tool, _ctx) = create_test_tool();

        assert_eq!(tool.name(), "clipboard_read");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Native);
    }
}
