//! `tool_usage` — "which installed MCP server / plugin / skill is nobody using?"
//!
//! The R8 face of [`crate::tools::usage`]. Aleph could already *remove* an
//! extension conversationally (`self_config` edits the `[unified_tools.mcp]`
//! section, `plugins.uninstall` / `skill_manage` do the rest) — what it could
//! not do was answer the question that should come first. This tool supplies
//! the evidence; the decision stays with the user, and the removal stays with
//! the tools that already own it (R3 — this one never uninstalls anything).
//!
//! Read-only except for `forget_orphans`, which drops sidecar rows whose origin
//! is no longer installed. That is bookkeeping on this module's own file, not a
//! change to anything installed.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::mcp::manager::McpManagerHandle;
use crate::tools::usage::report::{build_report_now, ExtensionKind, UsageEntry};
use crate::tools::usage::ToolUsageStore;
use crate::tools::AlephTool;

/// Default idle window, shared with the `ext/idle-extensions` doctor check so
/// the two faces cannot disagree about what "idle" means.
use crate::diagnostics::checks::idle_extensions::DEFAULT_IDLE_DAYS;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ToolUsageArgs {
    /// `"all"` (default), `"mcp"`, `"plugin"`, `"skill"`.
    #[serde(default)]
    #[schemars(description = "Filter by kind: all (default), mcp, plugin, skill.")]
    pub scope: Option<String>,

    /// Only return rows idle for at least this many days (or never used).
    #[serde(default)]
    #[schemars(
        description = "Return only rows idle this many days or never used. Omit for every row."
    )]
    pub idle_days: Option<i64>,

    /// Drop sidecar rows for origins that are no longer installed.
    #[serde(default)]
    #[schemars(
        description = "Delete usage records for uninstalled servers/plugins. Does not touch \
                       anything installed."
    )]
    pub forget_orphans: bool,
}

#[derive(Debug, Serialize)]
pub struct ToolUsageOutput {
    /// One line the model can relay verbatim.
    pub summary: String,
    pub entries: Vec<UsageEntry>,
    /// Sidecar rows with no installed owner.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<String>,
    /// Kinds that could not be enumerated. **Non-empty means this answer is
    /// partial** — the model must say so rather than reporting the rest as
    /// complete.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<&'static str>,
    /// Number of orphan rows dropped by `forget_orphans`.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub forgotten: usize,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

/// `tool_usage` — read-only invocation records per installed extension.
#[derive(Clone, Default)]
pub struct ToolUsageTool {
    /// Live MCP manager handle. `None` still answers, with `mcp` listed in
    /// `unavailable` — an unknown category is reported, never silently omitted.
    pub mcp: Option<McpManagerHandle>,
}

fn kind_matches(entry: &UsageEntry, scope: &str) -> bool {
    match scope {
        "mcp" => entry.kind == ExtensionKind::Mcp,
        "plugin" => entry.kind == ExtensionKind::Plugin,
        "skill" => entry.kind == ExtensionKind::Skill,
        _ => true,
    }
}

#[async_trait]
impl AlephTool for ToolUsageTool {
    const NAME: &'static str = "tool_usage";

    // States the two things the model cannot read off the output itself: that a
    // `—` count is a different claim from `0`, and that this tool does not
    // remove anything (so it must route removal through the tools that do).
    const DESCRIPTION: &'static str = "Invocation records for installed MCP servers, plugins and skills — call counts, last-used dates, and what has gone unused: the evidence for deciding what to uninstall. A `—` count (not 0) means that entry has no tool-call channel to measure (e.g. a hooks-only plugin) and is NOT evidence it is unused. Read-only; removal goes through self_config (MCP), plugins.uninstall, or skill_manage.";

    type Args = ToolUsageArgs;
    type Output = ToolUsageOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let scope = args.scope.as_deref().unwrap_or("all").to_ascii_lowercase();
        notify_tool_start(Self::NAME, &scope);

        let report = build_report_now(self.mcp.as_ref()).await;

        let threshold = args.idle_days;
        let entries: Vec<UsageEntry> = report
            .entries
            .iter()
            .filter(|e| kind_matches(e, &scope))
            .filter(|e| threshold.is_none_or(|d| e.is_idle(d)))
            .cloned()
            .collect();

        let forgotten = if args.forget_orphans && !report.orphans.is_empty() {
            let orphans = report.orphans.clone();
            let count = orphans.len();
            tokio::task::spawn_blocking(move || {
                if let Some(store) = ToolUsageStore::default_path() {
                    for key in &orphans {
                        store.forget(key);
                    }
                }
            })
            .await
            .ok();
            count
        } else {
            0
        };

        let idle_now = entries
            .iter()
            .filter(|e| e.is_idle(threshold.unwrap_or(DEFAULT_IDLE_DAYS)))
            .count();
        let summary = format!(
            "{} entr(ies){}; {idle_now} idle at {}d{}",
            entries.len(),
            if scope == "all" {
                String::new()
            } else {
                format!(" in scope {scope}")
            },
            threshold.unwrap_or(DEFAULT_IDLE_DAYS),
            if report.unavailable.is_empty() {
                String::new()
            } else {
                format!(
                    " — INCOMPLETE, could not enumerate {}",
                    report
                        .unavailable
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join("/")
                )
            }
        );
        notify_tool_result(Self::NAME, &summary, true);

        Ok(ToolUsageOutput {
            summary,
            entries,
            orphans: if forgotten > 0 {
                Vec::new()
            } else {
                report.orphans.clone()
            },
            unavailable: report.unavailable.iter().map(|k| k.as_str()).collect(),
            forgotten,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: ExtensionKind, id: &str) -> UsageEntry {
        UsageEntry {
            kind,
            id: id.into(),
            name: id.into(),
            enabled: true,
            usage: crate::tools::usage::report::UsageSignal::Measured { calls: 1 },
            errors: 0,
            first_used_at: None,
            last_used_at: None,
            idle_days: Some(1),
            tools: Default::default(),
            breakdown_partial: false,
            pinned: false,
        }
    }

    #[test]
    fn scope_filters_by_kind_and_all_keeps_everything() {
        let rows = vec![
            entry(ExtensionKind::Mcp, "a"),
            entry(ExtensionKind::Plugin, "b"),
            entry(ExtensionKind::Skill, "c"),
        ];
        assert_eq!(rows.iter().filter(|e| kind_matches(e, "mcp")).count(), 1);
        assert_eq!(rows.iter().filter(|e| kind_matches(e, "plugin")).count(), 1);
        assert_eq!(rows.iter().filter(|e| kind_matches(e, "skill")).count(), 1);
        assert_eq!(rows.iter().filter(|e| kind_matches(e, "all")).count(), 3);
    }

    /// An unrecognised scope must widen, not silently return nothing — an empty
    /// result reads as "you have no MCP servers", which is the one answer this
    /// tool must never invent.
    #[test]
    fn an_unknown_scope_falls_back_to_all() {
        let rows = vec![entry(ExtensionKind::Mcp, "a")];
        assert_eq!(rows.iter().filter(|e| kind_matches(e, "mcpp")).count(), 1);
    }

    /// The description carries a claim the output cannot: that `—` ≠ `0`.
    /// If that sentence is dropped the model will read a hooks-only plugin's
    /// missing count as "unused" and propose deleting it.
    #[test]
    fn the_description_explains_the_dash() {
        let d = <ToolUsageTool as AlephTool>::DESCRIPTION;
        assert!(d.contains('—'), "must explain the not-measurable marker");
        assert!(
            d.contains("never uninstalls") || d.contains("Read-only"),
            "must state it does not remove anything"
        );
    }
}
