//! Input argument types parsed out of the subagent tool JSON payload.

/// A2 — default cap on concurrently-running subagent spawns per top-level
/// agent run. Matches the deleted `Lane::Subagent` default, and is what
/// `[execution] max_concurrent_subagents` defaults to (that key binds to this
/// constant, not to a second literal).
pub const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;

/// Floor for [`clamp_max_concurrent_subagents`]. `0` is not "disabled", it is a
/// semaphore no child can ever acquire: every fan-out would park until its
/// batch deadline and return partial results with nothing attempted.
pub const MIN_CONCURRENT_SUBAGENTS: usize = 1;

/// Ceiling for [`clamp_max_concurrent_subagents`]. Generous — the real limits
/// (provider rate limits, the process's task budget) bite well before it — but
/// finite, so a typo in `config.toml` cannot turn one `batch_tasks` call into
/// an unbounded spawn storm.
pub const MAX_CONCURRENT_SUBAGENTS_CEILING: usize = 64;

/// Process-global cap read by [`SubagentTool::new`](super::SubagentTool::new)
/// when it builds a run's concurrency semaphore.
///
/// A process global rather than a constructor argument because `SubagentTool`
/// is constructed from several places (the gateway run loop, tool-scope tests,
/// embedding code) and a builder that only one of them calls is how a knob ends
/// up configurable in exactly one code path. A live `[execution]` patch binds
/// on the next session's first fan-out rather than resizing one already in
/// flight.
static MAX_CONCURRENT_SUBAGENTS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(DEFAULT_MAX_CONCURRENT_SUBAGENTS);

/// One semaphore per top-level session, shared across that session's runs.
///
/// The cap has to outlive the thing it caps. `SubagentTool` is rebuilt per
/// request (`run_loop/inner.rs` says so in as many words), and background
/// sub-agents are detached tasks that deliberately outlive the run that spawned
/// them — so a per-instance semaphore meant turn N's four children held permits
/// from one semaphore while turn N+1 got a brand-new one with four more. A
/// session fanning out four background children per turn could have dozens
/// hitting the provider at once while the operator had configured four, and the
/// knob's stated reason for existing is "a deployment behind a tight rate limit
/// had no way to narrow it".
///
/// `Weak`, not `Arc`: the entry disappears once the last live permit and the
/// last tool instance are gone (an `OwnedSemaphorePermit` holds its `Arc`, so
/// an in-flight background child keeps its own session's semaphore alive), and
/// the next fan-out builds a fresh one at the current configured size. Nothing
/// to evict, no TTL, no growth in an idle process.
///
/// This is the same lifetime correction `BackgroundAgentTracker` already
/// carries — that one is engine-lifetime because "a per-request tracker
/// silently dropped every result once the spawning run returned". Same
/// mismatch, opposite symptom: there results were lost, here the constraint is.
static SESSION_SEMAPHORES: std::sync::LazyLock<
    crate::sync_primitives::Mutex<
        std::collections::HashMap<String, std::sync::Weak<tokio::sync::Semaphore>>,
    >,
> = std::sync::LazyLock::new(|| {
    crate::sync_primitives::Mutex::new(std::collections::HashMap::new())
});

/// The concurrency semaphore a `SubagentTool` should use.
///
/// `None` — a caller with no owning session (CLI, direct construction, tests) —
/// gets a private semaphore, which is byte-for-byte the previous behaviour and
/// keeps process-global state out of unit tests.
#[must_use]
pub fn subagent_semaphore_for(session_key: Option<&str>) -> std::sync::Arc<tokio::sync::Semaphore> {
    let permits = max_concurrent_subagents();
    let Some(key) = session_key.filter(|k| !k.is_empty()) else {
        return std::sync::Arc::new(tokio::sync::Semaphore::new(permits));
    };
    let mut map = SESSION_SEMAPHORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = map.get(key).and_then(std::sync::Weak::upgrade) {
        return existing;
    }
    // Opportunistic prune while we already hold the lock: every dead entry is a
    // session whose children have all finished.
    map.retain(|_, weak| weak.strong_count() > 0);
    let fresh = std::sync::Arc::new(tokio::sync::Semaphore::new(permits));
    map.insert(key.to_string(), std::sync::Arc::downgrade(&fresh));
    fresh
}

