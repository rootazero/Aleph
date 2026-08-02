//! MCP OAuth Login Tool
//!
//! Drives the interactive OAuth 2.0 authorization flow for a remote MCP
//! server (R8: configuration through conversation): discovers authorization
//! server metadata, reuses or dynamically registers a client (RFC 7591),
//! hands the authorization URL back to the LLM to relay to the user, and
//! completes the PKCE token exchange in the background once the browser
//! callback arrives. On success the server is restarted so
//! `McpClient::start_remote_server` picks up the stored token.
//!
//! The tool returns immediately with the URL instead of blocking on the
//! callback — the user may be on a remote channel (Telegram, Slack) where
//! the approval happens minutes later (R5: no blocking interaction).

use std::pin::Pin;

use futures::Future;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AlephError, Result};
use crate::mcp::auth::{CallbackServer, OAuthProvider, OAuthStorage};
use crate::mcp::manager::{McpManagerHandle, McpTransportType};
use crate::sync_primitives::Arc;
use crate::tool_metadata::{ToolCategory, ToolDefinition};
use crate::tools::AlephToolDyn;

/// Arguments for `mcp_login` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpLoginArgs {
    /// Id of the configured remote MCP server to authorize
    pub server: String,
    /// Optional OAuth scope to request
    #[serde(default)]
    pub scope: Option<String>,
}

/// Output from `mcp_login` tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpLoginOutput {
    /// URL the user must open in a browser to approve access
    pub authorization_url: String,
    /// What to tell the user / what happens next
    pub instructions: String,
}

/// Tool that authorizes a remote MCP server via OAuth
pub struct McpLoginTool {
    handle: McpManagerHandle,
}

impl McpLoginTool {
    /// Create a new MCP login tool
    #[must_use]
    pub const fn new(handle: McpManagerHandle) -> Self {
        Self { handle }
    }
}

impl AlephToolDyn for McpLoginTool {
    fn name(&self) -> &str {
        "mcp_login"
    }

    fn definition(&self) -> ToolDefinition {
        let schema = schemars::schema_for!(McpLoginArgs);
        let parameters: Value = serde_json::to_value(&schema).unwrap_or_default();
        ToolDefinition::new(
            "mcp_login",
            "Authorize a remote MCP server via OAuth. Returns an authorization URL — relay it \
             to the user to open in a browser. After they approve, the token exchange completes \
             automatically in the background (5 minute window) and the server reconnects with \
             credentials. Use when a remote MCP server rejects credentials (auth_expired) or \
             requires login.",
            parameters,
            ToolCategory::Mcp,
        )
    }

    fn call(
        &self,
        args: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value>> + Send + '_>> {
        Box::pin(async move {
            let args: McpLoginArgs = serde_json::from_value(args)?;
            let server_id = args.server;

            // Resolve the server's URL from its managed configuration.
            let detail = self.handle.get_status(&server_id).await?.ok_or_else(|| {
                AlephError::NotFound(format!("MCP server not found: {server_id}"))
            })?;

            if matches!(detail.config.transport, McpTransportType::Stdio) {
                return Err(AlephError::IoError(format!(
                    "MCP server '{server_id}' uses stdio transport; OAuth applies only to remote (http/sse) servers"
                )));
            }
            let url = detail.config.url.clone().ok_or_else(|| {
                AlephError::IoError(format!("MCP server '{server_id}' has no URL configured"))
            })?;

            let storage = Arc::new(OAuthStorage::new(OAuthStorage::default_path()));
            let callback = CallbackServer::new();
            let provider =
                OAuthProvider::new(storage.clone(), &server_id, &url, callback.callback_url());

            let metadata = provider.discover_metadata().await?;

            // Reuse a previously registered client or register dynamically.
            // Credentials are only reused when this same authorization server
            // issued them; a changed issuer re-registers instead.
            let client_info = match provider.client_info_for(&metadata).await? {
                Some(info) => info,
                None => provider.register_client(&metadata).await?,
            };

            let authorization_url = provider
                .start_authorization(&metadata, &client_info.client_id, args.scope.as_deref())
                .await?;

            // Complete the exchange in the background; the callback server
            // shuts itself down after one callback or the timeout.
            let handle = self.handle.clone();
            let task_server = server_id.clone();
            tokio::spawn(async move {
                match callback.wait_for_callback().await {
                    Ok(cb) => {
                        match provider
                            .finish_authorization(
                                &metadata,
                                &client_info.client_id,
                                &cb.code,
                                &cb.state,
                                cb.iss.as_deref(),
                            )
                            .await
                        {
                            Ok(_) => {
                                tracing::info!(
                                    server = %task_server,
                                    "MCP OAuth authorization complete; restarting server"
                                );
                                if let Err(e) = handle.restart_server(&task_server).await {
                                    tracing::warn!(
                                        server = %task_server,
                                        error = %e,
                                        "Server restart after OAuth login failed"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    server = %task_server,
                                    error = %e,
                                    "OAuth token exchange failed"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %task_server,
                            error = %e,
                            "OAuth callback wait failed or timed out"
                        );
                    }
                }
            });

            let output = McpLoginOutput {
                authorization_url,
                instructions: "Send this URL to the user to open in a browser. After they \
                               approve access, the token exchange completes automatically \
                               (within a 5 minute window) and the MCP server reconnects with \
                               credentials."
                    .to_string(),
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
        let schema = schemars::schema_for!(McpLoginArgs);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("server"));
        assert!(json.contains("scope"));
    }

    #[test]
    fn test_args_parse_minimal() {
        let args: McpLoginArgs = serde_json::from_value(serde_json::json!({
            "server": "github"
        }))
        .unwrap();
        assert_eq!(args.server, "github");
        assert!(args.scope.is_none());
    }
}
