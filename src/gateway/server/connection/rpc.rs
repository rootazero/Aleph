//! Shared ordinary JSON-RPC execution; transport control stays with the adapter.

use std::{net::IpAddr, sync::Arc, time::Duration};
use tracing::{debug, warn};

use super::dispatch::{self, CallerSnapshot, DispatchPeer};
use crate::gateway::idempotency::{AcquireResult, IdempotencyGuard};
use crate::gateway::lane::{ChannelClass, Lane, LaneManager};
use crate::gateway::middleware::MiddlewareChain;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, IDEMPOTENCY_KEY_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS,
    RATE_LIMITED,
};
use crate::gateway::rate_limiter::{scope_for_method, RateLimitError, RateLimitKey, RateLimiter};

pub(super) struct RpcPlan {
    lane: Lane,
    pub(super) key: Option<String>,
}

impl RpcPlan {
    pub(super) fn without_cache(mut self) -> Self {
        self.key = None;
        self
    }
}

#[derive(Clone)]
pub(super) struct RpcExecutor {
    pub(super) middleware: MiddlewareChain,
    pub(super) lanes: Arc<LaneManager>,
    pub(super) idempotency: Arc<IdempotencyGuard>,
    pub(super) class: ChannelClass,
}

pub(super) fn check_origin_rate(
    limiter: &RateLimiter,
    origin: Option<IpAddr>,
    req: &JsonRpcRequest,
) -> Result<(), Box<JsonRpcResponse>> {
    let Some(origin) = origin else {
        return Ok(());
    };
    let key = RateLimitKey::new(&origin.to_string(), scope_for_method(&req.method));
    limiter.check_and_record(&key).map_err(|error| {
        Box::new(match error {
            RateLimitError::Exceeded { retry_after_ms, .. } => JsonRpcResponse::error_with_data(
                req.id.clone(),
                RATE_LIMITED,
                "Rate limit exceeded",
                serde_json::json!({"retry_after_ms": retry_after_ms}),
            ),
            RateLimitError::LockedOut {
                lockout_remaining_ms,
                ..
            } => JsonRpcResponse::error_with_data(
                req.id.clone(),
                RATE_LIMITED,
                "Rate limit lockout",
                serde_json::json!({"lockout_remaining_ms": lockout_remaining_ms}),
            ),
        })
    })
}

pub(super) fn prepare(
    req: &JsonRpcRequest,
    caller: &CallerSnapshot,
    require_key: bool,
) -> Result<RpcPlan, Box<JsonRpcResponse>> {
    const SEP: char = '\u{1f}';
    let raw_key = req
        .params
        .as_ref()
        .and_then(|params| params.get("idempotency_key"))
        .and_then(serde_json::Value::as_str);
    // Prevent a raw key from forging another principal/method's slot.
    if raw_key.is_some_and(|key| key.contains(SEP)) {
        return Err(Box::new(JsonRpcResponse::error(
            req.id.clone(),
            INVALID_PARAMS,
            "idempotency_key contains a reserved separator character",
        )));
    }
    let key = raw_key.map(|key| {
        let principal = dispatch::idempotency_principal(&req.method, caller);
        let namespaced = format!("{principal}{SEP}{}{SEP}{key}", req.method);
        debug_assert_eq!(namespaced.matches(SEP).count(), 2);
        namespaced
    });
    let lane = Lane::for_method(&req.method);
    if require_key && lane.needs_idempotency() && key.is_none() {
        warn!(method = %req.method, %lane, "Rejecting mutating RPC without idempotency_key (require_idempotency_key=true)");
        return Err(Box::new(JsonRpcResponse::error_with_data(
            req.id.clone(),
            IDEMPOTENCY_KEY_REQUIRED,
            "idempotency_key required for mutating RPCs",
            serde_json::json!({
                "method": req.method,
                "lane": lane.to_string(),
                "hint": "include a stable per-attempt idempotency_key (UUID v4) in params",
            }),
        )));
    }
    Ok(RpcPlan { lane, key })
}

impl RpcExecutor {
    pub(super) async fn execute(
        &self,
        text: &str,
        req: &JsonRpcRequest,
        plan: RpcPlan,
        peer: &DispatchPeer,
        start: &CallerSnapshot,
    ) -> String {
        let dispatch = || async {
            match self.lanes.acquire(&req.method, self.class).await {
                // Identity is read again after the permit, not inferred from the request.
                Ok(_permit) => {
                    dispatch::dispatch_for_peer(
                        text,
                        &self.middleware,
                        peer,
                        &req.method,
                        req.id.clone(),
                        start,
                    )
                    .await
                }
                Err(_) => serde_json::to_string(&JsonRpcResponse::error(
                    req.id.clone(),
                    INTERNAL_ERROR,
                    "Service congested, try again later",
                ))
                .unwrap_or_default(),
            }
        };
        // Selection re-applies a process-local assignment even if the same key
        // was used before an intervening switch.
        let Some(key) = plan.key.filter(|_| {
            plan.lane.needs_idempotency()
                && req.method != aleph_protocol::panel::PANEL_PROJECT_SELECT_METHOD
        }) else {
            return dispatch().await;
        };
        let value = match self.idempotency.try_acquire(&key) {
            AcquireResult::Proceed(slot) => {
                let response = dispatch().await;
                if let Ok(parsed) = serde_json::from_str::<JsonRpcResponse>(&response) {
                    if parsed.is_success() {
                        if let Some(result) = parsed.result {
                            slot.complete(result);
                        }
                    }
                }
                // Any non-success drops the open slot rather than caching it.
                return response;
            }
            AcquireResult::Cached(value) => {
                debug!(method = %req.method, "Idempotency hit");
                value
            }
            AcquireResult::Waiting(mut receiver) => {
                debug!(method = %req.method, "Awaiting in-flight duplicate");
                match tokio::time::timeout(Duration::from_secs(30), async {
                    let _ = receiver.changed().await;
                    receiver.borrow().clone()
                })
                .await
                {
                    Ok(Some(value)) => value,
                    _ => {
                        return serde_json::to_string(&JsonRpcResponse::error(
                            req.id.clone(),
                            INTERNAL_ERROR,
                            "Request timed out waiting for in-flight duplicate",
                        ))
                        .unwrap_or_default();
                    }
                }
            }
        };
        // Both cache hits and joined results bypass middleware; the live gate
        // must succeed after obtaining either result, before returning it.
        if !dispatch::revalidate_peer(peer, &req.method, start).await {
            return dispatch::viewer_denied_response(req.id.clone());
        }
        serde_json::to_string(&JsonRpcResponse::success(req.id.clone(), value)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
