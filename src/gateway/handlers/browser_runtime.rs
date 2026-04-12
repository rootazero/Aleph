//! Browser runtime RPC handlers: probe + install of fnm/node/cli/chromium/skills.
//!
//! NOTE: This file is a temporary stub. The browser::bootstrap module was removed
//! in favour of crate::runtimes::ensure_capability. This handler will be
//! replaced/deleted in the next task (Task 9).

use std::sync::Arc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// `browser.runtime_status` — return a snapshot of all component install states.
pub async fn handle_runtime_status(request: JsonRpcRequest) -> JsonRpcResponse {
    // TODO(Task 9): re-implement using runtimes::ledger snapshot.
    JsonRpcResponse::error(request.id, INTERNAL_ERROR, "not yet implemented".to_string())
}

/// `browser.refresh_runtime` — identical to `runtime_status`; separate name so the
/// UI can express a "manual refresh" intent distinct from the initial load.
pub async fn handle_refresh_runtime(request: JsonRpcRequest) -> JsonRpcResponse {
    handle_runtime_status(request).await
}

/// `browser.install_runtime` — kick off a background install of all missing
/// components and return immediately with `{"accepted": true}`.
///
/// Progress is streamed to connected WebSocket clients via `GatewayEvent::BrowserInstallProgress`.
pub async fn handle_install_runtime(
    request: JsonRpcRequest,
    _event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // TODO(Task 9): re-implement using runtimes::ensure_capability.
    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "accepted": true }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_refresh_runtime_delegates_to_status() {
        let req = JsonRpcRequest::with_id("browser.refresh_runtime", None, json!(2));
        let resp = handle_refresh_runtime(req).await;
        // Stub returns error for now — just verify it doesn't panic.
        let _ = resp;
    }

    #[tokio::test]
    async fn test_install_runtime_returns_accepted() {
        let req = JsonRpcRequest::with_id("browser.install_runtime", None, json!(3));
        let bus = Arc::new(GatewayEventBus::new());
        let resp = handle_install_runtime(req, bus).await;
        assert!(resp.result.is_some(), "expected accepted=true, got: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result.get("accepted"), Some(&json!(true)));
    }
}
