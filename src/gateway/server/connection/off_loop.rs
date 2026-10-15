//! Off-loop dispatch for audited cold read-only RPCs.
//!
//! `handle_connection` runs ONE `tokio::select!` loop per socket that both
//! answers inbound RPCs and forwards subscribed events. An RPC awaited inline
//! in that loop freezes the connection: no event is forwarded and no later
//! request (not even `events.subscribe`) is read until it returns. A cold
//! `extensions.catalog` (first call per process initialises the shared skill
//! system, ~40 s on a large home) therefore swallowed every `browser.live.*`
//! frame published to a freshly opened Panel (Task #56, native windows 5/7).
//!
//! Only an explicit, audited allowlist ([`OFF_LOOP_METHODS`]) runs as tasks
//! owned by the connection ([`OffLoopRpcs`]); the loop writes each response
//! when it completes. JSON-RPC responses carry their request `id`, so
//! out-of-order completion is wire-legal. The allowlist is deliberately NOT
//! derived from the lane / method-name suffix: a suffix proves a name, not
//! that the handler is pure, and every off-loop task competes for the shared
//! Query-lane permits. Everything else (including `connect` and `events.*`,
//! which mutate this connection's state) stays inline and ordered.
//!
//! Authority: because an off-loop RPC may wait in the queue and for a lane
//! permit while the loop keeps processing `connect` / demotions, the caller's
//! role is re-read LIVE at dispatch (`dispatch::dispatch_for_connection`):
//! the login wall (`auth::wall_admits`) is re-applied and the handler runs
//! under the live role, so the admin gate sees a demotion.
//!
//! Bounds (per connection): at most [`MAX_OFF_LOOP_RPCS`] run at once, at most
//! [`MAX_QUEUED_OFF_LOOP_RPCS`] wait behind them, and beyond that the request
//! is answered immediately with a JSON-RPC error for its id. A read-only
//! future is NEVER awaited inline when the set is full (that would recreate
//! the freeze). Tearing the connection down drops queued work, aborts active
//! work, and waits at most [`OFF_LOOP_SHUTDOWN_GRACE`] for the aborted tasks
//! to finish ([`OffLoopRpcs::shutdown`]) before the connection cleanup runs.
//! Abort only takes effect at a task's next `.await`; a task parked in a
//! synchronous section would otherwise hold teardown hostage. The main cold
//! skill scans, CatalogCache SQLite calls and extension discovery/manifest
//! scanning run on the blocking pool, but NOT all cold work does: residual
//! synchronous steps remain (WASM/Extism loading, mount data-dir/vault work,
//! projection discovery, user-hook refresh, legacy marker migration; see
//! `docs/reference/BROWSER_LIVE_VIEWER.md`), so an audited handler can still
//! occupy a runtime worker for a while and teardown relies on the grace. Past the grace, the cleanup proceeds and the already-aborted task
//! is cancelled at its next yield point; it owns only `Arc` clones of
//! server state, so it cannot touch freed connection resources.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;

use futures_util::FutureExt;
use tokio::task::JoinSet;
use tracing::error;

use crate::gateway::protocol::{JsonRpcResponse, INTERNAL_ERROR};

/// Per-connection cap on concurrently running off-loop RPCs. Kept small: the
/// Query-lane permit pool is shared by every connection, so a single
/// connection must not be able to hold a large share of it.
pub const MAX_OFF_LOOP_RPCS: usize = 4;

/// Per-connection cap on off-loop RPCs waiting for a running slot.
pub const MAX_QUEUED_OFF_LOOP_RPCS: usize = 32;

/// Upper bound on how long connection teardown waits for aborted off-loop
/// tasks to finish before running the connection cleanup anyway.
pub const OFF_LOOP_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Audited cold read methods that may leave the select loop. Each was read in
/// source and verified to mutate neither connection nor server state; each is
/// slow only on the first call per process (shared skill-system init):
///
/// * `extensions.catalog`   — `extensions::catalog::handle_catalog`
/// * `extensions.installed` — `extensions::catalog::handle_installed`
///   (`collect_installed` → skill init; Query lane via `Lane::override_for`)
/// * `skills.status`        — `skills::handle_status` (`full_status` snapshot)
///
/// Adding a method here requires the same source audit; do not widen this to
/// a lane or a suffix.
pub const OFF_LOOP_METHODS: &[&str] = &[
    "extensions.catalog",
    "extensions.installed",
    "skills.status",
];

