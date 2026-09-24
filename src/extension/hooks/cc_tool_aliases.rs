//! Claude Code tool names ↔ Aleph tool names — the one table.
//!
//! Three readers, one derivation: the hook matcher (`executor.rs`,
//! `matches_pattern`), a CC command's `allowed-tools:` (P4.7b) and a CC
//! agent's `tools:` (P4.8). Each reader used to be a place where `Read` would
//! silently name nothing. Every right-hand side is checked against the real
//! tool registry by `every_aleph_target_is_a_real_tool_name`.

/// `(claude_code_name, aleph_name)`. Left side is exact and case-sensitive
/// (CC matchers are). Names with no Aleph counterpart are deliberately
/// absent: `NotebookEdit`, `LS` (Aleph folds it into `file_ops list`),
/// `Skill` (CC invokes a skill; Aleph `skill_read` reads one — different
/// verb), `SlashCommand`, `TodoWrite` (Aleph `scratchpad` is a different
/// shape), `KillShell`, `BashOutput`, `EnterWorktree`/`ExitWorktree`. A
/// matcher naming one of these matches nothing, which is the correct
/// fail-closed answer.
pub(crate) const CC_TOOL_ALIASES: &[(&str, &str)] = &[
    ("Bash", "bash"),
    ("Read", "file_read"),
    ("Write", "file_write"),
    ("Edit", "file_edit"),
    ("MultiEdit", "file_edit"),
    ("Glob", "find"),
    ("Grep", "grep"),
    ("WebFetch", "web_fetch"),
    ("WebSearch", "search"),
    ("Task", "subagent"),
    ("Agent", "subagent"),
    ("AskUserQuestion", "ask_user"),
    ("ToolSearch", "tool_search"),
];

/// Claude Code → Aleph, or `None` for a name with no counterpart.
// Only test-called today: `normalize_cc_tool_entry` (below) is its one
// production caller, and that function is itself only test-called (same
// reason). Its production readers are P4.7b (a command's `allowed-tools:`)
// and P4.8 (an agent's `tools:`) — delete this attribute when either lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn aleph_name(cc: &str) -> Option<&'static str> {
    CC_TOOL_ALIASES
        .iter()
        .find(|(c, _)| *c == cc)
        .map(|(_, a)| *a)
}

/// Every Claude Code spelling of an Aleph tool name — the table's reverse
/// plus the MCP form: Aleph's `{server}__{tool}` is CC's `mcp__{server}__{tool}`.
/// (The plugin-level `mcp__plugin_<p>_<s>__` form needs the owning plugin,
/// which this module cannot know; matchers for it use `mcp__.*`.)
pub(crate) fn cc_spellings(aleph: &str) -> Vec<String> {
    let mut out: Vec<String> = CC_TOOL_ALIASES
        .iter()
        .filter(|(_, a)| *a == aleph)
        .map(|(c, _)| (*c).to_string())
        .collect();
    if let Some((server, tool)) = aleph.split_once("__") {
        if !server.is_empty() && !tool.is_empty() && !aleph.starts_with("mcp__") {
            out.push(format!("mcp__{server}__{tool}"));
        }
    }
    out
}

