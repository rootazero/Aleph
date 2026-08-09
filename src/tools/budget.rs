//! Per-tool wall-clock execution budgets.
//!
//! Static-classification table for built-in tools, mirroring the
//! `IDEMPOTENT_BUILTIN_TOOLS` pattern in `retry.rs`, plus the resolution
//! chain every `describe()` site uses: the tool's own declared budget
//! ([`crate::tools::runtime::LoopTool::max_duration_ms`]) → this table →
//! [`DEFAULT_TOOL_BUDGET_MS`].
//!
//! Every tool definition MUST come out of that chain with a budget.
//! While the table was the only source, the ~100 builtins absent from it
//! plus every MCP / plugin / skill tool advertised `None`, and the harness
//! read `None` as "no per-tool budget" — turning one slow call into a
//! run-level `StalledTurn` abort instead of a recoverable tool error.
//!
//! Values reflect empirical p99 of well-behaved invocations plus a
//! margin. Adjust based on production trace observations rather than
//! intuition.

/// Wall-clock budget for a tool that neither declares one nor appears in
/// [`BUILTIN_TOOL_BUDGETS_MS`]: unlisted builtins, plugin / extension tools,
/// skills.
///
/// Deliberately roomy — it is a backstop against a call that never returns,
/// not a latency target, and an overrun is a recoverable tool error the next
/// Think turn can react to. 300s matches the MCP client's own remote-call
/// ceiling (`McpClient::start_remote_server`), so a tool of unknown
/// provenance is never killed sooner than the slowest thing we already ship.
/// A genuinely hung run is still caught by the run-level stall tracker.
pub const DEFAULT_TOOL_BUDGET_MS: u64 = 300_000;

/// Slack added on top of an MCP server's configured request timeout so the
/// harness's wall clock cannot preempt the MCP client's own — the client
/// returns a `Timeout` tool error the model can act on, whereas the harness
/// side only sees an opaque overrun.
const MCP_TIMEOUT_HEADROOM_MS: u64 = 30_000;

/// The MCP client's own default request timeout when a server config leaves
/// `timeout_seconds` unset (`McpClient::start_remote_server`, 300s).
const MCP_DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Wall-clock budget per builtin tool. Tools omitted resolve to
/// [`DEFAULT_TOOL_BUDGET_MS`] via [`resolve_tool_budget_ms`]. Values are
/// milliseconds.
pub const BUILTIN_TOOL_BUDGETS_MS: &[(&str, u64)] = &[
    // Read-only / pure query — should be fast
    ("memory_search", 5_000),
    ("memory_browse", 5_000),
    ("memory_timeline", 5_000),
    ("memory_explore", 5_000),
    ("recall_context", 5_000),
    ("session_search", 5_000),
    ("user_profile", 3_000),
    ("skill_status", 3_000),
    // (`skill_reader` / `list_tools` / `search_tools` were phantom rows —
    // no registered tool ever bore those names; the real skill read tool is
    // `skill_read` below.)
    ("skill_read", 5_000),
    ("get_tool_schema", 2_000),
    ("note_orient", 3_000),
    ("note_schema", 3_000),
    // Legit slow — `search` raised 20s→45s after a real A-share research run
    // tripped the 20s ceiling on a slow query (the overrun is now a recoverable
    // tool error, not a run abort, but a roomier ceiling avoids needless retries).
    ("search", 45_000),
    ("web_fetch", 30_000),
    ("markdown_skill", 60_000),
    // Shell / code execution — these have user-supplied `timeout` in
    // args but absent a per-tool budget they fall through to the
    // harness-wide turn_timeout (often much longer). A 3-minute ceiling
    // here keeps a runaway `cargo build` / `pip install` from blocking
    // the entire turn while still leaving room for legit long jobs the
    // caller explicitly opts into via the `timeout` arg.
    ("bash", 180_000),
    ("code_exec", 180_000),
    // Blocks on a HUMAN, not on I/O. `ask_user` parks on a oneshot for
    // `clarification::DEFAULT_CLARIFY_TIMEOUT` (600s); the budget must sit
    // ABOVE that so the tool's own timeout is what fires. It used to be
    // absent from this table, so a user who took longer than the harness
    // fallback (120s) to answer killed the run — the blocking-clarification
    // path (12-factor F7) never reached its designed timeout in production.
    ("ask_user", 630_000),
    // Delegation: the child run is bounded by the caller's `timeout_secs`
    // (default 120s, model-settable per task) and a synchronous batch awaits
    // every child, so the parent-side wall clock legitimately runs long. This
    // is a backstop against a child that never returns, not a latency target.
    ("subagent", 1_800_000),
    // Blocks on OTHER RUNS finishing, not on I/O — the only wait face for a
    // workflow run or a team task DAG. Same reasoning as `ask_user`: the
    // budget must sit ABOVE the tool's own ceiling
    // (`task_manage::wait::MAX_WAIT_SECS`, 600s) so what fires is the tool's
    // timeout, which returns the completed/failed/pending tables it has
    // already computed. It used to be absent here, so it resolved to the 300s
    // default — exactly its own default wait, i.e. a coin flip — and any
    // longer wait the model asked for was guaranteed to be preempted by the
    // harness, throwing that progress away and reporting only an opaque
    // overrun.
    ("task_wait", 630_000),
];

/// Returns the configured wall-clock budget for a builtin tool, or
/// `None` if the tool is not listed.
///
/// Prefer [`resolve_tool_budget_ms`] at definition-building sites: a bare
/// `None` here means "not in the table", NOT "no budget".
#[must_use]
pub fn builtin_tool_budget_ms(name: &str) -> Option<u64> {
    BUILTIN_TOOL_BUDGETS_MS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ms)| *ms)
}

