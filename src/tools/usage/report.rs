//! The join: installed things × recorded activity.
//!
//! [`ToolUsageStore`](super::store::ToolUsageStore) only knows what was
//! *called*; it deliberately seeds nothing at registration. "Installed but
//! never called" therefore only exists as an answer once the sidecar is joined
//! against the live inventory of configured MCP servers / installed plugins /
//! registered skills — which is what this module does, once, for every
//! consumer.
//!
//! Skills keep their own, older sidecar (`<skills_dir>/.usage.json`, owned by
//! [`crate::skill::usage`]). This module **reads** it rather than mirroring it:
//! a second copy of the same fact is how the two drift, and the skill sidecar
//! also carries lifecycle state (`pinned`, `stale`) that the dream pipeline
//! owns. The report is the join point, not a replacement store.
//!
//! ## Two things zero can mean
//!
//! A plugin that ships hooks, skills or an MCP server but no tools of its own
//! has **no invocation channel this accounting can observe**. Reporting it as
//! `0 calls` would read as "installed and unused" and invite deleting a plugin
//! that runs on every turn. [`UsageSignal::NotMeasurable`] makes that
//! difference a type, not a convention a renderer has to remember — every
//! surface prints `—` plus the reason, never a zero.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::store::{OriginUsage, ToolUsageStore, UsageOrigin};

// ============================================================================
// Inventory — what is installed
// ============================================================================

/// One configured MCP server, as the manager knows it.
#[derive(Debug, Clone)]
pub struct InstalledMcp {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

/// One installed plugin, with the counts that decide whether "zero calls" is
/// even meaningful for it.
#[derive(Debug, Clone, Default)]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub tool_count: usize,
    pub hook_count: usize,
    pub skill_count: usize,
    pub command_count: usize,
    pub service_count: usize,
    pub mcp_server_count: usize,
}

/// One registered skill plus its own sidecar row.
#[derive(Debug, Clone)]
pub struct InstalledSkill {
    pub id: String,
    pub name: String,
    pub disabled: bool,
    /// `use_count + view_count`. Install/enable bumps (`patch_count`) are
    /// deliberately excluded: installing something is not using it, and
    /// counting it would make every fresh install look active for a month.
    pub activity: u64,
    /// Newest of `last_used_at` / `last_viewed_at`, for the same reason.
    pub last_active_at: Option<String>,
    pub pinned: bool,
    /// `false` for skills that ship inside the binary. See
    /// [`UsageEntry::removable`] for why a cleanup report needs this.
    pub removable: bool,
}

/// What is installed right now, per kind.
///
/// `unavailable` names the kinds that could **not** be enumerated — the MCP
/// manager handle is absent in the offline CLI, the extension manager is absent
/// before boot finishes. An un-enumerable kind is reported as unknown and never
/// silently as "nothing installed": a cleanup report that quietly omits every
/// MCP server reads exactly like a clean bill of health.
#[derive(Debug, Clone, Default)]
pub struct UsageInventory {
    pub mcp: Vec<InstalledMcp>,
    pub plugins: Vec<InstalledPlugin>,
    pub skills: Vec<InstalledSkill>,
    pub unavailable: Vec<ExtensionKind>,
}

// ============================================================================
// Report
// ============================================================================

/// Whether a zero here means anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum UsageSignal {
    /// This entry has an invocation channel; `calls` is the real count and
    /// `0` genuinely means "never used".
    Measured { calls: u64 },
    /// This entry has no invocation channel that tool-call accounting can see,
    /// so no number would be honest. `why` is shown verbatim to the user.
    NotMeasurable { why: String },
}

impl UsageSignal {
    /// The call count when one exists. `None` is the `—` every renderer prints.
    #[must_use]
    pub const fn calls(&self) -> Option<u64> {
        match self {
            Self::Measured { calls } => Some(*calls),
            Self::NotMeasurable { .. } => None,
        }
    }

