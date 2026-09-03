//! Tool-usage insights — read-only aggregation of `ToolInvocation` raw rows.
//!
//! Ports the per-tool half of hermes-agent's `insights.py` analytics engine
//! (`_get_tool_usage` + `_compute_tool_breakdown`) into a typed, single-pass
//! Rust aggregator. The data substrate already exists: every tool call is
//! captured as a [`RawMemorySource::ToolInvocation`] row by
//! [`crate::memory::tool_signal_sink::RawMemoryToolSink`].
//!
//! This module adds the **per-tool breakdown** that was missing: top-N tools
//! by invocation count, each with success/failure split, average latency, and
//! share of total. It is **read-only** — no mutation, no extra LLM call — and
//! is surfaced through the `insights.tools` admin RPC for introspection
//! (panel widgets / `aleph` CLI), mirroring the `dreaming.run_now` handler.
//!
//! ## Two readers, one aggregator
//!
//! [`aggregate_tool_usage`] (admin RPC) and [`aggregate_tool_failures`] (the
//! nightly `tool_failure_distill` dream stage) both fold the same rows through
//! the same [`fold_window`] core and the same [`fetch_tool_invocation_rows`]
//! read. The failure path only *adds* verbatim evidence samples on top of the
//! counts — it deliberately does not compute a second set of statistics, so
//! "how often did `bash` fail" cannot have two answers in this repo. The two
//! views do rank differently: the usage breakdown is most-used first, while
//! failure evidence is most-failed first, because a tool that only ever fails
//! must not be crowded out of the evidence by chattier successful tools.
//!
//! ## Scope boundary
//!
//! hermes' `insights.py` also reports per-model cost and per-platform token
//! breakdowns. Those are deliberately **out of scope** here: the
//! `ToolInvocation` raw row carries only `{tool_name, success, duration_ms}`
//! — no model id, token counts, pricing, or platform tag — so a faithful
//! cost/model report would require a schema change. Adding speculative
//! columns now would violate the non-destructive / YAGNI discipline. Only the
//! `session_id` already on every raw row is used, to surface a cheap
//! `distinct_sessions` count.

use std::collections::HashMap;

use serde::Serialize;

use crate::error::AlephError;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

/// Per-tool aggregated metrics over the insights window.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolBreakdown {
    /// Tool name as recorded by the signal sink.
    pub tool: String,
    /// Total invocations of this tool in the window.
    pub count: u64,
    /// Invocations that reported `success == true`.
    pub succeeded: u64,
    /// Invocations that reported `success == false`.
    pub failed: u64,
    /// `succeeded / count`, in `[0.0, 1.0]`. `0.0` when `count == 0`.
    pub success_rate: f64,
    /// Mean `duration_ms` across this tool's invocations (integer division).
    pub avg_duration_ms: u64,
    /// Share of total window invocations, in `[0.0, 100.0]`.
    pub percentage: f64,
}

/// Tool-usage report for one agent over a rolling time window.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolUsageReport {
    /// The window length the report covers, in seconds (echoed for context).
    pub window_seconds: i64,
    /// Total tool invocations across all tools in the window.
    pub total: u64,
    /// Window-wide successes.
    pub succeeded: u64,
    /// Window-wide failures.
    pub failed: u64,
    /// Window-wide `succeeded / total`, in `[0.0, 1.0]`.
    pub success_rate: f64,
    /// Window-wide mean latency across every invocation.
    pub avg_duration_ms: u64,
    /// Number of distinct tools seen (before top-N truncation).
    pub distinct_tools: usize,
    /// Number of distinct `session_id`s contributing invocations.
    pub distinct_sessions: usize,
    /// Per-tool breakdown, sorted by `count` desc then `tool` asc, capped to
    /// the requested top-N.
    pub tools: Vec<ToolBreakdown>,
    /// `true` when the underlying `get_raw_by_source` returned at least
    /// `fetch_limit` rows, meaning *more invocations exist* than the report
    /// counted. The backend's order is newest-first, so the dropped rows are
    /// the OLDEST in the partition — the right end to lose for a "recent
    /// behaviour" question, but the consumer (admin RPC, dream stage) needs
    /// to know the report is partial so it does not declare "the daemon is
    /// fine" while silently dropping a 200k-row history.
    pub truncated: bool,
}

