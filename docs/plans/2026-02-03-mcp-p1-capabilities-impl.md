# MCP P1 Capabilities Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable LLM to access MCP Resources and Prompts, not just Tools.

**Architecture:** Add `resources/list`, `resources/read`, `prompts/list`, `prompts/get` RPC support to `McpServerConnection`, update managers to use real connections, implement aggregation in actor, and create builtin tools for LLM access.

**Tech Stack:** Rust, Tokio async, serde_json, MCP JSON-RPC protocol

---

## Prerequisites

- P0 MCP Orchestration Layer completed (Tasks 1-7)
- `McpServerConnection` exists with `tools/list` and `tools/call` support
- `McpResourceManager` and `McpPromptManager` exist as stubs

---

## Task 1: Add Resources/Prompts RPC Types to jsonrpc.rs

**Files:**
- Modify: `core/src/mcp/jsonrpc.rs`

**Step 1: Add Resource RPC types**

Add after `ToolCallResult` struct (around line 347):

```rust
    // ===== Resources RPC Types =====

    /// Resource definition from server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ResourceDefinition {
        /// Resource URI
        pub uri: String,
        /// Human-readable name
        pub name: String,
        /// Resource description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// MIME type
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mime_type: Option<String>,
    }

    /// Resources list response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourcesListResult {
        /// Available resources
        pub resources: Vec<ResourceDefinition>,
    }

    /// Resource read request parameters
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourceReadParams {
        /// Resource URI to read
        pub uri: String,
    }

    /// Resource content in read response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum ResourceContentItem {
        /// Text content
        #[serde(rename = "text")]
        Text {
            uri: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            mime_type: Option<String>,
            text: String,
        },
        /// Binary/blob content (base64)
        #[serde(rename = "blob")]
        Blob {
            uri: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            mime_type: Option<String>,
            blob: String,
        },
    }

    /// Resource read response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourceReadResult {
        /// Resource contents
        pub contents: Vec<ResourceContentItem>,
    }
```

**Step 2: Add Prompt RPC types**

Add after the Resource types:

```rust
    // ===== Prompts RPC Types =====

    /// Prompt argument definition
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptArgument {
        /// Argument name
        pub name: String,
        /// Argument description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Whether required
        #[serde(default)]
        pub required: bool,
    }

    /// Prompt definition from server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptDefinition {
        /// Prompt name
        pub name: String,
        /// Prompt description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Prompt arguments
        #[serde(default)]
        pub arguments: Vec<PromptArgument>,
    }

    /// Prompts list response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptsListResult {
        /// Available prompts
        pub prompts: Vec<PromptDefinition>,
    }

    /// Prompt get request parameters
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptGetParams {
        /// Prompt name
        pub name: String,
        /// Prompt arguments
        #[serde(skip_serializing_if = "Option::is_none")]
        pub arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    }

    /// Message role in prompt response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum PromptRole {
        User,
        Assistant,
        System,
    }

    /// Content in a prompt message
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum PromptContentItem {
        /// Text content
        #[serde(rename = "text")]
        Text { text: String },
        /// Image content
        #[serde(rename = "image")]
        Image { data: String, mime_type: String },
        /// Resource reference
        #[serde(rename = "resource")]
        Resource {
            uri: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            text: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            mime_type: Option<String>,
        },
    }

    /// Message in prompt response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptMessage {
        /// Message role
        pub role: PromptRole,
        /// Message content
        pub content: PromptContentItem,
    }

    /// Prompt get response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptGetResult {
        /// Optional description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Prompt messages
        pub messages: Vec<PromptMessage>,
    }
```

**Step 3: Add tests for new types**

Add to `mod tests`:

