//! `BackgroundAgentTracker` — tracks sub-agents running in background tokio tasks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::agents::progress::{ProgressKind, SubagentProgress};
use crate::agents::subagent_tree_events;
use crate::sync_primitives::{Arc, RwLock};
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

/// Round-8 — characters of a finished sub-agent's result kept inline on
/// `SubagentNode.result_preview` so the panel can render
/// "completed: '...'" without a follow-up `check_status`. Matches
/// `loop_tool::LIST_RESULT_PREVIEW_CHARS` (200) for visual consistency
/// between the tree row and the `list` directory row.
const RESULT_PREVIEW_CHARS: usize = 200;

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

/// Round-8 — bounded preview of a finished sub-agent's result for
/// `SubagentNode.result_preview`. Returns `None` for empty results so the
/// field is omitted from the wire entirely (panels that never heard of it
/// see no key; new panels show a blank rather than an ellipsis for an
/// empty result).
///
/// UTF-8 safe by character, not by byte: slicing on `char_indices` keeps
/// CJK / emoji boundaries intact (P7 — the same trap `loop_tool::preview`
/// exists to avoid). The preview is informational only; the full result
/// is one `check_status` away.
pub(crate) fn preview_from_outcome(outcome: &CompletedOutcome) -> Option<String> {
    let raw = match outcome {
        CompletedOutcome::Ok { final_text, .. } => final_text.as_str(),
        CompletedOutcome::Err(msg) => msg.as_str(),
    };
    if raw.is_empty() {
        return None;
    }
    let head: String = raw.chars().take(RESULT_PREVIEW_CHARS).collect();
    if head.chars().count() < raw.chars().count() {
        Some(format!("{head}\u{2026}"))
    } else {
        Some(head)
    }
}

/// W24 — one progress event rendered as a single line of the cross-process
/// activity trail. Uses [`progress_activity`] for the verb so the persisted
/// trail and the live tree read the same vocabulary.
fn persisted_activity_line(event: &SubagentProgress) -> String {
    let mut line = format!("step {} {}", event.step, progress_activity(event.kind));
    if let Some(tool) = &event.tool_name {
        line.push_str(" tool=");
        line.push_str(tool);
    }
    if let Some(preview) = &event.preview {
        line.push_str(" :: ");
        line.push_str(preview);
    }
    line
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
    /// True for a *presence-only* registration ([`RunningRegistration`]): a sync
    /// fan-out seam that delivers its result inline and never reaches
    /// `mark_completed`. Such an entry exists solely so the cancel walks and the
    /// busy guard can see in-flight work, so it is excluded from the two
    /// *enumeration* faces — [`list_running`](BackgroundAgentTracker::list_running)
    /// (the LLM's `subagent list`) and
    /// [`flat_nodes`](BackgroundAgentTracker::flat_nodes) (the Panel tree) —
    /// where it would otherwise read as a pollable background sub-agent that
    /// `check_status` can never resolve and a tree node that never settles.
    presence_only: bool,
    /// Set by [`mark_consumed`](BackgroundAgentTracker::mark_consumed) while the
    /// agent is still running: the parent has already accounted for whatever
    /// this run turns into, so the completed entry must be born `consumed`.
    ///
    /// The one caller that needs this is the model-issued `cancel`: it fires the
    /// token and returns *before* the child unwinds, so at that moment there is
    /// no completed entry to stamp. Without carrying the intent across the
    /// transition, the cancelled child's `Err("sub-agent failed: cancelled")`
    /// reached `subagent_announce` un-consumed and spent a whole fresh parent
    /// turn reporting the failure of a sub-agent the parent itself just killed.
    consume_on_completion: bool,
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
/// primitive (codex `wait_agent` parity). `Completed` names the first
/// **undelivered** id in the set to finish. `TimedOut` lists the ids still
/// running when the window closed. `AllDelivered` means every id has already
/// been handed over and none is still running — the fan-out is drained.
/// `NotFound` means none of the ids is known (all unregistered / TTL-pruned).
#[derive(Debug, Clone)]
pub enum WaitAnyOutcome {
    Completed {
        request_id: String,
        snapshot: CompletedSnapshot,
    },
    TimedOut {
        still_running: Vec<String>,
    },
    /// Terminal for a fan-out drain loop: every id finished **and** was already
    /// delivered to the caller, with nothing left running. Re-returning one of
    /// those results instead is what made repeating the same `wait` arguments
    /// spin forever — see [`wait_any`](BackgroundAgentTracker::wait_any).
    AllDelivered {
        request_ids: Vec<String>,
    },
    /// Round-8 — every id in the set was unknown to the tracker (never
    /// registered, owned by another session, or TTL-pruned). `unknown_ids`
    /// is the same list the call site gets from `unknown_ids(&request_ids)`,
    /// so a single round-trip diagnoses typos even when the *whole* set is
    /// bad — the previous bool-shaped `NotFound` lost the breakdown and
    /// the model had to fish the typo out by re-issuing one id at a time.
    NotFound {
        unknown_ids: Vec<String>,
    },
}

/// Round-8 — live occupancy snapshot for `gateway.metrics.subagent_concurrency`.
/// Mirrors the §4.10 `ConcurrencySnapshot` shape so the same panel widget
/// renders both gauges (run slots + sub-agent slots) without two card types.
///
/// `running_total` includes **all** live registrations (presence-only
/// fan-out seats are counted in `presence_only_total` so the panel can
/// surface "4 sync fan-out children + 1 background child = 5 running" —
/// presence-only entries are not enumerated by the `subagent` tool, but
/// they ARE occupying the parent's Interrupt-demote budget).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SubagentSnapshot {
    /// Live sub-agents across the whole process (or one session when
    /// `scope = Some(session)`).
    pub running_total: usize,
    /// Per-session running count, sorted ascending by session key (stable
    /// for stable JSON output). Idle sessions are omitted.
    pub running_per_session: Vec<SessionRunning>,
    /// Running registrations that are *presence-only* (sync fan-out
    /// seams, MoA aggregators, team-chat members). Excluded from the
    /// `subagent` tool's enumeration faces by design.
    pub presence_only_total: usize,
    /// Finished entries still retrievable via `check_status` / `wait`
    /// (bounded by the TTL + count cap).
    pub completed_total: usize,
    /// Subset of `completed_total` whose result was already handed to the
    /// parent via an on-demand `wait` / `check_status` (or whose cancel
    /// carried the consume intent across the transition). The
    /// `consumed / completed` ratio is the dedup-hygiene gauge: a high
    /// completed count paired with a low consumed count means the parent
    /// is ignoring its results.
    pub consumed_total: usize,
}