/// Clamp an operator-supplied cap into `[MIN, CEILING]`.
#[must_use]
pub const fn clamp_max_concurrent_subagents(requested: usize) -> usize {
    if requested < MIN_CONCURRENT_SUBAGENTS {
        MIN_CONCURRENT_SUBAGENTS
    } else if requested > MAX_CONCURRENT_SUBAGENTS_CEILING {
        MAX_CONCURRENT_SUBAGENTS_CEILING
    } else {
        requested
    }
}

/// Install `[execution] max_concurrent_subagents`. Returns the value that
/// actually took effect after clamping, so a caller reporting back to an
/// operator reports the truth rather than the request.
pub fn set_max_concurrent_subagents(requested: usize) -> usize {
    let effective = clamp_max_concurrent_subagents(requested);
    MAX_CONCURRENT_SUBAGENTS.store(effective, std::sync::atomic::Ordering::Relaxed);
    effective
}

/// The cap a sub-agent fan-out started right now would run under.
#[must_use]
pub fn max_concurrent_subagents() -> usize {
    MAX_CONCURRENT_SUBAGENTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Default bounded window for the `wait` action (seconds). When it elapses with
/// the child still running, `wait` returns `still_running` and the parent model
/// may re-issue `wait` to keep blocking — a longer wait is expressed as
/// repeated bounded waits, never one unbounded block.
pub(super) const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 120;

/// Ceiling on a single `wait` window (seconds). Kept well under the subagent
/// tool's 1_800_000ms wall-clock budget so one `wait` can never hang the turn;
/// the model chunks a longer wait into repeated bounded calls.
pub(super) const MAX_WAIT_TIMEOUT_SECS: u64 = 600;

/// Default wall-clock ceiling for one `run` (seconds) when the caller omits
/// `timeout_secs`. Must stay in sync with the schema's advertised default.
pub(super) const DEFAULT_RUN_TIMEOUT_SECS: u64 = 120;

/// Round-8 — how many finished sub-agents the `list` action renders, newest
/// first. The full result text is one `check_status` away — `list` is a
/// directory, not a payload. Lives in `types.rs` next to the timeout
/// constants so a single module owns the sub-agent tool's shape
/// (entropic-reduction; the old inline const in `loop_tool.rs` was the
/// only Knob in the file not co-located with its peers).
pub(super) const MAX_LISTED_COMPLETED: usize = 20;

/// Round-8 — characters of a finished sub-agent's result kept on a `list`
/// directory row AND on `SubagentNode.result_preview` (Panel tree view).
/// Single source so the tree row and the `list` row read as the same
/// preview width.
pub(super) const LIST_RESULT_PREVIEW_CHARS: usize = 200;

/// Gap between a child's own wall-clock timeout and the `subagent` tool's
/// advertised budget, so the CHILD's `tokio::time::timeout` is what fires. The
/// model then reads "Sub-agent timed out after Ns" (actionable: it knows which
/// child, and can retry with a smaller task) instead of an opaque tool-budget
/// overrun on the whole call. Same clock-ordering invariant the `ask_user` row
/// in `tools::budget` documents, applied in the other direction.
const RUN_TIMEOUT_HEADROOM_SECS: u64 = 300;

/// Ceiling used when `subagent` is somehow absent from the builtin budget table.
const FALLBACK_MAX_RUN_TIMEOUT_SECS: u64 = 1_500;

/// Ceiling on one `run` (and on each `batch_tasks` entry), derived from the
/// tool's own budget so the two never drift apart.
///
/// A model-supplied `timeout_secs` is clamped into `[1, this]`: `0` would make
/// `tokio::time::timeout` elapse immediately ("timed out after 0s" before the
/// child ever thinks), and anything above this lets the harness budget fire
/// first, discarding a child that was still working.
pub(super) fn max_run_timeout_secs() -> u64 {
    crate::tools::budget::builtin_tool_budget_ms("subagent")
        .map(|ms| (ms / 1000).saturating_sub(RUN_TIMEOUT_HEADROOM_SECS))
        .filter(|secs| *secs > 0)
        .unwrap_or(FALLBACK_MAX_RUN_TIMEOUT_SECS)
}

/// Slack added on top of the computed fan-out wall clock before the sync batch
/// gives up and returns partial results.
///
/// The per-child `tokio::time::timeout` inside the spawner must stay the thing
/// that fires for a child that is actually *running* — that error names the
/// child and is actionable. The batch deadline is a backstop for a child that
/// never starts (its permit wait happens BEFORE its own timeout arms, so a
/// queued child is invisible to every per-child clock). Without this slack the
/// last wave's own timer and the batch deadline expire in the same instant and
/// which one wins is a scheduling race. Comes out of
/// [`RUN_TIMEOUT_HEADROOM_SECS`], which is an order of magnitude larger.
pub(super) const BATCH_FANOUT_SLACK_SECS: u64 = 30;

/// Grace window after the batch deadline in which still-running children are
/// asked to stop COOPERATIVELY (their cancel token) before the tasks are
/// aborted outright. A cooperative unwind runs the child's worktree cleanup and
/// MCP-scope shutdown; an abort leaves both to their Drop safety nets.
pub(super) const BATCH_CANCEL_GRACE_SECS: u64 = 2;

/// Upper bound on how long the abort drain may take. Dropping the `JoinSet`
/// aborts whatever is left anyway, so this only stops a child that refuses to
/// yield from holding the tool call open.
pub(super) const BATCH_ABORT_DRAIN_SECS: u64 = 5;

/// Wave-aware ceiling on ONE child's wall clock inside a synchronous batch.
///
/// The clamp in `parse` bounds a child by [`max_run_timeout_secs`], which
/// assumes **one child per call**. A batch of `rows` against `permits`
/// concurrency slots does not run flat: it runs in `ceil(rows / permits)`
/// WAVES, and an MoA reduce adds one more serial round after them. So the call's
/// real wall clock is `per_child × (waves + agg_rounds)`, and clamping each
/// child to the whole share guarantees the tool budget fires first and the
/// entire batch is discarded — five 1500 s children against four permits is two
/// waves, i.e. 3000 s of work inside a 1500 s share.
///
/// `permits` is read from the live semaphore rather than assumed, so permits
/// already held by this run's background children (which really do narrow this
/// batch's concurrency) shorten the per-child share instead of being wished
/// away.
pub(super) fn wave_aware_child_timeout_cap(rows: usize, permits: usize, agg_rounds: usize) -> u64 {
    let rounds = u64::try_from(wave_count(rows, permits).saturating_add(agg_rounds))
        .unwrap_or(u64::MAX)
        .max(1);
    (max_run_timeout_secs() / rounds).max(1)
}

/// How many serial waves a `rows`-wide fan-out takes through `permits`
/// concurrency slots. Single source: the per-child clamp divides the share by
/// it and the batch deadline multiplies it back out, so the two must agree by
/// construction rather than by two matching expressions.
pub(super) fn wave_count(rows: usize, permits: usize) -> usize {
    rows.max(1).div_ceil(permits.max(1))
}

/// Every top-level argument key the tool accepts.
///
/// This is the parser's half of the tool contract; the schema in `loop_tool.rs`
/// is the model-facing half, and `subagent_schema_properties_match_accepted_keys`
/// (tests) pins the two together — the schema is hand-written, so nothing else
/// stops them from drifting.
///
/// `name` is listed even though the schema does NOT advertise it: it has a
/// dedicated "sub-agents are not addressable teammates" rejection further down
/// the parser, and that specific message is far more useful than a generic
/// unknown-key error.
pub(super) const ACCEPTED_ARG_KEYS: &[&str] = &[
    "action",
    "agent_type",
    "aggregator_model",
    "batch_tasks",
    "context_summary",
    "model",
    "name",
    "proposer_models",
    "request_id",
    "request_ids",
    "run_in_background",
    "synthesis_instruction",
    "synthesize",
    "task",
    "team_name",
    "text",
    "timeout_secs",
    "to",
];

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
pub(super) enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
    /// Park until a background sub-agent finishes or the bounded window elapses
    /// (event-driven, no per-check LLM turn). One id → wait for that agent;
    /// many ids → wait for whichever finishes first (fan-out first-completion).
    /// Mirrors `check_status`'s output shape on completion; returns
    /// `still_running` on timeout.
    Wait {
        request_ids: Vec<String>,
        timeout_secs: u64,
    },
    /// Cancel a still-running background sub-agent by `request_id`. Hits the
    /// shared `CancellationToken`; the running task observes it on its next
    /// await point and unwinds cleanly. Idempotent — cancelling an unknown
    /// or already-completed request returns an error rather than panicking.
    Cancel(String),
    /// Send a message to a named teammate.
    SendMessage {
        to: String,
        text: String,
        team_name: String,
    },
    /// Read inbox messages.
    ReadInbox { team_name: String },
    /// Enumerate every background sub-agent (running + recently-completed)
    /// so the parent can recover `request_ids` it no longer holds.
    List,
}

