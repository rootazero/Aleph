//! `ext/idle-extensions` — which installed MCP server / plugin / skill is
//! nobody actually calling.
//!
//! Reads the join in [`crate::tools::usage::report`]; owns no accounting of its
//! own. Purely a sensor: it never disables or uninstalls anything, because
//! "this has been quiet for 40 days" is evidence, not a verdict — a server kept
//! for quarterly work is idle by design.
//!
//! ## Why every finding here is `Info`
//!
//! An unused extension is not a fault. Filing it as a `Warning` would push a
//! healthy install into a non-green posture on every run and train the reader
//! to skim past the severities that do mean something. The one thing this check
//! *does* escalate is its own blindness (see below).
//!
//! ## Registered by the daemon faces only, and never silently narrowed
//!
//! Like `providers/connectivity`, this check reaches the registry through an
//! explicit builder ([`DiagnosticEngine::with_extension_usage_check`]) rather
//! than `default_registry()` — the offline `aleph-server doctor` command has no
//! MCP actor and no extension manager to ask, so it does not run this check at
//! all. The two daemon faces (the `doctor` tool and `diagnostics.run`) both
//! register it, so they cannot disagree about the same machine.
//!
//! Passing `None` for the handle still registers it. Enumerating "what is
//! installed" needs those managers, and a run that cannot see one must report
//! that category as UNKNOWN — a report that quietly omits every MCP server
//! reads exactly like a clean bill of health, which is why the missing-category
//! finding is the one `Warning` this check can emit.

use std::time::Duration;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::mcp::manager::McpManagerHandle;
use crate::tools::usage::report::{build_report_now, ExtensionUsageReport, UsageEntry};

const ID: &str = "ext/idle-extensions";

/// Days of silence before an extension is worth mentioning.
///
/// Long on purpose. A month covers monthly reporting cycles and a two-week
/// holiday, so the check speaks about things that are genuinely dormant rather
/// than things that are merely between uses. Tunable per call site if a real
/// consumer ever needs a different window — it is not a config knob today
/// because nothing has asked for one (R10).
pub const DEFAULT_IDLE_DAYS: i64 = 30;

/// Cap on how many idle rows are named in one finding's detail. Beyond this the
/// detail says how many more there are — a truncation that announces itself.
const MAX_LISTED: usize = 12;

pub struct IdleExtensionsCheck {
    mcp: Option<McpManagerHandle>,
    idle_days: i64,
}

impl IdleExtensionsCheck {
    #[must_use]
    pub const fn new(mcp: Option<McpManagerHandle>) -> Self {
        Self {
            mcp,
            idle_days: DEFAULT_IDLE_DAYS,
        }
    }

    /// Pure rendering of an already-built report — the whole decision surface,
    /// separated from I/O so it is testable without a daemon.
    #[must_use]
    pub fn findings_for(report: &ExtensionUsageReport, idle_days: i64) -> Vec<Finding> {
        let mut findings = Vec::new();

        // Blindness first: a partial inventory changes how every other line
        // below should be read, so it must not be buried under them. This is
        // the one Warning the check can emit — silence about what it could not
        // see is the failure mode that makes a cleanup report dangerous.
        if !report.unavailable.is_empty() {
            let kinds: Vec<&str> = report.unavailable.iter().map(|k| k.as_str()).collect();
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Warning,
                    "Usage report is incomplete",
                    format!(
                        "Could not enumerate installed {}. Those categories are UNKNOWN in this \
                         run, not clean — do not read the lines below as covering them.",
                        kinds.join(" / ")
                    ),
                )
                .with_fix_hint(
                    "Run this from the live daemon (the `doctor` tool or `diagnostics.run`) so \
                     the MCP and extension registries are reachable.",
                ),
            );
        }

        let idle: Vec<&UsageEntry> = report.idle(idle_days).collect();
        if idle.is_empty() {
            if report.unavailable.is_empty() {
                findings.push(Finding::ok(
                    ID,
                    "No idle extensions",
                    format!(
                        "All {} installed extension(s) have been used within {idle_days} days.",
                        report.entries.len()
                    ),
                ));
            }
        } else {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Info,
                    format!("{} extension(s) idle for {idle_days}+ days", idle.len()),
                    render_idle(&idle),
                )
                .with_fix_hint(
                    "Ask me to clean these up, or inspect them with the `tool_usage` tool. \
                     Idle is not broken: keep anything you use seasonally, and `pin` a skill \
                     to exempt it permanently.",
                ),
            );
        }

        if !report.orphans.is_empty() {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Info,
                    format!("{} stale usage record(s)", report.orphans.len()),
                    format!(
                        "The usage sidecar still has rows for origins that are no longer \
                         installed: {}. Harmless, just clutter.",
                        report.orphans.join(", ")
                    ),
                )
                .with_fix_hint("`tool_usage` with `forget_orphans: true` drops them."),
            );
        }

        findings
    }
}

