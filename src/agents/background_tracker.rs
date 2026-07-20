//! `BackgroundAgentTracker` — tracks sub-agents running in background tokio tasks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::agents::progress::{ProgressKind, SubagentProgress};
use crate::agents::subagent_tree_events;
use crate::sync_primitives::RwLock;
use aleph_protocol::subagent_tree::{NodeLifecycle, SubagentNode};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// C1 — completed background results older than this are pruned opportunistically
/// on each new `register()`.
const BACKGROUND_RESULT_TTL: Duration = Duration::from_secs(3600);

/// C1 follow-up — hard count cap on retained completed results. The TTL prune
/// only fires from `register_with_meta`, so a burst of background spawns that
/// then goes idle would otherwise keep every completed entry alive past its
/// TTL forever (nothing re-triggers `cleanup`). `mark_completed` enforces this
/// cap on every insert, evicting oldest-by-completion, so the map is bounded by
/// count regardless of spawn cadence. Generous: each entry is a small struct,
/// and a parent rarely needs to poll more than a handful of recent results.
const MAX_COMPLETED_RESULTS: usize = 256;

/// B18 — how many of the running entry's most-recent progress events survive
/// into the completed entry. The live FIFO holds 50; a completed agent only
/// needs enough trail for the parent (and the model) to see *what the child was
/// doing when it died*, so we keep the tail and drop the rest. Bounded on
/// purpose: `completed` is capped at `MAX_COMPLETED_RESULTS` entries and every
/// one of them now carries this many small structs.
const PROGRESS_TAIL_LEN: usize = 10;

/// Process-global tracker. Every `ExecutionEngine` and the `subagent.tree` RPC
/// share this one instance, so a panel sees every background sub-agent
/// regardless of which run spawned it (the tracker was always documented as
/// "engine-lifetime, process-global"; this makes it literally so).
static GLOBAL_TRACKER: OnceLock<Arc<BackgroundAgentTracker>> = OnceLock::new();

/// Wall-clock unix milliseconds, saturating on clock errors.
fn now_unix_ms() -> u64 {
    subagent_tree_events::now_ms()
}

/// Tree metadata captured at spawn time, folded into the tracker node so
/// `flat_nodes` can rebuild the live sub-agent tree. Kept separate from the
/// back-compat `register` args (request_id / cancel / task) so the common test
/// path stays a 3-arg call.
#[derive(Debug, Clone, Default)]
pub struct SpawnMeta {
    /// Immediate parent node id. `None` = attaches under the session root (the
    /// structurally-common depth-1 case; see `subagent_tree` docs).
    pub parent_id: Option<String>,
    /// `ChainContext.depth` at spawn (1 = direct child of root).
    pub depth: u32,
    /// Owning top-level session key — the tree this node belongs to.
    pub root_session: String,
    /// Resolved model id, when known.
    pub model: Option<String>,
}

/// Snake_case activity label for a progress kind — fed to `SubagentNode.last_activity`
/// and the live `Progress` tree event. `pub(crate)` so `forwarding_trace_sink`
/// reuses the same labels (single source).
pub(crate) const fn progress_activity(kind: ProgressKind) -> &'static str {
    match kind {
        ProgressKind::ToolCalled => "tool_called",
        ProgressKind::ToolReturned => "tool_returned",
        ProgressKind::LlmThinking => "llm_thinking",
        ProgressKind::Cancelled => "cancelled",
    }
}

/// Map a terminal outcome + failure message to a typed lifecycle. The wall-clock
/// timeout prefix is matched exactly (mirrors `runtime.rs`) so a wrapped inner
/// "connection timed out" is not misclassified as a hard timeout.
///
/// Cancellation is classified by the **exact** producer form coming out of
/// `subagent_spawner::spawn`, which wraps `HarnessError::Cancelled`
/// (`#[error("cancelled")]`, lowercase) as `"sub-agent failed: cancelled"`.
/// Any looser substring match (e.g. a tool message that happens to mention
/// "cancel") must NOT be classified as Cancelled — it is a plain failure.
/// Single source for both the stored completed lifecycle (cold-start
/// `flat_nodes`) and the live `Settled` tree event — `spawn.rs` reuses it.
pub(crate) fn lifecycle_from_outcome(outcome: &CompletedOutcome) -> NodeLifecycle {
    match outcome {
        CompletedOutcome::Ok { .. } => NodeLifecycle::Completed,
        CompletedOutcome::Err(msg) => {
            if msg.starts_with("Sub-agent timed out") {
                NodeLifecycle::TimedOut
            } else if msg == "sub-agent failed: cancelled" {
                NodeLifecycle::Cancelled
            } else {
                NodeLifecycle::Failed
            }
        }
    }
}

pub struct BackgroundAgentTracker {
    running: RwLock<HashMap<String, RunningAgent>>,
    completed: RwLock<HashMap<String, CompletedAgent>>,
    /// Fires once on every transition into `completed` (see `mark_completed`)
    /// so [`wait`](Self::wait) can park until *something* finishes instead of
    /// forcing the parent model to spend an LLM turn per `check_status` poll.
    /// Coarse-grained on purpose — every waiter wakes and re-checks its own
    /// `request_id` (the running set is small) — mirroring the proven
    /// `builtin_tools::process_registry` completion notifier so a single shared
    /// signal is cheaper than one channel per background agent.
    completion: Notify,
}