/// Resolve the wall-clock budget a `ToolDefinition` must advertise:
/// the tool's own declaration wins, then the builtin table, then
/// [`DEFAULT_TOOL_BUDGET_MS`]. Never `None` — every `describe()` site
/// resolves through here so the harness always has a per-tool budget and
/// can treat an overrun as a recoverable tool error.
#[must_use]
pub fn resolve_tool_budget_ms(name: &str, declared: Option<u64>) -> u64 {
    declared
        .or_else(|| builtin_tool_budget_ms(name))
        .unwrap_or(DEFAULT_TOOL_BUDGET_MS)
}

/// Budget for a tool served by an MCP server whose config declares
/// `timeout_seconds` (`None` = the client's own 300s remote default).
///
/// The headroom is what keeps the two clocks ordered: the MCP client must be
/// the one that gives up, so the model receives its `Timeout` tool error.
#[must_use]
pub fn mcp_tool_budget_ms(timeout_seconds: Option<u64>) -> u64 {
    timeout_seconds
        .map_or(MCP_DEFAULT_TIMEOUT_MS, |s| s.saturating_mul(1_000))
        .saturating_add(MCP_TIMEOUT_HEADROOM_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_some_for_listed_read_only_tool() {
        assert_eq!(builtin_tool_budget_ms("memory_search"), Some(5_000));
    }

    #[test]
    fn returns_some_for_listed_slow_tool() {
        assert_eq!(builtin_tool_budget_ms("web_fetch"), Some(30_000));
    }

    #[test]
    fn returns_none_for_unlisted_tool() {
        assert_eq!(builtin_tool_budget_ms("definitely_not_a_real_tool"), None);
    }

    #[test]
    fn returns_none_for_empty_name() {
        assert_eq!(builtin_tool_budget_ms(""), None);
    }

    #[test]
    fn table_size_matches_expected_count() {
        // Locked at 20 entries (12 fast + 3 slow + 2 exec + ask_user +
        // subagent + task_wait; the 2026-07 phantom sweep removed the
        // never-registered `list_tools` / `search_tools` rows and renamed stale
        // `skill_reader` to `skill_read`). Bumping this requires updating the
        // table AND adjusting this constant in the same commit — the assertion
        // is a code-review signal, not a value check.
        assert_eq!(BUILTIN_TOOL_BUDGETS_MS.len(), 20);
    }

    #[test]
    fn bash_and_code_exec_have_budgets() {
        // Regression: prior to round-4 codex parity these two tools
        // were absent from the table and silently fell back to the
        // harness-wide turn_timeout.
        assert_eq!(builtin_tool_budget_ms("bash"), Some(180_000));
        assert_eq!(builtin_tool_budget_ms("code_exec"), Some(180_000));
    }

    #[test]
    fn ask_user_budget_outlives_the_clarification_timeout() {
        // The human gets `DEFAULT_CLARIFY_TIMEOUT` to answer; the harness
        // budget must not fire first, or a slow answer aborts the run.
        let clarify_ms =
            u64::try_from(crate::clarification::DEFAULT_CLARIFY_TIMEOUT.as_millis()).unwrap();
        let budget = builtin_tool_budget_ms("ask_user").expect("ask_user is budgeted");
        assert!(
            budget > clarify_ms,
            "ask_user budget {budget}ms must exceed the {clarify_ms}ms clarification timeout"
        );
    }

    /// Same rule, second tool that blocks on something other than I/O.
    ///
    /// `task_wait` used to be absent from the table, so it resolved to the 300s
    /// default while its own default wait was also 300s — a coin flip — and any
    /// longer wait the model asked for was certain to be killed by the harness,
    /// discarding the completed/failed/pending tables it had already built. No
    /// test could see it, because each clock was correct on its own; only
    /// putting them in one assertion catches it.
    #[test]
    fn task_wait_budget_outlives_its_own_clamped_ceiling() {
        let ceiling_ms = crate::builtin_tools::task_manage::wait::MAX_WAIT_SECS * 1_000;
        let budget = builtin_tool_budget_ms("task_wait").expect("task_wait is budgeted");
        assert!(
            budget > ceiling_ms,
            "task_wait budget {budget}ms must exceed its own {ceiling_ms}ms ceiling, \
             or the harness clock preempts the tool's and the progress report is lost"
        );
    }

    #[test]
    fn resolution_chain_prefers_declaration_then_table_then_default() {
        // Declared wins even for a table tool (MCP adapters declare theirs).
        assert_eq!(resolve_tool_budget_ms("memory_search", Some(9_000)), 9_000);
        // No declaration → table.
        assert_eq!(resolve_tool_budget_ms("memory_search", None), 5_000);
        // Neither → default. This is the branch that used to yield `None`
        // and hand the harness a run-killing stall instead of a tool error.
        assert_eq!(
            resolve_tool_budget_ms("some_plugin_tool", None),
            DEFAULT_TOOL_BUDGET_MS
        );
    }

    #[test]
    fn mcp_budget_leaves_headroom_over_the_server_timeout() {
        // Configured: the client's clock must expire first.
        assert!(mcp_tool_budget_ms(Some(600)) > 600_000);
        // Unset: the client's remote default is 300s, so the budget clears it.
        assert!(mcp_tool_budget_ms(None) > 300_000);
        // Absurd config must not wrap around into a tiny budget.
        assert_eq!(mcp_tool_budget_ms(Some(u64::MAX)), u64::MAX);
    }
}
