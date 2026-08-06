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
}

/// Aggregate `ToolInvocation` rows for `agent_id` whose `created_at` is at or
/// after `since_unix_secs` into a [`ToolUsageReport`].
///
/// Reuses [`RawMemoryStore::get_raw_by_source`]; backends with a smarter SQL
/// filter override it. `fetch_limit` bounds how many rows the store returns;
/// `top_n` bounds how many per-tool entries appear in the report.
pub async fn aggregate_tool_usage(
    store: &dyn RawMemoryStore,
    agent_id: &str,
    since_unix_secs: i64,
    window_seconds: i64,
    top_n: usize,
    fetch_limit: usize,
) -> Result<ToolUsageReport, AlephError> {
    let rows = store
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
        .await?;
    Ok(build_report(&rows, since_unix_secs, window_seconds, top_n))
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
    let mut per_tool: HashMap<String, Acc> = HashMap::new();
    let mut sessions: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut total: u64 = 0;
    let mut succeeded: u64 = 0;
    let mut failed: u64 = 0;
    let mut total_duration_ms: u64 = 0;

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

        total = total.saturating_add(1);
        total_duration_ms = total_duration_ms.saturating_add(*duration_ms);
        if let Some(sid) = r.session_id.as_deref() {
            sessions.insert(sid);
        }

        let acc = per_tool.entry(tool_name.clone()).or_default();
        acc.count = acc.count.saturating_add(1);
        acc.total_duration_ms = acc.total_duration_ms.saturating_add(*duration_ms);
        if *success {
            succeeded = succeeded.saturating_add(1);
            acc.succeeded = acc.succeeded.saturating_add(1);
        } else {
            failed = failed.saturating_add(1);
            acc.failed = acc.failed.saturating_add(1);
        }
    }

    let distinct_tools = per_tool.len();
    let mut tools: Vec<ToolBreakdown> = per_tool
        .into_iter()
        .map(|(tool, a)| ToolBreakdown {
            tool,
            count: a.count,
            succeeded: a.succeeded,
            failed: a.failed,
            success_rate: ratio(a.succeeded, a.count),
            avg_duration_ms: mean(a.total_duration_ms, a.count),
            percentage: ratio(a.count, total) * 100.0,
        })
        .collect();
    // Deterministic ordering: most-used first, ties broken by name.
    tools.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tool.cmp(&b.tool)));
    tools.truncate(top_n);

    ToolUsageReport {
        window_seconds,
        total,
        succeeded,
        failed,
        success_rate: ratio(succeeded, total),
        avg_duration_ms: mean(total_duration_ms, total),
        distinct_tools,
        distinct_sessions: sessions.len(),
        tools,
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

    #[tokio::test]
    async fn aggregate_tool_usage_reads_through_store() {
        let store = MemStore {
            rows: Mutex::new(vec![
                tool_row("1", "bash", true, 10, 100),
                tool_row("2", "bash", false, 20, 100),
                tool_row("3", "read", true, 5, 100),
            ]),
        };
        let report = aggregate_tool_usage(&store, "agent-1", 0, 86_400, 10, 1000)
            .await
            .unwrap();
        assert_eq!(report.total, 3);
        assert_eq!(report.tools[0].tool, "bash");
        assert_eq!(report.tools[0].count, 2);
        assert_eq!(report.window_seconds, 86_400);
    }
}