struct RunningAgent {
    cancel_token: CancellationToken,
    task_description: String,
    started_at: Instant,
    /// FIFO-capped progress events; capacity 50.
    progress: VecDeque<SubagentProgress>,
    /// Tree metadata captured at spawn.
    meta: SpawnMeta,
    /// Wall-clock spawn time (unix ms) for stable start-order sorting.
    started_at_ms: u64,
    /// Running tally of tool calls (incremented on each `ToolCalled`).
    tool_count: u32,
    /// Most recent tool name seen.
    last_tool: Option<String>,
    /// Most recent activity signal (progress kind, snake_case).
    last_activity: Option<String>,
}

struct CompletedAgent {
    outcome: CompletedOutcome,
    completed_at: Instant,
    /// Wall-clock seconds the run took (registration → completion).
    duration_secs: u64,
    /// Task description carried over from the running entry so `list` and
    /// `result_snapshot` can still name a finished agent.
    task_description: String,
    /// Tree metadata carried over from the running entry.
    meta: SpawnMeta,
    /// Typed terminal lifecycle derived from `outcome`.
    lifecycle: NodeLifecycle,
    /// Wall-clock spawn time (unix ms), carried over for stable sorting.
    started_at_ms: u64,
    /// Final tool tally carried over from the running entry.
    tool_count: u32,
    /// Final tool name seen, carried over.
    last_tool: Option<String>,
    /// Final activity signal, carried over.
    last_activity: Option<String>,
    /// B18 — the tail of the running entry's progress queue. Without this the
    /// whole trajectory died with the `RunningAgent`, so a *failed* background
    /// exploration reached the parent as a bare error string with no evidence of
    /// what was attempted. Negative results are kept deliberately.
    progress_tail: Vec<SubagentProgress>,
    /// Whether the parent already saw this result on-demand (via a `wait` or a
    /// `check_status` that returned the completed outcome). Set by
    /// [`mark_consumed`](BackgroundAgentTracker::mark_consumed) and read by
    /// `subagent_announce` so the proactive R5 announce does not re-deliver a
    /// result the model has already folded into its reasoning. Lives and dies
    /// with the completed entry, so it needs no separate pruning.
    consumed: bool,
}

/// Terminal outcome of a background subagent run.
///
/// `Ok` mirrors the foreground spawn path's `{result, iterations,
/// tool_calls_made}` response shape so `check_status` of a finished
/// background agent surfaces the same metrics regardless of which path
/// produced it. `Err` carries the failure message.
#[derive(Debug, Clone)]
pub enum CompletedOutcome {
    Ok {
        final_text: String,
        iterations: usize,
        tool_calls_made: usize,
        total_tokens: usize,
    },
    Err(String),
}

impl CompletedOutcome {
    /// Convenience constructor for a text-only success (tests, and callers
    /// with no run metrics to report).
    pub fn ok_text(text: impl Into<String>) -> Self {
        Self::Ok {
            final_text: text.into(),
            iterations: 0,
            tool_calls_made: 0,
            total_tokens: 0,
        }
    }
}

/// Non-destructive view of a finished background subagent.
#[derive(Debug, Clone)]
pub struct CompletedSnapshot {
    pub task: String,
    pub duration_secs: u64,
    pub outcome: CompletedOutcome,
    /// B18 — up to `PROGRESS_TAIL_LEN` final progress events, chronological.
    /// The failure path folds these into the error string it hands the model.
    pub progress_tail: Vec<SubagentProgress>,
}

/// Lightweight metadata for a still-running background subagent.
#[derive(Debug, Clone)]
pub struct RunningMeta {
    pub elapsed_secs: u64,
    pub task: String,
}

/// Outcome of a [`wait`](BackgroundAgentTracker::wait) on a single background
/// subagent. `Completed` carries the same non-destructive snapshot a poll
/// would return; `TimedOut` means the bounded wait window closed with the
/// agent still running (the caller may `wait` again); `NotFound` means the
/// `request_id` is unknown (never registered, or TTL-pruned).
#[derive(Debug, Clone)]
pub enum WaitOutcome {
    Completed(CompletedSnapshot),
    TimedOut { elapsed_secs: u64 },
    NotFound,
}

/// Outcome of a [`wait_any`](BackgroundAgentTracker::wait_any) over a SET of
/// background subagents — the fan-out "wait for whichever finishes first"
/// primitive (codex `wait_agent` parity). `Completed` names the first id in the
/// set to finish; draining the rest is the caller's job (drop it and
/// `wait_any` again). `TimedOut` lists the ids still running when the window
/// closed. `NotFound` means none of the ids is known (all unregistered /
/// TTL-pruned).
#[derive(Debug, Clone)]
pub enum WaitAnyOutcome {
    Completed {
        request_id: String,
        snapshot: CompletedSnapshot,
    },
    TimedOut {
        still_running: Vec<String>,
    },
    NotFound,
}

