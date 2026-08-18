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

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex;

use futures::Future;
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AlephError, Result};
use crate::mcp::auth::{CallbackServer, OAuthProvider, OAuthStorage};
use crate::mcp::manager::{McpManagerHandle, McpTransportType};
use crate::sync_primitives::Arc;
use crate::tool_metadata::{ToolCategory, ToolDefinition};
use crate::tools::AlephToolDyn;

/// BT-D-R4-23: per-server in-flight OAuth tracker. Without this, two
/// concurrent `mcp_login` calls for the same `server_id` would each return
/// their own authorization URL, each spawn a callback listener on a
/// different port, each write a pending state to the shared storage, and
/// race to overwrite each other's PKCE verifier. The user receives two
/// URLs and only one can succeed; the other flow's `finish_authorization`
/// sees a state mismatch and silently fails, leaving the user confused.
///
/// The tracker is process-local and keyed by `server_id`. Acquiring the
/// lock for the duration of `start_authorization` (and releasing it on
/// `Drop` of the guard after the background task is spawned) ensures that
/// at most one in-flight OAuth flow exists per server at any moment. A
/// concurrent call gets an explicit error rather than a duplicate URL.
static IN_FLIGHT: Lazy<Mutex<HashMap<String, Arc<()>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// RAII guard that releases the in-flight slot on drop.
struct InFlightGuard {
    server_id: String,
    permit: Arc<()>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = IN_FLIGHT.lock() {
            // Only remove if the permit is the same one we inserted. A new
            // flow may have raced to acquire it after the previous guard
            // was created (shouldn't happen with the current usage, but
            // defending against the pointer-equal swap keeps the map
            // honest across future refactors).
            if let Some(current) = map.get(&self.server_id) {
                if Arc::ptr_eq(current, &self.permit) {
                    map.remove(&self.server_id);
                }
            }
        }
    }
}

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

impl McpLoginTool {
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
        "Authorize a remote MCP server via OAuth. Returns an authorization URL — relay it \
         to the user to open in a browser. After they approve, the token exchange completes \
         automatically in the background (5 minute window) and the server reconnects with \
         credentials. Use when a remote MCP server rejects credentials (auth_expired) or \
         requires login.";

    /// The argument schema, measured beside the description because these
    /// tools sit in `default_core_tools()` — progressive disclosure never
    /// collapses them, so the schema ships in full on every request whose
    /// capability gate is open. (`mcp_login` is the one that is not core,
    /// and is measured anyway: being uncollapsed is the reason to measure,
    /// not the condition for it.)
    pub(crate) fn schema_value() -> Value {
        let schema = schemars::schema_for!(McpLoginArgs);
        serde_json::to_value(&schema).unwrap_or_default()
    }
}

impl AlephToolDyn for McpLoginTool {
    fn name(&self) -> &str {
        "mcp_login"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "mcp_login",
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
            let args: McpLoginArgs = serde_json::from_value(args)?;
            let server_id = args.server;

            // BT-D-R4-23: acquire the per-server in-flight permit. The
            // permit is held until the spawned background task completes
            // (the guard is moved into the task), so a second concurrent
            // call for the same server gets a clean error instead of a
            // duplicate URL that races to overwrite the PKCE state.
            let _guard = {
                let mut map = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
                if map.contains_key(&server_id) {
                    return Err(AlephError::IoError(format!(
                        "MCP OAuth login already in progress for server '{server_id}'; \
                         wait for the existing flow to complete or time out before \
                         starting another one."
                    )));
                }
                let permit = Arc::new(());
                map.insert(server_id.clone(), permit.clone());
                InFlightGuard {
                    server_id: server_id.clone(),
                    permit,
                }
            };

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
            // BT-D-R4-23: move the in-flight guard into the task so the
            // permit is held for the full flow lifetime. Drop happens on
            // task exit (success, error, panic — all paths).
            let handle = self.handle.clone();
            let task_server = server_id.clone();
            tokio::spawn(async move {
                let _guard = _guard;
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
