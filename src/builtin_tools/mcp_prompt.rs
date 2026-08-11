//! MCP Prompt Tool
//!
//! Allows LLM to get prompts from connected MCP servers.

use std::collections::HashMap;
use std::pin::Pin;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::tool_metadata::{ToolCategory, ToolDefinition};
use crate::tools::AlephToolDyn;

use super::mcp_resource::qualified_id;

/// Arguments for `mcp_get_prompt` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpGetPromptArgs {
    /// Prompt name (e.g., "`server_name:prompt_name`")
    pub name: String,
    /// Optional arguments to pass to the prompt
    #[serde(default)]
    pub arguments: Option<HashMap<String, Value>>,
}

/// Message in prompt output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptOutputMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
}

/// Output from `mcp_get_prompt` tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpGetPromptOutput {
    /// Optional description
    pub description: Option<String>,
    /// Prompt messages
    pub messages: Vec<PromptOutputMessage>,
}

/// Tool for getting MCP prompts
pub struct McpGetPromptTool {
    handle: McpManagerHandle,
}

impl McpGetPromptTool {
    /// Create a new MCP get prompt tool
    #[must_use]
    pub const fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl McpGetPromptTool {
    /// The description this tool ships to the model, hoisted out of
    /// `definition()` so a byte ratchet can reference it as a const.
    ///
    /// This is the FOURTH registration shape: `mcp/tool_bridge.rs` installs
    /// the tool straight into the process-wide MCP `ToolHandlerRegistry`
    /// that `run_loop` snapshots per request, so it appears in no catalog,
    /// reaches no `reg(` site, and is pushed by no tool service. Measured via
    /// `executor::BRIDGE_TOOL_DESCRIPTIONS`; a literal in that table would
    /// only move the drift one layer up, hence the const.
    pub(crate) const DESCRIPTION: &'static str =
        "Get a prompt template from a connected MCP server by its server-qualified \
         `name`. Call `mcp_list_prompts` first to discover available prompts and pass \
         the returned `name` exactly as given (it is an opaque identifier — do not \
         edit or shorten it).";

    /// The argument schema, measured beside the description because these
    /// tools sit in `default_core_tools()` — progressive disclosure never
    /// collapses them, so the schema ships in full on every request whose
    /// capability gate is open. (`mcp_login` is the one that is not core,
    /// and is measured anyway: being uncollapsed is the reason to measure,
    /// not the condition for it.)
    pub(crate) fn schema_value() -> Value {
        let schema = schemars::schema_for!(McpGetPromptArgs);
        serde_json::to_value(&schema).unwrap_or_default()
    }
}

impl AlephToolDyn for McpGetPromptTool {
    fn name(&self) -> &str {
        "mcp_get_prompt"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_get_prompt",
            Self::DESCRIPTION,
            Self::schema_value(),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpGetPromptArgs = serde_json::from_value(args)?;

            // Split the server-qualified name into (client, prompt_name) via the
            // shared LONGEST server-prefix matcher (see
            // `super::mcp_resource::resolve_server_qualified`) — previously this
            // duplicated the read-resource tool's shortest-match colon loop.
            let name = &args.name;
            let Some((client, prompt_name)) =
                super::mcp_resource::resolve_server_qualified(&self.handle, name).await
            else {
                return Err(crate::error::AlephError::NotFound(format!(
                    "No MCP server found for prompt: {name}"
                )));
            };

            let result = client.get_prompt(prompt_name, args.arguments).await?;

            let messages: Vec<PromptOutputMessage> = result
                .messages
                .into_iter()
                .map(|m| {
                    let content = match m.content {
                        crate::mcp::PromptContent::Text { text } => text,
                        crate::mcp::PromptContent::Image { data, mime_type } => {
                            // data is base64-encoded; approximate decoded size
                            let approx_bytes = data.len() * 3 / 4;
                            format!("[Image: {mime_type} (~{approx_bytes} bytes)]")
                        }
                        crate::mcp::PromptContent::Resource { uri, text } => {
                            text.unwrap_or_else(|| format!("[Resource: {uri}]"))
                        }
                    };
                    PromptOutputMessage {
                        role: m.role,
                        content,
                    }
                })
                .collect();

            let output = McpGetPromptOutput {
                description: result.description,
                messages,
            };

            Ok(serde_json::to_value(output)?)
        })
    }
}

/// Upper bound on entries returned by a single `mcp_list_prompts` call —
/// mirrors [`super::mcp_resource`]'s resource cap so a misbehaving server
/// cannot blow the model's context.
const MAX_PROMPT_ENTRIES: usize = 500;

/// Arguments for the `mcp_list_prompts` discovery tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpListPromptsArgs {
    /// Optional: restrict the listing to a single server by its id (or name).
    /// Omit to list prompts from every connected server.
    #[serde(default)]
    pub server: Option<String>,
}

