//! Clipboard Tool
//!
//! Provides read-only access to clipboard content.
//! Content is provided by Swift layer via UniFFI callback.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::SystemTool;
use crate::error::Result;
use crate::mcp::types::{McpResource, McpTool, McpToolResult};

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
}

/// Clipboard read service
///
/// Provides read-only access to clipboard content.
/// Content is provided by Swift layer via callback.
pub struct ClipboardService {
    /// Latest clipboard content (updated by Swift)
    content: Arc<RwLock<ClipboardContent>>,
}

impl ClipboardService {
    /// Create a new ClipboardService
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

    /// Get shared content reference for external updates
    pub fn content_handle(&self) -> Arc<RwLock<ClipboardContent>> {
        Arc::clone(&self.content)
    }
}

impl Default for ClipboardService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemTool for ClipboardService {
    fn name(&self) -> &str {
        "builtin:clipboard"
    }

    fn description(&self) -> &str {
        "Read-only access to clipboard content"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Ok(vec![McpResource {
            uri: "clipboard://current".to_string(),
            name: "Current Clipboard".to_string(),
            description: Some("Current clipboard content".to_string()),
            mime_type: Some("application/json".to_string()),
        }])
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        match uri {
            "clipboard://current" => {
                let content = self.content.read().await;
                Ok(serde_json::to_string_pretty(&json!({
                    "content_type": &content.content_type,
                    "has_text": content.text.is_some(),
                    "has_image": content.image_base64.is_some(),
                    "file_count": content.file_paths.len(),
                }))?)
            }
            _ => Err(crate::error::AetherError::NotFound(uri.to_string())),
        }
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![McpTool {
            name: "clipboard_read".to_string(),
            description: "Read current clipboard content (text, images, or files)".to_string(),
            input_schema: json!({
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
            requires_confirmation: false,
        }]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "clipboard_read" => {
                let format = args
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("all");

                let content = self.content.read().await;

                let result = match format {
                    "text" => json!({
                        "content_type": &content.content_type,
                        "text": content.text.as_deref().unwrap_or(""),
                        "has_content": content.text.is_some(),
                    }),
                    "image" => json!({
                        "content_type": &content.content_type,
                        "has_image": content.image_base64.is_some(),
                        "image_base64": content.image_base64.as_deref().unwrap_or(""),
                    }),
                    _ => json!({
                        "content_type": &content.content_type,
                        "text": content.text.as_deref(),
                        "has_image": content.image_base64.is_some(),
                        "file_paths": &content.file_paths,
                    }),
                };

                Ok(McpToolResult::success(result))
            }
            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, _tool_name: &str) -> bool {
        // Clipboard read is passive, never needs confirmation
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_clipboard_read_empty() {
        let service = ClipboardService::new();
        let result = service.call_tool("clipboard_read", json!({})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["content_type"], "empty");
    }

    #[tokio::test]
    async fn test_clipboard_read_text() {
        let service = ClipboardService::new();
        service
            .update_content(ClipboardContent::text("Hello World".to_string()))
            .await;

        let result = service
            .call_tool("clipboard_read", json!({"format": "text"}))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.content["text"], "Hello World");
        assert_eq!(result.content["content_type"], "text");
    }

    #[tokio::test]
    async fn test_clipboard_read_all() {
        let service = ClipboardService::new();
        service
            .update_content(ClipboardContent {
                text: Some("Test".to_string()),
                image_base64: None,
                file_paths: vec!["/path/to/file.txt".to_string()],
                content_type: "mixed".to_string(),
            })
            .await;

        let result = service
            .call_tool("clipboard_read", json!({"format": "all"}))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.content["text"], "Test");
        assert_eq!(result.content["file_paths"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_requires_no_confirmation() {
        let service = ClipboardService::new();
        assert!(!service.requires_confirmation("clipboard_read"));
    }

    #[test]
    fn test_tool_listing() {
        let service = ClipboardService::new();
        let tools = service.list_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "clipboard_read");
        assert!(!tools[0].requires_confirmation);
    }
}
