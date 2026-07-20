//! MCP Server Connection
//!
//! Manages the lifecycle and communication with an external MCP server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::{mcp as mcp_types, IdGenerator, JsonRpcNotification, JsonRpcRequest};
use crate::mcp::transport::{McpTransport, StdioTransport};
use crate::mcp::types::McpTool;
use crate::sync_primitives::Arc;

fn strip_server_prefix<'a>(s: &'a str, server_name: &str) -> &'a str {
    s.strip_prefix(&format!("{server_name}:")).unwrap_or(s)
}

/// Default timeout for the entire MCP server connection process
/// This includes: process spawn + initialize handshake + tools/list
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected
    Disconnected,
    /// Connection in progress
    Connecting,
    /// Connected and ready
    Ready,
    /// Connection failed
    Failed,
}

/// External MCP server connection
///
/// This struct manages the lifecycle and communication with an MCP server.
/// It uses a trait object (`Box<dyn McpTransport>`) to support different
/// transport implementations (stdio, HTTP, SSE).
pub struct McpServerConnection {
    /// Server name
    name: String,
    /// Transport layer (trait object for flexibility)
    transport: Arc<dyn McpTransport>,
    /// Request ID generator
    id_gen: IdGenerator,
    /// Server capabilities (after initialize)
    capabilities: RwLock<Option<mcp_types::ServerCapabilities>>,
    /// Cached tools list
    cached_tools: RwLock<Vec<McpTool>>,
    /// Cached resources list
    cached_resources: RwLock<Vec<crate::mcp::types::McpResource>>,
    /// Cached resource-templates list (parameterized URIs)
    cached_resource_templates: RwLock<Vec<crate::mcp::types::McpResourceTemplate>>,
    /// Cached prompts list
    cached_prompts: RwLock<Vec<crate::mcp::prompts::McpPrompt>>,
    /// Cached server instructions (from initialize response)
    cached_instructions: RwLock<Option<String>>,
    /// Connection state
    state: RwLock<ConnectionState>,
}

impl McpServerConnection {
    /// Connect to an external MCP server with timeout protection
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `command` - Command to execute
    /// * `args` - Command arguments
    /// * `env` - Environment variables
    /// * `cwd` - Working directory
    /// * `timeout` - Per-request timeout (defaults to 30s for individual RPCs).
    ///   The total connection timeout is always `DEFAULT_CONNECT_TIMEOUT` (300s)
    ///   to allow for slow server startup.
    pub async fn connect(
        name: impl Into<String>,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
        timeout: Option<Duration>,
    ) -> Result<Self> {
        let name = name.into();
        // Total connection timeout is always the larger default (300s) to allow
        // for slow server startup. Per-request timeout is set separately on the transport.
        let connect_timeout = DEFAULT_CONNECT_TIMEOUT;

        // Wrap entire connection process with timeout
        tokio::time::timeout(
            connect_timeout,
            Self::connect_internal(&name, command, args, env, cwd, timeout),
        )
        .await
        .map_err(|_| {
            AlephError::Timeout {
                suggestion: Some(format!(
                    "MCP server '{}' connection timed out after {}s. Check if the server is installed and responding.",
                    name,
                    connect_timeout.as_secs()
                )),
            }
        })?
    }

    /// Internal connection logic (without timeout wrapper)
    async fn connect_internal(
        name: &str,
        command: impl AsRef<str>,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&PathBuf>,
        timeout: Option<Duration>,
    ) -> Result<Self> {
        // Spawn the server process
        let mut transport = StdioTransport::spawn(name, command, args, env, cwd).await?;

        // Set per-request timeout if provided
        if let Some(t) = timeout {
            transport = transport.with_timeout(t);
        }

        Self::with_transport(name, Arc::new(transport)).await
    }