/// Whether `method` may run outside the connection's select loop.
pub fn dispatches_off_loop(method: &str) -> bool {
    OFF_LOOP_METHODS.contains(&method)
}

type BoxedDispatch = Pin<Box<dyn Future<Output = String> + Send + 'static>>;

/// Serialized JSON-RPC error answering `req_id` when the set is saturated.
fn saturated_response(req_id: Option<serde_json::Value>) -> String {
    serde_json::to_string(&JsonRpcResponse::error(
        req_id,
        INTERNAL_ERROR,
        "Too many concurrent read-only requests on this connection; retry shortly",
    ))
    .unwrap_or_default()
}

/// Wrap `dispatch` so a panic answers `INTERNAL_ERROR` for the same request
/// id instead of leaving the caller waiting forever.
async fn guarded(req_id: Option<serde_json::Value>, dispatch: BoxedDispatch) -> String {
    match AssertUnwindSafe(dispatch).catch_unwind().await {
        Ok(response) => response,
        Err(_) => {
            error!("off-loop RPC dispatch panicked; answering INTERNAL_ERROR");
            serde_json::to_string(&JsonRpcResponse::error(
                req_id,
                INTERNAL_ERROR,
                "Internal error while handling request",
            ))
            .unwrap_or_default()
        }
    }
}

/// Connection-owned bounded set of in-flight (and queued) read-only RPCs.
///
/// Invariant: the queue is non-empty only while at least one task runs (every
/// completion promotes a queued entry synchronously), so `is_empty()` is a
/// sufficient condition for the select arm that drains it.
#[derive(Default)]
pub struct OffLoopRpcs {
    tasks: JoinSet<String>,
    queue: VecDeque<(Option<serde_json::Value>, BoxedDispatch)>,
}