/// Aggregate `ToolInvocation` rows for `agent_id` whose `created_at` is at or
/// after `since_unix_secs` into a [`ToolUsageReport`].
///
/// Reuses [`RawMemoryStore::get_raw_by_source`]; backends with a smarter SQL
/// filter override it. `fetch_limit` bounds how many rows the store returns;
/// `top_n` bounds how many per-tool entries appear in the report.
pub async fn aggregate_tool_usage(
    store: &dyn RawMemoryStore,
    agent_ids: &[String],
    since_unix_secs: i64,
    window_seconds: i64,
    top_n: usize,
    fetch_limit: usize,
) -> Result<ToolUsageReport, AlephError> {
    let rows = fetch_tool_invocation_rows(store, agent_ids, fetch_limit).await?;
    // The backend's order is newest-first, so a truncation at fetch_limit drops
    // the OLDEST rows in the partition — the right end to lose for a "recent
    // behaviour" question. The report's `truncated` flag is the consumer's
    // signal that the daemon saw more invocations than it counted, so an admin
    // RPC can render "this report covers 50 000 of N (truncated)" instead of
    // declaring "the daemon is fine" while silently dropping history.
    let truncated = rows.len() >= fetch_limit;
    Ok(build_report_with_truncation(
        &rows,
        since_unix_secs,
        window_seconds,
        top_n,
        truncated,
    ))
}

/// The one read of `ToolInvocation` rows. Both public aggregators go through
/// it so a change to how the rows are addressed (source token, ordering,
/// bound) lands on both at once.
///
/// Backends order newest-first and apply `fetch_limit` in SQL, so a truncation
/// drops the OLDEST rows in the partition — which is the right end to lose for
/// a "recent behaviour" question.
/// Fetch the `tool_invocation` rows for a SET of partitions.
///
/// A set, not one id, because `RawMemoryToolSink` files every row under
/// `project_scope::session_write_id` — the composed partition — while the RPC
/// face is handed a base persona id. Reading one bare id found no rows at all
/// on a stock install and reported it as "this agent has not used any tools".
///
/// Each partition is fetched to the full `fetch_limit` and the merged rows are
/// re-ordered newest-first and capped once, so the aggregator downstream sees
/// exactly the shape it saw before: one newest-first list bounded by
/// `fetch_limit`, whose truncation drops the oldest rows.
async fn fetch_tool_invocation_rows(
    store: &dyn RawMemoryStore,
    agent_ids: &[String],
    fetch_limit: usize,
) -> Result<Vec<RawMemory>, AlephError> {
    let mut all = Vec::new();
    for agent_id in agent_ids {
        all.extend(
            store
                .get_raw_by_source(
                    // Probe variant; only the discriminator token matters for filtering.
                    RawMemorySource::ToolInvocation {
                        tool_name: String::new(),
                        success: false,
                        duration_ms: 0,
                    },
                    agent_id,
                    fetch_limit,
                )
                .await?,
        );
    }
    all.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    all.truncate(fetch_limit);
    Ok(all)
}

// --- Failure evidence (nightly `tool_failure_distill` reader) --------------

/// Distinct failure messages surfaced per tool. Small on purpose: the LLM is
/// being asked to name a recurring *pattern*, and three distinct signatures
/// show a pattern as well as thirty do at a tenth of the prompt.
const FAILURE_SAMPLES_PER_TOOL: usize = 3;
/// Per-sample character cap. `RawMemoryToolSink` already truncates the error
/// tail to 200 chars, so this only bounds pathological rows.
const FAILURE_SAMPLE_MAX_CHARS: usize = 300;