    /// `true` only for a measurable entry that has genuinely never been called.
    #[must_use]
    pub const fn is_never_used(&self) -> bool {
        matches!(self, Self::Measured { calls: 0 })
    }
}

/// Which registry a row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Mcp,
    Plugin,
    Skill,
}

impl ExtensionKind {
    /// Lowercase wire spelling, for renderers and log lines.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Plugin => "plugin",
            Self::Skill => "skill",
        }
    }
}

/// One row of the cleanup report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEntry {
    pub kind: ExtensionKind,
    pub id: String,
    pub name: String,
    /// Enabled/active per its own registry. A disabled entry is reported, not
    /// hidden — "disabled months ago and never re-enabled" is a cleanup signal.
    pub enabled: bool,
    #[serde(flatten)]
    pub usage: UsageSignal,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub errors: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    /// Whole days since the last recorded activity. `None` when never used or
    /// not measurable — deliberately not a large sentinel number, which sorts
    /// and renders as though it were a measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_days: Option<i64>,
    /// Per-tool breakdown for MCP/plugin entries.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, u64>,
    /// `true` when `tools` omits calls that `calls` still counts (the
    /// per-origin breakdown cap was hit).
    #[serde(default, skip_serializing_if = "is_false")]
    pub breakdown_partial: bool,
    /// Skill pinned against lifecycle auto-transitions; never propose deleting.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    /// `false` when no uninstall path can act on this row.
    ///
    /// Bundled skills ship inside the binary and
    /// [`SkillSystem::remove_skill`](crate::skill::SkillSystem::remove_skill)
    /// refuses them with `PermissionDenied`. Naming one in a cleanup report
    /// invites an action that cannot succeed — and on a fresh install the
    /// bundled set is the overwhelming majority of every "never used" row,
    /// so the report's first impression was 50+ items the reader is not
    /// allowed to act on.
    ///
    /// Defaults to `true` so a payload written before this field existed
    /// deserialises to the previous behaviour rather than to "nothing is
    /// removable".
    #[serde(default = "removable_default", skip_serializing_if = "is_true")]
    pub removable: bool,
}

/// Serde default for [`UsageEntry::removable`] — see that field's doc.
const fn removable_default() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(v: &u64) -> bool {
    *v == 0
}
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(v: &bool) -> bool {
    !*v
}
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(v: &bool) -> bool {
    *v
}

impl UsageEntry {
    /// Whether this row could be named in a cleanup report **at all**,
    /// independent of how quiet it has been.
    ///
    /// Three things disqualify a row, and all three are properties of the row
    /// rather than of its activity: it has no observable invocation channel
    /// (`NotMeasurable` — no number would be honest), the user pinned it, or
    /// nothing can uninstall it ([`Self::removable`]).
    #[must_use]
    pub fn is_cleanup_candidate(&self) -> bool {
        !self.pinned && self.removable && self.usage.calls().is_some()
    }

    /// Installed, actionable, and genuinely never called.
    ///
    /// Deliberately **disjoint** from [`Self::is_idle`]. These two used to be
    /// one predicate, which forced their one caller to describe both with a
    /// single sentence — and the sentence it chose asserted a duration
    /// ("idle for 30+ days") that a never-used row does not have: `idle_days`
    /// is `None` precisely because there is no last use to measure from. A
    /// machine installed ten minutes ago reported its whole bundled skill set
    /// as month-dormant.
    #[must_use]
    pub fn is_never_used(&self) -> bool {
        self.is_cleanup_candidate() && self.usage.is_never_used()
    }

    /// Installed, actionable, called at least once, and quiet for at least
    /// `threshold_days`. Every row this returns `true` for has a real measured
    /// `idle_days`, so a caller may quote the duration.
    #[must_use]
    pub fn is_idle(&self, threshold_days: i64) -> bool {
        self.is_cleanup_candidate()
            && !self.usage.is_never_used()
            && self.idle_days.is_some_and(|d| d >= threshold_days)
    }
}