impl OffLoopRpcs {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when nothing is running or queued.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.queue.is_empty()
    }

    /// Number of running tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Number of requests waiting for a running slot.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Run `dispatch` off-loop, queue it behind the running ones, or — when
    /// both are full — return the serialized error response for `req_id`.
    /// The future is never handed back, so it can never run inline.
    pub fn submit<F>(
        &mut self,
        req_id: Option<serde_json::Value>,
        dispatch: F,
    ) -> Result<(), String>
    where
        F: Future<Output = String> + Send + 'static,
    {
        if self.tasks.len() < MAX_OFF_LOOP_RPCS {
            self.tasks.spawn(guarded(req_id, Box::pin(dispatch)));
            Ok(())
        } else if self.queue.len() < MAX_QUEUED_OFF_LOOP_RPCS {
            self.queue.push_back((req_id, Box::pin(dispatch)));
            Ok(())
        } else {
            Err(saturated_response(req_id))
        }
    }

    /// Start queued work while there are free running slots.
    fn promote(&mut self) {
        while self.tasks.len() < MAX_OFF_LOOP_RPCS {
            let Some((req_id, dispatch)) = self.queue.pop_front() else {
                break;
            };
            self.tasks.spawn(guarded(req_id, dispatch));
        }
    }

    /// Next completed response. `None` when nothing is running; a cancelled
    /// task (only possible during teardown) is skipped. Cancel-safe: the
    /// queue is promoted synchronously right after a task is joined.
    pub async fn next_response(&mut self) -> Option<String> {
        loop {
            let joined = self.tasks.join_next().await?;
            self.promote();
            match joined {
                Ok(response) => return Some(response),
                Err(e) => {
                    error!("off-loop RPC task failed to join: {e}");
                    continue;
                }
            }
        }
    }

    /// Drop queued work, abort every running task, and wait up to
    /// [`OFF_LOOP_SHUTDOWN_GRACE`] for them to finish. Call before the
    /// connection cleanup. Returns `true` when every task finished in time.
    pub async fn shutdown(&mut self) -> bool {
        self.shutdown_within(OFF_LOOP_SHUTDOWN_GRACE).await
    }

    /// [`Self::shutdown`] with an explicit grace. On timeout the aborted
    /// handles are detached (dropping a `JoinSet` aborts, never blocks); each
    /// task is cancelled at its next yield point.
    pub async fn shutdown_within(&mut self, grace: std::time::Duration) -> bool {
        self.queue.clear();
        self.tasks.abort_all();
        let drained = tokio::time::timeout(grace, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        if !drained {
            error!(
                remaining = self.tasks.len(),
                "off-loop RPC tasks did not stop within the teardown grace; detaching"
            );
            self.tasks.detach_all();
        }
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_the_audited_cold_reads_leave_the_loop() {
        // The method that froze the Panel in Task #56, plus its audited peers.
        assert!(dispatches_off_loop("extensions.catalog"));
        assert!(dispatches_off_loop("extensions.installed"));
        assert!(dispatches_off_loop("skills.status"));
        // Other Query methods are NOT offloaded (no suffix/lane heuristic).
        assert!(!dispatches_off_loop("config.get"));
        assert!(!dispatches_off_loop("health"));
        assert!(!dispatches_off_loop("models.list"));
        // Connection-state mutations stay ordered on the loop.
        assert!(!dispatches_off_loop("connect"));
        assert!(!dispatches_off_loop("events.subscribe"));
        assert!(!dispatches_off_loop("events.unsubscribe"));
        assert!(!dispatches_off_loop("events.list"));
        // Everything with side effects stays inline.
        assert!(!dispatches_off_loop("agent.run"));
        assert!(!dispatches_off_loop("config.patch"));
        assert!(!dispatches_off_loop("extensions.install"));
        assert!(!dispatches_off_loop("extensions.toggle"));
        assert!(!dispatches_off_loop("chat.send"));
    }

    #[test]
    fn every_allowlisted_method_is_query_lane() {
        // The off-loop path bypasses the idempotency guard, so an allowlisted
        // method must never need one.
        for m in OFF_LOOP_METHODS {
            assert!(
                !crate::gateway::lane::Lane::for_method(m).needs_idempotency(),
                "{m} is allowlisted off-loop but is not Query lane"
            );
        }
    }

    #[tokio::test]
    async fn a_slow_dispatch_does_not_hold_back_a_fast_one() {
        let mut set = OffLoopRpcs::new();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        set.submit(Some(serde_json::json!(1)), async move {
            let _ = release_rx.await;
            "slow".to_string()
        })
        .unwrap();
        set.submit(Some(serde_json::json!(2)), async { "fast".to_string() })
            .unwrap();

        let first = tokio::time::timeout(Duration::from_secs(5), set.next_response())
            .await
            .expect("fast response must not wait for the slow one");
        assert_eq!(first.as_deref(), Some("fast"));
        assert_eq!(set.len(), 1, "slow dispatch still in flight");

        release_tx.send(()).unwrap();
        assert_eq!(set.next_response().await.as_deref(), Some("slow"));
        assert!(set.is_empty());
        assert_eq!(set.next_response().await, None);
    }

    #[tokio::test]
    async fn saturation_queues_then_errors_and_never_runs_inline() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let mut set = OffLoopRpcs::new();
        for i in 0..MAX_OFF_LOOP_RPCS {
            set.submit(Some(serde_json::json!(i)), std::future::pending::<String>())
                .unwrap();
        }
        assert_eq!(set.len(), MAX_OFF_LOOP_RPCS);

        // Queue fills up; the extra futures must not be polled (not inline,
        // not started) while every running slot is occupied.
        let polled = Arc::new(AtomicBool::new(false));
        for i in 0..MAX_QUEUED_OFF_LOOP_RPCS {
            let polled = polled.clone();
            set.submit(Some(serde_json::json!(1000 + i)), async move {
                polled.store(true, Ordering::SeqCst);
                std::future::pending::<String>().await
            })
            .unwrap();
        }
        assert_eq!(set.queued(), MAX_QUEUED_OFF_LOOP_RPCS);

        // One more: an immediate JSON-RPC error carrying the request id. The
        // slow future is dropped, never awaited.
        let ran_inline = Arc::new(AtomicBool::new(false));
        let flag = ran_inline.clone();
        let err = set
            .submit(Some(serde_json::json!("over")), async move {
                flag.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(3600)).await;
                "slow".to_string()
            })
            .expect_err("saturated: must answer with an error, not run");
        let resp: JsonRpcResponse = serde_json::from_str(&err).unwrap();
        assert!(!resp.is_success());
        assert_eq!(resp.id, Some(serde_json::json!("over")));
        assert!(!ran_inline.load(Ordering::SeqCst));

        tokio::task::yield_now().await;
        assert!(
            !polled.load(Ordering::SeqCst),
            "queued work must not start early"
        );
        assert_eq!(set.len(), MAX_OFF_LOOP_RPCS);
        assert_eq!(set.queued(), MAX_QUEUED_OFF_LOOP_RPCS);
    }

    #[tokio::test]
    async fn a_completion_promotes_the_queue() {
        let mut set = OffLoopRpcs::new();
        for i in 0..MAX_OFF_LOOP_RPCS {
            set.submit(Some(serde_json::json!(i)), async move { format!("r{i}") })
                .unwrap();
        }
        set.submit(Some(serde_json::json!("q")), async { "queued".to_string() })
            .unwrap();
        assert_eq!(set.queued(), 1);

        let mut seen = Vec::new();
        while let Some(r) = tokio::time::timeout(Duration::from_secs(5), set.next_response())
            .await
            .expect("queued work must eventually run")
        {
            seen.push(r);
        }
        assert_eq!(seen.len(), MAX_OFF_LOOP_RPCS + 1);
        assert!(seen.iter().any(|r| r == "queued"));
        assert!(set.is_empty());
    }

    #[tokio::test]
    async fn a_panicking_dispatch_answers_internal_error_for_its_id() {
        let mut set = OffLoopRpcs::new();
        set.submit(Some(serde_json::json!(42)), async {
            panic!("boom");
        })
        .unwrap();
        let raw = set.next_response().await.expect("a response, not silence");
        let resp: JsonRpcResponse = serde_json::from_str(&raw).unwrap();
        assert!(!resp.is_success());
        assert_eq!(resp.id, Some(serde_json::json!(42)));
    }

    #[tokio::test]
    async fn shutdown_aborts_active_and_drops_queued_work() {
        let (active_tx, active_rx) = tokio::sync::oneshot::channel::<()>();
        let (queued_tx, queued_rx) = tokio::sync::oneshot::channel::<()>();
        let mut set = OffLoopRpcs::new();
        set.submit(None, async move {
            let _keep = active_tx;
            std::future::pending::<String>().await
        })
        .unwrap();
        for _ in 1..MAX_OFF_LOOP_RPCS {
            set.submit(None, std::future::pending::<String>()).unwrap();
        }
        set.submit(None, async move {
            let _keep = queued_tx;
            std::future::pending::<String>().await
        })
        .unwrap();
        assert_eq!(set.queued(), 1);

        assert!(set.shutdown().await, "pending tasks abort promptly");
        assert!(set.is_empty());
        // Captures of both the aborted and the never-started future are gone.
        tokio::time::timeout(Duration::from_secs(5), active_rx)
            .await
            .expect("aborted task must drop its captures")
            .expect_err("sender dropped without sending");
        tokio::time::timeout(Duration::from_secs(5), queued_rx)
            .await
            .expect("queued future must be dropped")
            .expect_err("sender dropped without sending");
    }

    /// D2 (Task #56): a task stuck in synchronous code cannot be aborted until
    /// it yields; teardown must not wait for it past the grace.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_is_bounded_when_a_task_is_stuck_in_sync_code() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let mut set = OffLoopRpcs::new();
        set.submit(None, async move {
            let _ = entered_tx.send(());
            // Deliberately pins this worker: models a sync section.
            std::thread::sleep(Duration::from_millis(1500));
            String::new()
        })
        .unwrap();
        entered_rx.await.unwrap();

        let started = std::time::Instant::now();
        let drained = set.shutdown_within(Duration::from_millis(100)).await;
        let waited = started.elapsed();
        assert!(!drained, "the stuck task cannot finish within the grace");
        assert!(set.is_empty(), "stuck handle is detached, not retained");
        assert!(
            waited < Duration::from_millis(1000),
            "teardown waited {waited:?}; must be bounded by the grace"
        );
    }

    /// Source guard: the live loop must actually route allowlisted RPCs through
    /// the off-loop set, drain it in its own select arm, never await a
    /// read-only future inline, and shut the set down before cleanup.
    #[test]
    fn the_connection_loop_uses_the_off_loop_set() {
        let src = include_str!("mod.rs");
        assert!(src.contains("off_loop::dispatches_off_loop(&req.method)"));
        assert!(src.contains("off_loop_rpcs.submit("));
        assert!(src.contains("off_loop_rpcs.next_response(), if !off_loop_rpcs.is_empty()"));
        assert!(
            !src.contains("Err(dispatch) => Some(dispatch.await)"),
            "a saturated off-loop set must never run the future inline"
        );
        let shutdown = src
            .find("off_loop_rpcs.shutdown().await")
            .expect("connection teardown must shut the off-loop set down");
        let cleanup = src.find("cleanup::run_cleanup(").expect("run_cleanup call");
        assert!(shutdown < cleanup, "shutdown must precede run_cleanup");
    }
}