/// One discovered MCP prompt, carrying the exact `name` to hand to
/// `mcp_get_prompt` plus the argument shape it accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptEntry {
    /// Server id this prompt belongs to.
    pub server: String,
    /// Opaque, server-qualified id ready for `mcp_get_prompt` — pass verbatim;
    /// do not construct or edit it (see [`super::mcp_resource::qualified_id`]
    /// on the doubled server segment).
    pub name: String,
    /// Prompt description, when the server supplied one.
    pub description: Option<String>,
    /// Arguments this prompt accepts.
    pub arguments: Vec<crate::mcp::McpPromptArgument>,
}

/// Output from `mcp_list_prompts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpListPromptsOutput {
    /// Discovered prompts (capped at [`MAX_PROMPT_ENTRIES`]).
    pub prompts: Vec<McpPromptEntry>,
    /// Number of entries returned.
    pub count: usize,
    /// Set when the cap truncated the list; narrow with the `server` filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Discovery counterpart to [`McpGetPromptTool`]: lists the prompt templates
/// every connected server advertises so the model knows what it can fetch. Each
/// entry's `name` is an opaque, server-qualified id built by
/// [`super::mcp_resource::qualified_id`] — the exact form [`McpGetPromptTool`]
/// parses. Capability-gated by the MCP tool bridge alongside the get-prompt tool.
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

impl McpListPromptsTool {
    /// The description this tool ships to the model, hoisted out of
    /// `definition()` so a byte ratchet can reference it as a const.
    ///
    /// This is the FOURTH registration shape: `mcp/tool_bridge.rs` installs
    /// the tool straight into the process-wide MCP `ToolHandlerRegistry`
    /// that `run_loop` snapshots per request, so it appears in no catalog,
    /// reaches no `reg(` site, and is pushed by no tool service. Measured via
    /// `executor::BRIDGE_TOOL_DESCRIPTIONS`; a literal in that table would
    /// only move the drift one layer up, hence the const.
    pub(crate) const DESCRIPTION: &'static str =
        "Discover the prompt templates exposed by connected MCP servers. Call \
         this FIRST, then fetch one with `mcp_get_prompt`, copying the exact \
         server-qualified `name` returned here.";

    /// The argument schema, measured beside the description because these
    /// tools sit in `default_core_tools()` — progressive disclosure never
    /// collapses them, so the schema ships in full on every request whose
    /// capability gate is open. (`mcp_login` is the one that is not core,
    /// and is measured anyway: being uncollapsed is the reason to measure,
    /// not the condition for it.)
    pub(crate) fn schema_value() -> Value {
        let schema = schemars::schema_for!(McpListPromptsArgs);
        serde_json::to_value(&schema).unwrap_or_default()
    }
}

impl AlephToolDyn for McpListPromptsTool {
    fn name(&self) -> &str {
        "mcp_list_prompts"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_list_prompts",
            Self::DESCRIPTION,
            Self::schema_value(),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpListPromptsArgs = serde_json::from_value(args)?;
            let servers = self.handle.list_servers().await?;

            let mut entries = Vec::new();
            let mut truncated = false;
            'servers: for info in servers {
                if info.prompt_count == 0 {
                    continue;
                }
                if let Some(ref want) = args.server {
                    if &info.id != want && &info.name != want {
                        continue;
                    }
                }
                let Some(client) = self.handle.get_client(&info.id).await? else {
                    continue;
                };
                for p in client.list_prompts().await {
                    if entries.len() >= MAX_PROMPT_ENTRIES {
                        truncated = true;
                        break 'servers;
                    }
                    entries.push(McpPromptEntry {
                        // Server-qualify so the id round-trips through
                        // `mcp_get_prompt`'s single-strip parser (see
                        // `qualified_id` for why the segment ends up doubled).
                        name: qualified_id(&info.id, &p.name),
                        server: info.id.clone(),
                        description: p.description,
                        arguments: p.arguments,
                    });
                }
            }

            let count = entries.len();
            let note = truncated.then(|| {
                format!(
                    "Result truncated at {MAX_PROMPT_ENTRIES} prompts; \
                     re-run with the `server` filter to narrow."
                )
            });
            Ok(serde_json::to_value(McpListPromptsOutput {
                prompts: entries,
                count,
                note,
            })?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_args_schema() {
        let schema = schemars::schema_for!(McpGetPromptArgs);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("name"));
        assert!(json.contains("arguments"));
    }

    #[test]
    fn test_args_deserialize() {
        let json = json!({
            "name": "server:code_review",
            "arguments": {"code": "fn main() {}"}
        });
        let args: McpGetPromptArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.name, "server:code_review");
        assert!(args.arguments.is_some());
    }

    #[test]
    fn test_args_without_arguments() {
        let json = json!({"name": "server:simple_prompt"});
        let args: McpGetPromptArgs = serde_json::from_value(json).unwrap();
        assert!(args.arguments.is_none());
    }

    #[test]
    fn list_prompts_args_server_optional() {
        let args: McpListPromptsArgs = serde_json::from_value(json!({})).unwrap();
        assert!(args.server.is_none());
        let args: McpListPromptsArgs = serde_json::from_value(json!({"server": "gh"})).unwrap();
        assert_eq!(args.server.as_deref(), Some("gh"));
        let schema = serde_json::to_string(&schemars::schema_for!(McpListPromptsArgs)).unwrap();
        assert!(schema.contains("server"));
    }
}
