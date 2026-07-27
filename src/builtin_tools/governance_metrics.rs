//! `governance_metrics` tool — in-core reality probe for the loop-governance
//! audit ring.
//!
//! The weekly audit loop's job is "do the other loops' numbers still touch
//! reality". It used to read that reality by shelling out to `sqlite3` against
//! `~/.aleph/data/*.db`. But every `bash` command runs inside the per-session
//! `WorkspaceSandbox`, whose filesystem view is confined to
//! `~/.aleph/workspaces/{session}/`; `~/.aleph/data` (secrets, TLS keys, the
//! memory DB) is deliberately walled off. A headless cron probe cannot get a
//! human to approve the escalation, so the probe was dead on arrival.
//!
//! This tool serves the two memory-DB reality signals the audit needs —
//! recent user corrections and dreaming activity — straight from the typed
//! `SqliteMemoryBackend` in-core, no sandbox, no raw DB access. It is strictly
//! read-only. Cron run-counts are already exposed by `cron_manage(action=
//! "list")` (per-job `state.run_count` / `last_run_status`), so they stay
//! there rather than being duplicated here (R3 / R8).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::memory::store::sqlite::dream_reports::DreamPipelineStat;
use crate::memory::store::MemoryBackend;
use crate::tools::AlephTool;

/// Path prefix under which `flag_user_correction` records a real user
/// correction in `raw_memories`. The audit's "last N days real user correction count" signal.
const CORRECTION_PATH_PREFIX: &str = "aleph://correction/";

/// Default look-back window (days). Matches the audit loop's weekly cadence
/// (the old probes hard-coded 604800s = 7d).
const DEFAULT_WINDOW_DAYS: u32 = 7;

/// Upper bound on the window so a stray large value can't turn a "recent
/// activity" probe into a full-table scan of the whole history.
const MAX_WINDOW_DAYS: u32 = 365;

const SECONDS_PER_DAY: i64 = 86_400;

// ── Args / Output ───────────────────────────────────────────────────────────

/// Arguments for the `governance_metrics` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GovernanceMetricsArgs {
    /// Look-back window in days (default 7, clamped to 1..=365). All counts are
    /// for activity strictly after `now - window_days`.
    #[serde(default)]
    pub window_days: Option<u32>,
}

/// Output from the `governance_metrics` tool — the reality snapshot the audit
/// loop reconciles the loops' self-reports against. `Output` only needs
/// `Serialize` (no `JsonSchema`), so this stays decoupled from schemars and
/// `DreamPipelineStat` need not derive it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceMetricsOutput {
    /// The window actually applied (after clamping).
    pub window_days: u32,
    /// Epoch-seconds watermark; every count is for `timestamp > since_epoch_secs`.
    pub since_epoch_secs: i64,
    /// Number of real user corrections recorded in the window
    /// (`raw_memories` under `aleph://correction/`, all agents).
    pub corrections: usize,
    /// Dreaming activity in the window, grouped by pipeline type (runs +
    /// total memories synthesised). Empty means the dream daemon produced
    /// nothing this window.
    pub dreaming: Vec<DreamPipelineStat>,
}

// ── Tool struct ─────────────────────────────────────────────────────────────

/// Read-only governance reality probe over the memory DB.
#[derive(Clone)]
pub struct GovernanceMetricsTool {
    db: MemoryBackend,
}

impl GovernanceMetricsTool {
    #[must_use]
    pub const fn new(db: MemoryBackend) -> Self {
        Self { db }
    }
}