/// The joined view every consumer reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionUsageReport {
    pub entries: Vec<UsageEntry>,
    /// Sidecar rows whose origin is no longer installed. Safe to `forget()`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<String>,
    /// Kinds that could not be enumerated. Non-empty means the report is
    /// incomplete — say so rather than implying those kinds are clean.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<ExtensionKind>,
}

impl ExtensionUsageReport {
    /// Rows called at least once and quiet for at least `threshold_days`.
    /// Disjoint from [`Self::never_used`] — see [`UsageEntry::is_never_used`].
    pub fn idle(&self, threshold_days: i64) -> impl Iterator<Item = &UsageEntry> {
        self.entries
            .iter()
            .filter(move |e| e.is_idle(threshold_days))
    }

    /// Rows that are installed and actionable but have never been called.
    pub fn never_used(&self) -> impl Iterator<Item = &UsageEntry> {
        self.entries.iter().filter(|e| e.is_never_used())
    }
}

impl From<&UsageEntry> for aleph_protocol::extension_usage::UsageSummary {
    /// The wire projection of a report row, for the `mcp_config.list` /
    /// `plugins.list` columns. Derived from the entry rather than recomputed at
    /// each handler so the `—`-vs-`0` distinction has exactly one definition.
    fn from(e: &UsageEntry) -> Self {
        Self {
            calls: e.usage.calls(),
            not_measurable_reason: match &e.usage {
                UsageSignal::NotMeasurable { why } => Some(why.clone()),
                UsageSignal::Measured { .. } => None,
            },
            errors: e.errors,
            last_used_at: e.last_used_at.clone(),
            idle_days: e.idle_days,
        }
    }
}

/// Join an inventory against the recorded activity. Pure — `now` is a
/// parameter so idle arithmetic is testable without a clock.
#[must_use]
pub fn build_report(
    inventory: &UsageInventory,
    usage: &std::collections::HashMap<String, OriginUsage>,
    now: DateTime<Utc>,
) -> ExtensionUsageReport {
    let mut entries =
        Vec::with_capacity(inventory.mcp.len() + inventory.plugins.len() + inventory.skills.len());
    let mut claimed: Vec<String> = Vec::new();

    for server in &inventory.mcp {
        let key = UsageOrigin::mcp_key(&server.id);
        let row = usage.get(&key);
        claimed.push(key);
        entries.push(entry_from_origin(
            ExtensionKind::Mcp,
            &server.id,
            &server.name,
            server.enabled,
            row,
            now,
        ));
    }

    for plugin in &inventory.plugins {
        let key = UsageOrigin::plugin_key(&plugin.id);
        let row = usage.get(&key);
        claimed.push(key);
        if plugin.tool_count == 0 {
            entries.push(UsageEntry {
                kind: ExtensionKind::Plugin,
                id: plugin.id.clone(),
                name: plugin.name.clone(),
                enabled: plugin.active,
                usage: UsageSignal::NotMeasurable {
                    why: no_tools_reason(plugin),
                },
                errors: 0,
                first_used_at: None,
                last_used_at: None,
                idle_days: None,
                tools: BTreeMap::new(),
                breakdown_partial: false,
                pinned: false,
                // A plugin is uninstallable regardless of whether its calls
                // are measurable; `is_cleanup_candidate` excludes this row via
                // the `NotMeasurable` signal, not via this flag.
                removable: true,
            });
        } else {
            entries.push(entry_from_origin(
                ExtensionKind::Plugin,
                &plugin.id,
                &plugin.name,
                plugin.active,
                row,
                now,
            ));
        }
    }

    for skill in &inventory.skills {
        entries.push(UsageEntry {
            kind: ExtensionKind::Skill,
            id: skill.id.clone(),
            name: skill.name.clone(),
            enabled: !skill.disabled,
            usage: UsageSignal::Measured {
                calls: skill.activity,
            },
            errors: 0,
            first_used_at: None,
            last_used_at: skill.last_active_at.clone(),
            idle_days: skill
                .last_active_at
                .as_deref()
                .and_then(|t| days_since(t, now)),
            tools: BTreeMap::new(),
            breakdown_partial: false,
            pinned: skill.pinned,
            removable: skill.removable,
        });
    }

    // Rows recorded against something that is no longer installed. Only
    // computed for the kinds we could actually enumerate — otherwise an absent
    // MCP handle would declare every MCP row an orphan and invite deleting the
    // history of servers that are merely not visible from here.
    let mut orphans: Vec<String> = if inventory.unavailable.is_empty() {
        usage
            .keys()
            .filter(|k| !claimed.contains(k))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    orphans.sort();

    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(&b.id)));

    ExtensionUsageReport {
        entries,
        orphans,
        unavailable: inventory.unavailable.clone(),
    }
}

