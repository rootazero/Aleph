//! The session facts a hook payload carries that no fire site has to supply.
//!
//! Claude Code puts `transcript_path` and `cwd` (and `$CLAUDE_PROJECT_DIR`)
//! on EVERY event. Asking each fire site to remember them left most Claude
//! Code faces without them (判据 §11: fix the executor, not each instance).
//! The executor derives them here instead, from
//! what the run publishes, so every face fired inside a run carries them
//! whichever executor fires it, and a face fired outside one omits them:
//! absent means "unknown", never a guess.

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
    static TRANSCRIPTS: Arc<dyn TranscriptSource>;
}

/// Run `fut` with `source` answering the transcript lookup of every hook fired
/// inside it. A task spawned with `tokio::spawn` does not inherit the scope;
/// its hooks omit the key.
pub async fn with_transcript_source<F>(source: Arc<dyn TranscriptSource>, fut: F) -> F::Output
where
    F: std::future::Future,
{
    TRANSCRIPTS.scope(source, fut).await
}

/// The facts the executor derives for one hook action — once, so the stdin
/// payload and the environment of the same command read the same answer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SessionFacts {
    /// The payload's `cwd` AND `$CLAUDE_PROJECT_DIR` — one value, so the two
    /// cannot disagree. The fire site's explicit `working_dir`, else the run's
    /// project root (`projects::with_project_root`, the same task-local the
    /// project-scope gate reads), else the workspace the run is authorised to
    /// execute in (`sandbox::context::with_exec_workspace`). `None` outside a
    /// run: never the daemon's own cwd (the lie `thinker/runtime_context.rs`
    /// already removed once), never the hook's `plugin_root`.
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
            transcript_path: TRANSCRIPTS
                .try_with(|source| source.transcript_path(&context.session_id))
                .ok()
                .flatten(),
        }
    }
}
