//! Insights admin handlers
//!
//! Provides the `insights.tools` JSON-RPC method: a read-only per-tool usage
//! breakdown for an agent over a rolling time window, aggregated from the
//! `ToolInvocation` raw-memory rows the signal sink already records. Mirrors
//! the `dreaming.run_now` handler shape — introspection surface for the panel
//! / `aleph` CLI, no mutation.
//!
//! ## Request
//! ```json
//! {"agent_id": "main", "window_seconds": 86400, "top_n": 20}
//! ```
//! All fields optional: `agent_id` defaults to the routing default,
//! `window_seconds` to 24h, `top_n` to 20.
//!
//! ## Response (success)
//! ```json
//! {"ok": true, "report": {...ToolUsageReport...}}
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::json;

use crate::memory::insights::aggregate_tool_usage;
use crate::memory::store::MemoryBackend;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Default window: the last 24 hours (matches the dream cycle's 24h tool aggregate).
const DEFAULT_WINDOW_SECONDS: i64 = 86_400;
/// Default number of per-tool rows returned.
const DEFAULT_TOP_N: usize = 20;
/// Upper bound on rows fetched from the store for one report.
const FETCH_LIMIT: usize = 50_000;

#[derive(Debug, Default, Deserialize)]
struct ToolInsightsParams {
    agent_id: Option<String>,
    window_seconds: Option<i64>,
    top_n: Option<usize>,
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Aggregate per-tool usage for the requested agent/window.
pub async fn handle_tools(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: ToolInsightsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    // Reject non-positive windows / zero top_n by falling back to defaults.
    let window = params
        .window_seconds
        .filter(|w| *w > 0)
        .unwrap_or(DEFAULT_WINDOW_SECONDS);
    let top_n = params.top_n.filter(|n| *n > 0).unwrap_or(DEFAULT_TOP_N);
    let since = now_unix_secs().saturating_sub(window);

    match aggregate_tool_usage(db.as_ref(), agent_id, since, window, top_n, FETCH_LIMIT).await {
        Ok(report) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "report": report,
            }),
        ),
        Err(err) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("insights.tools failed: {}", err),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_default_when_absent_or_invalid() {
        // Absent params → all defaults.
        let p: ToolInsightsParams = serde_json::from_value(json!({})).unwrap();
        assert!(p.agent_id.is_none());
        let window = p.window_seconds.filter(|w| *w > 0).unwrap_or(DEFAULT_WINDOW_SECONDS);
        let top_n = p.top_n.filter(|n| *n > 0).unwrap_or(DEFAULT_TOP_N);
        assert_eq!(window, DEFAULT_WINDOW_SECONDS);
        assert_eq!(top_n, DEFAULT_TOP_N);

        // Non-positive window / zero top_n fall back to defaults.
        let p2: ToolInsightsParams =
            serde_json::from_value(json!({"window_seconds": 0, "top_n": 0})).unwrap();
        let window2 = p2.window_seconds.filter(|w| *w > 0).unwrap_or(DEFAULT_WINDOW_SECONDS);
        let top_n2 = p2.top_n.filter(|n| *n > 0).unwrap_or(DEFAULT_TOP_N);
        assert_eq!(window2, DEFAULT_WINDOW_SECONDS);
        assert_eq!(top_n2, DEFAULT_TOP_N);
    }

    #[test]
    fn params_parse_explicit_values() {
        let p: ToolInsightsParams = serde_json::from_value(
            json!({"agent_id": "alpha", "window_seconds": 3600, "top_n": 5}),
        )
        .unwrap();
        assert_eq!(p.agent_id.as_deref(), Some("alpha"));
        assert_eq!(p.window_seconds, Some(3600));
        assert_eq!(p.top_n, Some(5));
    }
}