    /// Create a connection with a custom transport
    ///
    /// This constructor allows creating connections with any transport implementation,
    /// enabling support for HTTP, SSE, or mock transports for testing.
    ///
    /// # Arguments
    /// * `name` - Server name for identification
    /// * `transport` - An `Arc`-wrapped transport implementing `McpTransport`
    ///
    /// # Example
    /// ```ignore
    /// let http_transport = HttpTransport::new("https://mcp.example.com").await?;
    /// let conn = McpServerConnection::with_transport("remote-server", Arc::new(http_transport)).await?;
    /// ```
    pub async fn with_transport(
        name: impl Into<String>,
        transport: Arc<dyn McpTransport>,
    ) -> Result<Self> {
        let name = name.into();
        let conn = Self {
            name,
            transport,
            id_gen: IdGenerator::new(),
            capabilities: RwLock::new(None),
            cached_tools: RwLock::new(Vec::new()),
            cached_resources: RwLock::new(Vec::new()),
            cached_resource_templates: RwLock::new(Vec::new()),
            cached_prompts: RwLock::new(Vec::new()),
            cached_instructions: RwLock::new(None),
            state: RwLock::new(ConnectionState::Connecting),
        };

        // Perform MCP initialize handshake
        conn.initialize().await?;

        Ok(conn)
    }

