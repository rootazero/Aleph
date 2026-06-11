//! Tools Cancel Handler
//!
//! Provides `tools.cancel_call` and `tools.in_flight` JSON-RPC methods.
//!
//! `tools.cancel_call` fires the `CancellationToken` registered by the
//! harness Act phase against a specific `tool_call_id`, terminating just
//! that call without aborting the whole run. The token's `tokio::select!`
//! arm (wired in Gap B across adapters, `ScopedToolService`, and subagent
//! runtimes) short-circuits the call and surfaces a cooperative-cancel
//! error to the LLM on the next turn.
//!
//! `tools.in_flight` is a diagnostic listing of every currently-running
//! tool call — useful for the CLI / panel UI to display before issuing a
//! targeted cancel.
//!
//! ## Request — cancel
//! ```json
//! {"call_id": "toolu_01ABC..."}
//! ```
//!
//! ## Response — cancel
//! ```json
//! {"ok": true, "cancelled": true}     // call_id found and token fired
//! {"ok": true, "cancelled": false}    // call_id not in flight (already done)
//! ```
//!
//! ## Request — `in_flight`
//! No params.
//!
//! ## Response — `in_flight`
//! ```json
//! {"calls": [{"call_id": "...", "tool_name": "bash", "started_at_ms": 123}]}
//! ```

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::tools::in_flight::InFlightToolCalls;

/// Parameters for `tools.cancel_call`.
#[derive(Debug, Clone, Deserialize)]
pub struct CancelParams {
    /// LLM-issued `tool_call_id` of the in-flight call to abort.
    pub call_id: String,
}

/// Cancel a single in-flight tool call.
///
/// `registry` is the process-wide in-flight registry installed at server
/// boot via `set_global_in_flight_tool_calls`. The harness's Act phase
/// registers each call under its `tool_call_id` for the duration of the
/// dispatch, so a matching `call_id` reliably points at a live token.
pub async fn handle_cancel(
    request: JsonRpcRequest,
    registry: InFlightToolCalls,
) -> JsonRpcResponse {
    let params: CancelParams = match request
        .params
        .as_ref()
        .map(|v| serde_json::from_value::<CancelParams>(v.clone()))
    {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            )
        }
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "params required: { call_id: string }",
            )
        }
    };

    if params.call_id.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "call_id must not be empty");
    }

    let cancelled = registry.cancel(&params.call_id);
    JsonRpcResponse::success(
        request.id,
        json!({
            "ok": true,
            "cancelled": cancelled,
            "call_id": params.call_id,
        }),
    )
}

/// List every currently-in-flight tool call. Diagnostic surface for CLI /
/// panel UIs that want to display cancel targets before the user picks one.
pub async fn handle_in_flight(
    request: JsonRpcRequest,
    registry: InFlightToolCalls,
) -> JsonRpcResponse {
    let snapshots = registry.list();
    match serde_json::to_value(&snapshots) {
        Ok(calls) => JsonRpcResponse::success(
            request.id,
            json!({
                "calls": calls,
                "count": snapshots.len(),
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to serialise in-flight snapshot: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tokio_util::sync::CancellationToken;

    fn req(method: &str, params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::from(1)),
            method: method.to_string(),
            params,
        }
    }

    #[tokio::test]
    async fn cancel_returns_cancelled_true_when_token_known() {
        let reg = InFlightToolCalls::new();
        let token = CancellationToken::new();
        let _guard = reg.register("call-7", "bash", token.clone());

        let resp = handle_cancel(
            req("tools.cancel_call", Some(json!({"call_id": "call-7"}))),
            reg,
        )
        .await;
        let body = resp.result.expect("expected success");
        assert_eq!(body["cancelled"], true);
        assert_eq!(body["call_id"], "call-7");
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_returns_cancelled_false_for_unknown_call() {
        let reg = InFlightToolCalls::new();
        let resp = handle_cancel(
            req("tools.cancel_call", Some(json!({"call_id": "nope"}))),
            reg,
        )
        .await;
        let body = resp.result.expect("expected success");
        assert_eq!(body["cancelled"], false);
    }

    #[tokio::test]
    async fn cancel_rejects_missing_params() {
        let reg = InFlightToolCalls::new();
        let resp = handle_cancel(req("tools.cancel_call", None), reg).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn cancel_rejects_empty_call_id() {
        let reg = InFlightToolCalls::new();
        let resp = handle_cancel(
            req("tools.cancel_call", Some(json!({"call_id": "  "}))),
            reg,
        )
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn in_flight_lists_registered_calls() {
        let reg = InFlightToolCalls::new();
        let _g1 = reg.register("a", "bash", CancellationToken::new());
        let _g2 = reg.register("b", "code_exec", CancellationToken::new());
        let resp = handle_in_flight(req("tools.in_flight", None), reg).await;
        let body = resp.result.expect("expected success");
        assert_eq!(body["count"], 2);
        let calls = body["calls"].as_array().expect("calls array");
        assert_eq!(calls.len(), 2);
    }
}
