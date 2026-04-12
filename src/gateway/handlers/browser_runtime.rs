//! Browser runtime RPC handlers: probe + install of fnm/node/cli/chromium/skills.

use std::sync::Arc;

use crate::browser::bootstrap::{self, BootstrapStatus, ProgressFn};
use crate::gateway::event_bus::{BrowserInstallProgressEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// `browser.runtime_status` — return a snapshot of all component install states.
pub async fn handle_runtime_status(request: JsonRpcRequest) -> JsonRpcResponse {
    let status = BootstrapStatus::probe().await;
    match serde_json::to_value(&status) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("serialize: {e}")),
    }
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
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let bus = event_bus.clone();
    tokio::spawn(async move {
        let bus2 = bus.clone();
        let progress: ProgressFn = Arc::new(move |step, status, line| {
            let log_line = if status == "log" { line.clone() } else { None };
            let error = if status == "failed" { line } else { None };
            let event = GatewayEvent::BrowserInstallProgress(BrowserInstallProgressEvent {
                step: step.as_str().to_string(),
                status: status.to_string(),
                log_line,
                error,
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = bus2.publish_json(&event);
        });
        let _ = bootstrap::install_missing(progress).await;
    });

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
    async fn test_runtime_status_returns_valid_json() {
        let req = JsonRpcRequest::with_id("browser.runtime_status", None, json!(1));
        let resp = handle_runtime_status(req).await;
        assert!(resp.result.is_some(), "expected a result, got: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert!(result.get("fnm").is_some(), "missing 'fnm' field");
        assert!(result.get("node").is_some(), "missing 'node' field");
        assert!(result.get("playwright_cli").is_some(), "missing 'playwright_cli' field");
        assert!(result.get("chromium").is_some(), "missing 'chromium' field");
        assert!(result.get("skills").is_some(), "missing 'skills' field");
    }

    #[tokio::test]
    async fn test_refresh_runtime_delegates_to_status() {
        let req = JsonRpcRequest::with_id("browser.refresh_runtime", None, json!(2));
        let resp = handle_refresh_runtime(req).await;
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert!(result.get("fnm").is_some());
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
