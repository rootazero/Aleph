//! Claude Code tool names ↔ Aleph tool names — the one table.
//!
//! Three readers, one derivation: the hook matcher (`executor.rs`,
//! `matches_pattern`), a CC command's `allowed-tools:` and a CC agent's
//! `tools:` (both `manifest/parsers.rs`, `restrict_tool_list`). Each reader
//! used to be a place where `Read` would silently name nothing. Every
//! right-hand side is checked against the real tool registry by
//! `every_aleph_target_is_a_real_tool_name`.

/// `(claude_code_name, aleph_name)`. Left side is exact and case-sensitive
/// (CC matchers are). Names with no Aleph counterpart are deliberately
/// absent — they are [`CC_TOOLS_WITHOUT_COUNTERPART`].
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
    // CC's `Skill` invokes a skill; `skill_read` is how Aleph loads one. Not
    // the same verb, but it is the tool that has to stay on the surface for a
    // command whose `allowed-tools` says `Skill` and whose body says "load
    // skill X first". A hook matcher on `Skill` fires on `skill_read`.
    ("Skill", "skill_read"),
];

/// Claude Code tools with no Aleph counterpart: `LS` (Aleph folds it into
/// `file_ops list`), `TodoWrite` (Aleph `scratchpad` is a different shape),
/// and the rest have no analogue. A hook matcher naming one matches nothing,
/// which is the correct fail-closed answer; an `allowed-tools:` / `tools:`
/// entry naming one is dropped by [`normalize_cc_tool_entry`], which for a
/// command's or an agent's restrict list means it cannot use it. A CC tool in
/// neither table is forwarded under its own name (the restrict-list reader
/// warns when the name is not spelled like an Aleph tool): a command's
/// registry refuses it by name (`register_skills`), an agent's allowlist
/// matches no tool with it.
pub(crate) const CC_TOOLS_WITHOUT_COUNTERPART: &[&str] = &[
    "NotebookEdit",
    "LS",
    "SlashCommand",
    "TodoWrite",
    "KillShell",
    "BashOutput",
    "EnterWorktree",
    "ExitWorktree",
];

/// Claude Code → Aleph, or `None` for a name with no counterpart.
pub(crate) fn aleph_name(cc: &str) -> Option<&'static str> {
    CC_TOOL_ALIASES
        .iter()
        .find(|(c, _)| *c == cc)
        .map(|(_, a)| *a)
}

/// Every Claude Code spelling of an Aleph tool name — the table's reverse
/// plus the MCP form: Aleph's `{server}__{tool}` is CC's `mcp__{server}__{tool}`.
/// A plugin's server needs no special case: its Aleph key is already
/// `plugin_{plugin}_{server}__{tool}` (`McpHandler::qualified_name` over
/// `mcp_config::plugin_server_id`), so the same prefix yields CC's
/// `mcp__plugin_{plugin}_{server}__{tool}`. [`normalize_cc_tool_entry`] is
/// the inverse.
///
/// Exact up to 64 characters only: Aleph cuts an MCP key to 64
/// (`sanitize_tool_name`) and Claude Code does not, so for a longer tool the
/// spelling produced here is a truncation of the one CC uses. A hook matcher
/// that spells the full long CC name misses; a prefix pattern still matches.
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
/// * `mcp__<rest>` → `<rest>`, the inverse of [`cc_spellings`]' MCP arm:
///   `mcp__<server>__<tool>` → `<server>__<tool>` and
///   `mcp__plugin_<plugin>_<server>__<tool>` → `plugin_<plugin>_<server>__<tool>`.
///   Nothing is taken apart, so an underscore in a plugin or server name
///   cannot move the boundary. `mcp__<server>` (every tool of one server)
///   names no single Aleph tool and is `None`. Exact up to 64 characters only
///   (see [`cc_spellings`]): a longer CC name, stripped, is longer than the
///   64-character key Aleph registered, names nothing, and the registry
///   refuses it by name.
/// * a [`CC_TOOLS_WITHOUT_COUNTERPART`] name is `None`.
/// * an Aleph name, or `*`, passes through unchanged.
pub(crate) fn normalize_cc_tool_entry(entry: &str, fold_scoped_bash: bool) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() || CC_TOOLS_WITHOUT_COUNTERPART.contains(&entry) {
        return None;
    }
    if let Some(head) = scoped_tool_head(entry) {
        return if fold_scoped_bash {
            aleph_name(head).map(str::to_string)
        } else {
            None
        };
    }
    if let Some(rest) = entry.strip_prefix("mcp__") {
        let (server, tool) = rest.rsplit_once("__")?;
        return (!server.is_empty() && !tool.is_empty()).then(|| rest.to_string());
    }
    Some(aleph_name(entry).map_or_else(|| entry.to_string(), str::to_string))
}

