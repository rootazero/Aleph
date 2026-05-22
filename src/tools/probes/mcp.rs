//! `McpServerProbe` — gates an MCP-backed tool by the underlying server's
//! transport liveness.
//!
//! Calls `McpClient::check_server_health` (the same probe the
//! `McpManagerActor` already runs every 5 s) and reports `Unhealthy` when
//! the server's transport is no longer alive. Default TTL (30 s) is fine
//! because the manager refreshes its own state more frequently — the
//! prompt-side gate only needs to be coarsely accurate.

use std::borrow::Cow;
use crate::sync_primitives::Arc;

use async_trait::async_trait;

use crate::mcp::McpClient;
use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthProbe};

pub struct McpServerProbe {
    client: Arc<McpClient>,
    server_id: String,
}

impl McpServerProbe {
    pub fn new(client: Arc<McpClient>, server_id: impl Into<String>) -> Self {
        Self {
            client,
            server_id: server_id.into(),
        }
    }
}

#[async_trait]
impl ToolHealthProbe for McpServerProbe {
    async fn probe(&self) -> ProbeResult {
        let alive = self
            .client
            .check_server_health()
            .await
            .get(&self.server_id)
            .copied()
            .unwrap_or(false);
        if alive {
            ProbeResult::Healthy
        } else {
            ProbeResult::Unhealthy {
                reason: HealthReason::DependencyDown(Cow::Owned(format!(
                    "mcp server '{}' transport down",
                    self.server_id
                ))),
                retry_after: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_reports_unhealthy_for_unknown_server() {
        // A fresh `McpClient` has no external servers, so any server_id
        // looks dead — exercising the `.get(...).unwrap_or(false)` path.
        let client = Arc::new(McpClient::new());
        let probe = McpServerProbe::new(client, "ghost");
        match probe.probe().await {
            ProbeResult::Unhealthy { reason, .. } => match reason {
                HealthReason::DependencyDown(msg) => {
                    assert!(msg.contains("ghost"));
                    assert!(msg.contains("transport down"));
                }
                other => panic!("expected DependencyDown, got {other:?}"),
            },
            ProbeResult::Healthy => panic!("expected Unhealthy for unknown server"),
        }
    }
}
