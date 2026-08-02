//! MCP Resource Tool
//!
//! Allows LLM to read resources from connected MCP servers.

use std::pin::Pin;
use std::sync::Arc;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::mcp::{McpClient, ResourceContent};
use crate::tool_metadata::{ToolCategory, ToolDefinition};
use crate::tools::AlephToolDyn;

/// Arguments for `mcp_read_resource` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpReadResourceArgs {
    /// Resource URI to read (e.g., "`server_name:file:///path/to/file`")
    pub uri: String,
}

/// Output from `mcp_read_resource` tool
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
    #[must_use]
    pub const fn new(handle: McpManagerHandle) -> Self {
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
            "Read a resource from a connected MCP server by its server-qualified `uri`. \
             Call `mcp_list_resources` first to discover available resources and pass the \
             returned `uri` exactly as given (it is an opaque identifier — do not edit or \
             shorten it). Do not guess a URI or read files off disk. Prefer reading an \
             MCP resource over a web search or a shell `cat` when a connected server \
             already exposes the content.",
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

            // Split the server-qualified uri into (client, resource_uri),
            // stripping exactly one server-prefix layer via LONGEST-prefix match
            // (see `resolve_server_qualified` for why shortest-match mis-routed
            // when one server id is a colon-prefix of another).
            let uri = &args.uri;
            let Some((client, resource_uri)) = resolve_server_qualified(&self.handle, uri).await
            else {
                return Err(crate::error::AlephError::NotFound(format!(
                    "No MCP server found for URI: {uri}"
                )));
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
            };

            Ok(serde_json::to_value(output)?)
        })
    }
}

/// Upper bound on entries returned by a single `mcp_list_resources` call.
/// Resources are typically few, but a misbehaving server could advertise
/// thousands; capping keeps the discovery result from blowing the model's
/// context. When hit, the model is told to narrow with the `server` filter.
const MAX_RESOURCE_ENTRIES: usize = 500;

/// Server-prefix candidates for a server-qualified id, **longest prefix first**.
/// Each item is `(candidate_server, remainder)` split at a `:`. Pure (no IO), so
/// the longest-first ordering is unit-testable independently of a live manager.
fn server_prefix_candidates(qualified: &str) -> Vec<(&str, &str)> {
    let mut out: Vec<(&str, &str)> = qualified
        .match_indices(':')
        .map(|(idx, _)| (&qualified[..idx], &qualified[idx + 1..]))
        .collect();
    out.reverse();
    out
}

/// Resolve a server-qualified id (`<server>:<rest>`) to its owning client plus
/// the remainder, stripping exactly ONE server-prefix layer.
///
/// Uses **longest** server-prefix match, mirroring the client's own prefix
/// resolution. Shortest/first match — the previous inline loop duplicated in
/// both `mcp_read_resource` and `mcp_get_prompt` — mis-routed when one server id
/// was a colon-prefix of another: a resource on a server literally named
/// `gh:sub` also satisfies `get_client("gh")`, so it resolved to server `gh` and
/// stripped the wrong layer. Iterating colon positions right-to-left tries the
/// longest candidate first; `get_client` returns `None` for any non-server
/// prefix (e.g. the `file://` inside a `file:///…` uri), so the first hit is the
/// longest *registered* server id. Shared by both MCP read tools.
pub(crate) async fn resolve_server_qualified<'a>(
    handle: &McpManagerHandle,
    qualified: &'a str,
) -> Option<(Arc<McpClient>, &'a str)> {
    for (candidate, remainder) in server_prefix_candidates(qualified) {
        if let Ok(Some(client)) = handle.get_client(candidate).await {
            return Some((client, remainder));
        }
    }
    None
}

/// Build the opaque, server-qualified id emitted for discovery.
///
/// `cached_key` is what the connection cache already hands back — itself
/// server-namespaced (`external/connection.rs` stores `format!("{server}:{uri}")`).
/// Re-prefixing the server id yields a doubled segment
/// (`qualified_id("github", "github:file:///R.md") == "github:github:file:///R.md"`).
/// That doubling is load-bearing: `mcp_read_resource` strips exactly one
/// leading `server:` layer (`&uri[idx+1..]`) and the client's own prefix
/// resolution strips the second, so a verbatim round-trip lands the bare `uri`
/// at the server. Presenting the id as opaque keeps a model from "helpfully"
/// collapsing the doubled segment and breaking the round-trip.
pub(crate) fn qualified_id(server: &str, cached_key: &str) -> String {
    format!("{server}:{cached_key}")
}

/// Arguments for the `mcp_list_resources` discovery tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpListResourcesArgs {
    /// Optional: restrict the listing to a single server by its id (or name).
    /// Omit to list resources from every connected server.
    #[serde(default)]
    pub server: Option<String>,
}