/// One tool's failure evidence: the counts (from the shared aggregator) plus a
/// few verbatim raw-row bodies as proof.
///
/// The samples are the row `content` exactly as `RawMemoryToolSink` wrote it —
/// deliberately NOT parsed into an "error signature" here. Deciding what the
/// signature is, and whether it is worth remembering, is the model's job (R7);
/// this struct only carries the evidence to it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolFailureEvidence {
    /// Tool name as recorded by the signal sink.
    pub tool: String,
    /// Failed invocations of this tool in the window.
    pub failed: u64,
    /// Total invocations of this tool in the window (the denominator).
    pub attempts: u64,
    /// Distinct failure bodies, newest first, capped.
    pub samples: Vec<String>,
}

/// Counts + evidence for the tools that failed in the window.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolFailureDigest {
    /// The very same report `insights.tools` renders — one aggregator, so the
    /// nightly stage and the admin RPC can never disagree about the counts.
    pub report: ToolUsageReport,
    /// Per-tool failure evidence: every tool with at least one in-window
    /// failure competes, ordered most-failed first (ties by name) and capped
    /// at the same `top_n` as the report breakdown. Selected on the failure
    /// axis rather than sliced out of the usage-ranked `report.tools`, so a
    /// cold tool's failures cannot be hidden by chattier successful tools.
    pub failures: Vec<ToolFailureEvidence>,
    /// Newest `created_at` among the in-window rows, or `0` when there were
    /// none. This — not "now" — is the watermark a consumer may commit: it is
    /// the last row it actually looked at, so a row written mid-cycle is
    /// picked up next time instead of being skipped.
    pub newest_created_at: i64,
}

/// Aggregate `ToolInvocation` rows into counts **and** failure evidence in one
/// store read.
///
/// Same window/limit semantics as [`aggregate_tool_usage`]; `top_n` bounds
/// both the per-tool breakdown (most-used first) and the failure-evidence list
/// (most-failed first). Both are drawn from the single shared fold, so the
/// counts cannot disagree — only the ranking axis differs.
///
/// Takes ONE partition, unlike its sibling [`aggregate_tool_usage`], and that
/// asymmetry is deliberate: the only caller is the nightly
/// `tool_failure_distill` stage, which the dream cycle runs once **per corpus**
/// — so `agent_id` here is already the composed partition, and unioning in the
/// org tier would distil the same rows into every corpus that shares it.
/// `aggregate_tool_usage` faces the Panel, where the id is a base persona and
/// the union is the whole point.
pub async fn aggregate_tool_failures(
    store: &dyn RawMemoryStore,
    agent_id: &str,
    since_unix_secs: i64,
    window_seconds: i64,
    top_n: usize,
    fetch_limit: usize,
) -> Result<ToolFailureDigest, AlephError> {
    let one = [agent_id.to_string()];
    let rows = fetch_tool_invocation_rows(store, &one, fetch_limit).await?;
    let truncated = rows.len() >= fetch_limit;
    Ok(build_failure_digest(
        &rows,
        since_unix_secs,
        window_seconds,
        top_n,
        truncated,
    ))
}