/// A single task within a batch execution.
#[derive(Debug, Clone)]
pub(super) struct BatchTask {
    pub(super) task: String,
    pub(super) agent_type: Option<String>,
    pub(super) model: Option<String>,
    pub(super) timeout_secs: Option<u64>,
}

#[derive(Debug)]
pub(super) struct RunArgs {
    pub(super) task: String,
    pub(super) agent_type: Option<String>,
    pub(super) model: Option<String>,
    pub(super) timeout_secs: u64,
    pub(super) run_in_background: bool,
    pub(super) context_summary: Option<String>,
    /// Batch tasks for parallel execution. When provided, all tasks run in
    /// background automatically and a list of `request_ids` is returned.
    pub(super) batch_tasks: Option<Vec<BatchTask>>,
    /// Mixture-of-Agents convenience: replicate the top-level `task` across
    /// these models as parallel proposers. `["claude-opus-4-8", "gpt-5"]`
    /// expands to one batch entry per model running the same prompt — the
    /// classic MoA shape. Ignored when explicit `batch_tasks` is supplied.
    pub(super) proposer_models: Option<Vec<String>>,
    /// When true, after a synchronous batch fans out and every proposer
    /// returns, run ONE aggregator sub-agent that synthesizes the proposals
    /// into a single answer (Mixture-of-Agents reduce step). Requires a
    /// foreground batch (`run_in_background = false`).
    pub(super) synthesize: bool,
    /// Model for the aggregator run. Falls back to the top-level `model`,
    /// then the agent's default. Use a strong model here for best synthesis.
    pub(super) aggregator_model: Option<String>,
    /// Optional extra guidance handed to the aggregator on top of the default
    /// "merge the strongest parts, resolve conflicts" instruction.
    pub(super) synthesis_instruction: Option<String>,
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;

