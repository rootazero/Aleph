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

/// Pure read-only filesystem inspection tools.
pub const READ_ONLY: &[&str] = &["glob", "grep", "read_file"];

/// READ_ONLY ∪ remote read tools ∪ subagent (Primary-only via Stage B guard).
/// SubAgent-mode agents that include INVESTIGATION still cannot spawn subagent
/// (Stage B `is_tool_allowed` mode-aware deny).
pub const INVESTIGATION: &[&str] = &[
    "glob",
    "grep",
    "read_file",
    "search",
    "web_fetch",
    "subagent",
];

/// Subset of INVESTIGATION safe for autonomous background execution: no
/// side effects, no exfiltration risk (no web_fetch). Excludes subagent
/// to defend against background recursion misuse beyond Stage B guarantees.
pub const ASYNC_SAFE: &[&str] = &["glob", "grep", "read_file", "search"];

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
        assert!(tools.contains(&"read_file"));
        assert!(tools.contains(&"grep"));
        assert!(tools.contains(&"glob"));
        assert!(!tools.contains(&"web_fetch"));
        assert!(!tools.contains(&"bash"));
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
