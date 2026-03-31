//! MCP Resource Tool
//!
//! Allows LLM to read resources from connected MCP servers.

use std::pin::Pin;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::mcp::ResourceContent;
use crate::tools::AlephToolDyn;

/// Arguments for mcp_read_resource tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpReadResourceArgs {
    /// Resource URI to read (e.g., "server_name:file:///path/to/file")
    pub uri: String,
}

/// Output from mcp_read_resource tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpReadResourceOutput {
    /// Resource content
    pub content: String,
    /// Content type (text, binary, image)
    pub content_type: String,
    /// MIME type if available
    pub mime_type: Option<String>,
}

/// Tool for reading MCP resources
pub struct McpReadResourceTool {
    handle: McpManagerHandle,
}

impl McpReadResourceTool {
    /// Create a new MCP read resource tool
    pub fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AlephToolDyn for McpReadResourceTool {
    fn name(&self) -> &str {
        "mcp_read_resource"
    }

    fn definition(&self) -> ToolDefinition {
        let schema = schemars::schema_for!(McpReadResourceArgs);
        let parameters: Value = serde_json::to_value(&schema).unwrap_or_default();
        ToolDefinition::new(
            "mcp_read_resource",
            "Read a resource from a connected MCP server. Use mcp.listResources to discover available resources first.",
            parameters,
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpReadResourceArgs = serde_json::from_value(args)?;

            // Extract server_id from URI prefix (e.g., "server_name:file:///path")
            // Try each colon position as a potential separator to handle server IDs
            // that contain colons (e.g., "plugin:foo/bar")
            let uri = &args.uri;
            let (client, resource_uri) = {
                let mut found = None;
                for (idx, _) in uri.match_indices(':') {
                    let candidate = &uri[..idx];
                    if let Ok(Some(c)) = self.handle.get_client(candidate).await {
                        found = Some((c, &uri[idx + 1..]));
                        break;
                    }
                }
                match found {
                    Some(pair) => pair,
                    None => {
                        return Err(crate::error::AlephError::NotFound(
                            format!("No MCP server found for URI: {}", uri)
                        ));
                    }
                }
            };

            let content = client.read_resource(resource_uri).await?;

            let output = match content {
                ResourceContent::Text(text) => McpReadResourceOutput {
                    content: text,
                    content_type: "text".to_string(),
                    mime_type: Some("text/plain".to_string()),
                },
                ResourceContent::Binary { data, mime_type } => {
                    use base64::Engine;
                    McpReadResourceOutput {
                        content: base64::engine::general_purpose::STANDARD.encode(&data),
                        content_type: "binary".to_string(),
                        mime_type: Some(mime_type),
                    }
                }
                ResourceContent::Image { data, mime_type } => {
                    use base64::Engine;
                    McpReadResourceOutput {
                        content: base64::engine::general_purpose::STANDARD.encode(&data),
                        content_type: "image".to_string(),
                        mime_type: Some(mime_type),
                    }
                }
            };

            Ok(serde_json::to_value(output)?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_args_schema() {
        let schema = schemars::schema_for!(McpReadResourceArgs);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("uri"));
    }

    #[test]
    fn test_args_deserialize() {
        let json = json!({"uri": "server:file:///test.txt"});
        let args: McpReadResourceArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.uri, "server:file:///test.txt");
    }
}