    /// The clamp is what stands between `config.toml` and a semaphore that
    /// either deadlocks every fan-out (`0`) or does not bound one at all.
    #[test]
    fn the_configured_cap_is_clamped_at_both_ends() {
        assert_eq!(clamp_max_concurrent_subagents(0), MIN_CONCURRENT_SUBAGENTS);
        assert_eq!(
            clamp_max_concurrent_subagents(usize::MAX),
            MAX_CONCURRENT_SUBAGENTS_CEILING
        );
        assert_eq!(clamp_max_concurrent_subagents(7), 7);
        assert_eq!(
            clamp_max_concurrent_subagents(DEFAULT_MAX_CONCURRENT_SUBAGENTS),
            DEFAULT_MAX_CONCURRENT_SUBAGENTS
        );
    }

    /// The setter reports what actually took effect, not what was asked for —
    /// an operator who writes `max_concurrent_subagents = 0` must not be told
    /// their fan-out is disabled when it is really running at width 1.
    /// `serial` because the three tests that install a cap share one process
    /// global (this one, `subagent_tool::tests::
    /// a_new_tool_fans_out_at_the_configured_concurrency`, and
    /// `config::live_apply::tests::
    /// the_execution_arm_installs_the_subagent_concurrency_cap`). Each does
    /// save → set → observe → restore, which the default parallel runner is
    /// free to interleave — an intermittent failure that reads as a real bug.
    #[test]
    #[serial_test::serial(subagent_concurrency_cap)]
    fn the_setter_reports_the_effective_value() {
        let restore = max_concurrent_subagents();
        assert_eq!(set_max_concurrent_subagents(0), MIN_CONCURRENT_SUBAGENTS);
        assert_eq!(max_concurrent_subagents(), MIN_CONCURRENT_SUBAGENTS);
        assert_eq!(
            set_max_concurrent_subagents(usize::MAX),
            MAX_CONCURRENT_SUBAGENTS_CEILING
        );
        set_max_concurrent_subagents(restore);
        assert_eq!(max_concurrent_subagents(), restore);
    }

