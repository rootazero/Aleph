//! Connection-owned acknowledged effects, not the read-only off-loop allowlist.
//! An invocation may await an event acknowledgement from this very connection.
//! Both permit acquisition and duplicate waits therefore leave its I/O loop.
use std::{collections::{HashMap, VecDeque}, future::Future, panic::AssertUnwindSafe, time::Duration};
use futures_util::{future::BoxFuture, FutureExt};
use tokio::{sync::RwLock, task::JoinSet};
use tokio_util::sync::CancellationToken;
use crate::{sync_primitives::Arc, gateway::{
    idempotency::{AcquireResult, IdempotencyGuard},
    lane::{ChannelClass, LaneManager}, middleware::MiddlewareChain,
    protocol::{JsonRpcResponse, INTERNAL_ERROR, SERVICE_UNAVAILABLE}, server::ConnectionState,
}};
use super::dispatch::{self, CallerSnapshot};

pub(super) const RUNNING: usize = 4;
pub(super) const QUEUED: usize = 32;
type Job = (Option<serde_json::Value>, BoxFuture<'static, String>);

pub(super) struct AcknowledgedEffects {
    root: CancellationToken,
    tasks: JoinSet<String>,
    queue: VecDeque<Job>,
    closed: bool,
}

impl AcknowledgedEffects {
    pub(super) fn new() -> Self {
        Self { root: CancellationToken::new(), tasks: JoinSet::new(), queue: VecDeque::new(), closed: false }
    }
    pub(super) fn token(&self) -> CancellationToken { self.root.child_token() }
    pub(super) fn is_empty(&self) -> bool { self.tasks.is_empty() }

    pub(super) fn submit(&mut self, id: Option<serde_json::Value>, job: impl Future<Output=String> + Send + 'static) -> Result<(), String> {
        if self.closed || (self.tasks.len() >= RUNNING && self.queue.len() >= QUEUED) {
            return Err(serde_json::to_string(&JsonRpcResponse::error(
                id, SERVICE_UNAVAILABLE, "UI effect queue full: Busy",
            )).unwrap_or_default());
        }
        let job = (id, Box::pin(job) as BoxFuture<'static, String>);
        if self.tasks.len() < RUNNING { self.spawn(job); } else { self.queue.push_back(job); }
        Ok(())
    }

    fn spawn(&mut self, (id, job): Job) {
        let root = self.root.clone();
        self.tasks.spawn(async move {
            tokio::select! {
                biased;
                _ = root.cancelled() => error(id, "UI effect cancelled: connection closed"),
                result = AssertUnwindSafe(job).catch_unwind() => {
                    result.unwrap_or_else(|_| error(id, "UI effect failed"))
                }
            }
        });
    }

    pub(super) async fn next_response(&mut self) -> Option<String> {
        let result = self.tasks.join_next().await?;
        if !self.closed {
            if let Some(job) = self.queue.pop_front() { self.spawn(job); }
        }
        Some(result.unwrap_or_else(|_| error(None, "UI effect task failed")))
    }

    /// Must precede surface close; drops queued RAII slots and wakes permit waits.
    pub(super) fn stop(&mut self) {
        self.closed = true;
        self.queue.clear();
        self.root.cancel();
    }

    /// Called after registry-owned surfaces have been closed, without registry locks.
    pub(super) async fn join(&mut self) {
        while self.tasks.join_next().await.is_some() {}
    }
}

pub(super) fn error(id: Option<serde_json::Value>, message: &str) -> String {
    serde_json::to_string(&JsonRpcResponse::error(id, INTERNAL_ERROR, message)).unwrap_or_default()
}

/// Only transport dependencies; authority is read from the live connection map.
#[derive(Clone)]
pub(super) struct EffectDispatch {
    pub middleware: MiddlewareChain,
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub lanes: Arc<LaneManager>,
    pub idempotency: Arc<IdempotencyGuard>,
    pub conn_id: String,
    pub class: ChannelClass,
    pub client_is_local: bool,
}

impl EffectDispatch {
    /// Cached replies still use live authentication. Waiting/Proceed run in the
    /// same bounded effect set, including their waits; there is no inline fallback.
    pub(super) async fn submit(&self, effects: &mut AcknowledgedEffects, text: String, id: Option<serde_json::Value>, start: CallerSnapshot, key: Option<String>) -> Result<(), String> {
        let acquisition = key.as_deref().map(|key| self.idempotency.try_acquire(key));
        if let Some(AcquireResult::Cached(value)) = acquisition {
            let response = if dispatch::live_ui_caller(&self.connections, &self.conn_id, &start).await.is_some() {
                serde_json::to_string(&JsonRpcResponse::success(id, value)).unwrap_or_default()
            } else { dispatch::viewer_denied_response(id) };
            return Err(response);
        }
        let context = self.clone();
        let token = effects.token();
        let response_id = id.clone();
        effects.submit(id, async move {
            if let Some(AcquireResult::Waiting(mut receiver)) = acquisition {
                let value = tokio::time::timeout(Duration::from_secs(30), async {
                    if receiver.borrow().is_none() { let _ = receiver.changed().await; }
                    receiver.borrow().clone()
                }).await;
                return match value {
                    Ok(Some(value)) if dispatch::live_ui_caller(&context.connections, &context.conn_id, &start).await.is_some() =>
                        serde_json::to_string(&JsonRpcResponse::success(response_id, value)).unwrap_or_default(),
                    Ok(Some(_)) => dispatch::viewer_denied_response(response_id),
                    _ => error(response_id, "Request timed out waiting for in-flight duplicate"),
                };
            }
            let slot = match acquisition { Some(AcquireResult::Proceed(slot)) => Some(slot), _ => None };
            let permit = tokio::select! {
                biased;
                _ = token.cancelled() => return error(response_id, "UI effect cancelled"),
                permit = context.lanes.acquire("capabilities.ui.invoke", context.class) => permit,
            };
            let Ok(_permit) = permit else { return error(response_id, "Service congested, try again later"); };
            let response = crate::gateway::capability_ui::with_ui_request_cancel(token, dispatch::dispatch_for_connection(
                &text, &context.middleware, &context.connections, &context.conn_id,
                context.client_is_local, "capabilities.ui.invoke", response_id, &start,
            )).await;
            if let Some(slot) = slot {
                // Display's successful result is JSON null. Deserializing it
                // as Option<Value> loses presence and would discard this slot.
                match serde_json::from_str::<serde_json::Value>(&response) {
                    Ok(parsed) if parsed.get("error").is_none_or(serde_json::Value::is_null) && parsed.get("result").is_some() =>
                        slot.complete(parsed["result"].clone()),
                    _ => slot.discard(),
                }
            }
            response
        })
    }
}