impl BackgroundAgentTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            running: RwLock::new(HashMap::new()),
            completed: RwLock::new(HashMap::new()),
            completion: Notify::new(),
        }
    }

    /// The process-global tracker instance (lazily created). Shared by every
    /// `ExecutionEngine` and the `subagent.tree` RPC so the live tree the panel
    /// reads is the same one the spawn path writes.
    #[must_use]
    pub fn global() -> Arc<BackgroundAgentTracker> {
        GLOBAL_TRACKER
            .get_or_init(|| Arc::new(BackgroundAgentTracker::new()))
            .clone()
    }

    /// Register a new background agent (back-compat 3-arg path; no tree
    /// metadata). Equivalent to `register_with_meta` with a default `SpawnMeta`.
    pub fn register(
        &self,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
    ) {
        self.register_with_meta(
            request_id,
            cancel_token,
            task_description,
            SpawnMeta::default(),
        );
    }

    /// Register a new background agent with tree metadata (parent/depth/root/
    /// model) so `flat_nodes` can reconstruct the sub-agent tree.
    pub fn register_with_meta(
        &self,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
        meta: SpawnMeta,
    ) {
        let mut running = self.running.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        running.insert(
            request_id,
            RunningAgent {
                cancel_token,
                task_description,
                started_at: Instant::now(),
                progress: VecDeque::with_capacity(50),
                meta,
                started_at_ms: now_unix_ms(),
                tool_count: 0,
                last_tool: None,
                last_activity: None,
            },
        );
        drop(running);
        self.cleanup(BACKGROUND_RESULT_TTL);
    }

    /// Mark a background agent as finished and store its outcome.
    ///
    /// The entry stays queryable **non-destructively** until pruned by the
    /// TTL in `cleanup`, so a parent may poll `check_status` for the same
    /// `request_id` more than once without the result vanishing.
    pub fn mark_completed(&self, request_id: &str, outcome: CompletedOutcome) {
        let now = Instant::now();
        // Snapshot the running entry's carry-over fields via a *read* borrow —
        // without removing it yet. The completed entry is inserted BEFORE the
        // running entry is removed (below), so the `request_id` is present in
        // `completed` for the entire transition and a concurrent `wait` /
        // `check_status` can never observe it absent from *both* maps — the
        // window that would otherwise read as a spurious NotFound. The brief
        // double-presence is harmless: every completed-first reader returns the
        // finished result and `flat_nodes` de-dupes by id.
        let (
            duration_secs,
            task_description,
            meta,
            started_at_ms,
            tool_count,
            last_tool,
            last_activity,
            progress_tail,
        ) = {
            let running = self.running.read().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            match running.get(request_id) {
                Some(agent) => {
                    // B18 — keep the last `PROGRESS_TAIL_LEN` events; the rest of
                    // the FIFO is dropped with the running entry as before.
                    let skip = agent.progress.len().saturating_sub(PROGRESS_TAIL_LEN);
                    let tail: Vec<SubagentProgress> =
                        agent.progress.iter().skip(skip).cloned().collect();
                    (
                        now.duration_since(agent.started_at).as_secs(),
                        agent.task_description.clone(),
                        agent.meta.clone(),
                        agent.started_at_ms,
                        agent.tool_count,
                        agent.last_tool.clone(),
                        agent.last_activity.clone(),
                        tail,
                    )
                }
                None => (
                    0,
                    String::new(),
                    SpawnMeta::default(),
                    now_unix_ms(),
                    0,
                    None,
                    None,
                    Vec::new(),
                ),
            }
        };
        let lifecycle = lifecycle_from_outcome(&outcome);
        {
            let mut completed = self.completed.write().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            completed.insert(
                request_id.to_string(),
                CompletedAgent {
                    outcome,
                    completed_at: now,
                    duration_secs,
                    task_description,
                    meta,
                    lifecycle,
                    started_at_ms,
                    tool_count,
                    last_tool,
                    last_activity,
                    progress_tail,
                    consumed: false,
                },
            );
            // C1 follow-up — bound the map by count. `mark_completed` is the only
            // site that grows `completed` and it has no TTL-prune trigger of its
            // own, so without this a long-lived process that spawns many background
            // subagents and then idles would retain results indefinitely. Evict the
            // oldest-by-completion entries beyond the cap while we still hold the
            // write lock (cheap: only sorts when actually over the cap).
            if completed.len() > MAX_COMPLETED_RESULTS {
                let overflow = completed.len() - MAX_COMPLETED_RESULTS;
                let mut by_age: Vec<(String, Instant)> = completed
                    .iter()
                    .map(|(id, agent)| (id.clone(), agent.completed_at))
                    .collect();
                by_age.sort_by_key(|(_, at)| *at);
                for (id, _) in by_age.into_iter().take(overflow) {
                    completed.remove(&id);
                }
            }
        }
        // The result is now queryable in `completed`; drop the running entry
        // (the id was never absent from both maps) and wake any `wait`ers so
        // they re-check and return the completed snapshot immediately.
        self.running
            .write()
            .unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            })
            .remove(request_id);
        self.completion.notify_waiters();
    }

    /// Cancel a running background agent. Returns `true` if the `request_id`
    /// was found in the running set and the `CancellationToken` was hit;
    /// `false` if no such running agent exists (already completed / never
    /// registered). The cooperative cancellation still relies on the
    /// running task observing the token at the next await point.
    pub fn cancel(&self, request_id: &str) -> bool {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        if let Some(agent) = running.get(request_id) {
            agent.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    /// Non-destructively read a finished agent's outcome. Returns `None`
    /// when the `request_id` was never registered or has been TTL-pruned.
    /// Unlike a consume, repeated polls return the same snapshot — this is
    /// what lets a parent re-check a completed subagent.
    pub fn result_snapshot(&self, request_id: &str) -> Option<CompletedSnapshot> {
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed.get(request_id).map(|c| CompletedSnapshot {
            task: c.task_description.clone(),
            duration_secs: c.duration_secs,
            outcome: c.outcome.clone(),
            progress_tail: c.progress_tail.clone(),
        })
    }

    /// Park until background subagent `request_id` finishes, or until `timeout`
    /// elapses — whichever comes first. Unlike a `check_status` poll (which
    /// costs the parent a full LLM turn per check) this sleeps on the
    /// [`completion`](Self::completion) notifier and only re-checks when *some*
    /// agent finishes, so it burns no CPU while waiting and returns the result
    /// the instant it lands. Mirrors `builtin_tools::process_registry::wait`.
    ///
    /// Returns [`WaitOutcome::Completed`] with the same non-destructive snapshot
    /// a poll gives (and marks the result consumed so the proactive announce
    /// does not re-deliver it), [`WaitOutcome::TimedOut`] when the window closes
    /// with the agent still running, or [`WaitOutcome::NotFound`] for an unknown
    /// / TTL-pruned id. Thin wrapper over [`wait_any`](Self::wait_any).
    pub async fn wait(&self, request_id: &str, timeout: Duration) -> WaitOutcome {
        let ids = [request_id.to_string()];
        match self.wait_any(&ids, timeout).await {
            WaitAnyOutcome::Completed { snapshot, .. } => WaitOutcome::Completed(snapshot),
            WaitAnyOutcome::TimedOut { .. } => WaitOutcome::TimedOut {
                elapsed_secs: self
                    .running_meta(request_id)
                    .map(|m| m.elapsed_secs)
                    .unwrap_or(0),
            },
            WaitAnyOutcome::NotFound => WaitOutcome::NotFound,
        }
    }

    /// Park until *any* subagent in `request_ids` finishes, or until `timeout`
    /// elapses — the fan-out first-completion primitive (codex `wait_agent`
    /// parity). Sleeps on the shared [`completion`](Self::completion) notifier
    /// (which wakes on every completion) and re-checks the whole set, so it
    /// costs no CPU while waiting and returns the instant the first result
    /// lands. The first-finished result is marked consumed so the announce does
    /// not re-deliver it; the caller drains the rest by dropping that id and
    /// calling `wait_any` again.
    pub async fn wait_any(&self, request_ids: &[String], timeout: Duration) -> WaitAnyOutcome {
        let deadline = Instant::now() + timeout;
        loop {
            // Arm the notifier BEFORE inspecting state: `Notified::enable`
            // registers this waiter so a `mark_completed` racing between our
            // state read and our await still wakes us (no lost wakeup).
            let notified = self.completion.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // First completed in the set wins. `mark_completed` inserts into
            // `completed` before removing from `running`, so a finished agent is
            // always visible here and never mistaken for absent.
            let mut any_running = false;
            for id in request_ids {
                if let Some(snapshot) = self.result_snapshot(id) {
                    self.mark_consumed(id);
                    return WaitAnyOutcome::Completed {
                        request_id: id.clone(),
                        snapshot,
                    };
                }
                if self.running_meta(id).is_some() {
                    any_running = true;
                }
            }
            // None completed and none running ⇒ every id is unknown.
            if !any_running {
                return WaitAnyOutcome::NotFound;
            }
            let now = Instant::now();
            if now >= deadline {
                return WaitAnyOutcome::TimedOut {
                    still_running: self.still_running_ids(request_ids),
                };
            }
            let remaining = deadline - now;
            tokio::select! {
                () = &mut notified => { /* something finished — re-check the set */ }
                () = tokio::time::sleep(remaining) => {
                    return WaitAnyOutcome::TimedOut {
                        still_running: self.still_running_ids(request_ids),
                    };
                }
            }
        }
    }

    /// The subset of `request_ids` still in the running set — the `wait_any`
    /// timeout arm reports these so the caller knows which to keep waiting on.
    fn still_running_ids(&self, request_ids: &[String]) -> Vec<String> {
        request_ids
            .iter()
            .filter(|id| self.running_meta(id).is_some())
            .cloned()
            .collect()
    }

    /// Mark a completed result as already delivered to the parent on-demand
    /// (via `wait`, or a `check_status` that returned the outcome) so the
    /// proactive `subagent_announce` does not spend a fresh parent turn
    /// re-announcing a result the model has already seen. No-op for an unknown
    /// or still-running id.
    pub fn mark_consumed(&self, request_id: &str) {
        let mut completed = self.completed.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        if let Some(agent) = completed.get_mut(request_id) {
            agent.consumed = true;
        }
    }

    /// Whether a completed result was already consumed on-demand. `false` for
    /// unknown / still-running ids (nothing to suppress).
    #[must_use]
    pub fn is_consumed(&self, request_id: &str) -> bool {
        self.completed
            .read()
            .unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            })
            .get(request_id)
            .is_some_and(|c| c.consumed)
    }

    /// List running agents as (`request_id`, `task_description`, `elapsed_secs`).
    pub fn list_running(&self) -> Vec<(String, String, u64)> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        running
            .iter()
            .map(|(id, agent)| {
                (
                    id.clone(),
                    agent.task_description.clone(),
                    agent.started_at.elapsed().as_secs(),
                )
            })
            .collect()
    }

    /// Whether any sub-agent is currently running under `root_session` (the
    /// owning top-level session key, `SessionKey::to_key_string()`). O(running)
    /// scan of the live set.
    ///
    /// Backs the gateway's `Interrupt` busy-input guard: a run driving a live
    /// sub-agent fan-out (team dispatch / parallel subagents) has expensive
    /// in-flight parallel work that a single mid-task course-correction must
    /// not destroy. When this returns `true` the Interrupt is demoted to the
    /// busy queue (wait for the current run to finish) instead of cancelling
    /// the sibling — hermes `run.py:5436-5446` demote-to-queue parity.
    pub fn session_has_running(&self, root_session: &str) -> bool {
        self.running
            .read()
            .unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            })
            .values()
            .any(|agent| agent.meta.root_session == root_session)
    }

    /// Lightweight metadata for one still-running agent. `None` when the
    /// `request_id` is unknown (never registered, or already completed).
    pub fn running_meta(&self, request_id: &str) -> Option<RunningMeta> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        running.get(request_id).map(|agent| RunningMeta {
            elapsed_secs: agent.started_at.elapsed().as_secs(),
            task: agent.task_description.clone(),
        })
    }

    /// Snapshot every finished agent as `(request_id, CompletedSnapshot)`.
    /// Backs the `list` action's view of results still retrievable by the
    /// parent. Bounded by the TTL prune in `cleanup`.
    pub fn all_completed(&self) -> Vec<(String, CompletedSnapshot)> {
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed
            .iter()
            .map(|(id, c)| {
                (
                    id.clone(),
                    CompletedSnapshot {
                        task: c.task_description.clone(),
                        duration_secs: c.duration_secs,
                        outcome: c.outcome.clone(),
                        progress_tail: c.progress_tail.clone(),
                    },
                )
            })
            .collect()
    }

    /// Snapshot every tracked sub-agent (running + completed) as flat protocol
    /// nodes, optionally filtered to one `root_session`. Backs the
    /// `subagent.tree` RPC: the gateway runs `aleph_protocol::build_tree` over
    /// this to produce the hierarchy. Bounded by the TTL prune on completed
    /// entries.
    #[must_use]
    pub fn flat_nodes(&self, root_session: Option<&str>) -> Vec<SubagentNode> {
        let mut out = Vec::new();
        // Completed first, recording every completed id: `mark_completed` briefly
        // holds a node in BOTH maps (it inserts into `completed` before removing
        // from `running`), so a running entry whose id is already completed must
        // be skipped below to avoid emitting a duplicate `node_id` into
        // `build_tree`.
        let completed_ids: HashSet<String> = {
            let completed = self.completed.read().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            for (id, agent) in completed.iter() {
                if root_session.is_some_and(|f| agent.meta.root_session != f) {
                    continue;
                }
                out.push(SubagentNode {
                    node_id: id.clone(),
                    parent_id: agent.meta.parent_id.clone(),
                    depth: agent.meta.depth,
                    root_session: agent.meta.root_session.clone(),
                    task: agent.task_description.clone(),
                    model: agent.meta.model.clone(),
                    lifecycle: agent.lifecycle,
                    started_at_ms: agent.started_at_ms,
                    elapsed_ms: agent.duration_secs.saturating_mul(1000),
                    tool_count: agent.tool_count,
                    last_tool: agent.last_tool.clone(),
                    last_activity: agent.last_activity.clone(),
                });
            }
            completed.keys().cloned().collect()
        };
        {
            let running = self.running.read().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            for (id, agent) in running.iter() {
                // Skip an id that is already emitted as completed (transition
                // double-presence) or filtered out by `root_session`.
                if completed_ids.contains(id) {
                    continue;
                }
                if root_session.is_some_and(|f| agent.meta.root_session != f) {
                    continue;
                }
                out.push(SubagentNode {
                    node_id: id.clone(),
                    parent_id: agent.meta.parent_id.clone(),
                    depth: agent.meta.depth,
                    root_session: agent.meta.root_session.clone(),
                    task: agent.task_description.clone(),
                    model: agent.meta.model.clone(),
                    lifecycle: NodeLifecycle::Running,
                    started_at_ms: agent.started_at_ms,
                    elapsed_ms: u64::try_from(agent.started_at.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                    tool_count: agent.tool_count,
                    last_tool: agent.last_tool.clone(),
                    last_activity: agent.last_activity.clone(),
                });
            }
        }
        out
    }

    /// Append a progress event to the running agent's queue.
    /// Capped at 50 events FIFO. Returns the node's updated `tool_count` when
    /// the `request_id` is a live agent, or `None` if unknown (race condition:
    /// tracker may have moved the entry to completed). The return lets the
    /// `ForwardingTraceSink` emit a live `Progress` tree event without a second
    /// lock acquisition.
    pub fn push_progress(&self, request_id: &str, event: SubagentProgress) -> Option<u32> {
        let mut running = self.running.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        let agent = running.get_mut(request_id)?;
        // Fold the event into the node's live tallies before storing it, so
        // `flat_nodes` reflects tool_count / last_tool without scanning the
        // FIFO buffer.
        if event.kind == ProgressKind::ToolCalled {
            agent.tool_count = agent.tool_count.saturating_add(1);
        }
        if let Some(tool) = &event.tool_name {
            agent.last_tool = Some(tool.clone());
        }
        agent.last_activity = Some(progress_activity(event.kind).to_string());
        if agent.progress.len() >= 50 {
            agent.progress.pop_front();
        }
        let tool_count = agent.tool_count;
        agent.progress.push_back(event);
        Some(tool_count)
    }

    /// Return up to `limit` most-recent progress events (chronological order).
    ///
    /// D7 — a finished agent falls back to the tail carried into the completed
    /// entry (`PROGRESS_TAIL_LEN` events). Before that fallback existed this
    /// returned an empty Vec the instant `mark_completed` ran, which is exactly
    /// when a parent polls a *failed* child. Empty Vec only for a genuinely
    /// unknown `request_id` (never registered, or TTL-pruned).
    pub fn progress_snapshot(&self, request_id: &str, limit: usize) -> Vec<SubagentProgress> {
        let live = {
            let running = self.running.read().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            running.get(request_id).map(|agent| {
                let start = agent.progress.len().saturating_sub(limit);
                agent
                    .progress
                    .iter()
                    .skip(start)
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        if let Some(events) = live {
            return events;
        }
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed
            .get(request_id)
            .map(|c| {
                let start = c.progress_tail.len().saturating_sub(limit);
                c.progress_tail
                    .get(start..)
                    .expect("invariant: start is within progress_tail bounds")
                    .to_vec()
            })
            .unwrap_or_default()
    }

    /// Remove completed entries older than `ttl`.
    pub fn cleanup(&self, ttl: Duration) {
        let mut completed = self.completed.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        completed.retain(|_, agent| agent.completed_at.elapsed() < ttl);
    }
}

impl Default for BackgroundAgentTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_list() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        let running = tracker.list_running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].0, "req-1");
    }

    #[test]
    fn session_has_running_matches_by_root_session() {
        let tracker = BackgroundAgentTracker::new();
        let root = "agent:parent-007:peer:user"; // SessionKey::to_key_string() form
        tracker.register_with_meta(
            "child-1".to_string(),
            CancellationToken::new(),
            "fan-out task".to_string(),
            SpawnMeta {
                root_session: root.to_string(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        // The owning session sees its live child; an unrelated session does not.
        assert!(tracker.session_has_running(root));
        assert!(!tracker.session_has_running("agent:other:peer:user"));

        // Once the child finishes it no longer counts as protected in-flight work.
        tracker.mark_completed("child-1", CompletedOutcome::ok_text("done"));
        assert!(!tracker.session_has_running(root));
    }

    #[test]
    fn mark_completed_moves_from_running() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        tracker.mark_completed("req-1", CompletedOutcome::ok_text("done"));
        assert!(tracker.list_running().is_empty());
        let snap = tracker
            .result_snapshot("req-1")
            .expect("completed entry present");
        match snap.outcome {
            CompletedOutcome::Ok { final_text, .. } => assert_eq!(final_text, "done"),
            CompletedOutcome::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    #[test]
    fn cancel_cancels_token() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        let token_clone = token.clone();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        tracker.cancel("req-1");
        assert!(token_clone.is_cancelled());
    }

    #[test]
    fn result_snapshot_returns_none_for_unknown() {
        let tracker = BackgroundAgentTracker::new();
        assert!(tracker.result_snapshot("unknown").is_none());
    }

    #[test]
    fn cleanup_removes_old_entries() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("old", CompletedOutcome::ok_text("old result"));
        tracker.cleanup(std::time::Duration::ZERO);
        assert!(tracker.result_snapshot("old").is_none());
    }

    #[test]
    fn mark_completed_caps_completed_count() {
        let tracker = BackgroundAgentTracker::new();
        // Insert well beyond the cap; the map must stay bounded and the most
        // recent inserts must survive (oldest-by-completion are evicted first).
        let total = MAX_COMPLETED_RESULTS + 50;
        for i in 0..total {
            tracker.mark_completed(&format!("rid-{i}"), CompletedOutcome::ok_text("r"));
        }
        assert_eq!(
            tracker.all_completed().len(),
            MAX_COMPLETED_RESULTS,
            "completed map must be bounded by MAX_COMPLETED_RESULTS"
        );
        // The very first inserts are the oldest → evicted; the last one stays.
        assert!(tracker.result_snapshot("rid-0").is_none());
        assert!(tracker
            .result_snapshot(&format!("rid-{}", total - 1))
            .is_some());
    }

    #[test]
    fn register_prunes_stale_completed_keeps_fresh() {
        let tracker = BackgroundAgentTracker::new();
        // A freshly completed entry must survive a register() call.
        tracker.mark_completed("fresh", CompletedOutcome::ok_text("r"));
        let token = CancellationToken::new();
        tracker.register("new-run".to_string(), token, "task".to_string());
        assert!(
            tracker.result_snapshot("fresh").is_some(),
            "register() must not evict a still-fresh completed entry"
        );
    }

    #[test]
    fn result_snapshot_is_non_destructive() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("rid", CompletedOutcome::ok_text("answer"));
        // Two consecutive polls must both succeed — a consume would make
        // the second one return None and confuse the parent agent.
        assert!(tracker.result_snapshot("rid").is_some());
        assert!(
            tracker.result_snapshot("rid").is_some(),
            "result_snapshot must not consume the completed entry"
        );
    }

    #[test]
    fn mark_completed_carries_run_metrics() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "do work".into());
        tracker.mark_completed(
            "rid",
            CompletedOutcome::Ok {
                final_text: "result".into(),
                iterations: 7,
                tool_calls_made: 3,
                total_tokens: 1234,
            },
        );
        let snap = tracker.result_snapshot("rid").expect("present");
        assert_eq!(snap.task, "do work");
        match snap.outcome {
            CompletedOutcome::Ok {
                iterations,
                tool_calls_made,
                total_tokens,
                ..
            } => {
                assert_eq!(iterations, 7);
                assert_eq!(tool_calls_made, 3);
                assert_eq!(total_tokens, 1234);
            }
            CompletedOutcome::Err(e) => panic!("expected Ok, got Err({e})"),
        }
    }

    #[test]
    fn running_meta_returns_task_and_none_for_unknown() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "explore repo".into());
        let meta = tracker.running_meta("rid").expect("running entry present");
        assert_eq!(meta.task, "explore repo");
        assert!(tracker.running_meta("ghost").is_none());
    }

    #[test]
    fn all_completed_enumerates_finished_agents() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("a", CompletedOutcome::ok_text("ra"));
        tracker.mark_completed("b", CompletedOutcome::Err("boom".into()));
        let mut ids: Vec<String> = tracker
            .all_completed()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    use crate::agents::progress::{ProgressKind, SubagentProgress};
    use std::time::SystemTime;

    fn fake_progress(step: usize) -> SubagentProgress {
        SubagentProgress {
            step,
            timestamp: SystemTime::now(),
            kind: ProgressKind::ToolCalled,
            tool_name: Some(format!("tool_{step}")),
            latency_ms: None,
            preview: None,
        }
    }

    #[test]
    fn tracker_push_progress_caps_at_50() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "task".into());

        for i in 0..51 {
            tracker.push_progress("rid", fake_progress(i));
        }

        let snap = tracker.progress_snapshot("rid", 100);
        assert_eq!(snap.len(), 50, "cap enforced at 50");
        assert_eq!(snap.first().unwrap().step, 1, "step 0 evicted FIFO");
        assert_eq!(snap.last().unwrap().step, 50);
    }

    #[test]
    fn tracker_progress_snapshot_returns_last_n() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "task".into());

        for i in 0..5 {
            tracker.push_progress("rid", fake_progress(i));
        }

        let snap = tracker.progress_snapshot("rid", 3);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].step, 2);
        assert_eq!(snap[2].step, 4);
    }

    /// B18/D7 — the trail must survive completion. It used to die with the
    /// `RunningAgent`, so a parent polling a *failed* child got an empty
    /// progress array and a bare error string: the negative result was lost.
    #[test]
    fn completed_agent_keeps_bounded_progress_tail() {
        let tracker = BackgroundAgentTracker::new();
        tracker.register("rid".into(), CancellationToken::new(), "explore".into());
        for i in 0..15 {
            tracker.push_progress("rid", fake_progress(i));
        }
        tracker.mark_completed("rid", CompletedOutcome::Err("boom".into()));

        let snap = tracker.result_snapshot("rid").expect("completed entry");
        assert_eq!(
            snap.progress_tail.len(),
            PROGRESS_TAIL_LEN,
            "tail is bounded at PROGRESS_TAIL_LEN"
        );
        assert_eq!(
            snap.progress_tail.last().unwrap().step,
            14,
            "the tail must end at the last thing the child did before it died"
        );

        // The reader path falls back to the completed map for a finished id.
        let after = tracker.progress_snapshot("rid", 10);
        assert_eq!(after.len(), PROGRESS_TAIL_LEN);
        assert_eq!(after.last().unwrap().tool_name.as_deref(), Some("tool_14"));
        // A never-registered id still reads empty.
        assert!(tracker.progress_snapshot("ghost", 10).is_empty());
    }

    #[test]
    fn tracker_push_unknown_id_no_op() {
        let tracker = BackgroundAgentTracker::new();
        tracker.push_progress("never-registered", fake_progress(0));
        assert!(tracker.progress_snapshot("never-registered", 10).is_empty());
    }

    #[test]
    fn flat_nodes_carries_meta_and_filters_by_root() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register_with_meta(
            "n1".into(),
            token.clone(),
            "explore".into(),
            SpawnMeta {
                parent_id: None,
                depth: 1,
                root_session: "agent:s1".into(),
                model: Some("opus".into()),
            },
        );
        tracker.register_with_meta(
            "n2".into(),
            token,
            "other".into(),
            SpawnMeta {
                parent_id: None,
                depth: 1,
                root_session: "agent:s2".into(),
                model: None,
            },
        );
        assert_eq!(tracker.flat_nodes(None).len(), 2);
        let s1 = tracker.flat_nodes(Some("agent:s1"));
        assert_eq!(s1.len(), 1);
        assert_eq!(s1[0].node_id, "n1");
        assert_eq!(s1[0].depth, 1);
        assert_eq!(s1[0].model.as_deref(), Some("opus"));
        assert_eq!(s1[0].lifecycle, NodeLifecycle::Running);
    }

    #[test]
    fn flat_nodes_reflects_completed_lifecycle_and_tools() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register_with_meta(
            "n1".into(),
            token,
            "work".into(),
            SpawnMeta {
                parent_id: None,
                depth: 1,
                root_session: "agent:s".into(),
                model: None,
            },
        );
        tracker.push_progress("n1", fake_progress(0)); // ToolCalled
        tracker.mark_completed("n1", CompletedOutcome::ok_text("done"));
        let nodes = tracker.flat_nodes(None);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].lifecycle, NodeLifecycle::Completed);
        assert_eq!(nodes[0].tool_count, 1);
        assert_eq!(nodes[0].last_activity.as_deref(), Some("tool_called"));
    }

    #[tokio::test]
    async fn wait_returns_completed_when_child_finishes() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.register("rid".into(), CancellationToken::new(), "work".into());
        let t2 = tracker.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            t2.mark_completed("rid", CompletedOutcome::ok_text("done"));
        });
        match tracker.wait("rid", Duration::from_secs(5)).await {
            WaitOutcome::Completed(snap) => match snap.outcome {
                CompletedOutcome::Ok { final_text, .. } => assert_eq!(final_text, "done"),
                CompletedOutcome::Err(e) => panic!("expected Ok, got Err({e})"),
            },
            other => panic!("expected Completed, got {other:?}"),
        }
        // wait() marks the result consumed so the announce won't re-deliver it.
        assert!(tracker.is_consumed("rid"));
    }

    #[tokio::test]
    async fn wait_times_out_while_child_still_running() {
        let tracker = BackgroundAgentTracker::new();
        tracker.register("rid".into(), CancellationToken::new(), "slow".into());
        match tracker.wait("rid", Duration::from_millis(30)).await {
            WaitOutcome::TimedOut { .. } => {}
            other => panic!("expected TimedOut, got {other:?}"),
        }
        // A timed-out wait must NOT mark the (still-running) agent consumed.
        assert!(!tracker.is_consumed("rid"));
    }

    #[tokio::test]
    async fn wait_returns_not_found_for_unknown_id() {
        let tracker = BackgroundAgentTracker::new();
        assert!(matches!(
            tracker.wait("ghost", Duration::from_millis(10)).await,
            WaitOutcome::NotFound
        ));
    }

    #[tokio::test]
    async fn wait_sees_result_completed_before_the_wait() {
        // A result that landed before the parent ever calls wait() is returned
        // immediately (completed-first check), not mistaken for NotFound.
        let tracker = BackgroundAgentTracker::new();
        tracker.register("rid".into(), CancellationToken::new(), "fast".into());
        tracker.mark_completed("rid", CompletedOutcome::ok_text("early"));
        match tracker.wait("rid", Duration::from_millis(10)).await {
            WaitOutcome::Completed(snap) => assert_eq!(snap.task, "fast"),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_completed_wakes_a_parked_waiter_no_lost_wakeup() {
        // The waiter parks, then a completion fires; notify_waiters must wake it
        // so it returns without waiting out the full (long) deadline.
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.register("rid".into(), CancellationToken::new(), "race".into());
        let t2 = tracker.clone();
        let waiter = tokio::spawn(async move { t2.wait("rid", Duration::from_secs(30)).await });
        // Give the waiter a moment to park on the notifier, then complete.
        tokio::time::sleep(Duration::from_millis(20)).await;
        tracker.mark_completed("rid", CompletedOutcome::ok_text("woke"));
        let outcome = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter must wake well before the 30s deadline")
            .expect("waiter task panicked");
        assert!(matches!(outcome, WaitOutcome::Completed(_)));
    }

    #[test]
    fn mark_consumed_is_noop_for_unknown_and_running() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_consumed("ghost"); // no panic, no-op
        assert!(!tracker.is_consumed("ghost"));
        tracker.register("rid".into(), CancellationToken::new(), "t".into());
        tracker.mark_consumed("rid"); // still running → nothing to mark yet
        assert!(!tracker.is_consumed("rid"));
        tracker.mark_completed("rid", CompletedOutcome::ok_text("x"));
        assert!(!tracker.is_consumed("rid"));
        tracker.mark_consumed("rid");
        assert!(tracker.is_consumed("rid"));
    }

    #[tokio::test]
    async fn wait_any_returns_first_to_finish() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.register("a".into(), CancellationToken::new(), "slow".into());
        tracker.register("b".into(), CancellationToken::new(), "fast".into());
        let t2 = tracker.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            t2.mark_completed("b", CompletedOutcome::ok_text("b-done"));
        });
        let ids = vec!["a".to_string(), "b".to_string()];
        match tracker.wait_any(&ids, Duration::from_secs(5)).await {
            WaitAnyOutcome::Completed {
                request_id,
                snapshot,
            } => {
                assert_eq!(request_id, "b");
                match snapshot.outcome {
                    CompletedOutcome::Ok { final_text, .. } => assert_eq!(final_text, "b-done"),
                    CompletedOutcome::Err(e) => panic!("expected Ok, got Err({e})"),
                }
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        // Only the finished id is consumed; the still-running sibling is not.
        assert!(tracker.is_consumed("b"));
        assert!(!tracker.is_consumed("a"));
    }

    #[tokio::test]
    async fn wait_any_times_out_lists_still_running() {
        let tracker = BackgroundAgentTracker::new();
        tracker.register("a".into(), CancellationToken::new(), "slow".into());
        tracker.register("b".into(), CancellationToken::new(), "slow".into());
        let ids = vec!["a".to_string(), "b".to_string()];
        match tracker.wait_any(&ids, Duration::from_millis(30)).await {
            WaitAnyOutcome::TimedOut { mut still_running } => {
                still_running.sort();
                assert_eq!(still_running, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_any_not_found_when_all_unknown() {
        let tracker = BackgroundAgentTracker::new();
        let ids = vec!["ghost1".to_string(), "ghost2".to_string()];
        assert!(matches!(
            tracker.wait_any(&ids, Duration::from_millis(10)).await,
            WaitAnyOutcome::NotFound
        ));
    }

    #[test]
    fn lifecycle_classifies_strict_against_producer_form() {
        // The single real cancellation producer is
        // `subagent_spawner::spawn` wrapping `HarnessError::Cancelled`
        // (`#[error("cancelled")]` per `harness/trait_def.rs`), which yields
        // exactly the string `"sub-agent failed: cancelled"`. Only that
        // exact form must classify as Cancelled — any error message that
        // merely contains the substring "cancel" (e.g. a wrapped provider
        // detail or a tool message) is a plain failure.
        use crate::agents::background_tracker::lifecycle_from_outcome;

        let real_cancel = CompletedOutcome::Err("sub-agent failed: cancelled".into());
        assert_eq!(
            lifecycle_from_outcome(&real_cancel),
            NodeLifecycle::Cancelled,
            "exact producer form must classify as Cancelled"
        );

        let timeout = CompletedOutcome::Err("Sub-agent timed out after 30s".into());
        assert_eq!(
            lifecycle_from_outcome(&timeout),
            NodeLifecycle::TimedOut,
            "wall-clock timeout prefix must still classify as TimedOut"
        );

        // Substring "cancel" inside an ordinary failure must NOT flip to
        // Cancelled — that's the misclassification the strict form fixes.
        let substring_cancel = CompletedOutcome::Err(
            "sub-agent failed: tool returned a message about cancel handshake".into(),
        );
        assert_eq!(
            lifecycle_from_outcome(&substring_cancel),
            NodeLifecycle::Failed,
            "loose substring match must not become Cancelled"
        );

        // Capitalised variants of unrelated errors are still failures.
        let capital = CompletedOutcome::Err("sub-agent failed: tool Cancel was hit".into());
        assert_eq!(
            lifecycle_from_outcome(&capital),
            NodeLifecycle::Failed,
            "capitalised 'Cancel' in an unrelated failure must not become Cancelled"
        );

        // And a plain unrelated failure stays Failed.
        let plain = CompletedOutcome::Err("sub-agent failed: provider 503".into());
        assert_eq!(
            lifecycle_from_outcome(&plain),
            NodeLifecycle::Failed,
            "unrelated failure must remain Failed"
        );
    }
}
