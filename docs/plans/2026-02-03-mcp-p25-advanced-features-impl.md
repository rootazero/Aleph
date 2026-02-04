# P2.5 MCP Advanced Features Extension Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend MCP capabilities with cross-server context injection, OAuth token refresh, streaming sampling responses, and human-in-the-loop approval.

**Architecture:** Four incremental features building on P2's bidirectional RPC foundation. Each feature is independent but ordered by complexity: context injection (low) → token refresh (medium) → streaming (medium) → approval dialogs (high).

**Tech Stack:** Rust, tokio, async-stream, reqwest, serde, tokio-tungstenite (for gateway notifications)

---

## Task 1: Add IncludeContext Enum Type

**Files:**
- Modify: `core/src/mcp/jsonrpc.rs`

**Step 1: Update include_context field type**

The current `include_context` field is `Option<String>`. Per MCP spec, it should be an enum with values `"thisServer"` or `"allServers"`.

In the `mcp` module section (around line 541), change:

```rust
    /// Include context from MCP servers
    /// - "thisServer": Include context from the requesting server only
    /// - "allServers": Include context from all connected servers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_context: Option<IncludeContext>,
```

Add the enum before `SamplingRequest`:

```rust
/// Context inclusion mode for sampling requests
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IncludeContext {
    /// Include context from the requesting server only
    ThisServer,
    /// Include context from all connected MCP servers
    AllServers,
}
```

**Step 2: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo check
```

**Step 3: Fix any test compilation errors**

Update tests in `sampling.rs` that use `include_context: None` - they should still work.

**Step 4: Commit**

```bash
git add core/src/mcp/jsonrpc.rs
git commit -m "feat(mcp): add IncludeContext enum type for sampling requests

Per MCP spec, include_context should be 'thisServer' or 'allServers'.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Create Context Injector Module

**Files:**
- Create: `core/src/mcp/context_injector.rs`
- Modify: `core/src/mcp/mod.rs`

**Step 1: Create context_injector.rs**

