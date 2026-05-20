//! Autonomous team task dispatcher.
//!
//! Event-driven, in-process scheduler that drives a team's coordination-task
//! DAG to completion. When a task is created or completes, the dispatcher is
//! signalled; it then claims every now-runnable task and launches its owner
//! agent — no polling, no separate processes.
//!
//! This is a *dumb loop* (R10): it performs **no reasoning**. Task
//! decomposition, routing, and assignment are the leader LLM's job, expressed
//! by creating `coord_tasks` rows with `blocked_by` edges. The dispatcher only
//! mechanically executes what the DAG already says is runnable.

pub mod handoff;
pub mod runner;
pub mod schedule;

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, Semaphore};

use crate::agents::swarm::tasks::{CoordTaskId, CoordTaskStore};
use crate::gateway::context::GatewayContext;
use crate::sync_primitives::Arc;
use crate::teams::artifacts::ArtifactStore;
use crate::teams::context::TeamInboxContextProvider;
use crate::teams::store::TeamStore;

pub use runner::{execute_member_task, MemberRunOutcome, MemberRunStatus};
pub use schedule::{is_dispatcher_managed, MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};

/// Tunables for the dispatcher loop.
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// Maximum number of member tasks executing concurrently.
    pub max_concurrent: usize,
    /// A task lock older than this (seconds) is considered stale.
    pub lock_ttl_secs: u64,
    /// Per-task execution timeout (seconds).
    pub task_timeout_secs: u64,
    /// Fallback wake interval (seconds) — catches any missed signal.
    pub fallback_tick_secs: u64,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            lock_ttl_secs: 900,
            task_timeout_secs: 600,
            fallback_tick_secs: 60,
        }
    }
}

/// Autonomous DAG dispatcher for team coordination tasks.
pub struct TeamDispatcher {
    pub(crate) coord_store: Arc<dyn CoordTaskStore>,
    pub(crate) team_store: Arc<dyn TeamStore>,
    pub(crate) artifact_store: Option<Arc<dyn ArtifactStore>>,
    pub(crate) inbox_provider: Option<Arc<TeamInboxContextProvider>>,
    pub(crate) context: GatewayContext,
    pub(crate) config: DispatcherConfig,
    pub(crate) signal: Arc<Notify>,
    pub(crate) semaphore: Arc<Semaphore>,
    pub(crate) running: Arc<Mutex<HashSet<CoordTaskId>>>,
    /// Cooperative shutdown signal. Fires from `Self::shutdown()` and breaks
    /// the `spawn_loop` `tokio::select!` so the loop exits cleanly on
    /// graceful shutdown / signal handling. Without this the dispatcher
    /// loop only stops when the process dies, which leaks an
    /// unfinished `dispatch_once` task on every clean restart.
    pub(crate) shutdown_token: tokio_util::sync::CancellationToken,
}

impl TeamDispatcher {
    /// Construct a dispatcher. `signal` is shared with `task_create` so a newly
    /// created task wakes the loop immediately.
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        team_store: Arc<dyn TeamStore>,
        artifact_store: Option<Arc<dyn ArtifactStore>>,
        inbox_provider: Option<Arc<TeamInboxContextProvider>>,
        context: GatewayContext,
        config: DispatcherConfig,
        signal: Arc<Notify>,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        Self {
            coord_store,
            team_store,
            artifact_store,
            inbox_provider,
            context,
            config,
            signal,
            semaphore,
            running: Arc::new(Mutex::new(HashSet::new())),
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Wake the dispatch loop for one tick.
    pub fn signal(&self) {
        self.signal.notify_one();
    }

    /// Trigger cooperative shutdown of the loop spawned by
    /// [`Self::spawn_loop`]. The currently-running `dispatch_once` is allowed
    /// to finish; no new ticks are scheduled. Idempotent.
    pub fn shutdown(&self) {
        self.shutdown_token.cancel();
    }

    /// Spawn the background dispatch loop. Returns immediately.
    ///
    /// The loop blocks until signalled, until the fallback interval elapses,
    /// or until [`Self::shutdown`] fires the `shutdown_token`. Because
    /// `Notify` stores one permit, a signal raised while a tick is in flight
    /// is never lost.
    pub fn spawn_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            tracing::info!(
                max_concurrent = self.config.max_concurrent,
                "TeamDispatcher loop started"
            );
            // Reconcile orphaned in-progress tasks left by a prior process.
            self.dispatch_once().await;
            loop {
                tokio::select! {
                    _ = self.shutdown_token.cancelled() => {
                        tracing::info!("TeamDispatcher loop received shutdown signal — exiting");
                        return;
                    }
                    _ = self.signal.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(self.config.fallback_tick_secs)) => {}
                }
                self.dispatch_once().await;
            }
        });
    }
}