```rust
    #[test]
    fn test_resource_definition_deserialization() {
        let json = r#"{
            "uri": "file:///test.txt",
            "name": "test.txt",
            "description": "A test file",
            "mimeType": "text/plain"
        }"#;
        let resource: mcp::ResourceDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(resource.uri, "file:///test.txt");
        assert_eq!(resource.mime_type, Some("text/plain".to_string()));
    }

    #[test]
    fn test_prompt_definition_deserialization() {
        let json = r#"{
            "name": "code_review",
            "description": "Review code changes",
            "arguments": [
                {"name": "code", "description": "Code to review", "required": true}
            ]
        }"#;
        let prompt: mcp::PromptDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(prompt.name, "code_review");
        assert_eq!(prompt.arguments.len(), 1);
        assert!(prompt.arguments[0].required);
    }

    #[test]
    fn test_resource_content_text() {
        let json = r#"{"type": "text", "uri": "file:///test.txt", "text": "Hello"}"#;
        let content: mcp::ResourceContentItem = serde_json::from_str(json).unwrap();
        assert!(matches!(content, mcp::ResourceContentItem::Text { .. }));
    }

    #[test]
    fn test_prompt_message_deserialization() {
        let json = r#"{"role": "user", "content": {"type": "text", "text": "Hello"}}"#;
        let msg: mcp::PromptMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg.role, mcp::PromptRole::User));
    }
```

**Step 4: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test jsonrpc::tests
```

**Step 5: Commit**

```bash
git add core/src/mcp/jsonrpc.rs
git commit -m "feat(mcp): add Resources and Prompts RPC types

Add JSON-RPC type definitions for resources/list, resources/read,
prompts/list, and prompts/get MCP protocol methods.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Add Resources/Prompts Methods to McpServerConnection

**Files:**
- Modify: `core/src/mcp/external/connection.rs`

**Step 1: Add cached resources and prompts fields**

Add to `McpServerConnection` struct after `cached_tools`:

```rust
    /// Cached resources list
    cached_resources: RwLock<Vec<crate::mcp::types::McpResource>>,
    /// Cached prompts list
    cached_prompts: RwLock<Vec<crate::mcp::prompts::McpPrompt>>,
```

**Step 2: Initialize caches in constructors**

In `with_transport()`, add to struct initialization:

```rust
            cached_resources: RwLock::new(Vec::new()),
            cached_prompts: RwLock::new(Vec::new()),
```

**Step 3: Add refresh methods**

Add after `refresh_tools()`:

```rust
    /// Refresh the cached resources list
    pub async fn refresh_resources(&self) -> Result<()> {
        // Check if server supports resources
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.resources.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support resources");
            return Ok(());
        }
        drop(caps);

        let request = JsonRpcRequest::new(self.id_gen.next(), "resources/list");
        let response = self.transport.send_request(&request).await?;

        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "MCP server '{}' resources/list failed: {}",
                self.name, e
            ))
        })?;

        let resources_result: mcp_types::ResourcesListResult =
            serde_json::from_value(result).map_err(|e| {
                AetherError::IoError(format!(
                    "Failed to parse resources list from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert to our McpResource format
        let resources: Vec<crate::mcp::types::McpResource> = resources_result
            .resources
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

    /// Refresh the cached prompts list
    pub async fn refresh_prompts(&self) -> Result<()> {
        // Check if server supports prompts
        let caps = self.capabilities.read().await;
        if caps.as_ref().and_then(|c| c.prompts.as_ref()).is_none() {
            tracing::debug!(server = %self.name, "Server does not support prompts");
            return Ok(());
        }
        drop(caps);

        let request = JsonRpcRequest::new(self.id_gen.next(), "prompts/list");
        let response = self.transport.send_request(&request).await?;

        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "MCP server '{}' prompts/list failed: {}",
                self.name, e
            ))
        })?;

        let prompts_result: mcp_types::PromptsListResult =
            serde_json::from_value(result).map_err(|e| {
                AetherError::IoError(format!(
                    "Failed to parse prompts list from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert to our McpPrompt format
        let prompts: Vec<crate::mcp::prompts::McpPrompt> = prompts_result
            .prompts
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
```

**Step 4: Add getter methods**

Add after `list_tools()`:

```rust
    /// Get cached resources list
    pub async fn list_resources(&self) -> Vec<crate::mcp::types::McpResource> {
        self.cached_resources.read().await.clone()
    }

    /// Get cached prompts list
    pub async fn list_prompts(&self) -> Vec<crate::mcp::prompts::McpPrompt> {
        self.cached_prompts.read().await.clone()
    }
```

**Step 5: Add read_resource method**

