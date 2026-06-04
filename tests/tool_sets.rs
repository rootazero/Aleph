//! Stage G integration tests: ensure migrated builtin agents preserve behavior.

use alephcore::agents::AgentRegistry;

/// The full set of tools relevant to the `explore` agent, both allowed and denied.
const EXPLORE_PROBE_TOOLS: &[&str] = &[
    // Allowed (INVESTIGATION named set: file_read/file_ops/search/web_fetch):
    "file_read",
    "file_ops",
    "search",
    "web_fetch",
    // Denied via explicit denied_tools:
    "file_write",
    "file_edit",
    "bash",
    // Mode-aware deny (SubAgent recursion guard blocks subagent):
    "subagent",
    // Unknown tool (must be denied):
    "totally_unknown_tool",
];

#[test]
fn migrated_explore_agent_keeps_behavior() {
    let registry = AgentRegistry::with_builtins();
    let explore = registry.get("explore").expect("explore agent registered");

    // After migration, allowed set comes from INVESTIGATION + denied filter +
    // Stage B mode-aware deny. This test asserts the EFFECTIVE behavior
    // matches what was hand-listed before the migration.
    let expected_allowed = ["file_read", "file_ops", "search", "web_fetch"];
    let expected_denied = [
        "file_write",
        "file_edit",
        "bash",
        "subagent",
        "totally_unknown_tool",
    ];

    for tool in &expected_allowed {
        assert!(
            explore.is_tool_allowed(tool),
            "explore.is_tool_allowed({tool}) must remain true after migration"
        );
    }
    for tool in &expected_denied {
        assert!(
            !explore.is_tool_allowed(tool),
            "explore.is_tool_allowed({tool}) must remain false after migration"
        );
    }

    // Sentinel: probe set covers all relevant cases.
    assert_eq!(
        EXPLORE_PROBE_TOOLS.len(),
        expected_allowed.len() + expected_denied.len(),
        "probe set bookkeeping"
    );
}