/// Pure core of [`aggregate_tool_failures`]: one [`fold_window`] pass feeds
/// both the report and the failure evidence, plus one more walk for verbatim
/// samples.
fn build_failure_digest(
    rows: &[RawMemory],
    since_unix_secs: i64,
    window_seconds: i64,
    top_n: usize,
    truncated: bool,
) -> ToolFailureDigest {
    let fold = fold_window(rows, since_unix_secs);
    let report = report_from_fold(&fold, window_seconds, top_n, truncated);

    let mut newest_created_at = 0i64;
    let mut samples: HashMap<&str, Vec<String>> = HashMap::new();
    for r in rows {
        if r.created_at < since_unix_secs {
            continue;
        }
        let RawMemorySource::ToolInvocation {
            tool_name, success, ..
        } = &r.source
        else {
            continue;
        };
        newest_created_at = newest_created_at.max(r.created_at);
        if *success {
            continue;
        }
        let bucket = samples.entry(tool_name.as_str()).or_default();
        if bucket.len() >= FAILURE_SAMPLES_PER_TOOL {
            continue;
        }
        // UTF-8 safe truncation (P7) — never `&s[..n]`.
        let body: String = r.content.chars().take(FAILURE_SAMPLE_MAX_CHARS).collect();
        if !bucket.iter().any(|existing| existing == &body) {
            bucket.push(body);
        }
    }

    // Evidence is selected by the failure axis over the FULL fold, not sliced
    // out of the usage-ranked (and `top_n`-truncated) `report.tools`: the
    // distiller's quorum counts every in-window failure, so its evidence must
    // be able to name every failing tool, or the quorum passes while the
    // evidence list reads empty and those failures are never distilled.
    let mut failures: Vec<ToolFailureEvidence> = fold
        .per_tool
        .iter()
        .filter(|(_, a)| a.failed > 0)
        .map(|(tool, a)| ToolFailureEvidence {
            tool: (*tool).to_string(),
            failed: a.failed,
            attempts: a.count,
            samples: samples.get(*tool).cloned().unwrap_or_default(),
        })
        .collect();
    failures.sort_by(|a, b| b.failed.cmp(&a.failed).then_with(|| a.tool.cmp(&b.tool)));
    failures.truncate(top_n);

    ToolFailureDigest {
        report,
        failures,
        newest_created_at,
    }
}

/// The report a partition with no invocations in the window produces.
///
/// Built by running the REAL aggregator over zero rows rather than
/// hand-writing the empty shape, so it is byte-identical to a genuine empty
/// result by construction and cannot drift as [`ToolUsageReport`] gains
/// fields. `insights.tools` returns this for a partition the caller cannot
/// see (P1, spec §11-1c) — the denial must be indistinguishable from "that
/// partition ran no tools", and it must not read the store to say so.
#[must_use]
pub fn empty_tool_usage_report(window_seconds: i64, top_n: usize) -> ToolUsageReport {
    build_report(&[], 0, window_seconds, top_n)
}

/// One tool's running tallies during aggregation.
#[derive(Default)]
struct Acc {
    count: u64,
    succeeded: u64,
    failed: u64,
    total_duration_ms: u64,
}

/// Pure aggregation core: filter `rows` to the window and fold them into a
/// report. Separated from the store fetch so it is testable without I/O.
fn build_report(
    rows: &[RawMemory],
    since_unix_secs: i64,
    window_seconds: i64,
    top_n: usize,
) -> ToolUsageReport {
    build_report_with_truncation(rows, since_unix_secs, window_seconds, top_n, false)
}

/// Same as [`build_report`] but lets the caller flag whether the input was
/// truncated by `fetch_limit`. The aggregator's caller already knows the
/// truncation; the pure helper used to silently drop the signal. Today the
/// pure callers (tests, `empty_tool_usage_report`) pass `false`.
fn build_report_with_truncation(
    rows: &[RawMemory],
    since_unix_secs: i64,
    window_seconds: i64,
    top_n: usize,
    truncated: bool,
) -> ToolUsageReport {
    report_from_fold(
        &fold_window(rows, since_unix_secs),
        window_seconds,
        top_n,
        truncated,
    )
}

/// The single tally over the in-window `ToolInvocation` rows: per-tool
/// accumulators plus report-level totals. Both the usage report and the
/// failure evidence are views over ONE of these, which is what keeps their
/// counts from ever disagreeing.
struct WindowFold<'a> {
    per_tool: HashMap<&'a str, Acc>,
    sessions: std::collections::HashSet<&'a str>,
    total: u64,
    succeeded: u64,
    failed: u64,
    total_duration_ms: u64,
}