```rust
//! MCP Context Injector
//!
//! Injects context from MCP servers into sampling requests.
//! Supports both single-server and all-server context modes.

use std::sync::Arc;

use crate::mcp::client::McpClient;
use crate::mcp::jsonrpc::mcp::{IncludeContext, SamplingMessage, SamplingContent, PromptRole};

/// Context that can be injected into sampling requests
#[derive(Debug, Clone)]
pub struct InjectedContext {
    /// Server name that provided the context
    pub server_name: String,
    /// Resources from the server
    pub resources: Vec<ResourceContext>,
    /// Available tools from the server
    pub tools: Vec<ToolContext>,
}

/// Resource context summary
#[derive(Debug, Clone)]
pub struct ResourceContext {
    /// Resource URI
    pub uri: String,
    /// Resource name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
}

/// Tool context summary
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
}

/// Injects context from MCP servers into sampling messages
pub struct ContextInjector;

impl ContextInjector {
    /// Gather context from MCP client based on include mode
    ///
    /// # Arguments
    /// * `client` - The MCP client to gather context from
    /// * `mode` - Context inclusion mode
    /// * `requesting_server` - Name of the server making the sampling request
    pub async fn gather_context(
        client: &McpClient,
        mode: &IncludeContext,
        requesting_server: &str,
    ) -> Vec<InjectedContext> {
        match mode {
            IncludeContext::ThisServer => {
                // Only include context from the requesting server
                Self::gather_server_context(client, requesting_server).await
                    .map(|ctx| vec![ctx])
                    .unwrap_or_default()
            }
            IncludeContext::AllServers => {
                // Include context from all servers
                Self::gather_all_context(client).await
            }
        }
    }

    /// Gather context from a specific server
    async fn gather_server_context(
        client: &McpClient,
        server_name: &str,
    ) -> Option<InjectedContext> {
        // Get resources and tools for this server
        let all_resources = client.list_resources().await;
        let all_tools = client.list_tools().await;

        // Filter to only include resources/tools from this server
        let resources: Vec<ResourceContext> = all_resources
            .into_iter()
            .filter(|r| r.uri.starts_with(&format!("{}:", server_name)))
            .map(|r| ResourceContext {
                uri: r.uri,
                name: r.name,
                description: r.description,
            })
            .collect();

        let tools: Vec<ToolContext> = all_tools
            .into_iter()
            .filter(|t| t.name.starts_with(&format!("{}:", server_name)))
            .map(|t| ToolContext {
                name: t.name,
                description: t.description,
            })
            .collect();

        if resources.is_empty() && tools.is_empty() {
            None
        } else {
            Some(InjectedContext {
                server_name: server_name.to_string(),
                resources,
                tools,
            })
        }
    }

    /// Gather context from all connected servers
    async fn gather_all_context(client: &McpClient) -> Vec<InjectedContext> {
        let server_names = client.service_names().await;
        let mut contexts = Vec::new();

        for name in server_names {
            if let Some(ctx) = Self::gather_server_context(client, &name).await {
                contexts.push(ctx);
            }
        }

        contexts
    }

    /// Format context as a system message for injection
    pub fn format_as_system_message(contexts: &[InjectedContext]) -> Option<SamplingMessage> {
        if contexts.is_empty() {
            return None;
        }

        let mut parts = vec!["Available MCP context:".to_string()];

        for ctx in contexts {
            parts.push(format!("\n## Server: {}", ctx.server_name));

            if !ctx.resources.is_empty() {
                parts.push("\n### Resources:".to_string());
                for r in &ctx.resources {
                    let desc = r.description.as_deref().unwrap_or("No description");
                    parts.push(format!("- {} ({}): {}", r.name, r.uri, desc));
                }
            }

            if !ctx.tools.is_empty() {
                parts.push("\n### Tools:".to_string());
                for t in &ctx.tools {
                    parts.push(format!("- {}: {}", t.name, t.description));
                }
            }
        }

        Some(SamplingMessage {
            role: PromptRole::System,
            content: SamplingContent::Text {
                text: parts.join("\n"),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty_context() {
        let result = ContextInjector::format_as_system_message(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_format_context_with_resources() {
        let context = InjectedContext {
            server_name: "test-server".to_string(),
            resources: vec![ResourceContext {
                uri: "test-server:file:///test.txt".to_string(),
                name: "test.txt".to_string(),
                description: Some("A test file".to_string()),
            }],
            tools: vec![],
        };

        let result = ContextInjector::format_as_system_message(&[context]);
        assert!(result.is_some());

        if let Some(msg) = result {
            if let SamplingContent::Text { text } = msg.content {
                assert!(text.contains("test-server"));
                assert!(text.contains("test.txt"));
            }
        }
    }
}
```

**Step 2: Update mod.rs exports**

Add to `core/src/mcp/mod.rs`:

```rust
mod context_injector;
pub use context_injector::{ContextInjector, InjectedContext, ResourceContext, ToolContext};
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test context_injector
```

**Step 4: Commit**

```bash
git add core/src/mcp/context_injector.rs core/src/mcp/mod.rs
git commit -m "feat(mcp): add ContextInjector for cross-server context

Implements context gathering from MCP servers based on
include_context mode (thisServer or allServers).

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Integrate Context Injection with SamplingHandler

**Files:**
- Modify: `core/src/mcp/sampling.rs`

**Step 1: Add context injection to handle_request**

Update imports at top:

```rust
use crate::mcp::context_injector::ContextInjector;
use crate::mcp::client::McpClient;
```

Add new field and methods to `SamplingHandler`:

```rust
    /// Optional MCP client for context injection
    client: Arc<RwLock<Option<Arc<McpClient>>>>,

    // In new():
    client: Arc::new(RwLock::new(None)),

    /// Set the MCP client for context injection
    pub async fn set_client(&self, client: Arc<McpClient>) {
        let mut c = self.client.write().await;
        *c = Some(client);
    }
