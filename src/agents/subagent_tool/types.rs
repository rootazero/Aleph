//! Input argument types parsed out of the subagent tool JSON payload.

/// A2 — default cap on concurrently-running subagent spawns per top-level
/// agent run. Matches the deleted `Lane::Subagent` default.
pub(super) const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
pub(super) enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
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
    /// Optional name — makes the agent addressable.
    pub(super) name: Option<String>,
    /// Optional team name — enables shared tasks and messages.
    pub(super) team_name: Option<String>,
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