/// Clamp the requested window and turn it into an epoch-seconds watermark.
/// Pure so the windowing is unit-testable without a clock. `now_secs` is the
/// current time in epoch seconds.
fn resolve_window(window_days: Option<u32>, now_secs: i64) -> (u32, i64) {
    let days = window_days
        .unwrap_or(DEFAULT_WINDOW_DAYS)
        .clamp(1, MAX_WINDOW_DAYS);
    // `.max(0)`: the watermark is epoch seconds and must never go negative
    // (signed `saturating_sub` floors at i64::MIN, not 0). A 0 watermark means
    // "all history", which is the correct degenerate near epoch-start.
    let since = now_secs
        .saturating_sub(i64::from(days) * SECONDS_PER_DAY)
        .max(0);
    (days, since)
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── AlephTool impl ──────────────────────────────────────────────────────────

#[async_trait]
impl AlephTool for GovernanceMetricsTool {
    const NAME: &'static str = "governance_metrics";
    const DESCRIPTION: &'static str = r#"Read-only governance reality probe for the loop-governance audit ring.

Returns, for a look-back window (default 7 days):
- `corrections`: how many real user corrections landed (raw memories under
  aleph://correction/) — the counter-metric for the Dreaming×correction
  Goodhart check. Pair it against each bucket's `feedback_distilled_sum`
  (are corrections actually being distilled into feedback rules?).
- `dreaming`: dream-pipeline activity grouped by pipeline_type — whether memory
  distillation kept producing this window. Each bucket carries {runs,
  synthesis_sum, consolidated_sum, woven_sum, archived_sum,
  feedback_distilled_sum}. IMPORTANT: `synthesis_sum` is 0 on every `consolidate`
  run by design (the NoteSynthesis stage lives only in the rarer `synthesize`
  pipeline), so a healthy nightly consolidate shows synthesis_sum=0 — read the
  other counters (and `runs`) for the distillation actually done.

This is the in-core replacement for the old `sqlite3 ~/.aleph/data/memory.db`
audit probes: same numbers, no sandbox escape, no raw DB access. For cron
run-counts use `cron_manage(action="list")` (each job's state.run_count /
last_run_status); for the loop topology use `loop_graph(action="status")`."#;

    type Args = GovernanceMetricsArgs;
    type Output = GovernanceMetricsOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "governance_metrics()".to_string(),
            "governance_metrics(window_days=30)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let (window_days, since_epoch_secs) = resolve_window(args.window_days, now_epoch_secs());

        let corrections = self
            .db
            .count_raw_by_path_prefix_since(CORRECTION_PATH_PREFIX, since_epoch_secs)?;
        let dreaming = self.db.dream_report_distribution_since(since_epoch_secs)?;

        Ok(GovernanceMetricsOutput {
            window_days,
            since_epoch_secs,
            corrections,
            dreaming,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_defaults_to_seven_days() {
        let (days, since) = resolve_window(None, 1_000_000);
        assert_eq!(days, 7);
        assert_eq!(since, 1_000_000 - 7 * SECONDS_PER_DAY);
    }

    #[test]
    fn window_is_clamped_to_bounds() {
        assert_eq!(resolve_window(Some(0), 1_000_000).0, 1);
        assert_eq!(resolve_window(Some(9_999), 1_000_000).0, MAX_WINDOW_DAYS);
    }

    #[test]
    fn window_saturates_near_epoch_start() {
        // A tiny `now` must not underflow the watermark.
        let (_, since) = resolve_window(Some(365), 10);
        assert_eq!(since, 0);
    }

    #[test]
    fn args_default_window_is_none() {
        let args: GovernanceMetricsArgs = serde_json::from_str("{}").unwrap();
        assert!(args.window_days.is_none());
    }

    #[tokio::test]
    async fn call_reports_corrections_and_dreaming_from_backend() {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
        use crate::memory::store::sqlite::dream_reports::PersistedDreamReport;
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(crate::memory::store::SqliteMemoryBackend::new(tmp.path()).unwrap());

        // A very recent correction (created_at ≈ now) must fall inside a 7d window.
        let now = now_epoch_secs();
        let mut corr = RawMemory::new(
            "boss wants X".to_string(),
            RawMemorySource::SessionCompressed,
        )
        .with_path("aleph://correction/c1")
        .with_agent("main");
        corr.created_at = now - 100;
        backend.insert_raw_memory(&corr).await.unwrap();

        backend
            .insert_dream_report(&PersistedDreamReport {
                id: "d1".to_string(),
                pipeline_type: "full".to_string(),
                started_at: now - 200,
                finished_at: now - 100,
                duration_ms: 100,
                synthesis_count: 4,
                notes_consolidated: 6,
                notes_woven: 1,
                notes_archived: 2,
                feedback_distilled: 3,
                errors: None,
                namespace: "owner".to_string(),
                evolution_json: None,
            })
            .unwrap();

        let tool = GovernanceMetricsTool::new(backend);
        let out = tool
            .call(GovernanceMetricsArgs { window_days: None })
            .await
            .unwrap();

        assert_eq!(out.window_days, 7);
        assert_eq!(out.corrections, 1);
        assert_eq!(out.dreaming.len(), 1);
        assert_eq!(out.dreaming[0].pipeline_type, "full");
        assert_eq!(out.dreaming[0].runs, 1);
        assert_eq!(out.dreaming[0].synthesis_sum, 4);
        assert_eq!(out.dreaming[0].consolidated_sum, 6);
        assert_eq!(out.dreaming[0].woven_sum, 1);
        assert_eq!(out.dreaming[0].archived_sum, 2);
        assert_eq!(out.dreaming[0].feedback_distilled_sum, 3);
    }
}