/// One `allowed-tools:` / `tools:` entry, as Claude Code writes it, into the
/// Aleph name the registries know.
///
/// * `Bash(gh pr view:*)` — CC's per-argument scoping. `fold_scoped_bash`
///   decides its fate: `true` (restrict semantics — a command's
///   `allowed-tools`) folds it to bare `bash`, the tier gate still governs
///   the call; `false` (pre-grant semantics — a skill's `allowed-tools`)
///   returns `None`, because pre-granting all of `bash` for a scoped grant
///   widens an approval skip.
/// * `mcp__<server>__<tool>` and `mcp__plugin_<plugin>_<server>__<tool>` →
///   `<server>__<tool>`.
/// * an Aleph name, or `*`, passes through unchanged.
// Only test-called today. Its production readers are P4.7b (a command's
// `allowed-tools:`) and P4.8 (an agent's `tools:`) — delete this attribute
// when either lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn normalize_cc_tool_entry(entry: &str, fold_scoped_bash: bool) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if let Some((head, _scope)) = entry.split_once('(') {
        return if fold_scoped_bash {
            aleph_name(head.trim()).map(str::to_string)
        } else {
            None
        };
    }
    if let Some(rest) = entry.strip_prefix("mcp__") {
        let (server, tool) = rest.rsplit_once("__")?;
        let server = server
            .strip_prefix("plugin_")
            .and_then(|s| s.split_once('_').map(|(_, srv)| srv))
            .unwrap_or(server);
        return Some(format!("{server}__{tool}"));
    }
    Some(aleph_name(entry).map_or_else(|| entry.to_string(), str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_aleph_target_is_a_real_tool_name() {
        // The table is hand-written; the registry is not. Every right-hand
        // side must be a name the executor can dispatch, or the alias maps a
        // CC hook onto nothing while looking like it works.
        // `BuiltinToolDefinition.name` is `&'static str`; `subagent` and
        // `tool_search` are the two dispatchable names that live outside that
        // catalog (`SUBAGENT_TOOL_NAME`, `ToolSearchTool::NAME`).
        let known: std::collections::HashSet<&str> = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|d| d.name)
            .chain([
                crate::agents::subagent_tool::SUBAGENT_TOOL_NAME,
                crate::tools::tool_search::ToolSearchTool::NAME,
            ])
            .collect();
        for (cc, aleph) in CC_TOOL_ALIASES {
            assert!(known.contains(aleph), "{cc} -> {aleph}: no such Aleph tool");
        }
    }

    #[test]
    fn cc_to_aleph_is_exact_and_case_sensitive() {
        assert_eq!(aleph_name("Bash"), Some("bash"));
        assert_eq!(aleph_name("Read"), Some("file_read"));
        assert_eq!(aleph_name("bash"), None, "an Aleph name is not a CC name");
        assert_eq!(aleph_name("NotebookEdit"), None, "documented gap");
    }

    #[test]
    fn aleph_to_cc_yields_every_spelling_including_mcp() {
        let mut bash = cc_spellings("bash");
        bash.sort();
        assert_eq!(bash, vec!["Bash"]);
        let mut edit = cc_spellings("file_edit");
        edit.sort();
        assert_eq!(edit, vec!["Edit", "MultiEdit"]);
        assert_eq!(
            cc_spellings("github__delete_repo"),
            vec!["mcp__github__delete_repo"]
        );
        assert!(
            cc_spellings("scratchpad").is_empty(),
            "no alias, no spelling"
        );
    }

    #[test]
    fn allowed_tools_entries_fold_by_mode() {
        // `Bash(gh pr view:*)` — CC's per-argument scoping. Restrict mode folds
        // it to bare `bash` (the tier gate still applies to the call);
        // pre-grant mode DROPS it (pre-granting all of bash for a scoped grant
        // would widen an approval skip).
        assert_eq!(
            normalize_cc_tool_entry("Bash(gh pr view:*)", true).as_deref(),
            Some("bash")
        );
        assert_eq!(normalize_cc_tool_entry("Bash(gh pr view:*)", false), None);
        assert_eq!(
            normalize_cc_tool_entry("Read", true).as_deref(),
            Some("file_read")
        );
        assert_eq!(
            normalize_cc_tool_entry("grep", true).as_deref(),
            Some("grep"),
            "Aleph names pass through"
        );
        assert_eq!(
            normalize_cc_tool_entry("mcp__srv__tool", true).as_deref(),
            Some("srv__tool")
        );
        assert_eq!(
            normalize_cc_tool_entry("mcp__plugin_x_srv__tool", true).as_deref(),
            Some("srv__tool")
        );
        assert_eq!(normalize_cc_tool_entry("*", true).as_deref(), Some("*"));
    }
}