fn entry_from_origin(
    kind: ExtensionKind,
    id: &str,
    name: &str,
    enabled: bool,
    row: Option<&OriginUsage>,
    now: DateTime<Utc>,
) -> UsageEntry {
    let usage = row.map_or(0, |r| r.call_count);
    UsageEntry {
        kind,
        id: id.to_string(),
        name: name.to_string(),
        enabled,
        usage: UsageSignal::Measured { calls: usage },
        errors: row.map_or(0, |r| r.error_count),
        first_used_at: row.and_then(|r| r.first_used_at.clone()),
        last_used_at: row.and_then(|r| r.last_used_at.clone()),
        idle_days: row
            .and_then(|r| r.last_used_at.as_deref())
            .and_then(|t| days_since(t, now)),
        tools: row.map(|r| r.tools.clone()).unwrap_or_default(),
        breakdown_partial: row.is_some_and(OriginUsage::breakdown_is_partial),
        pinned: false,
        // Both kinds this helper builds have a real uninstall path
        // (`mcp_config` remove / `plugins` uninstall). Only skills have a
        // subset the binary owns, so only the skill arm computes this.
        removable: true,
    }
}

/// Why a plugin has no measurable call count — names what it *does* ship so the
/// reader does not read "not measurable" as "does nothing".
fn no_tools_reason(plugin: &InstalledPlugin) -> String {
    let mut parts = Vec::new();
    if plugin.hook_count > 0 {
        parts.push(format!("{} hook(s)", plugin.hook_count));
    }
    if plugin.skill_count > 0 {
        parts.push(format!("{} skill(s)", plugin.skill_count));
    }
    if plugin.command_count > 0 {
        parts.push(format!("{} command(s)", plugin.command_count));
    }
    if plugin.mcp_server_count > 0 {
        parts.push(format!("{} MCP server(s)", plugin.mcp_server_count));
    }
    if plugin.service_count > 0 {
        parts.push(format!("{} service(s)", plugin.service_count));
    }
    if parts.is_empty() {
        return "ships no tools, so tool-call accounting cannot observe it".to_string();
    }
    format!(
        "ships no tools of its own (it provides {}); tool-call accounting cannot observe those \
         — check the mcp:/skill: rows it contributes",
        parts.join(", ")
    )
}

/// Whole days between an RFC3339 stamp and `now`. `None` when unparseable —
/// the caller then reports "unknown", never "fresh".
fn days_since(stamp: &str, now: DateTime<Utc>) -> Option<i64> {
    let then = DateTime::parse_from_rfc3339(stamp)
        .ok()?
        .with_timezone(&Utc);
    Some((now - then).num_days().max(0))
}

// ============================================================================
// Live collection
// ============================================================================