Add after getter methods:

```rust
    /// Read a resource by URI
    pub async fn read_resource(&self, uri: &str) -> Result<crate::mcp::resources::ResourceContent> {
        // Strip server namespace prefix if present
        let resource_uri = uri
            .strip_prefix(&format!("{}:", self.name))
            .unwrap_or(uri);

        let params = mcp_types::ResourceReadParams {
            uri: resource_uri.to_string(),
        };

        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "resources/read",
            serde_json::to_value(&params).map_err(|e| {
                AetherError::IoError(format!("Failed to serialize resource read params: {}", e))
            })?,
        );

        tracing::debug!(
            server = %self.name,
            uri = %resource_uri,
            "Reading resource"
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "Resource read '{}' on '{}' failed: {}",
                resource_uri, self.name, e
            ))
        })?;

        let read_result: mcp_types::ResourceReadResult =
            serde_json::from_value(result).map_err(|e| {
                AetherError::IoError(format!(
                    "Failed to parse resource read result from '{}': {}",
                    self.name, e
                ))
            })?;

        // Convert first content item to ResourceContent
        if let Some(content) = read_result.contents.into_iter().next() {
            match content {
                mcp_types::ResourceContentItem::Text { text, mime_type, .. } => {
                    Ok(crate::mcp::resources::ResourceContent::Text(text))
                }
                mcp_types::ResourceContentItem::Blob { blob, mime_type, .. } => {
                    // Decode base64
                    use base64::Engine;
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(&blob)
                        .map_err(|e| {
                            AetherError::IoError(format!("Failed to decode blob: {}", e))
                        })?;
                    Ok(crate::mcp::resources::ResourceContent::Binary {
                        data,
                        mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                    })
                }
            }
        } else {
            Ok(crate::mcp::resources::ResourceContent::Text(String::new()))
        }
    }
```

**Step 6: Add get_prompt method**

Add after `read_resource`:

```rust
    /// Get a prompt by name with optional arguments
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::mcp::prompts::PromptResult> {
        // Strip server namespace prefix if present
        let prompt_name = name
            .strip_prefix(&format!("{}:", self.name))
            .unwrap_or(name);

        let params = mcp_types::PromptGetParams {
            name: prompt_name.to_string(),
            arguments,
        };

        let request = JsonRpcRequest::with_params(
            self.id_gen.next(),
            "prompts/get",
            serde_json::to_value(&params).map_err(|e| {
                AetherError::IoError(format!("Failed to serialize prompt get params: {}", e))
            })?,
        );

        tracing::debug!(
            server = %self.name,
            prompt = %prompt_name,
            "Getting prompt"
        );

        let response = self.transport.send_request(&request).await?;
        let result = response.into_result().map_err(|e| {
            AetherError::IoError(format!(
                "Prompt get '{}' on '{}' failed: {}",
                prompt_name, self.name, e
            ))
        })?;

        let get_result: mcp_types::PromptGetResult =
            serde_json::from_value(result).map_err(|e| {
                AetherError::IoError(format!(
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
                    mcp_types::PromptContentItem::Resource { uri, text, .. } => {
                        crate::mcp::prompts::PromptContent::Resource { uri, text }
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
```

**Step 7: Update initialize to refresh all capabilities**

In `initialize()`, after `self.refresh_tools().await?;`, add:

```rust
        // Pre-fetch resources and prompts (non-fatal if not supported)
        if let Err(e) = self.refresh_resources().await {
            tracing::debug!(server = %self.name, error = %e, "Resources refresh failed (may not be supported)");
        }
        if let Err(e) = self.refresh_prompts().await {
            tracing::debug!(server = %self.name, error = %e, "Prompts refresh failed (may not be supported)");
        }
```

**Step 8: Add import for base64**

Add to `core/Cargo.toml` if not present:
```toml
base64 = "0.22"
```

**Step 9: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```

**Step 10: Commit**

```bash
git add core/src/mcp/external/connection.rs
git add core/Cargo.toml
git commit -m "feat(mcp): add resources and prompts support to McpServerConnection