fn fold_window(rows: &[RawMemory], since_unix_secs: i64) -> WindowFold<'_> {
    let mut fold = WindowFold {
        per_tool: HashMap::new(),
        sessions: std::collections::HashSet::new(),
        total: 0,
        succeeded: 0,
        failed: 0,
        total_duration_ms: 0,
    };

    for r in rows {
        if r.created_at < since_unix_secs {
            continue;
        }
        let RawMemorySource::ToolInvocation {
            tool_name,
            success,
            duration_ms,
        } = &r.source
        else {
            continue;
        };

        fold.total = fold.total.saturating_add(1);
        fold.total_duration_ms = fold.total_duration_ms.saturating_add(*duration_ms);
        if let Some(sid) = r.session_id.as_deref() {
            fold.sessions.insert(sid);
        }

        let acc = fold.per_tool.entry(tool_name.as_str()).or_default();
        acc.count = acc.count.saturating_add(1);
        acc.total_duration_ms = acc.total_duration_ms.saturating_add(*duration_ms);
        if *success {
            fold.succeeded = fold.succeeded.saturating_add(1);
            acc.succeeded = acc.succeeded.saturating_add(1);
        } else {
            fold.failed = fold.failed.saturating_add(1);
            acc.failed = acc.failed.saturating_add(1);
        }
    }

    fold
}

/// Render a [`WindowFold`] as the usage report: most-used first, top-N.
fn report_from_fold(
    fold: &WindowFold<'_>,
    window_seconds: i64,
    top_n: usize,
    truncated: bool,
) -> ToolUsageReport {
    let distinct_tools = fold.per_tool.len();
    let mut tools: Vec<ToolBreakdown> = fold
        .per_tool
        .iter()
        .map(|(tool, a)| ToolBreakdown {
            tool: (*tool).to_string(),
            count: a.count,
            succeeded: a.succeeded,
            failed: a.failed,
            success_rate: ratio(a.succeeded, a.count),
            avg_duration_ms: mean(a.total_duration_ms, a.count),
            percentage: ratio(a.count, fold.total) * 100.0,
        })
        .collect();
    // Deterministic ordering: most-used first, ties broken by name.
    tools.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tool.cmp(&b.tool)));
    tools.truncate(top_n);

    ToolUsageReport {
        window_seconds,
        total: fold.total,
        succeeded: fold.succeeded,
        failed: fold.failed,
        success_rate: ratio(fold.succeeded, fold.total),
        avg_duration_ms: mean(fold.total_duration_ms, fold.total),
        distinct_tools,
        distinct_sessions: fold.sessions.len(),
        tools,
        truncated,
    }
}

/// `numerator / denominator` as `f64`, or `0.0` when `denominator == 0`.
fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