/// Read the current inventory from whatever live handles exist.
///
/// `mcp` is optional because the offline `aleph doctor` has no manager actor to
/// ask; a missing handle lands in [`UsageInventory::unavailable`] instead of
/// producing an empty, falsely-clean MCP section.
pub async fn collect_inventory(
    mcp: Option<&crate::mcp::manager::McpManagerHandle>,
) -> UsageInventory {
    let mut inv = UsageInventory::default();

    match mcp {
        Some(handle) => match handle.list_server_configs().await {
            Ok(configs) => {
                inv.mcp = configs
                    .into_iter()
                    .map(|c| InstalledMcp {
                        id: c.id,
                        name: c.name,
                        // The MCP manager's spelling of "enabled" is
                        // `auto_start` — the same field `mcp_config.list`
                        // renders as `enabled` to the Panel.
                        enabled: c.auto_start,
                    })
                    .collect();
            }
            Err(e) => {
                tracing::warn!(error = %e, "usage report: MCP server list unavailable");
                inv.unavailable.push(ExtensionKind::Mcp);
            }
        },
        None => inv.unavailable.push(ExtensionKind::Mcp),
    }

    let Some(manager) = crate::extension::try_extension_manager() else {
        inv.unavailable.push(ExtensionKind::Plugin);
        inv.unavailable.push(ExtensionKind::Skill);
        return inv;
    };

    {
        let registry = manager.get_plugin_registry().await;
        inv.plugins = registry
            .list_plugins()
            .into_iter()
            .map(|p| InstalledPlugin {
                id: p.id.clone(),
                name: p.name.clone(),
                active: p.status.is_active(),
                tool_count: p.tool_names.len(),
                hook_count: p.hook_count,
                skill_count: p.skill_count,
                command_count: p.command_count,
                service_count: p.service_ids.len(),
                mcp_server_count: p.mcp_server_count,
            })
            .collect();
    }

    inv.skills = manager
        .skill_system()
        .full_status()
        .await
        .into_iter()
        .map(|e| {
            let usage = e.usage.as_ref();
            InstalledSkill {
                id: e.id.as_str().to_string(),
                name: e.name,
                disabled: e.disabled,
                // Uses + views. `patch_count` (install / enable / scope change)
                // is excluded: installing is not using, and counting it would
                // make every fresh install look active.
                activity: usage.map_or(0, |u| u.use_count.saturating_add(u.view_count)),
                last_active_at: usage.and_then(|u| {
                    [u.last_used_at.as_deref(), u.last_viewed_at.as_deref()]
                        .into_iter()
                        .flatten()
                        .max()
                        .map(str::to_string)
                }),
                pinned: usage.is_some_and(|u| u.pinned),
                // The single fact `remove_skill` gates on. Read from the
                // manifest source rather than re-derived from the id or the
                // path: those are two more ways to disagree with the code
                // that actually refuses the removal.
                removable: !matches!(e.source, crate::domain::skill::SkillSource::Bundled),
            }
        })
        .collect();

    inv
}