```

Update `handle_request` to inject context:

```rust
    pub async fn handle_request(
        &self,
        request_id: u64,
        params: Value,
        requesting_server: &str,
    ) -> Result<SamplingResponse> {
        let mut request: SamplingRequest = serde_json::from_value(params).map_err(|e| {
            AlephError::IoError(format!("Failed to parse sampling request: {}", e))
        })?;

        // Inject context if requested
        if let Some(ref mode) = request.include_context {
            if let Some(ref client) = *self.client.read().await {
                let contexts = ContextInjector::gather_context(client, mode, requesting_server).await;
                if let Some(context_msg) = ContextInjector::format_as_system_message(&contexts) {
                    // Prepend context message to messages
                    request.messages.insert(0, context_msg);
                }
            }
        }

        // ... rest of existing implementation
    }
```

**Step 2: Update call sites**

In `client.rs` where `handle_request` is called, pass the server name.

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test sampling
```

**Step 4: Commit**

```bash
git add core/src/mcp/sampling.rs core/src/mcp/client.rs
git commit -m "feat(mcp): integrate context injection with SamplingHandler

Context from MCP servers is now injected into sampling requests
based on the include_context parameter.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Add Token Refresh Method to OAuthProvider

**Files:**
- Modify: `core/src/mcp/auth/provider.rs`

**Step 1: Add refresh_token method**

Add after the `finish_authorization` method:

```rust
    /// Refresh an expired access token
    ///
    /// Uses the refresh_token grant to obtain a new access token.
    /// Automatically saves the new tokens to storage.
    pub async fn refresh_token(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<OAuthTokens> {
        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ];

        let response = self
            .client
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Token refresh failed: {}", e)))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "Token refresh failed: {}",
                body
            )));
        }

        let tokens = parse_token_response(response).await?;

        // Save new tokens
        self.storage
            .save_tokens(&self.server_name, &tokens)
            .await?;

        tracing::info!(
            server = %self.server_name,
            "OAuth tokens refreshed successfully"
        );

        Ok(tokens)
    }

    /// Check if tokens need refresh and refresh if possible
    ///
    /// Returns new tokens if refreshed, or existing tokens if still valid.
    pub async fn ensure_valid_token(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
    ) -> Result<Option<OAuthTokens>> {
        let tokens = match self.storage.get_tokens(&self.server_name).await? {
            Some(t) => t,
            None => return Ok(None),
        };

        if !tokens.is_expired() {
            return Ok(Some(tokens));
        }

        // Token is expired, try to refresh
        if let Some(ref refresh) = tokens.refresh_token {
            match self.refresh_token(metadata, client_id, refresh).await {
                Ok(new_tokens) => return Ok(Some(new_tokens)),
                Err(e) => {
                    tracing::warn!(
                        server = %self.server_name,
                        error = %e,
                        "Failed to refresh token, will need re-authorization"
                    );
                    return Ok(None);
                }
            }
        }

        // No refresh token, need re-authorization
        Ok(None)
    }
```

**Step 2: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test auth
```

**Step 3: Commit**

```bash
git add core/src/mcp/auth/provider.rs
git commit -m "feat(mcp): add OAuth token refresh support

Implements refresh_token method and ensure_valid_token helper
for automatic token refresh before expiration.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Create Token Refresh Manager

**Files:**
- Create: `core/src/mcp/auth/refresh.rs`
- Modify: `core/src/mcp/auth/mod.rs`

**Step 1: Create refresh.rs**

```rust
//! OAuth Token Refresh Manager
//!
//! Automatically refreshes OAuth tokens before they expire for SSE connections.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::time::{interval, Instant};

use crate::error::Result;
use crate::mcp::auth::{OAuthProvider, OAuthServerMetadata, OAuthStorage, OAuthTokens};

/// Configuration for token refresh behavior
#[derive(Debug, Clone)]
pub struct TokenRefreshConfig {
    /// Check interval for token expiration
    pub check_interval: Duration,
    /// Refresh tokens this long before expiration
    pub refresh_before_expiry: Duration,
}

