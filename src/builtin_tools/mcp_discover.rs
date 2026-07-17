//! MCP Discovery Tools
//!
//! `mcp_list_resources` and `mcp_list_prompts` let the LLM enumerate the
//! resources and prompt templates exposed by connected MCP servers. Each entry
//! is keyed by a **server-qualified** identifier (`server:uri` / `server:name`)
//! that can be passed verbatim to `mcp_read_resource` / `mcp_get_prompt`.
//!
//! Without these, the read tools required a URI/name the model had no way to
//! discover in-band — its only recourse was to `cat` files off disk. They are
//! capability-gated in [`crate::mcp::tool_bridge`] so each appears only while a
//! connected server actually advertises resources / prompts (no dead tool that
//! every call would reject).

use std::pin::Pin;

use futures::Future;
use serde::Serialize;
use serde_json::Value;

use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::tool_metadata::{ToolCategory, ToolDefinition};
use crate::tools::AlephToolDyn;

/// One discoverable MCP resource, keyed by a server-qualified URI.
#[derive(Debug, Clone, Serialize)]
pub struct ListedResource {
    /// Server-qualified URI to pass verbatim to `mcp_read_resource`
    /// (e.g. `github:file:///README.md`).
    pub uri: String,
    /// Human-readable resource name.
    pub name: String,
    /// Resource description, if the server provided one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Originating server id.
    pub server: String,
}

/// One argument accepted by a discoverable MCP prompt.
#[derive(Debug, Clone, Serialize)]
pub struct ListedPromptArg {
    /// Argument name.
    pub name: String,
    /// Whether the server marks this argument as required.
    pub required: bool,
}

/// One discoverable MCP prompt, keyed by a server-qualified name.
#[derive(Debug, Clone, Serialize)]
pub struct ListedPrompt {
    /// Server-qualified name to pass verbatim to `mcp_get_prompt`
    /// (e.g. `github:create_issue`).
    pub name: String,
    /// Prompt description, if the server provided one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arguments this prompt accepts.
    pub arguments: Vec<ListedPromptArg>,
    /// Originating server id.
    pub server: String,
}

/// Schema for a tool that takes no arguments.
fn no_args_schema() -> Value {
    serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// Tool that lists resources across all connected MCP servers.
pub struct McpListResourcesTool {
    handle: McpManagerHandle,
}

impl McpListResourcesTool {
    /// Create a new MCP list-resources tool.
    #[must_use]
    pub const fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AlephToolDyn for McpListResourcesTool {
    fn name(&self) -> &str {
        "mcp_list_resources"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_list_resources",
            "List readable resources exposed by connected MCP servers. Returns each \
             resource's server-qualified `uri` (e.g. `github:file:///README.md`), which \
             you pass verbatim to `mcp_read_resource` to read its content. Call this \
             first to discover what resources exist — do not guess URIs or read files off \
             disk.",
            no_args_schema(),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        _args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move {
            let servers = self.handle.list_servers().await.unwrap_or_default();
            let mut resources: Vec<ListedResource> = Vec::new();
            for server in servers {
                if server.resource_count == 0 {
                    continue;
                }
                let Ok(Some(client)) = self.handle.get_client(server.id.as_str()).await else {
                    continue;
                };
                for res in client.list_resources().await {
                    resources.push(ListedResource {
                        // Prefix with the server id so the URI round-trips through
                        // `mcp_read_resource`'s `server:uri` parser.
                        uri: format!("{}:{}", server.id, res.uri),
                        name: res.name,
                        description: res.description,
                        mime_type: res.mime_type,
                        server: server.id.clone(),
                    });
                }
            }
            Ok(serde_json::json!({ "count": resources.len(), "resources": resources }))
        })
    }
}

/// Tool that lists prompt templates across all connected MCP servers.
pub struct McpListPromptsTool {
    handle: McpManagerHandle,
}

impl McpListPromptsTool {
    /// Create a new MCP list-prompts tool.
    #[must_use]
    pub const fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AlephToolDyn for McpListPromptsTool {
    fn name(&self) -> &str {
        "mcp_list_prompts"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_list_prompts",
            "List prompt templates exposed by connected MCP servers. Returns each prompt's \
             server-qualified `name` (e.g. `github:create_issue`) and its arguments, which \
             you pass verbatim to `mcp_get_prompt`. Call this first to discover available \
             prompts.",
            no_args_schema(),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        _args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move {
            let servers = self.handle.list_servers().await.unwrap_or_default();
            let mut prompts: Vec<ListedPrompt> = Vec::new();
            for server in servers {
                if server.prompt_count == 0 {
                    continue;
                }
                let Ok(Some(client)) = self.handle.get_client(server.id.as_str()).await else {
                    continue;
                };
                for prompt in client.list_prompts().await {
                    prompts.push(ListedPrompt {
                        name: format!("{}:{}", server.id, prompt.name),
                        description: prompt.description,
                        arguments: prompt
                            .arguments
                            .into_iter()
                            .map(|a| ListedPromptArg {
                                name: a.name,
                                required: a.required,
                            })
                            .collect(),
                        server: server.id.clone(),
                    });
                }
            }
            Ok(serde_json::json!({ "count": prompts.len(), "prompts": prompts }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_uri_is_server_qualified_on_serialize() {
        let r = ListedResource {
            uri: "github:file:///README.md".to_string(),
            name: "readme".to_string(),
            description: None,
            mime_type: Some("text/markdown".to_string()),
            server: "github".to_string(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["uri"], "github:file:///README.md");
        // `description` is elided when None; `mime_type` is present.
        assert!(v.get("description").is_none());
        assert_eq!(v["mime_type"], "text/markdown");
    }

    #[test]
    fn prompt_name_is_server_qualified_on_serialize() {
        let p = ListedPrompt {
            name: "github:create_issue".to_string(),
            description: Some("Open an issue".to_string()),
            arguments: vec![ListedPromptArg {
                name: "title".to_string(),
                required: true,
            }],
            server: "github".to_string(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["name"], "github:create_issue");
        assert_eq!(v["arguments"][0]["name"], "title");
        assert_eq!(v["arguments"][0]["required"], true);
    }

    #[test]
    fn no_args_schema_is_empty_object() {
        let schema = no_args_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].as_object().unwrap().is_empty());
    }
}
