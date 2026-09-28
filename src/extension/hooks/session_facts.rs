//! The session facts a hook payload carries that no fire site has to supply.
//!
//! Claude Code puts `transcript_path` and `cwd` (and `$CLAUDE_PROJECT_DIR`)
//! on EVERY event. Asking each fire site to remember them left most Claude
//! Code faces without them (判据 §11: fix the executor, not each instance).
//! The executor derives them here instead, from task-locals the run
//! publishes — so a face carries a fact exactly when the task it fires in
//! can still see that task-local. Absent means "unknown", never a guess.
//!
//! Where each fact reaches today:
//!
//! - **The run task** (`BeforeAgentStart`, `SessionStart`,
//!   `UserPromptSubmit`, compaction, `AgentEnd`): both, when the run has a
//!   project root or an exec workspace (`cwd`) and a file-backed store
//!   (`transcript_path`).
//! - **The harness task** `orchestrator::dispatch` spawns (every tool face —
//!   `PreToolUse` / `PostToolUse` / `PermissionRequest` / `PermissionDenied`
//!   / …, `Stop`, the provider faces, a foreground sub-agent's
//!   `SubagentStart` / `SubagentStop`): it re-establishes the transcript
//!   source and the project root but not the exec workspace, so `cwd` is
//!   there only in a project run.
//! - **Work spawned through `CarriedAttribution`** (sync fan-out batch legs,
//!   background sub-agents): it carries the project root and exec workspace
//!   the spawning task had (so `cwd` as there), but not the transcript
//!   source — no `transcript_path`.
//! - **Faces fired outside any run** (`SessionEnd` from the RPC, inbound
//!   `MessageReceived`, the gateway start/stop observers, `aleph-server hooks
//!   test`): neither.

use std::path::PathBuf;

use super::HookContext;
use crate::sync_primitives::Arc;

/// Answers "which file holds this session's transcript" for the hook payload.
///
/// Declared here, where it is read, and implemented by the owner of the
/// session store (the gateway publishes the live store through
/// [`with_transcript_source`]), so the executor never names the gateway.
pub trait TranscriptSource: Send + Sync {
    /// `Some` only for a file a hook can open now; `None` when the store keeps
    /// no file for `session_id` (the SQLite backend) or has none yet.
    fn transcript_path(&self, session_id: &str) -> Option<PathBuf>;
}

tokio::task_local! {
    static TRANSCRIPTS: Option<Arc<dyn TranscriptSource>>;
}

/// Run `fut` with `source` answering the transcript lookup of every hook fired
/// inside it. Takes `Option` and always scopes, so `None` shadows an outer
/// value — the shape of `projects::with_project_root` and
/// `sandbox::context::with_exec_workspace`.
///
/// A task spawned with `tokio::spawn` does not inherit the scope. The
/// harness task in `orchestrator::dispatch` — the spawn between a run and
/// its tool hooks — re-establishes it from [`current_transcript_source`];
/// work spawned further out (the sub-agent paths through
/// `CarriedAttribution`) omits the key.
pub async fn with_transcript_source<F>(
    source: Option<Arc<dyn TranscriptSource>>,
    fut: F,
) -> F::Output
where
    F: std::future::Future,
{
    TRANSCRIPTS.scope(source, fut).await
}

/// The source in scope, for a caller that must carry it across a
/// `tokio::spawn` (read it BEFORE the spawn; inside, it is already gone).
#[must_use]
pub fn current_transcript_source() -> Option<Arc<dyn TranscriptSource>> {
    TRANSCRIPTS.try_with(Clone::clone).ok().flatten()
}

/// The facts the executor derives for one hook action — once, so the stdin
/// payload and the environment of the same command read the same answer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SessionFacts {
    /// The payload's `cwd` AND `$CLAUDE_PROJECT_DIR` — one value, so the two
    /// cannot disagree. The fire site's explicit `working_dir`, else the run's
    /// project root (`projects::with_project_root`, the same task-local the
    /// project-scope gate reads), else the workspace the run is authorised to
    /// execute in (`sandbox::context::with_exec_workspace`). `None` when none
    /// of those is in scope — outside a run, and in the harness task of a
    /// run without a project (it does not carry the exec workspace; see the
    /// module doc) — never the daemon's own cwd (the lie
    /// `thinker/runtime_context.rs` already removed once), never the hook's
    /// `plugin_root`.
    ///
    /// Read from the RAW task-local, not `VisibilityCtx::for_session`: that
    /// one falls back to the daemon cwd itself, so it is never `None`.
    pub(super) cwd: Option<PathBuf>,
    /// `transcript_path`, from the published [`TranscriptSource`].
    pub(super) transcript_path: Option<PathBuf>,
}

impl SessionFacts {
    pub(super) fn derive(context: &HookContext) -> Self {
        Self {
            cwd: context
                .working_dir
                .clone()
                .or_else(crate::projects::current_project_root)
                .or_else(crate::sandbox::context::current_exec_workspace),
            transcript_path: current_transcript_source()
                .and_then(|source| source.transcript_path(&context.session_id)),
        }
    }
}