/// One discovered MCP resource, carrying the exact `uri` to hand to
/// `mcp_read_resource`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceEntry {
    /// Server id this resource belongs to.
    pub server: String,
    /// Opaque, server-qualified id ready for `mcp_read_resource` — pass
    /// verbatim; do not construct or edit it (see [`qualified_id`] on the
    /// doubled server segment).
    pub uri: String,
    /// Human-readable resource name.
    pub name: String,
    /// Resource description, when the server supplied one.
    pub description: Option<String>,
    /// MIME type, when known.
    pub mime_type: Option<String>,
}

/// Output from `mcp_list_resources`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpListResourcesOutput {
    /// Discovered resources (capped at [`MAX_RESOURCE_ENTRIES`]).
    pub resources: Vec<McpResourceEntry>,
    /// Number of entries returned.
    pub count: usize,
    /// Set when the cap truncated the list; narrow with the `server` filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Discovery counterpart to [`McpReadResourceTool`]: lists the resources every
/// connected server advertises so the model knows what it can read. Each entry's
/// `uri` is an opaque, server-qualified id built by [`qualified_id`] — the exact
/// form [`McpReadResourceTool`] parses — so the model copies it verbatim into
/// `mcp_read_resource` with no path guessing (the gap that used to push it to a
/// raw `cat`). Capability-gated by the MCP tool bridge alongside the read tool.
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
        let schema = schemars::schema_for!(McpListResourcesArgs);
        let parameters: Value = serde_json::to_value(&schema).unwrap_or_default();
        ToolDefinition::new(
            "mcp_list_resources",
            "Discover the readable resources (docs, files, data) exposed by \
             connected MCP servers. Call this FIRST, then read one with \
             `mcp_read_resource`, copying the exact server-qualified `uri` \
             returned here. Prefer MCP resources over a web search or shell `cat` \
             when a connected server already exposes the content.",
            parameters,
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpListResourcesArgs = serde_json::from_value(args)?;
            let servers = self.handle.list_servers().await?;

            let mut entries = Vec::new();
            let mut truncated = false;
            'servers: for info in servers {
                if info.resource_count == 0 {
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
                for r in client.list_resources().await {
                    if entries.len() >= MAX_RESOURCE_ENTRIES {
                        truncated = true;
                        break 'servers;
                    }
                    entries.push(McpResourceEntry {
                        // Server-qualify so the id round-trips through
                        // `mcp_read_resource`'s single-strip parser (see
                        // `qualified_id` for why the segment ends up doubled).
                        uri: qualified_id(&info.id, &r.uri),
                        server: info.id.clone(),
                        name: r.name,
                        description: r.description,
                        mime_type: r.mime_type,
                    });
                }
            }

            let count = entries.len();
            let note = truncated.then(|| {
                format!(
                    "Result truncated at {MAX_RESOURCE_ENTRIES} resources; \
                     re-run with the `server` filter to narrow."
                )
            });
            Ok(serde_json::to_value(McpListResourcesOutput {
                resources: entries,
                count,
                note,
            })?)
        })
    }
}

/// Arguments for the `mcp_list_resource_templates` discovery tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpListResourceTemplatesArgs {
    /// Optional: restrict the listing to a single server by its id (or name).
    /// Omit to list templates from every connected server.
    #[serde(default)]
    pub server: Option<String>,
}

/// One discovered MCP resource *template* — a parameterized URI the model fills
/// in, then reads via `mcp_read_resource`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceTemplateEntry {
    /// Server id this template belongs to. Prefix the filled URI with
    /// `<server>:` when calling `mcp_read_resource`.
    pub server: String,
    /// RFC-6570 URI template, e.g. `file:///{path}`. Substitute the `{...}`
    /// parameters, then read with `mcp_read_resource(uri = "<server>:<filled>")`.
    pub uri_template: String,
    /// Human-readable name.
    pub name: String,
    /// Description, when the server supplied one.
    pub description: Option<String>,
    /// MIME type of produced resources, when known.
    pub mime_type: Option<String>,
}

/// Output from `mcp_list_resource_templates`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpListResourceTemplatesOutput {
    /// Discovered templates (capped at [`MAX_RESOURCE_ENTRIES`]).
    pub templates: Vec<McpResourceTemplateEntry>,
    /// Number of entries returned.
    pub count: usize,
    /// Set when the cap truncated the list; narrow with the `server` filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Discovery tool for MCP resource *templates* (parameterized URIs). Some
/// servers expose resources ONLY by template (e.g. `file:///{path}`) and no
/// concrete resources, so [`McpListResourcesTool`] returns empty for them and
/// the model would dead-end into a shell `cat`. This surfaces the templates so
/// the model can fill a parameter and read via [`McpReadResourceTool`]. Unlike
/// concrete resources there is no [`qualified_id`] doubling — the template is a
/// raw pattern; the model constructs a single-prefixed `<server>:<filled uri>`,
/// exactly the form the reader's colon-split parser expects. Mirrors codex's
/// `list_mcp_resource_templates`. Capability-gated by the MCP tool bridge
/// alongside the resource read/list tools.
pub struct McpListResourceTemplatesTool {
    handle: McpManagerHandle,
}

