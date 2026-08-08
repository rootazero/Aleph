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

pub mod acp_bridge;
pub mod clarify;
pub mod handoff;
pub mod runner;
pub mod schedule;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::{Mutex, Notify, Semaphore};

use crate::agents::swarm::tasks::{CoordTaskId, CoordTaskStore};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::context::GatewayContext;
use crate::sync_primitives::Arc;
use crate::teams::artifacts::ArtifactStore;
use crate::teams::context::TeamInboxContextProvider;
use crate::teams::store::TeamStore;

pub use acp_bridge::{AcpMemberRef, ACP_MEMBER_PREFIX};
pub use runner::{execute_member_task, MemberDispatchTarget, MemberRunOutcome, MemberRunStatus};
pub use schedule::{is_dispatcher_managed, MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};

/// How many times a task may be recovered from a **crash** (its worker
/// vanished mid-run and `reclaim_orphaned` looped it back to `Pending`)
/// before the dispatcher gives up and fails it terminally.
///
/// Distinct from [`DispatcherConfig::default_max_retries`], which bounds
/// *clean* attempt failures. A crash orphan deliberately spends no retry
/// budget, and `reclaim_orphaned` re-stamps `started_at` on every
/// re-dispatch — so `zombie_ttl_secs` can never see the age accumulate and a
/// task that reliably kills its worker was re-dispatched without any bound at
/// all. Counted by
/// [`recovery_abandons_since`](crate::agents::swarm::tasks::retry::recovery_abandons_since),
/// which shares the `retry_budget_reset_at` anchor, so an operator hard-retry
/// re-arms this ceiling exactly as it re-arms the retry ladder.
///
/// `2` mirrors the reference implementation's `MAX_RECOVERIES`: two free
/// recoveries, terminal on the third crash. `0` fails the first crash orphan.
///
/// Deliberately a constant rather than a [`DispatcherConfig`] field: the boot
/// site builds that struct with an exhaustive literal in the `aleph-server`
/// bin crate, so a new field cannot be added from here without editing it.
/// See the parked follow-up in the round report.
pub const MAX_TASK_RECOVERIES: u32 = 2;

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
    /// A task that has been `InProgress` for longer than this (seconds) and is
    /// not running in this process is declared a **zombie** and force-failed.
    /// Acts as a safety net for the rare window where a worker process dies
    /// after the `run_task` spawn but before the outcome-recording code runs —
    /// in that case `reclaim_orphaned` would loop it back to Pending forever.
    ///
    /// Inspired by `ClawTeam`'s `list_zombie_agents(max_hours=2.0)`. Default
    /// is generous (2 hours) — set lower for short-task workloads, but never
    /// below `task_timeout_secs` or healthy long-running tasks get clobbered.
    pub zombie_ttl_secs: u64,
    /// Maximum concurrent tasks a *single owner* (agent / ACP member) may hold
    /// at once. `0` disables the hard cap.
    ///
    /// Independent of [`Self::max_concurrent`], which bounds the **process**.
    /// This bounds each **agent** so one owner cannot monopolise the whole
    /// concurrency pool and starve other team members — the multi-agent
    /// fairness guarantee. Maps to openclaw's per-lane `maxConcurrent` and
    /// hermes' per-source limit, where each owner is a lane.
    ///
    /// Note: even with the hard cap disabled (`0`), the scheduler still applies
    /// **load-balanced round-robin** across owners so freed slots prefer the
    /// least-busy agent — see [`select_schedulable`](super::schedule::select_schedulable).
    pub max_per_owner: usize,
    /// Default number of times a task whose attempt fails (cleanly `Failed` or
    /// `Timeout`) is automatically re-dispatched before being marked terminally
    /// `Failed`. A per-task `max_retries` in the task metadata overrides this.
    ///
    /// On a retry the task is reset to `Pending`; the next tick re-claims it and
    /// the hand-off builder injects the prior attempts as recovery context, so
    /// the resuming member resumes instead of cold-starting (R9 — *how* to
    /// recover lives in the prompt; this is only the mechanical ceiling).
    ///
    /// `0` reproduces the pre-enhancement behaviour (first failure is terminal).
    /// Default `2` = up to **3 total attempts**, mirroring `ClawTeam`'s bounded
    /// retry. Zombies (a worker that vanished, caught by [`Self::zombie_ttl_secs`])
    /// never retry — they have already exhausted their budget.
    pub default_max_retries: u32,
    /// Base delay (seconds) for **exponential retry backoff**. The delay before
    /// the Nth retry is `base * 2^(N-1)`, capped at [`Self::retry_backoff_cap_secs`]
    /// (see [`backoff_secs`](crate::agents::swarm::tasks::retry::backoff_secs)).
    ///
    /// Without backoff a reset task is re-claimed on the very next tick and a
    /// transient failure (rate-limit / overload) spends the whole retry budget
    /// in milliseconds. `0` disables backoff (immediate re-dispatch — the
    /// pre-enhancement behaviour). Default `5`.
    pub retry_backoff_base_secs: u64,
    /// Upper bound (seconds) on a single retry's backoff delay, so exponential
    /// growth cannot push a re-dispatch arbitrarily far out. Default `120`.
    pub retry_backoff_cap_secs: u64,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            lock_ttl_secs: 900,
            task_timeout_secs: 600,
            fallback_tick_secs: 60,
            zombie_ttl_secs: 7200, // 2 hours — same threshold ClawTeam uses
            // 0 = no hard per-owner cap; load-balanced round-robin still
            // spreads slots across owners regardless of this value.
            max_per_owner: 0,
            // Up to 3 total attempts (initial + 2 retries) before FailedFinal.
            default_max_retries: 2,
            // Exponential backoff: 5s, 10s, 20s, … capped at 2 minutes. Spaces
            // retries so a transient provider error can clear before re-attempt.
            retry_backoff_base_secs: 5,
            retry_backoff_cap_secs: 120,
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
    /// Tasks this process is actively running, mapped to their **owner**
    /// (agent / ACP member id). The owner value powers in-flight-aware
    /// per-owner fair scheduling in [`select_schedulable`](schedule::select_schedulable);
    /// the keys alone preserve the original "don't re-claim a running task"
    /// semantics for reclamation.
    pub(crate) running: Arc<Mutex<HashMap<CoordTaskId, String>>>,
    /// Workflow run ids whose terminal origin-channel notification has been
    /// attempted this process (see `schedule::settle`). In-memory claim for
    /// the send window; the durable `workflow_notified` metadata marker makes
    /// delivery once-only across restarts. Pruned each sweep to runs still
    /// live-and-unnotified so it stays bounded.
    pub(crate) notified_workflow_runs: Arc<Mutex<HashSet<String>>>,
    /// Channel registry used to reach a workflow run's originating channel:
    /// delivers a `clarify` step's question, and pushes a settled run's
    /// terminal summary (`schedule::settle`). Held as a `OnceCell` because the
    /// registry is injected after the dispatcher is constructed (the same
    /// deferred-injection the tool registry uses). Unset / unfilled → clarify
    /// steps fail with a clear reason rather than stalling the DAG, and the
    /// settle sweep stays silent.
    pub(crate) channels: Option<Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>>,
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
            running: Arc::new(Mutex::new(HashMap::new())),
            notified_workflow_runs: Arc::new(Mutex::new(HashSet::new())),
            channels: None,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Wire the channel-registry cell so workflow `clarify` steps can deliver
    /// their question to the user's originating channel once the registry is
    /// injected. Without it, clarify steps fail fast (the rest of the dispatcher
    /// is unaffected).
    pub fn with_clarify_delivery(
        mut self,
        channels: Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>>,
    ) -> Self {
        self.channels = Some(channels);
        self
    }

    /// Wake the dispatch loop for one tick.
    pub fn signal(&self) {
        self.signal.notify_one();
    }

    /// Subscribe the dispatch loop to coordination-task transition events on
    /// the `GlobalBus`, so ANY status write — builtin tool, panel RPC, review
    /// verdict (`workflow_step_review` approve/retry/skip), admin control
    /// (`team_task_control` resume/retry), clarify answer — wakes the loop
    /// immediately. Before this, only the two tools that held the raw signal
    /// (`task_create`, `workflow run`) were event-driven; every other
    /// unblocking transition waited for the fallback tick (up to 60s of dead
    /// time per review verdict in a gated pipeline), contradicting the module
    /// doc's "no polling" promise. An idle wake is a cheap no-op tick, so no
    /// filtering by transition kind is needed (R10 — mechanical, no
    /// judgement). Call once after [`Self::spawn_loop`]; the subscription
    /// lives for the process lifetime (the bus holds it, no RAII guard).
    pub fn subscribe_task_events(self: &Arc<Self>) {
        use crate::event::{EventFilter, EventType, GlobalBus};
        let signal = Arc::clone(&self.signal);
        tokio::spawn(async move {
            GlobalBus::global()
                .subscribe_async(
                    EventFilter::new(vec![
                        EventType::TeamTaskAssigned,
                        EventType::TeamTaskUpdated,
                        EventType::TeamTaskCompleted,
                        EventType::TeamTaskFailed,
                    ]),
                    move |_event| {
                        signal.notify_one();
                    },
                )
                .await;
            tracing::info!(
                "TeamDispatcher subscribed to task transition events (event-driven wake)"
            );
        });
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