impl Default for TokenRefreshConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(60),
            refresh_before_expiry: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Tracked server for token refresh
struct TrackedServer {
    client_id: String,
    metadata: OAuthServerMetadata,
    last_refresh: Instant,
}

/// Manages automatic token refresh for multiple servers
pub struct TokenRefreshManager {
    storage: Arc<OAuthStorage>,
    servers: RwLock<HashMap<String, TrackedServer>>,
    config: TokenRefreshConfig,
    shutdown: RwLock<bool>,
}

impl TokenRefreshManager {
    /// Create a new token refresh manager
    pub fn new(storage: Arc<OAuthStorage>, config: TokenRefreshConfig) -> Self {
        Self {
            storage,
            servers: RwLock::new(HashMap::new()),
            config,
            shutdown: RwLock::new(false),
        }
    }

    /// Register a server for token refresh monitoring
    pub async fn register_server(
        &self,
        server_name: &str,
        client_id: &str,
        metadata: OAuthServerMetadata,
    ) {
        let mut servers = self.servers.write().await;
        servers.insert(
            server_name.to_string(),
            TrackedServer {
                client_id: client_id.to_string(),
                metadata,
                last_refresh: Instant::now(),
            },
        );
        tracing::debug!(server = %server_name, "Registered server for token refresh");
    }

    /// Unregister a server from token refresh monitoring
    pub async fn unregister_server(&self, server_name: &str) {
        let mut servers = self.servers.write().await;
        servers.remove(server_name);
        tracing::debug!(server = %server_name, "Unregistered server from token refresh");
    }

    /// Run the refresh loop (call in background task)
    pub async fn run(&self) {
        let mut ticker = interval(self.config.check_interval);

        loop {
            ticker.tick().await;

            if *self.shutdown.read().await {
                tracing::info!("Token refresh manager shutting down");
                break;
            }

            self.check_and_refresh_all().await;
        }
    }

    /// Stop the refresh manager
    pub async fn shutdown(&self) {
        let mut shutdown = self.shutdown.write().await;
        *shutdown = true;
    }

    /// Check all servers and refresh tokens as needed
    async fn check_and_refresh_all(&self) {
        let servers = self.servers.read().await;

        for (name, server) in servers.iter() {
            if let Err(e) = self.check_and_refresh_server(name, server).await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Failed to refresh token"
                );
            }
        }
    }

    /// Check and refresh a single server's token
    async fn check_and_refresh_server(
        &self,
        server_name: &str,
        server: &TrackedServer,
    ) -> Result<()> {
        let tokens = match self.storage.get_tokens(server_name).await? {
            Some(t) => t,
            None => return Ok(()), // No tokens to refresh
        };

        // Check if token needs refresh
        if !self.should_refresh(&tokens) {
            return Ok(());
        }

        // Get refresh token
        let refresh_token = match tokens.refresh_token {
            Some(ref t) => t.clone(),
            None => return Ok(()), // Can't refresh without refresh_token
        };

        // Create provider and refresh
        let provider = OAuthProvider::new(
            self.storage.clone(),
            server_name,
            "", // Server URL not needed for refresh
            "", // Callback URL not needed for refresh
        );

        let _new_tokens = provider
            .refresh_token(&server.metadata, &server.client_id, &refresh_token)
            .await?;

        tracing::info!(server = %server_name, "Token refreshed successfully");

        Ok(())
    }

    /// Check if token should be refreshed
    fn should_refresh(&self, tokens: &OAuthTokens) -> bool {
        if let Some(expires_at) = tokens.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            let refresh_threshold = self.config.refresh_before_expiry.as_secs() as i64;
            expires_at - refresh_threshold < now
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_token_refresh_manager_creation() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        let config = TokenRefreshConfig::default();

        let manager = TokenRefreshManager::new(storage, config);

        // Should create without panicking
        assert!(true);
        drop(manager);
    }

    #[tokio::test]
    async fn test_register_unregister_server() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        let manager = TokenRefreshManager::new(storage, TokenRefreshConfig::default());

        let metadata = OAuthServerMetadata {
            authorization_endpoint: "https://example.com/auth".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec![],
            grant_types_supported: vec![],
            code_challenge_methods_supported: vec![],
        };

        manager.register_server("test", "client_id", metadata).await;

        {
            let servers = manager.servers.read().await;
            assert!(servers.contains_key("test"));
        }

        manager.unregister_server("test").await;

        {
            let servers = manager.servers.read().await;
            assert!(!servers.contains_key("test"));
        }
    }
}
```

**Step 2: Update mod.rs**

Add to `core/src/mcp/auth/mod.rs`:

```rust
mod refresh;
pub use refresh::{TokenRefreshConfig, TokenRefreshManager};
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test refresh
```

**Step 4: Commit**

```bash
git add core/src/mcp/auth/refresh.rs core/src/mcp/auth/mod.rs
git commit -m "feat(mcp): add TokenRefreshManager for automatic token refresh