impl McpListResourceTemplatesTool {
    /// Create a new MCP list-resource-templates tool.
    #[must_use]
    pub const fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AlephToolDyn for McpListResourceTemplatesTool {
    fn name(&self) -> &str {
        "mcp_list_resource_templates"
    }

    fn definition(&self) -> ToolDefinition {
        let schema = schemars::schema_for!(McpListResourceTemplatesArgs);
        let parameters: Value = serde_json::to_value(&schema).unwrap_or_default();
        ToolDefinition::new(
            "mcp_list_resource_templates",
            "Discover parameterized resource templates (RFC-6570 URIs like \
             `file:///{path}`) exposed by connected MCP servers — useful when a \
             server exposes resources only by template, so `mcp_list_resources` \
             comes back empty. Substitute the `{...}` parameters, then read the \
             result with `mcp_read_resource(uri=\"<server>:<filled uri>\")` \
             (prefix the filled URI with the entry's `server`). Prefer this over \
             a web search or shell `cat` when a connected server can produce the \
             content.",
            parameters,
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpListResourceTemplatesArgs = serde_json::from_value(args)?;
            let servers = self.handle.list_servers().await?;

            let mut entries = Vec::new();
            let mut truncated = false;
            'servers: for info in servers {
                if info.resource_template_count == 0 {
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
                for t in client.list_resource_templates().await {
                    if entries.len() >= MAX_RESOURCE_ENTRIES {
                        truncated = true;
                        break 'servers;
                    }
                    entries.push(McpResourceTemplateEntry {
                        server: info.id.clone(),
                        // Raw RFC-6570 pattern, NOT server-qualified: the model
                        // fills the parameters and prefixes `<server>:` itself
                        // when calling `mcp_read_resource` (see struct doc).
                        uri_template: t.uri_template,
                        name: t.name,
                        description: t.description,
                        mime_type: t.mime_type,
                    });
                }
            }

            let count = entries.len();
            let note = truncated.then(|| {
                format!(
                    "Result truncated at {MAX_RESOURCE_ENTRIES} templates; \
                     re-run with the `server` filter to narrow."
                )
            });
            Ok(serde_json::to_value(McpListResourceTemplatesOutput {
                templates: entries,
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

    #[test]
    fn server_prefix_candidates_are_longest_first() {
        // Colons at the server boundary AND inside the uri scheme. The doubled
        // `qualified_id` form `gh:sub:file:///x` must yield candidates
        // longest-first so a server literally named `gh:sub` is probed BEFORE
        // the shorter `gh` — otherwise a `gh:sub` resource mis-routes to `gh`.
        assert_eq!(
            server_prefix_candidates("gh:sub:file:///x"),
            vec![
                ("gh:sub:file", "///x"),
                ("gh:sub", "file:///x"),
                ("gh", "sub:file:///x"),
            ]
        );
        // No colon → no server-qualified split → no candidates (caller 404s).
        assert!(server_prefix_candidates("nocolons").is_empty());
    }

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
    fn list_resources_args_server_optional() {
        // Empty args → list every server.
        let args: McpListResourcesArgs = serde_json::from_value(json!({})).unwrap();
        assert!(args.server.is_none());
        // Explicit filter round-trips.
        let args: McpListResourcesArgs = serde_json::from_value(json!({"server": "docs"})).unwrap();
        assert_eq!(args.server.as_deref(), Some("docs"));
        // Schema advertises the optional field.
        let schema = serde_json::to_string(&schemars::schema_for!(McpListResourcesArgs)).unwrap();
        assert!(schema.contains("server"));
    }

    #[test]
    fn list_resource_templates_args_server_optional() {
        // Empty args → list every server's templates.
        let args: McpListResourceTemplatesArgs = serde_json::from_value(json!({})).unwrap();
        assert!(args.server.is_none());
        // Explicit filter round-trips.
        let args: McpListResourceTemplatesArgs =
            serde_json::from_value(json!({"server": "fs"})).unwrap();
        assert_eq!(args.server.as_deref(), Some("fs"));
    }

    #[test]
    fn resource_template_entry_serializes_raw_pattern() {
        // The template entry carries the RAW uri_template (no server prefix) plus
        // the `server` field the model uses to build `<server>:<filled>`.
        let entry = McpResourceTemplateEntry {
            server: "fs".to_string(),
            uri_template: "file:///{path}".to_string(),
            name: "Files".to_string(),
            description: None,
            mime_type: None,
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["uri_template"], "file:///{path}");
        assert_eq!(v["server"], "fs");
    }
}