fn render_idle(idle: &[&UsageEntry]) -> String {
    let mut lines: Vec<String> = idle
        .iter()
        .take(MAX_LISTED)
        .map(|e| {
            let when = e.usage.calls().map_or_else(
                || "not measurable".to_string(),
                |calls| {
                    if calls == 0 {
                        "never used".to_string()
                    } else {
                        e.idle_days.map_or_else(
                            || format!("{calls} call(s), last use unknown"),
                            |d| format!("{calls} call(s), last used {d}d ago"),
                        )
                    }
                },
            );
            let state = if e.enabled { "" } else { ", disabled" };
            format!("  {}:{} — {when}{state}", e.kind.as_str(), e.id)
        })
        .collect();
    if idle.len() > MAX_LISTED {
        lines.push(format!("  … and {} more", idle.len() - MAX_LISTED));
    }
    lines.join("\n")
}

#[async_trait]
impl HealthCheck for IdleExtensionsCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Idle extensions"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // Read-only in every posture. Nothing here is mechanically repairable:
        // uninstalling something because it was quiet is a judgement call, and
        // `--fix` must never make it.
        let report = build_report_now(self.mcp.as_ref()).await;
        Self::findings_for(&report, self.idle_days)
    }

    /// Two registry reads and one small file read; the only unbounded part is
    /// the MCP manager actor's mailbox, which is why this is well under the
    /// default rather than at it.
    fn timeout(&self) -> Duration {
        Duration::from_secs(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::usage::report::{ExtensionKind, UsageEntry, UsageSignal};

    fn entry(kind: ExtensionKind, id: &str, calls: u64, idle_days: Option<i64>) -> UsageEntry {
        UsageEntry {
            kind,
            id: id.into(),
            name: id.into(),
            enabled: true,
            usage: UsageSignal::Measured { calls },
            errors: 0,
            first_used_at: None,
            last_used_at: None,
            idle_days,
            tools: Default::default(),
            breakdown_partial: false,
            pinned: false,
        }
    }

    fn report(entries: Vec<UsageEntry>) -> ExtensionUsageReport {
        ExtensionUsageReport {
            entries,
            orphans: Vec::new(),
            unavailable: Vec::new(),
        }
    }

    #[test]
    fn a_clean_install_reports_ok() {
        let r = report(vec![entry(ExtensionKind::Mcp, "live", 40, Some(2))]);
        let f = IdleExtensionsCheck::findings_for(&r, 30);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
        assert_eq!(f[0].title, "No idle extensions");
    }

    #[test]
    fn idle_rows_are_info_never_warning() {
        let r = report(vec![
            entry(ExtensionKind::Mcp, "old", 3, Some(70)),
            entry(ExtensionKind::Plugin, "never", 0, None),
        ]);
        let f = IdleExtensionsCheck::findings_for(&r, 30);
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].severity,
            Severity::Info,
            "an unused extension is not a fault; Warning would poison the posture"
        );
        assert!(f[0].detail.contains("mcp:old"));
        assert!(f[0].detail.contains("never used"));
    }

    /// The dangerous silence: if the inventory could not be enumerated, saying
    /// nothing reads exactly like a clean bill of health.
    #[test]
    fn an_unenumerable_kind_is_reported_and_suppresses_the_all_clear() {
        let mut r = report(Vec::new());
        r.unavailable = vec![ExtensionKind::Mcp];
        let f = IdleExtensionsCheck::findings_for(&r, 30);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);
        assert!(f[0].detail.contains("UNKNOWN"));
        assert!(
            !f.iter().any(|x| x.title == "No idle extensions"),
            "must not claim all-clear over a category it could not see"
        );
    }

    #[test]
    fn a_not_measurable_plugin_is_never_listed_as_idle() {
        let mut e = entry(ExtensionKind::Plugin, "hooky", 0, None);
        e.usage = UsageSignal::NotMeasurable {
            why: "ships no tools".into(),
        };
        let f = IdleExtensionsCheck::findings_for(&report(vec![e]), 30);
        assert_eq!(f[0].title, "No idle extensions");
    }

    #[test]
    fn long_lists_announce_their_truncation() {
        let entries: Vec<UsageEntry> = (0..MAX_LISTED + 4)
            .map(|i| entry(ExtensionKind::Mcp, &format!("s{i}"), 0, None))
            .collect();
        let f = IdleExtensionsCheck::findings_for(&report(entries), 30);
        assert!(f[0].detail.contains("… and 4 more"));
    }

    #[test]
    fn orphan_rows_get_their_own_finding() {
        let mut r = report(vec![entry(ExtensionKind::Mcp, "live", 5, Some(1))]);
        r.orphans = vec!["mcp:removed".into()];
        let f = IdleExtensionsCheck::findings_for(&r, 30);
        assert_eq!(f.len(), 2);
        assert!(f.iter().any(|x| x.title.contains("stale usage record")));
    }
}