Background task monitors token expiration and refreshes before
they expire, keeping SSE connections alive.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Add Streaming Types for Sampling

**Files:**
- Modify: `core/src/mcp/jsonrpc.rs`
- Modify: `core/src/mcp/sampling.rs`

**Step 1: Add streaming response types**

In `jsonrpc.rs`, add streaming variants:

```rust
    /// Streaming sampling response chunk
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SamplingChunk {
        /// Partial text content
        pub delta: String,
        /// Whether this is the final chunk
        #[serde(default)]
        pub is_final: bool,
        /// Model that generated the response (only in final chunk)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub model: Option<String>,
        /// Stop reason (only in final chunk)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stop_reason: Option<StopReason>,
    }
```

**Step 2: Add streaming callback type in sampling.rs**

```rust
use futures::Stream;
use std::pin::Pin;

/// Callback for streaming sampling requests
pub type StreamingSamplingCallback = Box<
    dyn Fn(SamplingRequest) -> Pin<Box<dyn Stream<Item = Result<SamplingChunk>> + Send>>
        + Send
        + Sync,
>;
```

**Step 3: Add streaming handler to SamplingHandler**

```rust
    /// Streaming callback
    streaming_callback: Arc<RwLock<Option<StreamingSamplingCallback>>>,

    /// Set streaming callback
    pub async fn set_streaming_callback<F, S>(&self, callback: F)
    where
        F: Fn(SamplingRequest) -> S + Send + Sync + 'static,
        S: Stream<Item = Result<SamplingChunk>> + Send + 'static,
    {
        let mut cb = self.streaming_callback.write().await;
        *cb = Some(Box::new(move |req| Box::pin(callback(req))));
    }

    /// Check if streaming is available
    pub async fn has_streaming(&self) -> bool {
        self.streaming_callback.read().await.is_some()
    }
```

**Step 4: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo check
```

**Step 5: Commit**

```bash
git add core/src/mcp/jsonrpc.rs core/src/mcp/sampling.rs
git commit -m "feat(mcp): add streaming types for sampling responses

Adds SamplingChunk for partial responses and streaming callback
infrastructure in SamplingHandler.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Add Approval Request Types

**Files:**
- Modify: `core/src/mcp/jsonrpc.rs`

**Step 1: Add approval types**

Add in the `mcp` module:

```rust
    /// Request for human approval of an action
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ApprovalRequest {
        /// Unique request ID
        pub request_id: String,
        /// Description of the action requiring approval
        pub action: String,
        /// Server requesting approval
        pub server_name: String,
        /// Details for the user to review
        #[serde(skip_serializing_if = "Option::is_none")]
        pub details: Option<serde_json::Value>,
        /// Timeout for response (seconds)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub timeout_seconds: Option<u32>,
    }

    /// Response to an approval request
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ApprovalResponse {
        /// Whether the action was approved
        pub approved: bool,
        /// Optional reason for rejection
        #[serde(skip_serializing_if = "Option::is_none")]
        pub reason: Option<String>,
    }

    /// Approval decision
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ApprovalDecision {
        /// User approved the action
        Approved,
        /// User rejected the action
        Rejected,
        /// Request timed out
        Timeout,
    }
```

