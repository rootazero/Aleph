//! MCP Discovery Tools
//!
//! `mcp_list_resources` and `mcp_list_prompts` let the LLM enumerate the
//! resources and prompt templates exposed by connected MCP servers. Each entry
//! is keyed by an **opaque, server-qualified** identifier that is meant to be
//! passed *verbatim* to `mcp_read_resource` / `mcp_get_prompt` — the model
//! should never construct, edit, or de-duplicate it.
//!
//! Without these, the read tools required a URI/name the model had no way to
//! discover in-band — its only recourse was to `cat` files off disk. They are
//! capability-gated in [`crate::mcp::tool_bridge`] so each appears only while a
//! connected server actually advertises resources / prompts (no dead tool that
//! every call would reject).
//!
//! ## Identifier shape (why it is opaque, not a clean `server:uri`)
//!
//! The connection cache already stores each resource/prompt under a
//! server-namespaced key (`connection.rs` builds `format!("{server}:{uri}")`),
//! and [`qualified_id`] re-prefixes the server id on top of that, so the id
//! carries the server segment **twice** (`github:github:file:///…`). This is
//! deliberate and load-bearing: `mcp_read_resource`'s parser strips exactly one
//! leading `server:` layer (`&uri[idx+1..]`) before handing the still-namespaced
//! remainder to the client, whose own `find_server_by_prefix` strips the second.
//! The two strips are symmetric with the two prefixes, so a verbatim round-trip
//! lands the bare `uri` at the server. Presenting the id as opaque keeps a model
//! from "helpfully" collapsing the doubled segment and breaking the round-trip.

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
    /// Opaque, server-qualified id — pass verbatim to `mcp_read_resource`; do
    /// not construct or edit it (see the module docs on the doubled prefix).
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
    /// Opaque, server-qualified id — pass verbatim to `mcp_get_prompt`; do not
    /// construct or edit it (see the module docs on the doubled prefix).
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

/// Build the opaque, server-qualified id emitted for discovery.
///
/// `cached_key` is what the connection cache already hands back — itself
/// server-namespaced (`connection.rs` stores `format!("{server}:{uri}")`).
/// Re-prefixing the server id yields a doubled segment
/// (`qualified_id("github", "github:file:///R.md") == "github:github:file:///R.md"`).
/// That doubling is intentional: it feeds the two symmetric single-strip layers
/// on the read path (`mcp_read_resource` strips one, the client strips one), so
/// the discovered id round-trips verbatim. See the module docs.
fn qualified_id(server: &str, cached_key: &str) -> String {
    format!("{server}:{cached_key}")
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
             resource's server-qualified `uri` — an opaque identifier you pass to \
             `mcp_read_resource` exactly as returned (do not edit or shorten it). Call \
             this first to discover what resources exist — do not guess URIs or read \
             files off disk.",
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
                        // Server-qualify so the id round-trips through
                        // `mcp_read_resource`'s single-strip parser (see
                        // `qualified_id` for why the segment ends up doubled).
                        uri: qualified_id(&server.id, &res.uri),
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
             server-qualified `name` — an opaque identifier you pass to `mcp_get_prompt` \
             exactly as returned (do not edit or shorten it) — plus its arguments. Call \
             this first to discover available prompts.",
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
                        name: qualified_id(&server.id, &prompt.name),
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
    fn qualified_id_doubles_the_already_namespaced_key() {
        // The connection cache hands back an ALREADY server-namespaced key, and
        // `qualified_id` re-prefixes on top of it. The doubled segment is
        // load-bearing: `mcp_read_resource` strips exactly one `server:` layer
        // and the client strips the second, so the id round-trips verbatim. Lock
        // the invariant so a future refactor of either strip layer fails loudly
        // here instead of silently breaking the round-trip.
        assert_eq!(
            qualified_id("github", "github:file:///README.md"),
            "github:github:file:///README.md"
        );
        assert_eq!(
            qualified_id("github", "github:create_issue"),
            "github:github:create_issue"
        );
    }

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