Implement refresh_resources, refresh_prompts, list_resources,
list_prompts, read_resource, and get_prompt methods for full
MCP capability coverage.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Update McpClient with Resources/Prompts Methods

**Files:**
- Modify: `core/src/mcp/client.rs`

**Step 1: Add list_resources method**

Add after `list_tools`:

```rust
    /// List all available resources from external servers
    pub async fn list_resources(&self) -> Vec<crate::mcp::types::McpResource> {
        let mut resources = Vec::new();

        let servers = self.external_servers.read().await;
        for connection in servers.values() {
            resources.extend(connection.list_resources().await);
        }

        resources
    }

    /// List all available prompts from external servers
    pub async fn list_prompts(&self) -> Vec<crate::mcp::prompts::McpPrompt> {
        let mut prompts = Vec::new();

        let servers = self.external_servers.read().await;
        for connection in servers.values() {
            prompts.extend(connection.list_prompts().await);
        }

        prompts
    }
```

**Step 2: Add read_resource method**

Add after `list_prompts`:

```rust
    /// Read a resource by URI
    ///
    /// The URI should include the server prefix (e.g., "server_name:file:///path")
    pub async fn read_resource(&self, uri: &str) -> Result<crate::mcp::resources::ResourceContent> {
        let servers = self.external_servers.read().await;

        // Check if URI has server prefix
        if let Some((server_name, _resource_uri)) = uri.split_once(':') {
            // Try server with matching prefix
            if let Some(connection) = servers.get(server_name) {
                return connection.read_resource(uri).await;
            }
        }

        // Try all servers
        for connection in servers.values() {
            // Check if this server has this resource
            let resources = connection.list_resources().await;
            if resources.iter().any(|r| r.uri == uri) {
                return connection.read_resource(uri).await;
            }
        }

        Err(AetherError::NotFound(format!("Resource not found: {}", uri)))
    }

    /// Get a prompt by name with optional arguments
    ///
    /// The name should include the server prefix (e.g., "server_name:prompt_name")
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::mcp::prompts::PromptResult> {
        let servers = self.external_servers.read().await;

        // Check if name has server prefix
        if let Some((server_name, _prompt_name)) = name.split_once(':') {
            // Try server with matching prefix
            if let Some(connection) = servers.get(server_name) {
                return connection.get_prompt(name, arguments).await;
            }
        }

        // Try all servers
        for connection in servers.values() {
            // Check if this server has this prompt
            let prompts = connection.list_prompts().await;
            if prompts.iter().any(|p| p.name == name) {
                return connection.get_prompt(name, arguments).await;
            }
        }

        Err(AetherError::NotFound(format!("Prompt not found: {}", name)))
    }
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo check
```

**Step 4: Commit**

```bash
git add core/src/mcp/client.rs
git commit -m "feat(mcp): add resources and prompts methods to McpClient

Implement list_resources, list_prompts, read_resource, and get_prompt
that aggregate across all connected MCP servers.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Update McpManagerActor Aggregation Methods

**Files:**
- Modify: `core/src/mcp/manager/actor.rs`

**Step 1: Update aggregate_resources implementation**

Replace the stub with:

```rust
    /// Aggregate resources from all healthy servers
    async fn aggregate_resources(&self) -> Vec<McpResource> {
        let mut all_resources = Vec::new();

        for (server_id, client) in &self.clients {
            // Check health - only aggregate from healthy servers
            if let Some(health) = self.health_states.get(server_id) {
                if !matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded { .. }) {
                    continue;
                }
            }

            let resources = client.list_resources().await;
            all_resources.extend(resources);
        }

        all_resources
    }
```

**Step 2: Update aggregate_prompts implementation**

Replace the stub with:

```rust
    /// Aggregate prompts from all healthy servers
    async fn aggregate_prompts(&self) -> Vec<McpPrompt> {
        let mut all_prompts = Vec::new();

        for (server_id, client) in &self.clients {
            // Check health - only aggregate from healthy servers
            if let Some(health) = self.health_states.get(server_id) {
                if !matches!(health.status, HealthStatus::Healthy | HealthStatus::Degraded { .. }) {
                    continue;
                }
            }

            let prompts = client.list_prompts().await;
            all_prompts.extend(prompts);
        }

        all_prompts
    }
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test mcp::manager
```

**Step 4: Commit**

```bash
git add core/src/mcp/manager/actor.rs
git commit -m "feat(mcp): implement real aggregation for resources and prompts

