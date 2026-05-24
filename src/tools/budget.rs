//! Per-tool wall-clock execution budgets.
//!
//! Static-classification table for built-in tools, mirroring the
//! `IDEMPOTENT_BUILTIN_TOOLS` pattern in `retry.rs`. Tools omitted from
//! the table fall back to the harness-wide `turn_timeout`; if that is
//! also `None`, the call runs unbounded (legacy behaviour).
//!
//! Values reflect empirical p99 of well-behaved invocations plus a
//! margin. Adjust based on production trace observations rather than
//! intuition.

/// Wall-clock budget per builtin tool. Tools omitted fall back to the
/// harness-wide `turn_timeout`. Values are milliseconds.
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
    ("skill_reader", 5_000),
    ("list_tools", 2_000),
    ("get_tool_schema", 2_000),
    ("note_orient", 3_000),
    ("note_schema", 3_000),
    // Legit slow
    ("search", 20_000),
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
];

/// Returns the configured wall-clock budget for a builtin tool, or
/// `None` if the tool is not listed. `None` callers fall back to the
/// harness-wide `turn_timeout`.
pub fn builtin_tool_budget_ms(name: &str) -> Option<u64> {
    BUILTIN_TOOL_BUDGETS_MS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ms)| *ms)
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
        // Locked at 18 entries (13 fast + 3 slow + 2 exec). Bumping this
        // requires updating the table AND adjusting this constant in the
        // same commit — the assertion is a code-review signal, not a
        // value check.
        assert_eq!(BUILTIN_TOOL_BUDGETS_MS.len(), 18);
    }

    #[test]
    fn bash_and_code_exec_have_budgets() {
        // Regression: prior to round-4 codex parity these two tools
        // were absent from the table and silently fell back to the
        // harness-wide turn_timeout.
        assert_eq!(builtin_tool_budget_ms("bash"), Some(180_000));
        assert_eq!(builtin_tool_budget_ms("code_exec"), Some(180_000));
    }
}