/// One session's live sub-agent count. The session key is the
/// `SessionKey::to_key_string()` form (`agent:<id>:peer:user`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionRunning {
    pub session: String,
    pub count: usize,
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
        self.insert_running(
            request_id,
            cancel_token,
            task_description,
            meta,
            /*presence_only*/ false,
        );
    }

    /// Register a *presence-only* entry — visible to the cancel walks and the
    /// busy guard, hidden from the enumeration faces (`list_running` /
    /// `flat_nodes`). Backs [`RunningRegistration`]; see `RunningAgent`'s
    /// `presence_only` field for why the distinction exists.
    pub fn register_presence_only(
        &self,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
        meta: SpawnMeta,
    ) {
        self.insert_running(
            request_id,
            cancel_token,
            task_description,
            meta,
            /*presence_only*/ true,
        );
    }

    fn insert_running(
        &self,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
        meta: SpawnMeta,
        presence_only: bool,
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
                presence_only,
                consume_on_completion: false,
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
        // Read once, before the `running` borrow below, so no code path ever
        // holds two of this struct's locks at the same time. Covers the case
        // where the id has no running entry (already completed, or never
        // registered): a `mark_consumed` that already landed on the completed
        // entry must survive a re-completion rather than be reset to `false`.
        let already_consumed = self.is_consumed(request_id);
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
            born_consumed,
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
                        agent.consume_on_completion || already_consumed,
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
                    already_consumed,
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
                    consumed: born_consumed,
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

    /// Remove a running entry WITHOUT recording a completed outcome. Backs the
    /// running-only registrations ([`RunningRegistration`]): sync fan-out seams
    /// (subagent sync batch / MoA, `team_delegate`, team-chat member runs)
    /// deliver their results inline at the call seam, so a completed entry
    /// would only pollute `list` / `check_status` and hand the proactive
    /// announce an already-delivered result. Wakes `wait` parkers so a waiter
    /// on a delisted id resolves to `NotFound` instead of blocking out its
    /// full window.
    pub fn remove_running(&self, request_id: &str) {
        self.running
            .write()
            .unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            })
            .remove(request_id);
        self.completion.notify_waiters();
    }

    /// Request-ids of still-running agents registered under `parent_id`
    /// (`SpawnMeta.parent_id`). Backs the `teams.chat.cancel` tree walk: the
    /// group-chat fan-out registers every member run under the RPC-minted
    /// parent run_id, and cancel enumerates them here to fire each engine
    /// per-run token. O(running) scan, same as `session_has_running`.
    ///
    /// Round-8 — sorted by `(started_at_ms, request_id)` for stable
    /// enumeration across `teams.chat.cancel` re-walks and panel rebuilds.
    /// The cancel walker iterates this list to fire each engine per-run
    /// token; a stable order means the same fan-out tree always cancels in
    /// the same sequence (helpful for `cancel_session` audit logs that
    /// record the order of token fires).
    #[must_use]
    pub fn running_children_of(&self, parent_id: &str) -> Vec<String> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        let mut rows: Vec<(&String, u64)> = running
            .iter()
            .filter(|(_, agent)| agent.meta.parent_id.as_deref() == Some(parent_id))
            .map(|(id, agent)| (id, agent.started_at_ms))
            .collect();
        rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        rows.into_iter().map(|(id, _)| id.clone()).collect()
    }

    /// Request-ids of still-running registrations owned by `root_session`
    /// (`SpawnMeta.root_session`, the top-level session key in
    /// `SessionKey::to_key_string()` form). Backs the leader-cancel walk: when a
    /// leader session is cancelled, its in-flight delegated member runs are
    /// enumerated here and each engine per-run token is fired. O(running) scan,
    /// same as `session_has_running`. Ids that are not live engine runs (e.g.
    /// in-process subagents) simply yield a harmless `cancel` miss at the seam.
    #[must_use]
    pub fn running_runs_of_session(&self, root_session: &str) -> Vec<String> {
        self.running
            .read()
            .unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            })
            .iter()
            .filter(|(_, agent)| agent.meta.root_session == root_session)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// W11 — the **addressing** chokepoint: may a caller owning `scope` read,
    /// wait on, or cancel `request_id`?
    ///
    /// The enumeration face (`list_running` / `all_completed` / `flat_nodes` /
    /// `subagent_snapshot`) is already session-scoped, but scoping enumeration
    /// alone only makes a foreign id hard to *guess* — it does not make it
    /// unreachable. The tracker is process-global, so any model that came by
    /// another session's request_id (an announce echo, a log line, a paste)
    /// could still `check_status` its output or `cancel` its run. Every
    /// by-id accessor on the model-facing path runs through here first.
    ///
    /// `scope = None` is unrestricted, matching every other `scope` parameter
    /// on this type: CLI / direct construction / tests / RPC surfaces that
    /// performed their own authorization.
    ///
    /// An out-of-scope id is deliberately indistinguishable from a typo — the
    /// callers fold both into the same "unknown request_id" answer, so the
    /// existence of another session's run is not itself disclosed.
    #[must_use]
    pub fn addressable(&self, request_id: &str, scope: Option<&str>) -> bool {
        let Some(want) = scope else {
            return true;
        };
        {
            // Running first: `mark_completed` inserts into `completed` before
            // removing from `running`, so an id can be briefly in both maps.
            // Either copy carries the same `root_session`, but reading the
            // live one keeps the answer stable across the transition.
            let running = self.running.read().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            if let Some(agent) = running.get(request_id) {
                return agent.meta.root_session == want;
            }
        }
        self.completed
            .read()
            .unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            })
            .get(request_id)
            .is_some_and(|c| c.meta.root_session == want)
    }

    /// Cancel a running background agent. Returns `true` if the `request_id`
    /// was found in the running set and the `CancellationToken` was hit;
    /// `false` if no such running agent exists (already completed / never
    /// registered). The cooperative cancellation still relies on the
    /// running task observing the token at the next await point.
    ///
    /// Unscoped face, for callers that authorized the id themselves (the
    /// `teams.chat.cancel` RPC walks its own run tree; CLI / tests hold no
    /// session). Model-facing callers must use
    /// [`cancel_in_scope`](Self::cancel_in_scope).
    pub fn cancel(&self, request_id: &str) -> bool {
        self.cancel_in_scope(request_id, None)
    }

    /// [`cancel`](Self::cancel) behind the [`addressable`](Self::addressable)
    /// chokepoint. An out-of-scope id reports `false` — the same answer a
    /// never-registered id gets, so the caller's "no such sub-agent" message
    /// covers both without disclosing the foreign run.
    pub fn cancel_in_scope(&self, request_id: &str, scope: Option<&str>) -> bool {
        if !self.addressable(request_id, scope) {
            return false;
        }
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

    /// Round-8 — live sub-agent occupancy snapshot, the §4.11 mirror of
    /// §4.10's `run_concurrency`. Drives the `gateway.metrics.subagent_concurrency`
    /// RPC so a panel can render "N running / M completed / K consumed" with
    /// a per-session breakdown of the in-flight set (which the
    /// `subagent.tree` cold-start mirrors).
    ///
    /// `scope` mirrors [`flat_nodes`](Self::flat_nodes)'s filter: `Some` keeps
    /// only entries owned by that session, `None` returns the process-wide
    /// totals (for CLI / tests / cross-session ops dashboards).
    ///
    /// Cheap: two read locks (running + completed) per call, O(n) over the
    /// small backing maps. No allocations beyond the per-session vec.
    pub fn subagent_snapshot(&self, scope: Option<&str>) -> SubagentSnapshot {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });

        // Per-session running counts (drives the per-agent breakdown; naming
        // is the same as `ConcurrencyLimiter::per_agent` for cross-§ symmetry).
        let mut per_session: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        let mut running_total = 0usize;
        let mut presence_only_total = 0usize;
        for agent in running.values() {
            if scope.is_some_and(|s| agent.meta.root_session != s) {
                continue;
            }
            running_total += 1;
            if agent.presence_only {
                presence_only_total += 1;
            }
            *per_session
                .entry(agent.meta.root_session.clone())
                .or_insert(0) += 1;
        }

        let mut completed_total = 0usize;
        let mut consumed_total = 0usize;
        for agent in completed.values() {
            if scope.is_some_and(|s| agent.meta.root_session != s) {
                continue;
            }
            completed_total += 1;
            if agent.consumed {
                consumed_total += 1;
            }
        }

        SubagentSnapshot {
            running_total,
            running_per_session: per_session
                .into_iter()
                .map(|(session, count)| SessionRunning { session, count })
                .collect(),
            presence_only_total,
            completed_total,
            consumed_total,
        }
    }

    /// Non-destructively read a finished agent's outcome. Returns `None`
    /// when the `request_id` was never registered, has been TTL-pruned, or
    /// belongs to a session other than `scope` (W11 — see
    /// [`addressable`](Self::addressable)). Unlike a consume, repeated polls
    /// return the same snapshot — this is what lets a parent re-check a
    /// completed subagent.
    pub fn result_snapshot(
        &self,
        request_id: &str,
        scope: Option<&str>,
    ) -> Option<CompletedSnapshot> {
        if !self.addressable(request_id, scope) {
            return None;
        }
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
    pub async fn wait(
        &self,
        request_id: &str,
        scope: Option<&str>,
        timeout: Duration,
    ) -> WaitOutcome {
        let ids = [request_id.to_string()];
        match self.wait_any(&ids, scope, timeout).await {
            WaitAnyOutcome::Completed { snapshot, .. } => WaitOutcome::Completed(snapshot),
            // A single-id wait stays idempotent: `wait_any` reports a
            // second wait on the same id as `AllDelivered` (that distinction
            // only matters for draining a SET), but the caller asked about one
            // agent and the honest answer is still its result. Falls through to
            // `NotFound` only if the entry was TTL-pruned between the two.
            WaitAnyOutcome::AllDelivered { .. } => match self.result_snapshot(request_id, scope) {
                Some(snapshot) => WaitOutcome::Completed(snapshot),
                None => WaitOutcome::NotFound,
            },
            WaitAnyOutcome::TimedOut { .. } => WaitOutcome::TimedOut {
                elapsed_secs: self
                    .running_meta(request_id, scope)
                    .map(|m| m.elapsed_secs)
                    .unwrap_or(0),
            },
            // Single-id wait: a one-element set is by definition "every id
            // is unknown" when we land here, so the unknown list has at
            // most the one id. The caller-side distinction is collapsed
            // to the existing `WaitOutcome::NotFound`; the per-id
            // breakdown is not surfaced at this layer (only `wait_any`
            // callers care about a list).
            WaitAnyOutcome::NotFound { .. } => WaitOutcome::NotFound,
        }
    }

    /// Park until *any* subagent in `request_ids` finishes, or until `timeout`
    /// elapses — the fan-out first-completion primitive (codex `wait_agent`
    /// parity). Sleeps on the shared [`completion`](Self::completion) notifier
    /// (which wakes on every completion) and re-checks the whole set, so it
    /// costs no CPU while waiting and returns the instant the first result
    /// lands.
    ///
    /// # Delivered results are skipped, not re-returned
    ///
    /// Only an **undelivered** completion satisfies the wait; the returned one
    /// is marked consumed on the way out. This is what lets a fan-out be
    /// drained by *repeating the same call*: `wait([a,b,c])` yields `a`, then
    /// `b`, then `c`, then `AllDelivered`. Returning the first completed id
    /// regardless of delivery made the obvious drain loop — a model re-issuing
    /// its previous arguments — hand back `a` instantly and forever, burning
    /// one LLM turn per iteration. Bookkeeping the caller would otherwise have
    /// to do by hand is mechanical, so the harness does it (R10 scaffolding, no
    /// judgement about *what* the results mean).
    ///
    /// # Scope
    ///
    /// W11 — every by-id lookup below goes through
    /// [`addressable`](Self::addressable), and it does so **inside the loop**,
    /// not once on entry. A foreign id that completes while this call is
    /// parked must not be claimable on the re-check either; filtering only at
    /// the top would have left exactly that window open.
    pub async fn wait_any(
        &self,
        request_ids: &[String],
        scope: Option<&str>,
        timeout: Duration,
    ) -> WaitAnyOutcome {
        let deadline = Instant::now() + timeout;
        loop {
            // Arm the notifier BEFORE inspecting state: `Notified::enable`
            // registers this waiter so a `mark_completed` racing between our
            // state read and our await still wakes us (no lost wakeup).
            let notified = self.completion.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // First *undelivered* completion in the set wins. `mark_completed`
            // inserts into `completed` before removing from `running`, so a
            // finished agent is always visible here and never mistaken for
            // absent.
            let mut any_running = false;
            let mut delivered: Vec<String> = Vec::new();
            let mut fresh: Option<(String, CompletedSnapshot)> = None;
            for id in request_ids {
                // Scope is re-applied on EVERY lap, not just the first: an
                // out-of-scope id that finishes while we are parked must not
                // become claimable when the notifier wakes us.
                if let Some(snapshot) = self.result_snapshot(id, scope) {
                    if self.is_consumed(id) {
                        delivered.push(id.clone());
                    } else if fresh.is_none() {
                        fresh = Some((id.clone(), snapshot));
                    }
                    continue;
                }
                if self.running_meta(id, scope).is_some() {
                    any_running = true;
                }
            }
            if let Some((request_id, snapshot)) = fresh {
                self.mark_consumed(&request_id);
                return WaitAnyOutcome::Completed {
                    request_id,
                    snapshot,
                };
            }
            // Nothing fresh and nothing running ⇒ the set is finished: either
            // fully drained, or every id is unknown.
            if !any_running {
                return if delivered.is_empty() {
                    // Round-8 — every id is unknown; the previous bool
                    // variant lost the breakdown. Carry the unknown list
                    // through so the model can fix the call in one shot
                    // (mirrors `annotate_unknown` on the success paths).
                    WaitAnyOutcome::NotFound {
                        unknown_ids: self.unknown_ids(request_ids, scope),
                    }
                } else {
                    WaitAnyOutcome::AllDelivered {
                        request_ids: delivered,
                    }
                };
            }
            let now = Instant::now();
            if now >= deadline {
                return WaitAnyOutcome::TimedOut {
                    still_running: self.still_running_ids(request_ids, scope),
                };
            }
            let remaining = deadline - now;
            tokio::select! {
                () = &mut notified => { /* something finished — re-check the set */ }
                () = tokio::time::sleep(remaining) => {
                    return WaitAnyOutcome::TimedOut {
                        still_running: self.still_running_ids(request_ids, scope),
                    };
                }
            }
        }
    }

    /// The subset of `request_ids` still in the running set — the `wait_any`
    /// timeout arm reports these so the caller knows which to keep waiting on.
    fn still_running_ids(&self, request_ids: &[String], scope: Option<&str>) -> Vec<String> {
        request_ids
            .iter()
            .filter(|id| self.running_meta(id, scope).is_some())
            .cloned()
            .collect()
    }

    /// Mark this run's outcome as already accounted for by the parent, so the
    /// proactive `subagent_announce` does not spend a fresh parent turn
    /// re-announcing something the model has already seen.
    ///
    /// Works in **both** phases of a run's life, because the two producers sit
    /// on opposite sides of the transition:
    ///
    /// * already completed (`wait` / `check_status` returned the outcome) —
    ///   stamps the completed entry directly;
    /// * still running (the model-issued `cancel` — it fires the token and
    ///   returns long before the child unwinds) — records the intent on the
    ///   running entry, and `mark_completed` folds it into the completed entry
    ///   at the moment it is born.
    ///
    /// Covering only the first case left cancellation as the single most likely
    /// producer of a spurious announce: a whole parent turn spent reporting
    /// `sub-agent failed: cancelled` for a child the parent itself killed.
    /// No-op for an id that is in neither map.
    pub fn mark_consumed(&self, request_id: &str) {
        {
            let mut completed = self.completed.write().unwrap_or_else(|e| {
                warn!("BackgroundAgentTracker lock poisoned, recovering");
                e.into_inner()
            });
            if let Some(agent) = completed.get_mut(request_id) {
                agent.consumed = true;
                drop(completed);
                // The other half of "the parent knows", alongside the announce
                // path's success arm. A model that polled the result itself has
                // been told just as surely as one that received an announce, so
                // a restart must not re-deliver it.
                crate::agents::background_persistence::record_announced(request_id);
                return;
            }
        }
        let mut running = self.running.write().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        if let Some(agent) = running.get_mut(request_id) {
            agent.consume_on_completion = true;
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

    /// List running agents as (`request_id`, `task_description`, `elapsed_secs`),
    /// optionally scoped to one `root_session`.
    ///
    /// Backs the `subagent` tool's `list` action, so it reports only ids the
    /// parent model can actually act on: presence-only entries are skipped
    /// because they never reach `completed`, which would make every follow-up
    /// `check_status` / `wait` on such an id answer "no background sub-agent
    /// found" the moment the inline result was already returned at its own seam.
    ///
    /// `scope` mirrors [`flat_nodes`](Self::flat_nodes)'s filter, and for the
    /// same reason: the tracker is **process-global**, so an unscoped
    /// enumeration handed one session's model the live request_ids of every
    /// other session's background sub-agents — ids it could then `check_status`
    /// (reading another session's output) or `cancel`. `None` keeps the whole
    /// process visible, for callers that genuinely have no owning session (CLI
    /// / direct construction / tests).
    pub fn list_running(&self, scope: Option<&str>) -> Vec<(String, String, u64)> {
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        let mut rows: Vec<(String, String, u64)> = running
            .iter()
            .filter(|(_, agent)| !agent.presence_only)
            .filter(|(_, agent)| scope.is_none_or(|s| agent.meta.root_session == s))
            .map(|(id, agent)| {
                (
                    id.clone(),
                    agent.task_description.clone(),
                    agent.started_at.elapsed().as_secs(),
                )
            })
            .collect();
        // Deterministic order (longest-running first, id as tiebreak): the
        // backing map's iteration order is random, so an unsorted list reshuffles
        // between two `list` calls in one turn and reads as churn to the model.
        rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        rows
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
    /// `request_id` is unknown (never registered, or already completed) or
    /// belongs to a session other than `scope` (W11 — see
    /// [`addressable`](Self::addressable)).
    pub fn running_meta(&self, request_id: &str, scope: Option<&str>) -> Option<RunningMeta> {
        if !self.addressable(request_id, scope) {
            return None;
        }
        let running = self.running.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        running.get(request_id).map(|agent| RunningMeta {
            elapsed_secs: agent.started_at.elapsed().as_secs(),
            task: agent.task_description.clone(),
        })
    }

    /// Snapshot every finished agent as `(request_id, CompletedSnapshot)`,
    /// optionally scoped to one `root_session`. Backs the `list` action's view
    /// of results still retrievable by the parent. Bounded by the TTL prune in
    /// `cleanup` and the count cap in `mark_completed`.
    ///
    /// See [`list_running`](Self::list_running) for why `scope` exists; `None`
    /// keeps the pre-scoping process-wide view.
    pub fn all_completed(&self, scope: Option<&str>) -> Vec<(String, CompletedSnapshot)> {
        let completed = self.completed.read().unwrap_or_else(|e| {
            warn!("BackgroundAgentTracker lock poisoned, recovering");
            e.into_inner()
        });
        let mut in_scope: Vec<(&String, &CompletedAgent)> = completed
            .iter()
            .filter(|(_, c)| scope.is_none_or(|s| c.meta.root_session == s))
            .collect();
        // Newest completion first (id as tiebreak). The backing map iterates in
        // random order, so any downstream cap would otherwise drop an arbitrary
        // subset — silent truncation that reads as "these are all of them".
        in_scope.sort_by(|a, b| {
            b.1.completed_at
                .cmp(&a.1.completed_at)
                .then_with(|| a.0.cmp(b.0))
        });
        in_scope
            .into_iter()
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

    /// The subset of `request_ids` the caller cannot address — in neither the
    /// running nor the completed map (typo'd or TTL-pruned), or owned by a
    /// session other than `scope`.
    ///
    /// `wait` used to silently ignore these: a set with one bad id parked for
    /// the full window and reported only on the good ones, so a typo looked
    /// exactly like a slow sub-agent. Reporting them lets the model fix the
    /// call instead of waiting again.
    ///
    /// W11 — folding the out-of-scope case in here is what makes the tool's
    /// "unknown request_id" wording true again: before the addressing
    /// chokepoint existed, a foreign id was omitted from this list *and*
    /// readable through `check_status`, so the sentence shown to the model was
    /// a lie in both directions.
    #[must_use]
    pub fn unknown_ids(&self, request_ids: &[String], scope: Option<&str>) -> Vec<String> {
        request_ids
            .iter()
            .filter(|id| {
                self.result_snapshot(id, scope).is_none() && self.running_meta(id, scope).is_none()
            })
            .cloned()
            .collect()
    }

    /// Snapshot every tracked sub-agent (running + completed) as flat protocol
    /// nodes, optionally filtered to one `root_session`. Backs the
    /// `subagent.tree` RPC: the gateway runs `aleph_protocol::build_tree` over
    /// this to produce the hierarchy. Bounded by the TTL prune on completed
    /// entries.
    ///
    /// This snapshot is the *cold start* of a live event stream (the panel merges
    /// it, then applies `Spawned` / `Progress` / `Settled` deltas), so it may only
    /// contain nodes that participate in that stream. Presence-only entries do
    /// not: nothing emits `Spawned` for them and their RAII delist emits no
    /// `Settled`, so including them left the panel with rows frozen at `Running`
    /// forever. They are skipped here for exactly that reason.
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
                    // Round-8 — inline preview so the panel renders
                    // "completed: '...'" from this single RPC. A
                    // failed/cancelled entry's `Err` is short enough to fit
                    // verbatim, which the user actually wants to read in the
                    // tree (the full text is one `check_status` away).
                    result_preview: preview_from_outcome(&agent.outcome),
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
                // double-presence), a presence-only entry (never settles — see
                // the method doc), or one filtered out by `root_session`.
                if agent.presence_only || completed_ids.contains(id) {
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
                    // Running nodes have no terminal result yet.
                    result_preview: None,
                });
            }
        }
        // Round-8 — deterministic cold-start order. The backing HashMaps iterate
        // in random order, so two consecutive `flat_nodes` calls from a panel
        // would otherwise shuffle siblings between rebuilds. Sort by spawn
        // time (with id tiebreak) so a node's relative position in the tree is
        // stable across reloads — same shape as `protocol::build_tree`'s
        // sibling sort (started_at_ms, node_id), keeping the two ordering
        // contracts in agreement.
        out.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
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
        let trail_line = persisted_activity_line(&event);
        agent.progress.push_back(event);
        drop(running);
        // W24 — the same signal, appended to the cross-process trail so a
        // restart can still show what the child was doing when its daemon
        // died. Filtered by `background_persistence` to ids it knows about, so
        // sync fan-out / presence-only registrations write nothing; a complete
        // no-op until the boot path enables persistence.
        crate::agents::background_persistence::record_activity(request_id, &trail_line);
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
                c.progress_tail.get(start..).unwrap_or_default().to_vec()
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

/// RAII *running-only* registration.
///
/// Sync fan-out seams (subagent sync batch / MoA aggregator, `team_delegate`,
/// team-chat member runs) deliver their results inline to the caller, so they
/// must never retain a completed tracker entry (no re-delivery via `list` /
/// `check_status`, no proactive announce). What they DO need is presence in
/// the running set while in flight, so:
///
///   * the gateway Interrupt demote guard
///     ([`session_has_running`](BackgroundAgentTracker::session_has_running))
///     refuses to tear down a parent mid-fan-out, and
///   * `teams.chat.cancel` can enumerate live members of a fan-out tree
///     ([`running_children_of`](BackgroundAgentTracker::running_children_of)).
///
/// Registered as *presence-only*
/// ([`register_presence_only`](BackgroundAgentTracker::register_presence_only)):
/// the cancel walks and the busy guard see the entry, the enumeration faces
/// (`list_running` / `flat_nodes`) do not — an id that never reaches `completed`
/// must not be advertised to the model as pollable, nor to the panel as a tree
/// node awaiting a `Settled` event that no one will ever send.
///
/// Registers on construction, delists on `Drop` — which runs on normal scope
/// exit, panic unwind, AND future-drop when the owning task is cancelled, so
/// entries cannot leak into the guard permanently.
pub struct RunningRegistration {
    tracker: Arc<BackgroundAgentTracker>,
    request_id: String,
}

impl RunningRegistration {
    /// Register `request_id` in the running set and return the guard that
    /// delists it on drop. `cancel_token` is stored like any registration —
    /// a `cancel` action on this id fires it (a no-op signal for seams that
    /// have no in-flight abort channel; those pass a fresh token).
    #[must_use]
    pub fn register(
        tracker: Arc<BackgroundAgentTracker>,
        request_id: String,
        cancel_token: CancellationToken,
        task_description: String,
        meta: SpawnMeta,
    ) -> Self {
        tracker.register_presence_only(request_id.clone(), cancel_token, task_description, meta);
        Self {
            tracker,
            request_id,
        }
    }
}

impl Drop for RunningRegistration {
    fn drop(&mut self) {
        self.tracker.remove_running(&self.request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Register a background agent owned by `root_session`.
    fn register_in(tracker: &BackgroundAgentTracker, id: &str, root_session: &str) {
        tracker.register_with_meta(
            id.to_string(),
            CancellationToken::new(),
            format!("task for {id}"),
            SpawnMeta {
                root_session: root_session.to_string(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
    }

    /// A late `mark_completed` on an id whose *completed* entry was already
    /// delivered must not resurrect it as fresh news.
    #[test]
    fn re_completing_a_delivered_id_keeps_it_consumed() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("req-late", CompletedOutcome::ok_text("first"));
        tracker.mark_consumed("req-late");

        tracker.mark_completed("req-late", CompletedOutcome::ok_text("second"));

        assert!(tracker.is_consumed("req-late"));
    }

    /// The fan-out drain loop: re-issuing the SAME id set walks the completions
    /// one at a time instead of handing back the first one forever.
    #[tokio::test]
    async fn wait_any_walks_the_set_instead_of_repeating_one_result() {
        let tracker = BackgroundAgentTracker::new();
        let ids = vec!["fan-a".to_string(), "fan-b".to_string()];
        for id in &ids {
            register_in(&tracker, id, "sess");
        }
        tracker.mark_completed("fan-a", CompletedOutcome::ok_text("A"));
        tracker.mark_completed("fan-b", CompletedOutcome::ok_text("B"));

        let short = Duration::from_millis(50);
        let first = tracker.wait_any(&ids, None, short).await;
        let second = tracker.wait_any(&ids, None, short).await;
        let third = tracker.wait_any(&ids, None, short).await;

        let delivered = |outcome: &WaitAnyOutcome| match outcome {
            WaitAnyOutcome::Completed { request_id, .. } => request_id.clone(),
            other => panic!("expected a completion, got {other:?}"),
        };
        assert_ne!(
            delivered(&first),
            delivered(&second),
            "the second wait must not repeat the first result"
        );

        match third {
            WaitAnyOutcome::AllDelivered { mut request_ids } => {
                request_ids.sort();
                assert_eq!(request_ids, ids, "the drained set is reported back");
            }
            other => panic!("a drained set must terminate, got {other:?}"),
        }
    }

    /// The half that made the old behaviour a *loop*: with one result already
    /// delivered and a sibling still running, the wait must park for the
    /// sibling — not hand back the delivered one instantly.
    #[tokio::test]
    async fn wait_any_parks_for_a_running_sibling_after_one_is_delivered() {
        let tracker = BackgroundAgentTracker::new();
        let ids = vec!["mix-done".to_string(), "mix-running".to_string()];
        for id in &ids {
            register_in(&tracker, id, "sess");
        }
        tracker.mark_completed("mix-done", CompletedOutcome::ok_text("done"));

        let short = Duration::from_millis(50);
        assert!(matches!(
            tracker.wait_any(&ids, None, short).await,
            WaitAnyOutcome::Completed { .. }
        ));

        match tracker.wait_any(&ids, None, short).await {
            WaitAnyOutcome::TimedOut { still_running } => {
                assert_eq!(still_running, vec!["mix-running".to_string()]);
            }
            other => panic!("must wait for the live sibling, got {other:?}"),
        }
    }

    /// A single-id `wait` is a question about ONE agent, so it keeps answering
    /// with that agent's result. Only draining a SET needs the `AllDelivered`
    /// terminal, and `wait` maps it back.
    #[tokio::test]
    async fn single_wait_stays_idempotent_after_delivery() {
        let tracker = BackgroundAgentTracker::new();
        register_in(&tracker, "solo", "sess");
        tracker.mark_completed("solo", CompletedOutcome::ok_text("payload"));

        let short = Duration::from_millis(50);
        for attempt in 0..2 {
            match tracker.wait("solo", None, short).await {
                WaitOutcome::Completed(snap) => match snap.outcome {
                    CompletedOutcome::Ok { final_text, .. } => assert_eq!(final_text, "payload"),
                    other => panic!("attempt {attempt}: unexpected outcome {other:?}"),
                },
                other => panic!("attempt {attempt}: expected the result, got {other:?}"),
            }
        }
    }

    /// The tracker is process-global, so an unscoped enumeration handed one
    /// session's model live request_ids belonging to every other session — ids
    /// it could then `check_status` (reading foreign output) or `cancel`.
    #[test]
    fn enumeration_faces_scope_to_the_owning_session() {
        let tracker = BackgroundAgentTracker::new();
        register_in(&tracker, "mine-live", "sess-mine");
        register_in(&tracker, "mine-done", "sess-mine");
        register_in(&tracker, "theirs-live", "sess-theirs");
        register_in(&tracker, "theirs-done", "sess-theirs");
        tracker.mark_completed("mine-done", CompletedOutcome::ok_text("m"));
        tracker.mark_completed("theirs-done", CompletedOutcome::ok_text("t"));

        let running: Vec<String> = tracker
            .list_running(Some("sess-mine"))
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(running, vec!["mine-live".to_string()]);

        let completed: Vec<String> = tracker
            .all_completed(Some("sess-mine"))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(completed, vec!["mine-done".to_string()]);

        // No owning session (CLI / direct construction) still sees everything.
        assert_eq!(tracker.list_running(None).len(), 2);
        assert_eq!(tracker.all_completed(None).len(), 2);
    }

    /// A cap over a randomly-ordered map drops an arbitrary subset. `list`
    /// truncates to the newest, so the order has to come from here.
    #[test]
    fn all_completed_is_newest_first() {
        let tracker = BackgroundAgentTracker::new();
        for id in ["c1", "c2", "c3"] {
            register_in(&tracker, id, "sess");
            tracker.mark_completed(id, CompletedOutcome::ok_text(id));
            // `completed_at` is an Instant; force a distinguishable ordering.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let ids: Vec<String> = tracker
            .all_completed(Some("sess"))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ids,
            vec!["c3".to_string(), "c2".to_string(), "c1".to_string()]
        );
    }

    /// A typo'd id used to be indistinguishable from a slow sub-agent: the wait
    /// parked the whole window and reported only on the ids that resolved.
    #[test]
    fn unknown_ids_names_the_ids_in_neither_map() {
        let tracker = BackgroundAgentTracker::new();
        register_in(&tracker, "known-live", "sess");
        register_in(&tracker, "known-done", "sess");
        tracker.mark_completed("known-done", CompletedOutcome::ok_text("x"));

        let probe = vec![
            "known-live".to_string(),
            "known-done".to_string(),
            "typo".to_string(),
        ];
        assert_eq!(tracker.unknown_ids(&probe, None), vec!["typo".to_string()]);
    }

    #[test]
    fn register_and_list() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        let running = tracker.list_running(None);
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
    fn running_runs_of_session_returns_ids_for_matching_root_session() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let leader = "agent:leader:main";
        let _r1 = RunningRegistration::register(
            Arc::clone(&tracker),
            "run-A".to_string(),
            CancellationToken::new(),
            "member A".to_string(),
            SpawnMeta {
                parent_id: None,
                depth: 1,
                root_session: leader.to_string(),
                model: None,
            },
        );
        let _r2 = RunningRegistration::register(
            Arc::clone(&tracker),
            "run-B".to_string(),
            CancellationToken::new(),
            "unrelated".to_string(),
            SpawnMeta {
                parent_id: None,
                depth: 1,
                root_session: "agent:other:main".to_string(),
                model: None,
            },
        );
        let mut ids = tracker.running_runs_of_session(leader);
        ids.sort();
        assert_eq!(ids, vec!["run-A".to_string()]);
        // Dropping the guard delists it.
        drop(_r1);
        assert!(tracker.running_runs_of_session(leader).is_empty());
    }

    #[test]
    fn mark_completed_moves_from_running() {
        let tracker = BackgroundAgentTracker::new();
        let token = CancellationToken::new();
        tracker.register("req-1".to_string(), token, "test task".to_string());
        tracker.mark_completed("req-1", CompletedOutcome::ok_text("done"));
        assert!(tracker.list_running(None).is_empty());
        let snap = tracker
            .result_snapshot("req-1", None)
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
        assert!(tracker.result_snapshot("unknown", None).is_none());
    }

    #[test]
    fn cleanup_removes_old_entries() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("old", CompletedOutcome::ok_text("old result"));
        tracker.cleanup(std::time::Duration::ZERO);
        assert!(tracker.result_snapshot("old", None).is_none());
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
            tracker.all_completed(None).len(),
            MAX_COMPLETED_RESULTS,
            "completed map must be bounded by MAX_COMPLETED_RESULTS"
        );
        // The very first inserts are the oldest → evicted; the last one stays.
        assert!(tracker.result_snapshot("rid-0", None).is_none());
        assert!(tracker
            .result_snapshot(&format!("rid-{}", total - 1), None)
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
            tracker.result_snapshot("fresh", None).is_some(),
            "register() must not evict a still-fresh completed entry"
        );
    }

    #[test]
    fn result_snapshot_is_non_destructive() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("rid", CompletedOutcome::ok_text("answer"));
        // Two consecutive polls must both succeed — a consume would make
        // the second one return None and confuse the parent agent.
        assert!(tracker.result_snapshot("rid", None).is_some());
        assert!(
            tracker.result_snapshot("rid", None).is_some(),
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
        let snap = tracker.result_snapshot("rid", None).expect("present");
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
        let meta = tracker
            .running_meta("rid", None)
            .expect("running entry present");
        assert_eq!(meta.task, "explore repo");
        assert!(tracker.running_meta("ghost", None).is_none());
    }

    #[test]
    fn all_completed_enumerates_finished_agents() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed("a", CompletedOutcome::ok_text("ra"));
        tracker.mark_completed("b", CompletedOutcome::Err("boom".into()));
        let mut ids: Vec<String> = tracker
            .all_completed(None)
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

        let snap = tracker
            .result_snapshot("rid", None)
            .expect("completed entry");
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

    /// Round-8 — a finished node's result preview rides `flat_nodes` so a
    /// panel can render "completed: '...'" without a follow-up RPC. Empty
    /// outcomes omit the field entirely.
    #[test]
    fn flat_nodes_completed_carries_result_preview() {
        let tracker = BackgroundAgentTracker::new();
        tracker.register("ok".into(), CancellationToken::new(), "task".into());
        tracker.mark_completed(
            "ok",
            CompletedOutcome::ok_text("hello world, the quick brown fox jumps over"),
        );
        tracker.register("err".into(), CancellationToken::new(), "task".into());
        tracker.mark_completed("err", CompletedOutcome::Err("connection refused".into()));
        tracker.register("empty".into(), CancellationToken::new(), "task".into());
        tracker.mark_completed("empty", CompletedOutcome::ok_text(""));
        tracker.register("live".into(), CancellationToken::new(), "task".into());

        let nodes = tracker.flat_nodes(None);
        let by_id: std::collections::HashMap<&str, &SubagentNode> =
            nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();

        let ok = by_id["ok"];
        assert_eq!(
            ok.result_preview.as_deref(),
            Some("hello world, the quick brown fox jumps over"),
            "short results pass through verbatim"
        );
        let err = by_id["err"];
        assert_eq!(
            err.result_preview.as_deref(),
            Some("connection refused"),
            "error messages ride the preview verbatim (always short)"
        );
        let empty = by_id["empty"];
        assert!(
            empty.result_preview.is_none(),
            "empty outcomes omit the field so old panels see no key"
        );
        let live = by_id["live"];
        assert!(
            live.result_preview.is_none(),
            "running nodes have no terminal result yet"
        );
    }

    /// Round-8 — the preview truncates at the char boundary (CJK / emoji
    /// safe) and ellipsises. The bug class: byte-slicing a model-authored
    /// string is how this panics on multi-byte chars (P7).
    #[test]
    fn preview_truncates_on_char_boundary_with_ellipsis() {
        use crate::agents::background_tracker::preview_from_outcome;
        // 250 CJK chars; RESULT_PREVIEW_CHARS=200; expect 200 chars + ellipsis.
        let long = "中".repeat(250);
        let outcome = CompletedOutcome::ok_text(&long);
        let preview = preview_from_outcome(&outcome).expect("non-empty");
        let chars: Vec<char> = preview.chars().collect();
        assert_eq!(
            chars.len(),
            RESULT_PREVIEW_CHARS + 1,
            "head is RESULT_PREVIEW_CHARS chars plus an ellipsis"
        );
        assert_eq!(chars[RESULT_PREVIEW_CHARS], '\u{2026}');
        // No mid-codepoint truncation: every char must be a complete '中'.
        assert!(chars[..RESULT_PREVIEW_CHARS].iter().all(|&c| c == '中'));
    }

    /// Round-8 — `flat_nodes` returns a deterministic spawn order across
    /// two calls. Before the sort, the backing HashMap iterated in random
    /// order, so a panel rebuild would shuffle siblings between reloads.
    #[test]
    fn flat_nodes_is_stable_across_repeated_calls() {
        let tracker = BackgroundAgentTracker::new();
        for id in ["n3", "n1", "n4", "n2", "n5"] {
            tracker.register(id.into(), CancellationToken::new(), "task".into());
            // Force a distinguishable spawn time per id so the sort by
            // `started_at_ms` (then id) is meaningful.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let first: Vec<String> = tracker
            .flat_nodes(None)
            .into_iter()
            .map(|n| n.node_id)
            .collect();
        let second: Vec<String> = tracker
            .flat_nodes(None)
            .into_iter()
            .map(|n| n.node_id)
            .collect();
        assert_eq!(first, second, "flat_nodes order must be deterministic");
        assert_eq!(
            first,
            vec![
                "n3".to_string(),
                "n1".to_string(),
                "n4".to_string(),
                "n2".to_string(),
                "n5".to_string()
            ],
            "sort is by started_at_ms, then node_id (insertion order with tiebreak)"
        );
    }

    /// Round-8 — `subagent_snapshot` reports running / completed / consumed
    /// per session, plus the presence-only subtotal. Mirrors §4.10's
    /// `ConcurrencySnapshot` shape so the same panel widget renders both
    /// gauges.
    #[test]
    fn subagent_snapshot_reports_per_session_running_and_completed() {
        let tracker = BackgroundAgentTracker::new();
        // 2 running + 1 completed in sess-A
        register_in(&tracker, "a-live-1", "sess-A");
        register_in(&tracker, "a-live-2", "sess-A");
        register_in(&tracker, "a-done", "sess-A");
        tracker.mark_completed("a-done", CompletedOutcome::ok_text("x"));
        // 1 running in sess-B
        register_in(&tracker, "b-live", "sess-B");
        // 1 presence-only in sess-A (sync fan-out seam). The public
        // surface takes `&self` so we can hit the SAME tracker instance
        // the snapshot reads from — no `Arc` round-trip needed.
        tracker.register_presence_only(
            "presence-only".into(),
            CancellationToken::new(),
            "fan-out".into(),
            SpawnMeta {
                root_session: "sess-A".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        // Consume one completed entry to drive `consumed_total`.
        tracker.mark_consumed("a-done");

        // Process-wide view.
        let snap = tracker.subagent_snapshot(None);
        assert_eq!(
            snap.running_total, 4,
            "2 sess-A + 1 sess-B + 1 presence-only"
        );
        assert_eq!(snap.presence_only_total, 1);
        assert_eq!(snap.completed_total, 1);
        assert_eq!(snap.consumed_total, 1);

        // Per-session breakdown (BTreeMap → sorted ascending by session key).
        let by_session: std::collections::HashMap<String, usize> = snap
            .running_per_session
            .iter()
            .map(|r| (r.session.clone(), r.count))
            .collect();
        assert_eq!(by_session["sess-A"], 3, "2 live + 1 presence-only");
        assert_eq!(by_session["sess-B"], 1);

        // Scoped to sess-A only.
        let a_only = tracker.subagent_snapshot(Some("sess-A"));
        assert_eq!(a_only.running_total, 3);
        assert_eq!(a_only.presence_only_total, 1);
        assert_eq!(a_only.completed_total, 1);
        assert_eq!(a_only.consumed_total, 1);
        assert_eq!(a_only.running_per_session.len(), 1);

        // Scoped to a session with nothing.
        let empty = tracker.subagent_snapshot(Some("sess-none"));
        assert_eq!(empty.running_total, 0);
        assert_eq!(empty.completed_total, 0);
        assert!(empty.running_per_session.is_empty());
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
        match tracker.wait("rid", None, Duration::from_secs(5)).await {
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
        match tracker.wait("rid", None, Duration::from_millis(30)).await {
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
            tracker.wait("ghost", None, Duration::from_millis(10)).await,
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
        match tracker.wait("rid", None, Duration::from_millis(10)).await {
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
        let waiter =
            tokio::spawn(async move { t2.wait("rid", None, Duration::from_secs(30)).await });
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
    fn mark_consumed_ignores_unknown_ids_and_carries_across_completion() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_consumed("ghost"); // no panic, no-op
        assert!(!tracker.is_consumed("ghost"));

        tracker.register("rid".into(), CancellationToken::new(), "t".into());
        tracker.mark_consumed("rid");
        // Still running: there is no outcome yet, so there is nothing to
        // suppress and nothing to report as consumed.
        assert!(!tracker.is_consumed("rid"));

        tracker.mark_completed("rid", CompletedOutcome::ok_text("x"));
        // ...but the intent survived the transition. This assertion is
        // deliberately INVERTED from its original form: dropping the intent on
        // the floor is precisely what let a model-issued `cancel` — which fires
        // the token and returns before the child unwinds — produce a spurious
        // announce turn about a sub-agent the parent itself killed.
        assert!(
            tracker.is_consumed("rid"),
            "consuming a running agent must make its result born consumed"
        );

        // Stamping an already-consumed completed entry stays idempotent.
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
        match tracker.wait_any(&ids, None, Duration::from_secs(5)).await {
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
        match tracker
            .wait_any(&ids, None, Duration::from_millis(30))
            .await
        {
            WaitAnyOutcome::TimedOut { mut still_running } => {
                still_running.sort();
                assert_eq!(still_running, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_any_not_found_carries_the_unknown_id_list() {
        // Round-8 — `NotFound` is no longer a bool; it carries the id list
        // so a caller can fix a typo in one round-trip instead of fishing
        // by issuing single-id waits. The variant means what its doc says:
        // *every* id in the set is unknown, so there is nothing to wait for.
        let tracker = BackgroundAgentTracker::new();
        tracker.register("known".into(), CancellationToken::new(), "live".into());
        let ids = vec!["ghost1".to_string(), "ghost2".to_string()];
        match tracker
            .wait_any(&ids, None, Duration::from_millis(10))
            .await
        {
            WaitAnyOutcome::NotFound { unknown_ids } => {
                let mut got = unknown_ids.clone();
                got.sort();
                assert_eq!(got, vec!["ghost1".to_string(), "ghost2".to_string()]);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// A typo does **not** abort a wait that still has live children in it.
    ///
    /// The tempting reading — "any unknown id ⇒ `NotFound`" — is actively
    /// harmful: the tool layer maps `NotFound` to a non-retryable
    /// `ToolResult::Error`, so one mistyped id in a five-way fan-out would
    /// throw away the wait on the four real children *and* record a verdict
    /// about the call in the cross-batch failure memo. The set keeps waiting;
    /// the typo is diagnosed out-of-band by [`unknown_ids`](BackgroundAgentTracker::unknown_ids),
    /// which `loop_tool` folds into every outcome as `unknown_request_ids`
    /// (`annotate_unknown`) — including the interrupted-wait report.
    #[tokio::test]
    async fn wait_any_keeps_waiting_when_ghosts_share_the_set_with_a_live_id() {
        let tracker = BackgroundAgentTracker::new();
        tracker.register("known".into(), CancellationToken::new(), "live".into());
        let ids = vec![
            "known".to_string(),
            "ghost1".to_string(),
            "ghost2".to_string(),
        ];
        match tracker
            .wait_any(&ids, None, Duration::from_millis(10))
            .await
        {
            WaitAnyOutcome::TimedOut { still_running } => {
                assert_eq!(still_running, vec!["known".to_string()]);
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
        // …and the ghosts are still nameable, which is what the tool layer
        // reports alongside the timeout.
        let mut unknown = tracker.unknown_ids(&ids, None);
        unknown.sort();
        assert_eq!(unknown, vec!["ghost1".to_string(), "ghost2".to_string()]);
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

    /// W12 — a running-only registration is visible to the Interrupt demote
    /// guard while in flight, and on drop leaves NEITHER a running NOR a
    /// completed entry (no announce source, nothing for `list`/`check_status`
    /// to re-deliver).
    #[test]
    fn running_registration_delists_on_drop_without_completed_entry() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let root = "agent:leader:peer:user";
        {
            let _reg = RunningRegistration::register(
                tracker.clone(),
                "sync-child".into(),
                CancellationToken::new(),
                "sync batch child".into(),
                SpawnMeta {
                    root_session: root.into(),
                    depth: 1,
                    ..SpawnMeta::default()
                },
            );
            assert!(
                tracker.session_has_running(root),
                "in-flight sync fan-out must be visible to the demote guard"
            );
        }
        assert!(!tracker.session_has_running(root));
        assert!(tracker.running_meta("sync-child", None).is_none());
        assert!(
            tracker.result_snapshot("sync-child", None).is_none(),
            "running-only registration must not retain a completed entry"
        );
    }

    /// A presence-only registration must NOT appear in the `subagent` tool's
    /// `list` action: its result is delivered inline at its own seam and it never
    /// reaches `completed`, so an advertised id would answer every follow-up
    /// `check_status` with "no background sub-agent found".
    #[test]
    fn presence_only_registration_is_hidden_from_list_running() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.register(
            "real-bg".into(),
            CancellationToken::new(),
            "background spawn".into(),
        );
        let _reg = RunningRegistration::register(
            tracker.clone(),
            "moa-proposal".into(),
            CancellationToken::new(),
            "MoA proposal".into(),
            SpawnMeta::default(),
        );
        let listed: Vec<String> = tracker
            .list_running(None)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        assert_eq!(listed, vec!["real-bg".to_string()]);
    }

    /// Same entry must be absent from the panel tree snapshot: nothing emits
    /// `Spawned` for it and its delist emits no `Settled`, so a snapshot row
    /// would sit frozen at `Running` for the rest of the session.
    #[test]
    fn presence_only_registration_is_hidden_from_flat_nodes() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let root = "agent:leader:peer:user";
        tracker.register_with_meta(
            "real-bg".into(),
            CancellationToken::new(),
            "background spawn".into(),
            SpawnMeta {
                root_session: root.into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        let _reg = RunningRegistration::register(
            tracker.clone(),
            "sync-child".into(),
            CancellationToken::new(),
            "sync batch child".into(),
            SpawnMeta {
                root_session: root.into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        let ids: Vec<String> = tracker
            .flat_nodes(Some(root))
            .into_iter()
            .map(|n| n.node_id)
            .collect();
        assert_eq!(ids, vec!["real-bg".to_string()]);
    }

    /// …while staying fully visible to everything the cancel path needs: the
    /// busy guard, the leader-cancel walk, the fan-out tree walk, and a
    /// cancel-by-id. Hiding it from the *enumeration* faces must not make it
    /// uncancellable.
    #[test]
    fn presence_only_registration_stays_visible_to_cancel_walks() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let root = "agent:leader:peer:user";
        let token = CancellationToken::new();
        let probe = token.clone();
        let _reg = RunningRegistration::register(
            tracker.clone(),
            "member-1".into(),
            token,
            "team-chat member".into(),
            SpawnMeta {
                parent_id: Some("tree-run".into()),
                depth: 1,
                root_session: root.into(),
                model: None,
            },
        );
        assert!(tracker.session_has_running(root), "busy guard must see it");
        assert_eq!(
            tracker.running_runs_of_session(root),
            vec!["member-1".to_string()],
            "leader-cancel walk must see it"
        );
        assert_eq!(
            tracker.running_children_of("tree-run"),
            vec!["member-1".to_string()],
            "fan-out tree walk must see it"
        );
        assert!(tracker.cancel("member-1"), "cancel-by-id must still hit");
        assert!(probe.is_cancelled());
    }

    /// W12 risk — the guard must delist on panic unwind so entries never leak
    /// into the demote guard permanently.
    #[test]
    fn running_registration_delists_on_panic_unwind() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let t2 = tracker.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _reg = RunningRegistration::register(
                t2,
                "panicky".into(),
                CancellationToken::new(),
                "t".into(),
                SpawnMeta::default(),
            );
            panic!("simulated fan-out child panic");
        }));
        assert!(result.is_err(), "inner closure must have panicked");
        assert!(
            tracker.running_meta("panicky", None).is_none(),
            "guard must delist during panic unwind"
        );
    }

    /// A `wait` parked on a running-only id must resolve to NotFound when the
    /// registration drops (delist wakes the completion notifier) instead of
    /// blocking out its full window.
    #[tokio::test]
    async fn wait_on_delisted_running_only_id_resolves_not_found() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let reg = RunningRegistration::register(
            tracker.clone(),
            "sync-x".into(),
            CancellationToken::new(),
            "t".into(),
            SpawnMeta::default(),
        );
        let t2 = tracker.clone();
        let waiter =
            tokio::spawn(async move { t2.wait("sync-x", None, Duration::from_secs(30)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(reg);
        let outcome = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("waiter must wake well before the 30s deadline")
            .expect("waiter task panicked");
        assert!(matches!(outcome, WaitOutcome::NotFound));
    }

    /// W13 — the teams.chat.cancel walk: members registered under a parent
    /// run_id are enumerable, unrelated entries are not, and a finished
    /// (dropped) member leaves the walk.
    #[test]
    fn running_children_of_walks_only_the_given_tree() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let tree = "tree-run-1";
        let _parent = RunningRegistration::register(
            tracker.clone(),
            tree.into(),
            CancellationToken::new(),
            "fan-out".into(),
            SpawnMeta {
                root_session: tree.into(),
                ..SpawnMeta::default()
            },
        );
        let m1 = RunningRegistration::register(
            tracker.clone(),
            "m1".into(),
            CancellationToken::new(),
            "member".into(),
            SpawnMeta {
                parent_id: Some(tree.into()),
                root_session: tree.into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        let _m2 = RunningRegistration::register(
            tracker.clone(),
            "m2".into(),
            CancellationToken::new(),
            "member".into(),
            SpawnMeta {
                parent_id: Some(tree.into()),
                root_session: tree.into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        let _other = RunningRegistration::register(
            tracker.clone(),
            "other".into(),
            CancellationToken::new(),
            "unrelated".into(),
            SpawnMeta::default(),
        );
        let mut kids = tracker.running_children_of(tree);
        kids.sort();
        assert_eq!(kids, vec!["m1".to_string(), "m2".to_string()]);
        drop(m1);
        assert_eq!(tracker.running_children_of(tree), vec!["m2".to_string()]);
    }

    /// W13 — the poison-then-walk order teams.chat.cancel relies on:
    /// `cancel(tree_run_id)` fires the stored tree-level token (stopping new
    /// member spawns), and the walk then lists in-flight members for the
    /// engine-side per-run abort.
    #[test]
    fn tree_cancel_fires_tree_token_and_members_stay_enumerable() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        let tree_token = CancellationToken::new();
        let _tree = RunningRegistration::register(
            tracker.clone(),
            "tree".into(),
            tree_token.clone(),
            "fan-out".into(),
            SpawnMeta {
                root_session: "tree".into(),
                ..SpawnMeta::default()
            },
        );
        let _m = RunningRegistration::register(
            tracker.clone(),
            "m1".into(),
            tree_token.child_token(),
            "member".into(),
            SpawnMeta {
                parent_id: Some("tree".into()),
                root_session: "tree".into(),
                depth: 1,
                ..SpawnMeta::default()
            },
        );
        assert!(
            tracker.cancel("tree"),
            "tree node must be found while the fan-out runs"
        );
        assert!(
            tree_token.is_cancelled(),
            "poison must fire the tree-level token"
        );
        assert_eq!(tracker.running_children_of("tree"), vec!["m1".to_string()]);
    }

    /// Round-8 — `running_children_of` returns a deterministic order
    /// `(started_at_ms, request_id)`, so a `teams.chat.cancel` re-walk
    /// reaches members in the same sequence and audit logs read stably.
    #[test]
    fn running_children_of_sorts_by_started_at_then_id() {
        let tracker = BackgroundAgentTracker::new();
        let tree = "tree";
        for id in ["m2", "m1", "m3"] {
            tracker.register_with_meta(
                id.into(),
                CancellationToken::new(),
                "member".into(),
                SpawnMeta {
                    parent_id: Some(tree.into()),
                    root_session: tree.into(),
                    depth: 1,
                    ..SpawnMeta::default()
                },
            );
            // Force a distinguishable spawn time per id so the sort is
            // meaningful (and not just falling back to the id tiebreak).
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let first = tracker.running_children_of(tree);
        let second = tracker.running_children_of(tree);
        assert_eq!(first, second, "running_children_of must be deterministic");
        assert_eq!(
            first,
            vec!["m2".to_string(), "m1".to_string(), "m3".to_string()],
            "sort is by started_at_ms (insertion order with 2ms gap) then id"
        );
    }

    // -- W11: the addressing face ----------------------------------------

    /// The tracker is process-global. Scoping only the ENUMERATION face made
    /// a foreign request_id hard to guess, not unreachable: a model holding
    /// one (announce echo, log line, paste) could still read the other
    /// session's result and cancel its run. Every by-id verb now goes through
    /// [`BackgroundAgentTracker::addressable`].
    #[test]
    fn foreign_request_id_is_not_addressable() {
        let tracker = BackgroundAgentTracker::new();
        register_in(&tracker, "mine", "s-mine");
        register_in(&tracker, "theirs", "s-other");
        tracker.mark_completed("theirs", CompletedOutcome::ok_text("secret result"));

        let mine = Some("s-mine");

        assert!(tracker.addressable("mine", mine));
        assert!(!tracker.addressable("theirs", mine));

        // Read faces: a foreign id answers exactly like a never-registered one.
        assert!(
            tracker.result_snapshot("theirs", mine).is_none(),
            "another session's completed result must not be readable"
        );
        assert!(tracker.running_meta("theirs", mine).is_none());
        assert!(
            !tracker.cancel_in_scope("theirs", mine),
            "another session's run must not be cancellable"
        );

        // The unscoped face (CLI / direct construction / tests) is unchanged.
        assert!(tracker.result_snapshot("theirs", None).is_some());
        assert!(tracker.addressable("theirs", None));
    }

    /// The `annotate_unknown` sentence shown to the model claims the id is
    /// unknown. Before the addressing chokepoint a foreign id was omitted
    /// from this list AND readable through `check_status` — the sentence was
    /// false in both directions. An out-of-scope id must now be reported the
    /// same way a typo is, so the two stay indistinguishable.
    #[test]
    fn unknown_ids_reports_out_of_scope_ids() {
        let tracker = BackgroundAgentTracker::new();
        register_in(&tracker, "mine", "s-mine");
        register_in(&tracker, "theirs", "s-other");

        let probe = vec!["mine".to_string(), "theirs".to_string(), "typo".to_string()];
        let mut unknown = tracker.unknown_ids(&probe, Some("s-mine"));
        unknown.sort();
        assert_eq!(
            unknown,
            vec!["theirs".to_string(), "typo".to_string()],
            "an out-of-scope id must be reported as unknown, exactly like a typo"
        );

        assert!(
            tracker
                .unknown_ids(&probe, None)
                .contains(&"typo".to_string()),
            "unscoped callers still only lose genuinely unknown ids"
        );
        assert!(!tracker
            .unknown_ids(&probe, None)
            .contains(&"theirs".to_string()));
    }

    /// The scope check lives INSIDE `wait_any`'s loop, not just at its
    /// entrance: a foreign id that completes while the call is parked must
    /// not be claimable on the notifier-driven re-check either. Filtering
    /// once on entry would have left exactly that window open.
    #[tokio::test]
    async fn wait_any_does_not_claim_a_foreign_completion_while_parked() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        register_in(&tracker, "mine", "s-mine");
        register_in(&tracker, "theirs", "s-other");

        let ids = vec!["mine".to_string(), "theirs".to_string()];
        let waiter = {
            let t = tracker.clone();
            let ids = ids.clone();
            tokio::spawn(async move {
                t.wait_any(&ids, Some("s-mine"), Duration::from_secs(5))
                    .await
            })
        };

        // The foreign run finishes first and wakes every parker.
        tokio::time::sleep(Duration::from_millis(30)).await;
        tracker.mark_completed("theirs", CompletedOutcome::ok_text("not yours"));

        // ...and the in-scope one finishes after it, so the wait resolves
        // deterministically instead of relying on a timeout.
        tokio::time::sleep(Duration::from_millis(30)).await;
        tracker.mark_completed("mine", CompletedOutcome::ok_text("yours"));

        match waiter.await.expect("waiter task must not panic") {
            WaitAnyOutcome::Completed {
                request_id,
                snapshot,
            } => {
                assert_eq!(
                    request_id, "mine",
                    "the foreign completion must never satisfy this wait"
                );
                match snapshot.outcome {
                    CompletedOutcome::Ok { final_text, .. } => assert_eq!(final_text, "yours"),
                    other => panic!("expected the in-scope result, got {other:?}"),
                }
            }
            other => panic!("expected the in-scope completion, got {other:?}"),
        }
    }

    /// W10 fallback — a sub-agent cancelled while QUEUED behind the
    /// concurrency semaphore may settle without ever having had a session (or
    /// even a running entry, if its registration was never made). The
    /// no-running-entry arm of `mark_completed` must still produce a terminal
    /// node, and the byte-exact producer string must still classify it as
    /// Cancelled rather than Failed — otherwise the panel keeps a `Spawned`
    /// row with no matching terminal state.
    #[test]
    fn cancelled_while_queued_settles_even_without_a_running_entry() {
        let tracker = BackgroundAgentTracker::new();
        tracker.mark_completed(
            "queued-then-cancelled",
            CompletedOutcome::Err("sub-agent failed: cancelled".to_string()),
        );

        let nodes = tracker.flat_nodes(None);
        let node = nodes
            .iter()
            .find(|n| n.node_id == "queued-then-cancelled")
            .expect("a terminal node must exist even with no prior registration");
        assert_eq!(
            node.lifecycle,
            NodeLifecycle::Cancelled,
            "the queue-cancel string must settle as Cancelled, not Failed"
        );
    }
}