Replace stub implementations with actual aggregation across
all healthy MCP servers.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Create mcp_read_resource Builtin Tool

**Files:**
- Create: `core/src/builtin_tools/mcp_resource.rs`
- Modify: `core/src/builtin_tools/mod.rs`

**Step 1: Create mcp_resource.rs**

```rust
//! MCP Resource Tool
//!
//! Allows LLM to read resources from connected MCP servers.

use std::pin::Pin;
use std::sync::Arc;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::tools::AetherToolDyn;

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

impl AetherToolDyn for McpReadResourceTool {
    fn name(&self) -> &str {
        "mcp_read_resource"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_read_resource",
            "Read a resource from a connected MCP server. Use mcp.listResources to discover available resources first.",
            schemars::schema_for!(McpReadResourceArgs),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpReadResourceArgs = serde_json::from_value(args)?;

            // Get client for the server
            let uri = &args.uri;
            let server_id = uri.split(':').next().unwrap_or("");

            let client = self
                .handle
                .get_client(server_id)
                .await?
                .ok_or_else(|| crate::error::AetherError::NotFound(
                    format!("MCP server not found: {}", server_id)
                ))?;

            let content = client.read_resource(uri).await?;

            let output = match content {
                crate::mcp::resources::ResourceContent::Text(text) => McpReadResourceOutput {
                    content: text,
                    content_type: "text".to_string(),
                    mime_type: Some("text/plain".to_string()),
                },
                crate::mcp::resources::ResourceContent::Binary { data, mime_type } => {
                    use base64::Engine;
                    McpReadResourceOutput {
                        content: base64::engine::general_purpose::STANDARD.encode(&data),
                        content_type: "binary".to_string(),
                        mime_type: Some(mime_type),
                    }
                }
                crate::mcp::resources::ResourceContent::Image { data, mime_type } => {
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
```

**Step 2: Update mod.rs**

Add to module declarations:
```rust
pub mod mcp_resource;
```

Add to exports:
```rust
pub use mcp_resource::{McpReadResourceArgs, McpReadResourceOutput, McpReadResourceTool};
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test mcp_resource
```

**Step 4: Commit**

```bash
git add core/src/builtin_tools/mcp_resource.rs
git add core/src/builtin_tools/mod.rs
git commit -m "feat(tools): add mcp_read_resource builtin tool

Allow LLM to read resources from connected MCP servers
using the McpManagerHandle.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Create mcp_get_prompt Builtin Tool

**Files:**
- Create: `core/src/builtin_tools/mcp_prompt.rs`
- Modify: `core/src/builtin_tools/mod.rs`

**Step 1: Create mcp_prompt.rs**

```rust
//! MCP Prompt Tool
//!
//! Allows LLM to get prompts from connected MCP servers.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::tools::AetherToolDyn;

/// Arguments for mcp_get_prompt tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpGetPromptArgs {
    /// Prompt name (e.g., "server_name:prompt_name")
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