/// Integer mean (`sum / count`), or `0` when `count == 0`.
fn mean(sum: u64, count: u64) -> u64 {
    sum.checked_div(count).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;
    use async_trait::async_trait;

    fn tool_row(
        id: &str,
        tool: &str,
        success: bool,
        duration_ms: u64,
        created_at: i64,
    ) -> RawMemory {
        RawMemory {
            id: id.to_string(),
            content: String::new(),
            source: RawMemorySource::ToolInvocation {
                tool_name: tool.to_string(),
                success,
                duration_ms,
            },
            agent_id: "agent-1".to_string(),
            session_id: Some(format!("sess-{id}")),
            path: None,
            attachment_text: None,
            is_processed: false,
            created_at,
        }
    }

    #[test]
    fn empty_rows_produce_zeroed_report() {
        let report = build_report(&[], 0, 3600, 10);
        assert_eq!(report.total, 0);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.success_rate, 0.0);
        assert_eq!(report.avg_duration_ms, 0);
        assert_eq!(report.distinct_tools, 0);
        assert_eq!(report.distinct_sessions, 0);
        assert!(report.tools.is_empty());
        assert_eq!(report.window_seconds, 3600);
    }

    #[test]
    fn buckets_per_tool_with_success_failure_and_latency() {
        let rows = vec![
            tool_row("1", "bash", true, 10, 100),
            tool_row("2", "bash", false, 30, 100),
            tool_row("3", "read", true, 5, 100),
        ];
        let report = build_report(&rows, 0, 3600, 10);
        assert_eq!(report.total, 3);
        assert_eq!(report.succeeded, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.distinct_tools, 2);
        assert_eq!(report.distinct_sessions, 3);

        // Sorted by count desc → bash (2) before read (1).
        assert_eq!(report.tools[0].tool, "bash");
        assert_eq!(report.tools[0].count, 2);
        assert_eq!(report.tools[0].succeeded, 1);
        assert_eq!(report.tools[0].failed, 1);
        assert!((report.tools[0].success_rate - 0.5).abs() < 1e-9);
        assert_eq!(report.tools[0].avg_duration_ms, 20); // (10 + 30) / 2
        assert!((report.tools[0].percentage - (2.0 / 3.0 * 100.0)).abs() < 1e-9);

        assert_eq!(report.tools[1].tool, "read");
        assert_eq!(report.tools[1].count, 1);
    }

    #[test]
    fn rows_before_cutoff_are_excluded() {
        let rows = vec![
            tool_row("old", "bash", true, 10, 50),
            tool_row("new", "bash", true, 10, 150),
        ];
        let report = build_report(&rows, 100, 3600, 10);
        assert_eq!(report.total, 1, "only the row at/after the cutoff counts");
        assert_eq!(report.tools[0].count, 1);
    }

    #[test]
    fn non_tool_rows_are_ignored() {
        let mut rows = vec![tool_row("1", "bash", true, 10, 100)];
        rows.push(RawMemory {
            id: "x".into(),
            content: String::new(),
            source: RawMemorySource::Transcript,
            agent_id: "agent-1".into(),
            session_id: Some("sess-x".into()),
            path: None,
            attachment_text: None,
            is_processed: false,
            created_at: 100,
        });
        let report = build_report(&rows, 0, 3600, 10);
        assert_eq!(report.total, 1);
        assert_eq!(report.distinct_tools, 1);
    }

    #[test]
    fn top_n_truncates_but_distinct_tools_counts_all() {
        let rows = vec![
            tool_row("1", "aaa", true, 1, 100),
            tool_row("2", "aaa", true, 1, 100),
            tool_row("3", "aaa", true, 1, 100),
            tool_row("4", "bbb", true, 1, 100),
            tool_row("5", "bbb", true, 1, 100),
            tool_row("6", "ccc", true, 1, 100),
        ];
        let report = build_report(&rows, 0, 3600, 2);
        assert_eq!(
            report.distinct_tools, 3,
            "all tools counted before truncation"
        );
        assert_eq!(report.tools.len(), 2, "only top-2 returned");
        assert_eq!(report.tools[0].tool, "aaa"); // count 3
        assert_eq!(report.tools[1].tool, "bbb"); // count 2
    }

    #[test]
    fn equal_count_ties_break_by_name() {
        let rows = vec![
            tool_row("1", "zebra", true, 1, 100),
            tool_row("2", "alpha", true, 1, 100),
        ];
        let report = build_report(&rows, 0, 3600, 10);
        assert_eq!(report.tools[0].tool, "alpha", "ties break alphabetically");
        assert_eq!(report.tools[1].tool, "zebra");
    }

    // ---- async path through a store (mirrors tool_signal_sink tests) ----

    struct MemStore {
        rows: Mutex<Vec<RawMemory>>,
    }

    #[async_trait]
    impl RawMemoryStore for MemStore {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
            self.rows.lock().unwrap().push(raw.clone());
            Ok(())
        }
        async fn get_unprocessed_raw_memories(
            &self,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(self.rows.lock().unwrap().clone())
        }
        async fn mark_raw_as_processed(&self, _ids: &[String]) -> Result<usize, AlephError> {
            Ok(0)
        }
        async fn count_unprocessed(&self, _agent_id: &str) -> Result<usize, AlephError> {
            Ok(self.rows.lock().unwrap().len())
        }
        async fn get_raw_by_path_prefix(
            &self,
            _path_prefix: &str,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }
        async fn get_raw_by_source(
            &self,
            _source: RawMemorySource,
            _agent_id: &str,
            _limit: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(self.rows.lock().unwrap().clone())
        }
    }

    // ---- failure digest (the nightly distiller's read) ----

    fn failing_row(id: &str, tool: &str, body: &str, created_at: i64) -> RawMemory {
        let mut r = tool_row(id, tool, false, 5, created_at);
        r.content = body.to_string();
        r
    }

    /// The digest's numbers must BE the shared aggregator's numbers, not a
    /// second tally: a failure count that disagreed with `insights.tools`
    /// would make the nightly prompt argue with the admin RPC.
    #[test]
    fn failure_digest_counts_come_from_the_shared_report() {
        let rows = vec![
            failing_row("1", "bash", "tool bash failed in 5ms: exit 127", 100),
            failing_row("2", "bash", "tool bash failed in 5ms: exit 127", 101),
            tool_row("3", "bash", true, 5, 102),
            tool_row("4", "read", true, 5, 103),
        ];
        let digest = build_failure_digest(&rows, 0, 3600, 10, false);
        let report = build_report(&rows, 0, 3600, 10);
        assert_eq!(digest.report, report, "one aggregator, not two");
        assert_eq!(digest.failures.len(), 1, "only bash failed");
        assert_eq!(digest.failures[0].tool, "bash");
        assert_eq!(digest.failures[0].failed, 2);
        assert_eq!(digest.failures[0].attempts, 3);
    }

    /// Evidence is verbatim and deduped: two byte-identical failures are one
    /// signature, and the sample cap bounds what reaches the prompt.
    #[test]
    fn failure_samples_are_deduped_and_capped() {
        let mut rows = vec![
            failing_row("a", "bash", "tool bash failed: exit 127", 100),
            failing_row("b", "bash", "tool bash failed: exit 127", 101),
        ];
        for i in 0..10 {
            rows.push(failing_row(
                &format!("d{i}"),
                "bash",
                &format!("tool bash failed: distinct {i}"),
                200 + i,
            ));
        }
        let digest = build_failure_digest(&rows, 0, 3600, 10, false);
        let samples = &digest.failures[0].samples;
        assert!(
            samples.len() <= FAILURE_SAMPLES_PER_TOOL,
            "samples must be capped, got {}",
            samples.len()
        );
        let mut sorted = samples.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), samples.len(), "samples must be distinct");
        // The count still reflects EVERY failure, not just the sampled ones.
        assert_eq!(digest.failures[0].failed, 12);
    }

    /// A long body must not be sliced mid-codepoint, and must not blow the
    /// prompt budget. `&s[..n]` on this input panics.
    #[test]
    fn failure_sample_truncation_is_utf8_safe() {
        let body = "错".repeat(FAILURE_SAMPLE_MAX_CHARS + 50);
        let rows = vec![failing_row("1", "bash", &body, 100)];
        let digest = build_failure_digest(&rows, 0, 3600, 10, false);
        let s = &digest.failures[0].samples[0];
        assert_eq!(s.chars().count(), FAILURE_SAMPLE_MAX_CHARS);
    }

    /// The watermark a consumer commits is the newest row it LOOKED AT, and it
    /// counts successes too: a successful invocation after the last failure
    /// still means "I have read up to here". Committing `now` instead would
    /// skip rows written between the read and the commit.
    #[test]
    fn newest_created_at_is_the_last_row_seen_not_the_last_failure() {
        let rows = vec![
            failing_row("1", "bash", "boom", 100),
            tool_row("2", "bash", true, 5, 500),
        ];
        let digest = build_failure_digest(&rows, 0, 3600, 10, false);
        assert_eq!(digest.newest_created_at, 500);
        // Rows below the cutoff are neither counted nor allowed to move it.
        let digest = build_failure_digest(&rows, 200, 3600, 10, false);
        assert_eq!(digest.newest_created_at, 500);
        assert!(digest.failures.is_empty(), "the failure is out of window");
    }

    /// A failing tool must not be hidden by chattier successful tools: the
    /// evidence list is selected most-failed first from ALL in-window tools,
    /// not sliced out of the usage top-N. Otherwise the distiller's quorum
    /// (whole-window failure count) is met while its evidence list is empty,
    /// and the failures are silently never distilled.
    #[test]
    fn failure_evidence_survives_usage_top_n_truncation() {
        let mut rows = Vec::new();
        // Six successful tools, five calls each — they own the usage top-5.
        for t in 0..6 {
            for i in 0..5 {
                rows.push(tool_row(&format!("s{t}-{i}"), &format!("hot{t}"), true, 5, 100));
            }
        }
        // A cold tool that only ever failed, three times.
        for i in 0..3 {
            rows.push(failing_row(
                &format!("c{i}"),
                "coldtool",
                "tool coldtool failed: exit 1",
                100,
            ));
        }
        let digest = build_failure_digest(&rows, 0, 3600, 5, false);
        assert_eq!(digest.report.failed, 3);
        assert!(
            !digest.report.tools.iter().any(|b| b.tool == "coldtool"),
            "precondition: the cold tool must be outside the usage top-N"
        );
        assert_eq!(digest.failures.len(), 1);
        assert_eq!(digest.failures[0].tool, "coldtool");
        assert_eq!(digest.failures[0].failed, 3);
        assert_eq!(digest.failures[0].attempts, 3);
        assert!(digest.failures[0].samples[0].contains("exit 1"));
    }

    #[test]
    fn failure_evidence_is_ordered_most_failed_first() {
        let rows = vec![
            failing_row("a", "minor", "boom", 100),
            failing_row("b", "major", "boom", 100),
            failing_row("c", "major", "boom", 101),
        ];
        let digest = build_failure_digest(&rows, 0, 3600, 10, false);
        let order: Vec<&str> = digest.failures.iter().map(|f| f.tool.as_str()).collect();
        assert_eq!(order, vec!["major", "minor"]);
    }

    #[test]
    fn no_failures_yields_an_empty_evidence_list_and_zero_watermark() {
        let digest = build_failure_digest(&[], 0, 3600, 10, false);
        assert!(digest.failures.is_empty());
        assert_eq!(digest.newest_created_at, 0);
        assert_eq!(digest.report.total, 0);
    }

    #[tokio::test]
    async fn aggregate_tool_failures_reads_through_store() {
        let store = MemStore {
            rows: Mutex::new(vec![
                failing_row("1", "bash", "tool bash failed: exit 1", 100),
                tool_row("2", "read", true, 5, 100),
            ]),
        };
        let digest = aggregate_tool_failures(&store, "agent-1", 0, 86_400, 10, 1000)
            .await
            .unwrap();
        assert_eq!(digest.report.failed, 1);
        assert_eq!(digest.failures.len(), 1);
        assert_eq!(digest.failures[0].tool, "bash");
        assert!(digest.failures[0].samples[0].contains("exit 1"));
    }

    #[tokio::test]
    async fn aggregate_tool_usage_reads_through_store() {
        let store = MemStore {
            rows: Mutex::new(vec![
                tool_row("1", "bash", true, 10, 100),
                tool_row("2", "bash", false, 20, 100),
                tool_row("3", "read", true, 5, 100),
            ]),
        };
        let report = aggregate_tool_usage(&store, &["agent-1".to_string()], 0, 86_400, 10, 1000)
            .await
            .unwrap();
        assert_eq!(report.total, 3);
        assert_eq!(report.tools[0].tool, "bash");
        assert_eq!(report.tools[0].count, 2);
        assert_eq!(report.window_seconds, 86_400);
    }
}