/// The tool half of a Claude Code per-argument scoped entry —
/// `Bash(git *)` → `Bash` — or `None` for an unscoped one. The one
/// recognition of "scoped": [`normalize_cc_tool_entry`] folds or drops on it,
/// and a command's reader logs the coarsening with it.
pub(crate) fn scoped_tool_head(entry: &str) -> Option<&str> {
    entry
        .trim()
        .split_once('(')
        .map(|(head, _scope)| head.trim())
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
        // A command's `Skill` must keep the tool that loads a skill.
        assert_eq!(aleph_name("Skill"), Some("skill_read"));
        assert_eq!(cc_spellings("skill_read"), vec!["Skill"]);
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
            Some("plugin_x_srv__tool"),
            "a plugin server's Aleph key keeps its `plugin_<p>_` part"
        );
        assert_eq!(
            normalize_cc_tool_entry("mcp__github", true),
            None,
            "a server-wide grant names no single Aleph tool"
        );
        assert_eq!(normalize_cc_tool_entry("*", true).as_deref(), Some("*"));
    }

    #[test]
    fn a_cc_tool_with_no_counterpart_is_dropped_never_forwarded() {
        // Forwarded, `TodoWrite` would reach `register_skills` as an unknown
        // name and cost the command its slash entry.
        for cc in CC_TOOLS_WITHOUT_COUNTERPART {
            assert_eq!(aleph_name(cc), None, "{cc} is in both tables");
            assert_eq!(normalize_cc_tool_entry(cc, true), None, "{cc}");
            assert_eq!(normalize_cc_tool_entry(cc, false), None, "{cc}");
        }
    }

    /// The CC side is what Claude Code writes; the Aleph side comes from the
    /// naming owner (`McpHandler::qualified_name` over
    /// `mcp_config::plugin_server_id`), never a literal. A dash in the plugin
    /// id and an underscore in the server name are the shapes a split of the
    /// CC name into plugin / server parts would get wrong.
    #[test]
    fn mcp_names_round_trip_through_alephs_own_naming() {
        let aleph = |server_id: String| {
            crate::tools::handlers::mcp::McpHandler::new(
                crate::sync_primitives::Arc::new(crate::mcp::McpClient::new()),
                server_id,
                "do_thing".into(),
                String::new(),
                serde_json::json!({"type": "object"}),
            )
            .qualified_name()
        };
        for (cc, server_id) in [
            (
                "mcp__plugin_my-plug_my_srv__do_thing",
                crate::extension::mcp_config::plugin_server_id("my-plug", "my_srv"),
            ),
            ("mcp__github__do_thing", "github".to_string()),
        ] {
            let name = aleph(server_id);
            assert_eq!(
                normalize_cc_tool_entry(cc, true).as_deref(),
                Some(name.as_str()),
                "{cc}"
            );
            assert!(
                cc_spellings(&name).iter().any(|s| s == cc),
                "{name} must be spelled {cc} for a CC matcher"
            );
        }
    }
}