/// Output from mcp_get_prompt tool
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
    pub fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AetherToolDyn for McpGetPromptTool {
    fn name(&self) -> &str {
        "mcp_get_prompt"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_get_prompt",
            "Get a prompt template from a connected MCP server. Use mcp.listPrompts to discover available prompts first.",
            schemars::schema_for!(McpGetPromptArgs),
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpGetPromptArgs = serde_json::from_value(args)?;

            // Get client for the server
            let name = &args.name;
            let server_id = name.split(':').next().unwrap_or("");

            let client = self
                .handle
                .get_client(server_id)
                .await?
                .ok_or_else(|| crate::error::AetherError::NotFound(
                    format!("MCP server not found: {}", server_id)
                ))?;

            let result = client.get_prompt(name, args.arguments).await?;

            let messages: Vec<PromptOutputMessage> = result
                .messages
                .into_iter()
                .map(|m| {
                    let content = match m.content {
                        crate::mcp::prompts::PromptContent::Text { text } => text,
                        crate::mcp::prompts::PromptContent::Image { data, mime_type } => {
                            format!("[Image: {} ({})]", mime_type, data.len())
                        }
                        crate::mcp::prompts::PromptContent::Resource { uri, text } => {
                            text.unwrap_or_else(|| format!("[Resource: {}]", uri))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
```

**Step 2: Update mod.rs**

Add to module declarations:
```rust
pub mod mcp_prompt;
```

Add to exports:
```rust
pub use mcp_prompt::{McpGetPromptArgs, McpGetPromptOutput, McpGetPromptTool, PromptOutputMessage};
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test mcp_prompt
```

**Step 4: Commit**

```bash
git add core/src/builtin_tools/mcp_prompt.rs
git add core/src/builtin_tools/mod.rs
git commit -m "feat(tools): add mcp_get_prompt builtin tool

Allow LLM to get prompt templates from connected MCP servers
using the McpManagerHandle.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Update McpResourceManager and McpPromptManager

**Files:**
- Modify: `core/src/mcp/resources.rs`
- Modify: `core/src/mcp/prompts.rs`

**Step 1: Update McpResourceManager**

Replace the stub implementations in `resources.rs`:

```rust
    /// List resources from a specific server
    pub async fn list(&self, server: &str) -> Result<Vec<McpResource>> {
        let all_resources = self.client.list_resources().await;
        let prefix = format!("{}:", server);

        Ok(all_resources
            .into_iter()
            .filter(|r| r.uri.starts_with(&prefix))
            .collect())
    }

    /// Read a resource by URI from a specific server
    pub async fn read(&self, server: &str, uri: &str) -> Result<ResourceContent> {
        // Ensure URI has server prefix
        let full_uri = if uri.starts_with(&format!("{}:", server)) {
            uri.to_string()
        } else {
            format!("{}:{}", server, uri)
        };

        self.client.read_resource(&full_uri).await
    }
```

**Step 2: Update McpPromptManager**

Replace the stub implementations in `prompts.rs`:

```rust
    /// List prompts from a specific server
    pub async fn list(&self, server: &str) -> Result<Vec<McpPrompt>> {
        let all_prompts = self.client.list_prompts().await;
        let prefix = format!("{}:", server);

        Ok(all_prompts
            .into_iter()
            .filter(|p| p.name.starts_with(&prefix))
            .collect())
    }

    /// Get a prompt by name with optional arguments
    pub async fn get(
        &self,
        server: &str,
        name: &str,
        arguments: Option<HashMap<String, Value>>,
    ) -> Result<PromptResult> {
        // Ensure name has server prefix
        let full_name = if name.starts_with(&format!("{}:", server)) {
            name.to_string()
        } else {
            format!("{}:{}", server, name)
        };

        self.client.get_prompt(&full_name, arguments).await
    }
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/core && cargo test resources && cargo test prompts
```

**Step 4: Commit**

```bash
git add core/src/mcp/resources.rs
git add core/src/mcp/prompts.rs
git commit -m "feat(mcp): implement real McpResourceManager and McpPromptManager

Replace stub implementations with actual calls to McpClient
for resources and prompts operations.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

This implementation plan covers **P1 (Capabilities Alignment)** with:

1. **Task 1**: Resources and Prompts RPC types in jsonrpc.rs
2. **Task 2**: McpServerConnection resources/prompts methods
3. **Task 3**: McpClient aggregation methods
4. **Task 4**: McpManagerActor real aggregation implementation
5. **Task 5**: mcp_read_resource builtin tool
6. **Task 6**: mcp_get_prompt builtin tool
7. **Task 7**: McpResourceManager and McpPromptManager implementation

**After P1:**
- LLM can list resources from MCP servers via `mcp.listResources` RPC
- LLM can read resources using `mcp_read_resource` tool
- LLM can list prompts from MCP servers via `mcp.listPrompts` RPC
- LLM can get prompts using `mcp_get_prompt` tool

**Testing Strategy:**
- Each task includes unit tests
- Integration test: Start an MCP server that exposes resources/prompts, verify aggregation
- Manual test: Use the builtin tools with a real MCP server like `@modelcontextprotocol/server-filesystem`