/// Collect the inventory and join it with the sidecar in one call — what the
/// `tool_usage` tool and the `ext/idle-extensions` check both invoke.
pub async fn build_report_now(
    mcp: Option<&crate::mcp::manager::McpManagerHandle>,
) -> ExtensionUsageReport {
    let inventory = collect_inventory(mcp).await;
    let usage = tokio::task::spawn_blocking(|| {
        ToolUsageStore::default_path()
            .map(|s| s.snapshot())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    build_report(&inventory, &usage, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn mcp(id: &str) -> InstalledMcp {
        InstalledMcp {
            id: id.into(),
            name: id.into(),
            enabled: true,
        }
    }

    fn row(calls: u64, last: &str) -> OriginUsage {
        OriginUsage {
            call_count: calls,
            last_used_at: Some(last.into()),
            ..Default::default()
        }
    }

    fn skill(id: &str, activity: u64, last: Option<&str>, removable: bool) -> InstalledSkill {
        InstalledSkill {
            id: id.into(),
            name: id.into(),
            disabled: false,
            activity,
            last_active_at: last.map(str::to_string),
            pinned: false,
            removable,
        }
    }

    /// `remove_skill` refuses `SkillSource::Bundled` with `PermissionDenied`,
    /// so a cleanup report that names one is inviting an action that cannot
    /// succeed. On a fresh install the bundled set is ~all of the never-used
    /// rows, which is how the first `doctor` run on a ten-minute-old machine
    /// came to propose deleting 53 things.
    #[test]
    fn a_bundled_skill_is_never_a_cleanup_candidate() {
        let inv = UsageInventory {
            skills: vec![skill("bundled-one", 0, None, false)],
            ..Default::default()
        };
        let report = build_report(&inv, &HashMap::new(), now());
        assert!(
            report.entries[0].usage.is_never_used(),
            "the raw signal still reports the honest zero"
        );
        assert!(
            !report.entries[0].is_cleanup_candidate(),
            "but nothing can act on it, so it is not a candidate"
        );
        assert_eq!(report.never_used().count(), 0);
        assert_eq!(report.idle(30).count(), 0);
    }

    /// The two populations must partition the candidates: a row that has never
    /// been called has no `idle_days` to quote, and a row with a measured
    /// duration is by definition not never-used. They were one predicate, and
    /// the single sentence their one caller had to write was false for half of
    /// them.
    #[test]
    fn never_used_and_idle_are_disjoint() {
        let inv = UsageInventory {
            skills: vec![
                skill("quiet", 4, Some("2026-06-01T00:00:00Z"), true),
                skill("untouched", 0, None, true),
                skill("busy", 9, Some("2026-08-09T00:00:00Z"), true),
            ],
            ..Default::default()
        };
        let report = build_report(&inv, &HashMap::new(), now());

        let never: Vec<&str> = report.never_used().map(|e| e.id.as_str()).collect();
        let idle: Vec<&str> = report.idle(30).map(|e| e.id.as_str()).collect();
        assert_eq!(never, vec!["untouched"]);
        assert_eq!(idle, vec!["quiet"]);
        assert!(
            never.iter().all(|n| !idle.contains(n)),
            "no row may be counted by both"
        );
        assert!(
            report.idle(30).all(|e| e.idle_days.is_some()),
            "every idle row must carry the duration its caller will quote"
        );
    }

    #[test]
    fn an_installed_server_with_no_row_reports_never_used() {
        let inv = UsageInventory {
            mcp: vec![mcp("ghost")],
            ..Default::default()
        };
        let report = build_report(&inv, &HashMap::new(), now());
        let e = &report.entries[0];
        assert_eq!(e.usage, UsageSignal::Measured { calls: 0 });
        assert!(e.usage.is_never_used());
        assert_eq!(e.idle_days, None, "never used has no idle measurement");
        // The line above is exactly why `is_idle` must be false here: this row
        // has no duration, so the "idle for N+ days" heading cannot describe
        // it. It belongs to the never-used population instead.
        assert!(e.is_never_used());
        assert!(!e.is_idle(30));
    }

    #[test]
    fn idle_days_come_from_the_last_call() {
        let inv = UsageInventory {
            mcp: vec![mcp("old"), mcp("fresh")],
            ..Default::default()
        };
        let mut usage = HashMap::new();
        usage.insert("mcp:old".to_string(), row(5, "2026-06-01T00:00:00Z"));
        usage.insert("mcp:fresh".to_string(), row(5, "2026-08-09T00:00:00Z"));
        let report = build_report(&inv, &usage, now());

        let old = report.entries.iter().find(|e| e.id == "old").unwrap();
        let fresh = report.entries.iter().find(|e| e.id == "fresh").unwrap();
        assert_eq!(old.idle_days, Some(70));
        assert_eq!(fresh.idle_days, Some(1));
        assert!(old.is_idle(30));
        assert!(!fresh.is_idle(30));
    }

    #[test]
    fn an_unparseable_stamp_is_unknown_not_fresh() {
        let inv = UsageInventory {
            mcp: vec![mcp("weird")],
            ..Default::default()
        };
        let mut usage = HashMap::new();
        usage.insert("mcp:weird".to_string(), row(3, "not-a-date"));
        let report = build_report(&inv, &usage, now());
        assert_eq!(report.entries[0].idle_days, None);
        assert!(
            !report.entries[0].is_idle(30),
            "an unreadable stamp must not be silently treated as idle either"
        );
    }

    /// The requirement that made `UsageSignal` an enum: a plugin with no tools
    /// must never print `0`, which reads as "installed and unused".
    #[test]
    fn a_plugin_without_tools_is_not_measurable_rather_than_zero() {
        let inv = UsageInventory {
            plugins: vec![InstalledPlugin {
                id: "hooky".into(),
                name: "hooky".into(),
                active: true,
                tool_count: 0,
                hook_count: 3,
                ..Default::default()
            }],
            ..Default::default()
        };
        let report = build_report(&inv, &HashMap::new(), now());
        let e = &report.entries[0];
        assert_eq!(e.usage.calls(), None, "must render as `—`, not 0");
        assert!(!e.usage.is_never_used());
        assert!(!e.is_idle(0), "not measurable can never be a cleanup候补");
        match &e.usage {
            UsageSignal::NotMeasurable { why } => assert!(why.contains("3 hook(s)")),
            other => panic!("expected NotMeasurable, got {other:?}"),
        }
    }

    #[test]
    fn a_plugin_with_tools_is_measured_normally() {
        let inv = UsageInventory {
            plugins: vec![InstalledPlugin {
                id: "toolish".into(),
                name: "toolish".into(),
                active: true,
                tool_count: 2,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut usage = HashMap::new();
        usage.insert("plugin:toolish".to_string(), row(9, "2026-08-01T00:00:00Z"));
        let report = build_report(&inv, &usage, now());
        assert_eq!(report.entries[0].usage.calls(), Some(9));
    }

    #[test]
    fn a_pinned_skill_is_never_a_cleanup_candidate() {
        let inv = UsageInventory {
            skills: vec![InstalledSkill {
                id: "keep".into(),
                name: "keep".into(),
                disabled: false,
                activity: 0,
                last_active_at: None,
                pinned: true,
                removable: true,
            }],
            ..Default::default()
        };
        let report = build_report(&inv, &HashMap::new(), now());
        assert!(report.entries[0].usage.is_never_used());
        assert!(!report.entries[0].is_idle(30), "pinned opts out");
    }

    #[test]
    fn rows_for_uninstalled_origins_surface_as_orphans() {
        let inv = UsageInventory {
            mcp: vec![mcp("live")],
            ..Default::default()
        };
        let mut usage = HashMap::new();
        usage.insert("mcp:live".to_string(), row(1, "2026-08-09T00:00:00Z"));
        usage.insert("mcp:removed".to_string(), row(40, "2026-05-01T00:00:00Z"));
        let report = build_report(&inv, &usage, now());
        assert_eq!(report.orphans, vec!["mcp:removed".to_string()]);
    }

    /// The failure this guards is the dangerous one: an absent handle must not
    /// turn every recorded server into a deletable orphan.
    #[test]
    fn nothing_is_called_an_orphan_while_a_kind_is_unenumerable() {
        let inv = UsageInventory {
            unavailable: vec![ExtensionKind::Mcp],
            ..Default::default()
        };
        let mut usage = HashMap::new();
        usage.insert("mcp:real".to_string(), row(40, "2026-05-01T00:00:00Z"));
        let report = build_report(&inv, &usage, now());
        assert!(report.orphans.is_empty());
        assert_eq!(report.unavailable, vec![ExtensionKind::Mcp]);
    }

    #[test]
    fn breakdown_partial_rides_through_to_the_report() {
        let inv = UsageInventory {
            mcp: vec![mcp("chatty")],
            ..Default::default()
        };
        let mut usage = HashMap::new();
        usage.insert(
            "mcp:chatty".to_string(),
            OriginUsage {
                call_count: 100,
                other_tool_calls: 7,
                ..Default::default()
            },
        );
        let report = build_report(&inv, &usage, now());
        assert!(report.entries[0].breakdown_partial);
    }
}