**Step 2: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo check
```

**Step 3: Commit**

```bash
git add core/src/mcp/jsonrpc.rs
git commit -m "feat(mcp): add approval request types for human-in-the-loop

Defines ApprovalRequest, ApprovalResponse, and ApprovalDecision
for MCP servers to request user confirmation.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 8: Create Approval Handler Module

**Files:**
- Create: `core/src/mcp/approval.rs`
- Modify: `core/src/mcp/mod.rs`

**Step 1: Create approval.rs**

```rust
//! MCP Approval Handler
//!
//! Manages human-in-the-loop approval requests from MCP servers.
//! Approval requests are forwarded to the UI layer via Gateway events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, RwLock};
use tokio::time::timeout;

use crate::error::{AlephError, Result};
use crate::mcp::jsonrpc::mcp::{ApprovalDecision, ApprovalRequest, ApprovalResponse};

/// Callback for presenting approval requests to the user
pub type ApprovalPresentCallback = Box<
    dyn Fn(ApprovalRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Pending approval request
struct PendingApproval {
    request: ApprovalRequest,
    respond_to: oneshot::Sender<ApprovalResponse>,
}

/// Handles approval requests from MCP servers
pub struct ApprovalHandler {
    /// Pending approvals by request ID
    pending: Arc<RwLock<HashMap<String, PendingApproval>>>,
    /// Callback to present requests to UI
    present_callback: Arc<RwLock<Option<ApprovalPresentCallback>>>,
    /// Default timeout for approvals
    default_timeout: Duration,
}

impl ApprovalHandler {
    /// Create a new approval handler
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            present_callback: Arc::new(RwLock::new(None)),
            default_timeout: Duration::from_secs(60),
        }
    }

    /// Set the callback for presenting approval requests
    pub async fn set_present_callback<F, Fut>(&self, callback: F)
    where
        F: Fn(ApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut cb = self.present_callback.write().await;
        *cb = Some(Box::new(move |req| Box::pin(callback(req))));
    }

    /// Request approval from the user
    ///
    /// Returns the user's decision or Timeout if no response within timeout.
    pub async fn request_approval(&self, request: ApprovalRequest) -> Result<ApprovalDecision> {
        let request_id = request.request_id.clone();
        let timeout_secs = request.timeout_seconds.unwrap_or(60);
        let timeout_duration = Duration::from_secs(timeout_secs as u64);

        // Create response channel
        let (tx, rx) = oneshot::channel();

        // Store pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                request_id.clone(),
                PendingApproval {
                    request: request.clone(),
                    respond_to: tx,
                },
            );
        }

        // Present to user
        {
            let callback = self.present_callback.read().await;
            if let Some(ref cb) = *callback {
                cb(request).await;
            } else {
                tracing::warn!("No approval callback registered, auto-rejecting");
                return Ok(ApprovalDecision::Rejected);
            }
        }

        // Wait for response with timeout
        match timeout(timeout_duration, rx).await {
            Ok(Ok(response)) => {
                // Clean up
                let mut pending = self.pending.write().await;
                pending.remove(&request_id);

                if response.approved {
                    Ok(ApprovalDecision::Approved)
                } else {
                    Ok(ApprovalDecision::Rejected)
                }
            }
            Ok(Err(_)) => {
                // Channel closed (shouldn't happen)
                let mut pending = self.pending.write().await;
                pending.remove(&request_id);
                Ok(ApprovalDecision::Rejected)
            }
            Err(_) => {
                // Timeout
                let mut pending = self.pending.write().await;
                pending.remove(&request_id);
                Ok(ApprovalDecision::Timeout)
            }
        }
    }

    /// Submit user's response to an approval request
    pub async fn respond(&self, request_id: &str, approved: bool, reason: Option<String>) -> Result<()> {
        let mut pending = self.pending.write().await;

        if let Some(approval) = pending.remove(request_id) {
            let response = ApprovalResponse { approved, reason };
            let _ = approval.respond_to.send(response);
            Ok(())
        } else {
            Err(AlephError::NotFound(format!(
                "No pending approval with ID: {}",
                request_id
            )))
        }
    }

    /// Get all pending approval requests
    pub async fn list_pending(&self) -> Vec<ApprovalRequest> {
        let pending = self.pending.read().await;
        pending.values().map(|p| p.request.clone()).collect()
    }

    /// Cancel a pending approval request
    pub async fn cancel(&self, request_id: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(request_id);
    }
}

impl Default for ApprovalHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_approval_handler_creation() {
        let handler = ApprovalHandler::new();
        assert!(handler.list_pending().await.is_empty());
    }

    #[tokio::test]
    async fn test_respond_to_nonexistent() {
        let handler = ApprovalHandler::new();
        let result = handler.respond("nonexistent", true, None).await;
        assert!(result.is_err());
    }
}
```