    /// The cap has to survive the tool instance that reads it.
    ///
    /// `SubagentTool` is rebuilt per request while background children are
    /// detached and outlive their run, so a per-instance semaphore let turn N+1
    /// hand out a second full allocation on top of turn N's still-held permits.
    /// Two lookups for one session must be the same object; two sessions must
    /// not share one.
    #[test]
    fn one_session_shares_one_semaphore_across_its_runs() {
        let a1 = subagent_semaphore_for(Some("agent:main:peer:alice"));
        let a2 = subagent_semaphore_for(Some("agent:main:peer:alice"));
        assert!(
            std::sync::Arc::ptr_eq(&a1, &a2),
            "a session's second run must share the first run's cap"
        );

        let b = subagent_semaphore_for(Some("agent:main:peer:bob"));
        assert!(
            !std::sync::Arc::ptr_eq(&a1, &b),
            "separate sessions keep separate caps"
        );

        // Taking a permit on one is visible to the other — the actual property,
        // not just pointer identity.
        let before = a2.available_permits();
        let _permit = a1.try_acquire().expect("a permit is available");
        assert_eq!(a2.available_permits(), before - 1);
    }

    /// No session (CLI, tests, direct construction) keeps its own semaphore, so
    /// none of this touches process-global state on those paths.
    #[test]
    fn an_unowned_tool_gets_a_private_semaphore() {
        let a = subagent_semaphore_for(None);
        let b = subagent_semaphore_for(None);
        assert!(!std::sync::Arc::ptr_eq(&a, &b));
        // An empty key is "no session", not a session named "".
        let c = subagent_semaphore_for(Some(""));
        let d = subagent_semaphore_for(Some(""));
        assert!(!std::sync::Arc::ptr_eq(&c, &d));
    }

    /// The whole point, stated as permits rather than pointers: a background
    /// child still holding a permit must count against the NEXT run's cap, and
    /// once every child and tool is gone the entry is collectable so the next
    /// fan-out reads the current configured size.
    ///
    /// `serial` because it reads the shared cap global.
    #[test]
    #[serial_test::serial(subagent_concurrency_cap)]
    fn a_live_child_still_counts_against_the_next_runs_cap() {
        let cap = max_concurrent_subagents();
        let key = "agent:main:peer:ephemeral";

        // Turn N: one background child takes a permit, then the tool instance
        // that built the semaphore is dropped at end of request.
        let child_permit = {
            let sem = subagent_semaphore_for(Some(key));
            sem.clone()
                .try_acquire_owned()
                .expect("a permit is available")
        };

        // Turn N+1 — the bug was that this handed out a brand-new allocation.
        let next_turn = subagent_semaphore_for(Some(key));
        assert_eq!(
            next_turn.available_permits(),
            cap - 1,
            "a still-running background child must consume the next run's budget"
        );

        drop(child_permit);
        drop(next_turn);

        let after = subagent_semaphore_for(Some(key));
        assert_eq!(
            after.available_permits(),
            cap,
            "once the session is idle its cap is whole again"
        );
    }
}
