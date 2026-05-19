//! Named tool sets for declarative agent allowlists (P2 Stage G).
//!
//! Per locked design (Q7-1 simplified positive): only 3 positive sets, no
//! ALL_AGENT_DENIED_TOOLS auto-deny, no allow_override field. Defense layers
//! (recursion guard via Stage B, user-frontmatter mode forcing via Stage E)
//! live elsewhere.
//!
//! Tool names match those registered in `src/builtin_tools/` (see
//! `crate::builtin_tools::register_*`). This file's 3 constants are the only
//! place tool sets are defined; AgentDef.allowed_tool_sets references them by name.

// Tool names below MUST match the names registered in
// `src/builtin_tools/` (see each tool's `AlephTool::NAME`). Earlier values
// referenced legacy stubs ("glob", "grep", "read_file") that never existed
// in the dispatcher, leaving INVESTIGATION-mode agents with effectively
// 1–2 visible tools and forcing them to give up after one think turn.
//
// Current canonical builtin names in this category:
//   file_read   — single-file read
//   file_ops    — list/search/read/write/edit/move/copy/delete/mkdir
//   search      — remote web search
//   web_fetch   — remote URL fetch
//   subagent    — sub-agent spawn (gated for SubAgent mode by Stage B guard)

/// Pure read-only filesystem inspection tools.
pub const READ_ONLY: &[&str] = &["file_read", "file_ops"];

/// READ_ONLY ∪ remote read tools ∪ subagent (Primary-only via Stage B guard).
/// SubAgent-mode agents that include INVESTIGATION still cannot spawn subagent
/// (Stage B `is_tool_allowed` mode-aware deny).
///
/// Note: `file_ops` is admitted for its `list`/`search`/`read` operations.
/// Its write-side operations are gated separately by the agent's
/// `denied_tools` and the sandbox's `denied_paths` policy.
pub const INVESTIGATION: &[&str] = &[
    "file_read",
    "file_ops",
    "search",
    "web_fetch",
    "subagent",
];

/// Subset of INVESTIGATION safe for autonomous background execution: no
/// exfiltration risk (no web_fetch). Excludes subagent to defend against
/// background recursion misuse beyond Stage B guarantees.
pub const ASYNC_SAFE: &[&str] = &["file_read", "file_ops", "search"];

/// Resolve a set name to its tool list. Returns None for unknown names so
/// callers can warn (loader) or treat as empty allowance (is_tool_allowed)
/// without rejecting valid agent definitions.
pub fn resolve(set_name: &str) -> Option<&'static [&'static str]> {
    match set_name {
        "READ_ONLY" => Some(READ_ONLY),
        "INVESTIGATION" => Some(INVESTIGATION),
        "ASYNC_SAFE" => Some(ASYNC_SAFE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_set_resolves_to_known_tools() {
        let tools = resolve("READ_ONLY").expect("READ_ONLY exists");
        assert!(tools.contains(&"file_read"));
        assert!(tools.contains(&"file_ops"));
        assert!(!tools.contains(&"web_fetch"));
        assert!(!tools.contains(&"bash"));
        assert!(!tools.contains(&"file_write"));
        assert!(!tools.contains(&"file_edit"));
    }

    /// Regression: tool names referenced here drifted out of sync with the
    /// actual dispatcher names ("glob"/"grep"/"read_file" were never
    /// registered; the real builtins are "file_read"/"file_ops"). This
    /// test pins the set membership to canonical builtin names so future
    /// renames either update both ends or fail loudly.
    #[test]
    fn tool_sets_reference_only_canonical_builtin_names() {
        // Names match the `AlephTool::NAME` constants in `src/builtin_tools/`.
        const KNOWN_BUILTINS: &[&str] = &[
            "file_read",
            "file_ops",
            "file_write",
            "file_edit",
            "search",
            "web_fetch",
            "subagent",
        ];
        for (set_name, set) in [
            ("READ_ONLY", READ_ONLY),
            ("INVESTIGATION", INVESTIGATION),
            ("ASYNC_SAFE", ASYNC_SAFE),
        ] {
            for tool in set {
                assert!(
                    KNOWN_BUILTINS.contains(tool),
                    "tool '{tool}' in set '{set_name}' is not a canonical builtin name"
                );
            }
        }
    }

    #[test]
    fn investigation_is_superset_of_read_only() {
        let read_only = resolve("READ_ONLY").unwrap();
        let investigation = resolve("INVESTIGATION").unwrap();
        for tool in read_only {
            assert!(
                investigation.contains(tool),
                "INVESTIGATION must contain READ_ONLY tool '{tool}'"
            );
        }
    }

    #[test]
    fn async_safe_excludes_subagent() {
        let async_safe = resolve("ASYNC_SAFE").unwrap();
        assert!(!async_safe.contains(&"subagent"));
    }

    #[test]
    fn async_safe_excludes_web_fetch() {
        // Exfiltration risk: ASYNC_SAFE is safe-to-run-autonomously; web_fetch
        // would let a background agent leak data via URL parameters.
        let async_safe = resolve("ASYNC_SAFE").unwrap();
        assert!(!async_safe.contains(&"web_fetch"));
    }

    #[test]
    fn unknown_set_resolves_none() {
        assert!(resolve("FOOBAR").is_none());
        assert!(resolve("read_only").is_none()); // case-sensitive
        assert!(resolve("").is_none());
    }
}