**Step 2: Update mod.rs**

Add to `core/src/mcp/mod.rs`:

```rust
mod approval;
pub use approval::{ApprovalHandler, ApprovalPresentCallback};
```

Also export the new types from jsonrpc:

```rust
pub use jsonrpc::mcp::{ApprovalDecision, ApprovalRequest, ApprovalResponse};
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test approval
```

**Step 4: Commit**

```bash
git add core/src/mcp/approval.rs core/src/mcp/mod.rs
git commit -m "feat(mcp): add ApprovalHandler for human-in-the-loop

Implements request/response flow for MCP servers to request
user confirmation before performing sensitive actions.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 9: Add Gateway RPC for Approval UI

**Files:**
- Modify: `core/src/gateway/handlers/mcp.rs`

**Step 1: Add approval RPC handlers**

Add new handlers for approval requests:

```rust
/// Handle mcp.list_pending_approvals
pub async fn handle_list_pending_approvals(
    state: &GatewayState,
    _params: Option<Value>,
) -> Result<Value, JsonRpcError> {
    let approvals = state.approval_handler.list_pending().await;
    Ok(serde_json::to_value(approvals).unwrap_or_default())
}

/// Handle mcp.respond_approval
pub async fn handle_respond_approval(
    state: &GatewayState,
    params: Option<Value>,
) -> Result<Value, JsonRpcError> {
    let params = params.ok_or_else(|| JsonRpcError::invalid_params("Missing params"))?;

    #[derive(Deserialize)]
    struct RespondParams {
        request_id: String,
        approved: bool,
        reason: Option<String>,
    }

    let p: RespondParams = serde_json::from_value(params)
        .map_err(|e| JsonRpcError::invalid_params(e.to_string()))?;

    state
        .approval_handler
        .respond(&p.request_id, p.approved, p.reason)
        .await
        .map_err(|e| JsonRpcError::internal_error(e.to_string()))?;

    Ok(json!({"success": true}))
}
```

**Step 2: Register handlers in router**

**Step 3: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo check
```

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/mcp.rs core/src/gateway/router.rs
git commit -m "feat(gateway): add approval RPC handlers

Exposes mcp.list_pending_approvals and mcp.respond_approval
for UI to display and respond to approval requests.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

This implementation plan covers P2.5 MCP Advanced Features:

1. **Tasks 1-3**: Cross-server context injection (include_context support)
2. **Tasks 4-5**: OAuth token refresh for long SSE connections
3. **Task 6**: Streaming sampling response infrastructure
4. **Tasks 7-9**: Human-in-the-loop approval dialog framework

**After P2.5:**
- Sampling requests can include context from single or all MCP servers
- OAuth tokens are automatically refreshed before expiration
- Streaming sampling responses are supported at the type level
- MCP servers can request user approval for sensitive actions

**Future P3 Extensions:**
- Rich approval dialogs with form inputs
- Streaming response forwarding via SSE
- Context injection with resource content (not just metadata)
- Per-server approval policies and rules
