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

        // Two disjoint populations, two findings. They were one finding until
        // the title it had to share turned out to be false for half of them:
        // "idle for 30+ days" quotes a duration that a never-called row does
        // not have, so a machine installed ten minutes ago announced its
        // entire bundled skill set as month-dormant. See
        // `UsageEntry::is_never_used`.
        let never: Vec<&UsageEntry> = report.never_used().collect();
        let idle: Vec<&UsageEntry> = report.idle(idle_days).collect();

        if never.is_empty() && idle.is_empty() && report.unavailable.is_empty() {
            let actionable = report
                .entries
                .iter()
                .filter(|e| e.is_cleanup_candidate())
                .count();
            findings.push(Finding::ok(
                ID,
                "No idle extensions",
                // Counts candidates, not rows: pinned, not-measurable and
                // bundled entries are never proposed for cleanup, so claiming
                // they "have been used" would be inventing an observation
                // about entries this check does not measure.
                format!(
                    "All {actionable} removable extension(s) have been used within \
                     {idle_days} days."
                ),
            ));
        }

        if !never.is_empty() {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Info,
                    format!("{} extension(s) installed but never used", never.len()),
                    render_rows(&never, &|_| "never used".to_string()),
                )
                .with_fix_hint(
                    "Nothing here has ever been invoked. Inspect with the `tool_usage` tool, \
                     or ask me to remove the ones you no longer want. Skills bundled with \
                     Aleph are excluded — they ship inside the binary and cannot be removed.",
                ),
            );
        }

        if !idle.is_empty() {
            findings.push(
                Finding::problem(
                    ID,
                    Severity::Info,
                    format!("{} extension(s) idle for {idle_days}+ days", idle.len()),
                    render_rows(&idle, &|e| {
                        // `is_idle` guarantees both of these are present: it
                        // requires a measurable count and a measured
                        // `idle_days`. The defaults are unreachable.
                        format!(
                            "{} call(s), last used {}d ago",
                            e.usage.calls().unwrap_or_default(),
                            e.idle_days.unwrap_or_default()
                        )
                    }),
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

/// One line per row, capped at [`MAX_LISTED`].
///
/// `when` renders the activity phrase. The two callers pass disjoint sets
/// (never-called vs measured-and-quiet), so neither carries the other's
/// branch — which is what let the old single renderer print "never used"
/// under a heading that claimed a measured duration.
fn render_rows(rows: &[&UsageEntry], when: &dyn Fn(&UsageEntry) -> String) -> String {
    let mut lines: Vec<String> = rows
        .iter()
        .take(MAX_LISTED)
        .map(|e| {
            let state = if e.enabled { "" } else { ", disabled" };
            format!("  {}:{} — {}{state}", e.kind.as_str(), e.id, when(e))
        })
        .collect();
    if rows.len() > MAX_LISTED {
        lines.push(format!("  … and {} more", rows.len() - MAX_LISTED));
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
            removable: true,
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
        assert_eq!(f.len(), 2, "never-used and idle are separate findings");
        assert!(
            f.iter().all(|x| x.severity == Severity::Info),
            "an unused extension is not a fault; Warning would poison the posture"
        );
    }

    /// The regression this split exists for. A machine installed ten minutes
    /// ago has a large never-used set and a *zero*-size idle set; the old
    /// single finding folded them together under a title that quoted a
    /// duration none of them had.
    #[test]
    fn never_used_rows_are_never_described_as_month_dormant() {
        let r = report(vec![
            entry(ExtensionKind::Skill, "fresh-a", 0, None),
            entry(ExtensionKind::Skill, "fresh-b", 0, None),
        ]);
        let f = IdleExtensionsCheck::findings_for(&r, 30);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].title, "2 extension(s) installed but never used");
        assert!(
            !f.iter().any(|x| x.title.contains("idle for")),
            "a row with no last use cannot be quoted a duration"
        );
        assert!(
            !f[0].detail.contains("d ago"),
            "nor may the detail invent one"
        );
    }

    /// `remove_skill` refuses bundled skills, so naming one here proposes an
    /// action that returns `PermissionDenied`.
    #[test]
    fn a_bundled_skill_is_not_offered_for_cleanup() {
        let mut e = entry(ExtensionKind::Skill, "bundled-one", 0, None);
        e.removable = false;
        let f = IdleExtensionsCheck::findings_for(&report(vec![e]), 30);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].title, "No idle extensions");
    }

    #[test]
    fn the_two_findings_partition_the_candidates() {
        let r = report(vec![
            entry(ExtensionKind::Mcp, "old", 3, Some(70)),
            entry(ExtensionKind::Plugin, "never", 0, None),
        ]);
        let f = IdleExtensionsCheck::findings_for(&r, 30);
        let never = f
            .iter()
            .find(|x| x.title.contains("never used"))
            .expect("never-used finding");
        let idle = f
            .iter()
            .find(|x| x.title.contains("idle for"))
            .expect("idle finding");
        assert!(never.detail.contains("plugin:never"));
        assert!(!never.detail.contains("mcp:old"));
        assert!(idle.detail.contains("mcp:old"));
        assert!(idle.detail.contains("70d ago"));
        assert!(!idle.detail.contains("plugin:never"));
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