    /// Perform MCP initialize handshake
    async fn initialize(&self) -> Result<()> {
        let params = mcp_types::InitializeParams::aleph_default();
        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "initialize",
            serde_json::to_value(&params).map_err(|e| {
                AlephError::IoError(format!("Failed to serialize initialize params: {e}"))
            })?,
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AlephError::IoError(format!(
                "MCP server '{}' initialize failed: {}",
                self.name, e
            ))
        })?;

        // Parse initialize result
        let init_result: mcp_types::InitializeResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to parse initialize result from '{}': {}",
                    self.name, e
                ))
            })?;

        tracing::info!(
            server = %self.name,
            protocol = %init_result.protocol_version,
            server_name = ?init_result.server_info.as_ref().map(|i| &i.name),
            "MCP server initialized"
        );

        // Honor the negotiated revision on every subsequent request. The
        // Streamable HTTP transport echoes it on the `MCP-Protocol-Version`
        // header; other transports treat this as a no-op.
        self.transport
            .set_protocol_version(&init_result.protocol_version);

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            *caps = Some(init_result.capabilities);
        }

        // Store instructions (if provided by server)
        {
            let mut inst = self.cached_instructions.write().await;
            *inst = init_result.instructions;
        }

        // Send initialized notification (per JSON-RPC spec, notifications have no id)
        let notification = JsonRpcNotification::new("notifications/initialized");
        if let Err(e) = self.transport.send_notification(&notification).await {
            tracing::warn!(
                server = %self.name,
                error = %e,
                "Failed to send initialized notification (non-fatal)"
            );
        }

        // Update state
        {
            let mut state = self.state.write().await;
            *state = ConnectionState::Ready;
        }

        // Pre-fetch tools list
        self.refresh_tools().await?;

        // Pre-fetch resources and prompts (non-fatal if not supported)
        if let Err(e) = self.refresh_resources().await {
            tracing::debug!(server = %self.name, error = %e, "Resources refresh failed (may not be supported)");
        }
        if let Err(e) = self.refresh_resource_templates().await {
            tracing::debug!(server = %self.name, error = %e, "Resource templates refresh failed (may not be supported)");
        }
        if let Err(e) = self.refresh_prompts().await {
            tracing::debug!(server = %self.name, error = %e, "Prompts refresh failed (may not be supported)");
        }

        Ok(())
    }

    /// Drain a paginated MCP list method, following the spec `nextCursor`
    /// chain until the server stops returning one.
    ///
    /// The first page omits `params` (the cursor is optional per spec); each
    /// subsequent page echoes the previous page's `nextCursor` back as
    /// `params.cursor`. `extract` parses one page's raw JSON-RPC result into
    /// `(items, next_cursor)`. `MAX_PAGES` bounds a server whose cursor never
    /// terminates (or fails to advance) so a buggy or hostile server cannot
    /// pin the connection in an unbounded fetch loop.
    async fn drain_paginated<T, F>(&self, method: &str, mut extract: F) -> Result<Vec<T>>
    where
        F: FnMut(Value) -> Result<(Vec<T>, Option<String>)>,
    {
        const MAX_PAGES: usize = 100;
        let mut items: Vec<T> = Vec::new();
        let mut cursor: Option<String> = None;

        for _ in 0..MAX_PAGES {
            let request = match &cursor {
                Some(c) => {
                    JsonRpcRequest::with_params(self.id_gen.next(), method, json!({ "cursor": c }))
                }
                None => JsonRpcRequest::new(self.id_gen.next(), method),
            };
            let response = self.transport.send_request(&request).await?;
            let result = response.into_result().map_err(|e| {
                AlephError::IoError(format!(
                    "MCP server '{}' {} failed: {}",
                    self.name, method, e
                ))
            })?;

            let (page, next) = extract(result)?;
            items.extend(page);
            match next {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => return Ok(items),
            }
        }

        tracing::warn!(
            server = %self.name,
            method,
            max_pages = MAX_PAGES,
            collected = items.len(),
            "MCP list pagination hit the page cap; truncating (server cursor did not terminate)"
        );
        Ok(items)
    }

    /// Refresh the cached tools list
    pub async fn refresh_tools(&self) -> Result<()> {
        let raw_tools = self
            .drain_paginated("tools/list", |result| {
                let page: mcp_types::ToolsListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse tools list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.tools, page.next_cursor))
            })
            .await?;

        // Convert to our McpTool format. External tool metadata is untrusted:
        // normalize the input schema so strict providers do not reject malformed
        // schemas, and flag descriptions that look like prompt injection.
        let tools: Vec<McpTool> = raw_tools
            .into_iter()
            .map(|t| {
                let description = t.description.unwrap_or_default();
                if let Some(marker) = crate::mcp::scan_description_for_injection(&description) {
                    tracing::warn!(
                        server = %self.name,
                        tool = %t.name,
                        marker,
                        "MCP tool description matches a prompt-injection heuristic; \
                         tool is still surfaced but flagged for review"
                    );
                }
                // Behavioral hints (MCP `ToolAnnotations`) are consumed
                // conservatively: read-only/idempotent only relax scheduling
                // when explicitly true; an explicit destructive hint routes
                // the tool through the user-confirmation gate. Absent
                // annotations behave exactly like the pre-annotation default
                // (exclusive, no confirmation).
                let annotations = t.annotations.unwrap_or_default();
                McpTool {
                    name: format!("{}:{}", self.name, t.name), // Namespace with server name
                    description,
                    input_schema: crate::mcp::normalize_tool_schema(
                        t.input_schema.unwrap_or_else(|| json!({"type": "object"})),
                    ),
                    requires_confirmation: annotations.is_destructive(),
                    read_only: annotations.is_read_only(),
                    idempotent: annotations.is_idempotent(),
                }
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            tool_count = tools.len(),
            "Cached tools list"
        );

        // Update cache
        {
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
        }

        Ok(())
    }

    /// Refresh the cached resources list
    pub async fn refresh_resources(&self) -> Result<()> {
        // Check if server supports resources
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.resources.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support resources");
            return Ok(());
        }
        drop(caps);

        let raw_resources = self
            .drain_paginated("resources/list", |result| {
                let page: mcp_types::ResourcesListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse resources list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.resources, page.next_cursor))
            })
            .await?;

        // Convert to our McpResource format
        let resources: Vec<crate::mcp::types::McpResource> = raw_resources
            .into_iter()
            .map(|r| crate::mcp::types::McpResource {
                uri: format!("{}:{}", self.name, r.uri), // Namespace with server
                name: r.name,
                description: r.description,
                mime_type: r.mime_type,
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            resource_count = resources.len(),
            "Cached resources list"
        );

        let mut cached = self.cached_resources.write().await;
        *cached = resources;

        Ok(())
    }

    /// Refresh the cached resource-templates list (`resources/templates/list`).
    ///
    /// Templates live under the same `resources` capability as concrete
    /// resources. Many servers that support resources do not implement the
    /// templates method; a drain error is non-fatal (the caller logs it at
    /// debug) and simply leaves the cache empty. The stored `uri_template`
    /// keeps its raw RFC-6570 form (NO `server:` prefix) — it is a pattern the
    /// model fills in, then reads as `mcp_read_resource(uri = "<server>:<filled>")`.
    pub async fn refresh_resource_templates(&self) -> Result<()> {
        // Templates are advertised under the resources capability.
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.resources.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support resources (skip templates)");
            return Ok(());
        }
        drop(caps);

        let raw_templates = self
            .drain_paginated("resources/templates/list", |result| {
                let page: mcp_types::ResourceTemplatesListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse resource templates list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.resource_templates, page.next_cursor))
            })
            .await?;

        // Convert to our McpResourceTemplate format (raw pattern, no prefix).
        let templates: Vec<crate::mcp::types::McpResourceTemplate> = raw_templates
            .into_iter()
            .map(|t| crate::mcp::types::McpResourceTemplate {
                uri_template: t.uri_template,
                name: t.name,
                description: t.description,
                mime_type: t.mime_type,
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            resource_template_count = templates.len(),
            "Cached resource templates list"
        );

        let mut cached = self.cached_resource_templates.write().await;
        *cached = templates;

        Ok(())
    }

    /// Refresh the cached prompts list
    pub async fn refresh_prompts(&self) -> Result<()> {
        // Check if server supports prompts
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.prompts.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support prompts");
            return Ok(());
        }
        drop(caps);

        let raw_prompts = self
            .drain_paginated("prompts/list", |result| {
                let page: mcp_types::PromptsListResult =
                    serde_json::from_value(result).map_err(|e| {
                        AlephError::IoError(format!(
                            "Failed to parse prompts list from '{}': {}",
                            self.name, e
                        ))
                    })?;
                Ok((page.prompts, page.next_cursor))
            })
            .await?;

        // Convert to our McpPrompt format
        let prompts: Vec<crate::mcp::prompts::McpPrompt> = raw_prompts
            .into_iter()
            .map(|p| crate::mcp::prompts::McpPrompt {
                name: format!("{}:{}", self.name, p.name), // Namespace with server
                description: p.description,
                arguments: p
                    .arguments
                    .into_iter()
                    .map(|a| crate::mcp::prompts::McpPromptArgument {
                        name: a.name,
                        description: a.description,
                        required: a.required,
                    })
                    .collect(),
            })
            .collect();

        tracing::debug!(
            server = %self.name,
            prompt_count = prompts.len(),
            "Cached prompts list"
        );

        let mut cached = self.cached_prompts.write().await;
        *cached = prompts;

        Ok(())
    }

    /// Get cached tools list
    pub async fn list_tools(&self) -> Vec<McpTool> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_tools.read().await.clone()
    }

    /// Get cached resources list
    pub async fn list_resources(&self) -> Vec<crate::mcp::types::McpResource> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_resources.read().await.clone()
    }

    /// Get cached resource-templates list
    pub async fn list_resource_templates(&self) -> Vec<crate::mcp::types::McpResourceTemplate> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_resource_templates.read().await.clone()
    }

    /// Get cached prompts list
    pub async fn list_prompts(&self) -> Vec<crate::mcp::prompts::McpPrompt> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_prompts.read().await.clone()
    }

    /// Get server-provided instructions (if any).
    pub async fn instructions(&self) -> Option<String> {
        // rust-doctor-disable-next-line excessive-clone
        self.cached_instructions.read().await.clone()
    }

    /// Check if this connection provides a specific tool
    pub async fn has_tool(&self, name: &str) -> bool {
        // Check with and without namespace prefix
        let full_name = if name.starts_with(&format!("{}:", self.name)) {
            name.to_string()
        } else {
            format!("{}:{}", self.name, name)
        };

        self.cached_tools
            .read()
            .await
            .iter()
            .any(|t| t.name == full_name || t.name == name)
    }

    /// Call a tool on this server
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        // Strip server namespace prefix if present
        let tool_name = strip_server_prefix(name, &self.name);

        let params = mcp_types::ToolCallParams {
            name: tool_name.to_string(),
            arguments: Some(arguments),
        };

        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "tools/call",
            serde_json::to_value(&params).map_err(|e| {
                AlephError::IoError(format!("Failed to serialize tool call params: {e}"))
            })?,
        );

        tracing::debug!(
            server = %self.name,
            tool = %tool_name,
            "Calling tool"
        );

        let response = match self.transport.send_request(&request).await {
            Ok(r) => r,
            Err(e) => {
                // Classify the failure for actionable diagnostics, then surface
                // the original error unchanged (variant-preserving, non-breaking).
                let kind = crate::mcp::classify_mcp_error(&e.to_string());
                tracing::warn!(
                    server = %self.name,
                    tool = %tool_name,
                    error_kind = kind.as_str(),
                    error = %e,
                    "MCP tool call failed at transport"
                );
                return Err(e);
            }
        };
        let result = response.into_result().map_err(|e| {
            let kind = crate::mcp::classify_mcp_error(&e.to_string());
            AlephError::IoError(format!(
                "Tool call '{}' on '{}' failed: {}{}",
                tool_name,
                self.name,
                e,
                kind.guidance_suffix()
            ))
        })?;

        // Parse tool call result
        let call_result: mcp_types::ToolCallResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Tool '{}' returned malformed result from '{}': {}",
                    tool_name, self.name, e
                ))
            })?;

        // Convert result to Value
        if call_result.is_error == Some(true) {
            let error_text = call_result
                .content
                .into_iter()
                .filter_map(|c| match c {
                    mcp_types::ToolResultContent::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            let kind = crate::mcp::classify_mcp_error(&error_text);
            return Err(AlephError::IoError(format!(
                "Tool '{}' returned error: {}{}",
                tool_name,
                error_text,
                kind.guidance_suffix()
            )));
        }

        // Extract content from result. Unknown blocks become an explicit
        // marker so the model knows something was elided rather than silently
        // seeing a shorter result.
        let content: Vec<Value> = call_result
            .content
            .into_iter()
            .map(|c| match c {
                mcp_types::ToolResultContent::Text { text } => {
                    json!({"type": "text", "text": text})
                }
                mcp_types::ToolResultContent::Image { data, mime_type } => {
                    json!({"type": "image", "data": data, "mimeType": mime_type})
                }
                mcp_types::ToolResultContent::Audio { data, mime_type } => {
                    json!({"type": "audio", "data": data, "mimeType": mime_type})
                }
                mcp_types::ToolResultContent::ResourceLink {
                    uri,
                    name,
                    description,
                } => {
                    json!({"type": "resource_link", "uri": uri, "name": name, "description": description})
                }
                mcp_types::ToolResultContent::Resource { resource } => {
                    json!({
                        "type": "resource",
                        "uri": resource.uri,
                        "mimeType": resource.mime_type,
                        "text": resource.text,
                        "blob": resource.blob,
                    })
                }
                mcp_types::ToolResultContent::Unknown => {
                    json!({"type": "text", "text": "[unsupported MCP content type omitted]"})
                }
            })
            .collect();

        Ok(json!({
            "content": content,
        }))
    }

    /// Read a resource by URI
    pub async fn read_resource(&self, uri: &str) -> Result<crate::mcp::resources::ResourceContent> {
        // Strip server namespace prefix if present
        let resource_uri = strip_server_prefix(uri, &self.name);

        let params = mcp_types::ResourceReadParams {
            uri: resource_uri.to_string(),
        };

        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "resources/read",
            serde_json::to_value(&params).map_err(|e| {
                AlephError::IoError(format!("Failed to serialize resource read params: {e}"))
            })?,
        );

        tracing::debug!(
            server = %self.name,
            uri = %resource_uri,
            "Reading resource"
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AlephError::IoError(format!(
                "Resource read '{}' on '{}' failed: {}",
                resource_uri, self.name, e
            ))
        })?;

        let read_result: mcp_types::ResourceReadResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to parse resource read result from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert first content item to ResourceContent
        if let Some(content) = read_result.contents.into_iter().next() {
            match content {
                mcp_types::ResourceContentItem::Text { text, .. } => {
                    Ok(crate::mcp::resources::ResourceContent::Text(text))
                }
                mcp_types::ResourceContentItem::Blob {
                    blob, mime_type, ..
                } => {
                    // Decode base64
                    use base64::Engine;
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(&blob)
                        .map_err(|e| AlephError::IoError(format!("Failed to decode blob: {e}")))?;
                    Ok(crate::mcp::resources::ResourceContent::Binary {
                        data,
                        mime_type: mime_type
                            .unwrap_or_else(|| "application/octet-stream".to_string()),
                    })
                }
            }
        } else {
            Ok(crate::mcp::resources::ResourceContent::Text(String::new()))
        }
    }

    /// Get a prompt by name with optional arguments
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::mcp::prompts::PromptResult> {
        // Strip server namespace prefix if present
        let prompt_name = strip_server_prefix(name, &self.name);

        let params = mcp_types::PromptGetParams {
            name: prompt_name.to_string(),
            arguments,
        };

        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "prompts/get",
            serde_json::to_value(&params).map_err(|e| {
                AlephError::IoError(format!("Failed to serialize prompt get params: {e}"))
            })?,
        );

        tracing::debug!(
            server = %self.name,
            prompt = %prompt_name,
            "Getting prompt"
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AlephError::IoError(format!(
                "Prompt get '{}' on '{}' failed: {}",
                prompt_name, self.name, e
            ))
        })?;

        let get_result: mcp_types::PromptGetResult =
            serde_json::from_value(result).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to parse prompt get result from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert to our PromptResult format
        let messages = get_result
            .messages
            .into_iter()
            .map(|m| {
                let role = match m.role {
                    mcp_types::PromptRole::User => "user",
                    mcp_types::PromptRole::Assistant => "assistant",
                    mcp_types::PromptRole::System => "system",
                };
                let content = match m.content {
                    mcp_types::PromptContentItem::Text { text } => {
                        crate::mcp::prompts::PromptContent::Text { text }
                    }
                    mcp_types::PromptContentItem::Image { data, mime_type } => {
                        crate::mcp::prompts::PromptContent::Image { data, mime_type }
                    }
                    // Audio has no internal counterpart; degrade to a text
                    // marker rather than dropping the message.
                    mcp_types::PromptContentItem::Audio { mime_type, .. } => {
                        crate::mcp::prompts::PromptContent::Text {
                            text: format!("[audio content: {mime_type}]"),
                        }
                    }
                    mcp_types::PromptContentItem::Resource { resource } => {
                        crate::mcp::prompts::PromptContent::Resource {
                            uri: resource.uri,
                            text: resource.text,
                        }
                    }
                    mcp_types::PromptContentItem::Unknown => {
                        crate::mcp::prompts::PromptContent::Text {
                            text: "[unsupported MCP content type omitted]".to_string(),
                        }
                    }
                };
                crate::mcp::prompts::PromptMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .collect();

        Ok(crate::mcp::prompts::PromptResult {
            description: get_result.description,
            messages,
        })
    }

    /// Get current connection state
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Get server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Check if server is running
    pub async fn is_running(&self) -> bool {
        self.transport.is_alive().await
    }

    /// Active liveness probe: send an MCP `ping` and report whether the server
    /// is reachable over the wire.
    ///
    /// Any JSON-RPC reply — a success *or* a `method not found` error — proves
    /// the transport round-trips, so a server that does not implement `ping` is
    /// still counted alive. Only a transport-level failure (timeout, refused,
    /// broken pipe) reports unreachable. This is the only real signal for a
    /// stateless HTTP transport, whose [`is_running`](Self::is_running) can do
    /// no better than always return `true`; it doubles as a keepalive that
    /// holds long-lived connections open against idle disconnects.
    pub async fn ping(&self) -> bool {
        let request = JsonRpcRequest::new(self.id_gen.next(), "ping");
        self.transport.send_request(&request).await.is_ok()
    }

    /// Install a handler for server-initiated notifications on this
    /// connection's transport (e.g. `notifications/tools/list_changed`).
    ///
    /// Transports that cannot receive notifications keep the default no-op.
    pub fn set_notification_handler(&self, handler: crate::mcp::transport::NotificationCallback) {
        self.transport.set_notification_handler(handler);
    }

    /// Close the connection
    pub async fn close(&self) -> Result<()> {
        tracing::info!(server = %self.name, "Closing MCP connection");

        {
            let mut state = self.state.write().await;
            *state = ConnectionState::Disconnected;
        }

        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Most tests require an actual MCP server to be available
    // These are basic structure tests

    #[test]
    fn test_connection_state() {
        assert_eq!(ConnectionState::Disconnected, ConnectionState::Disconnected);
        assert_ne!(ConnectionState::Ready, ConnectionState::Failed);
    }

    #[tokio::test]
    async fn test_connect_nonexistent() {
        let result = McpServerConnection::connect(
            "test-fail",
            "/nonexistent/mcp/server",
            &[],
            &HashMap::new(),
            None,
            None,
        )
        .await;

        assert!(result.is_err());
    }
}
