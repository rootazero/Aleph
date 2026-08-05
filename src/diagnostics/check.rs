//! The `HealthCheck` contract and run posture.
//!
//! Every domain check (filesystem, lock, config, consent) implements one
//! trait. The engine treats them uniformly and runs them concurrently. A
//! check owns BOTH detection and — when asked — mechanical repair, so the
//! "can I fix this?" knowledge lives next to the "is this broken?" logic
//! rather than in a separate switchboard (avoids the openclaw detect/repair
//! id-matching seam).

use std::time::Duration;

use async_trait::async_trait;

use super::finding::Finding;

/// Wall-clock ceiling a check gets before the engine gives up on it.
///
/// Every check either touches the filesystem, scans the process table, shells
/// out (`which`), or opens a SQLite file — all of which can block on something
/// outside this process. Without a deadline one wedged check hangs the whole
/// report, and because the `doctor` tool runs inside an agent turn, that means
/// a hung *turn* whose only symptom is silence. A timed-out check is folded
/// into an ordinary `Warning` finding naming the check, so the rest of the
/// battery still reports.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Controls whether a run is allowed to mutate state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// Read-only detection for human output.
    Inspect,
    /// Read-only detection for machine output (CLI renders JSON). Behaves
    /// identically to `Inspect` at the check level — the difference is only
    /// in how the engine's report is formatted.
    Lint,
    /// Detect, then apply mechanical repairs for repairable findings.
    Fix,
}

impl Posture {
    /// Whether checks may mutate state on this run.
    #[must_use]
    pub const fn allows_repair(self) -> bool {
        matches!(self, Self::Fix)
    }
}

/// A single diagnostic domain. Implementations resolve the resources they
/// inspect at construction time (paths, registries) so they can be unit
/// tested against temp fixtures without touching the real home directory.
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Stable, namespaced identifier, e.g. `core/data-dir`.
    fn id(&self) -> &'static str;

    /// Short human label for headers.
    fn title(&self) -> &'static str;

    /// Run detection. When `posture.allows_repair()` and a problem is
    /// repairable, the check should apply the repair and record the outcome
    /// on the returned finding via `Finding::with_repair`.
    async fn run(&self, posture: Posture) -> Vec<Finding>;

    /// This check's wall-clock ceiling. Override only when the check has a
    /// *known* inner bound that legitimately exceeds
    /// [`DEFAULT_CHECK_TIMEOUT`] — state the inner bound in the override's
    /// doc comment so the number stays traceable to a real budget rather than
    /// becoming a magic constant.
    fn timeout(&self) -> Duration {
        DEFAULT_CHECK_TIMEOUT
    }
}
