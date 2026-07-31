//! Unified diagnostic finding model shared by every health check.
//!
//! A single `Finding` carries enough structure for three consumers:
//! the human CLI renderer, the `--json` lint surface (CI), and the LLM
//! `doctor` tool. The model is deliberately flat and string-typed at the
//! edges so it serializes cleanly and survives schema drift.

use serde::Serialize;

/// Severity ordering matters: `Error > Warning > Info`. The derived `Ord`
/// backs the threshold comparison in [`Finding::is_problem`] (`severity > Info`)
/// and lets callers sort or compare findings by severity without a hand-written
/// comparator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Result of a repair attempt, attached to a `Finding` after the engine
/// runs in `Fix` posture. Absent in `Inspect`/`Lint`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum RepairOutcome {
    /// The mechanical repair succeeded.
    Repaired { detail: String },
    /// The repair was attempted but failed.
    Failed { error: String },
    /// The finding is repairable but the repair was intentionally not run
    /// (e.g. unsafe to apply automatically — a human decision is required).
    Skipped { reason: String },
}

/// A single diagnostic observation produced by a [`HealthCheck`](super::HealthCheck).
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// Stable, namespaced id of the originating check (e.g. `core/data-dir`).
    pub check_id: &'static str,
    pub severity: Severity,
    /// One-line headline.
    pub title: String,
    /// Human detail of what was observed.
    pub detail: String,
    /// Suggested manual remediation, shown when no automatic repair applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_hint: Option<String>,
    /// True when the engine can mechanically repair this in `Fix` posture.
    pub repairable: bool,
    /// Populated only after a `Fix`-posture run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair_outcome: Option<RepairOutcome>,
    /// Machine-readable labels for cross-finding aggregation (e.g. the
    /// total-outage gate counts `provider-reachable` tags instead of string
    /// matching on titles). Empty for most findings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl Finding {
    /// Build an `ok` (info) finding — used so a check that found nothing
    /// wrong can still report that it ran and what it inspected.
    pub fn ok(check_id: &'static str, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check_id,
            severity: Severity::Info,
            title: title.into(),
            detail: detail.into(),
            fix_hint: None,
            repairable: false,
            repair_outcome: None,
            tags: Vec::new(),
        }
    }

    /// Build a problem finding at the given severity.
    pub fn problem(
        check_id: &'static str,
        severity: Severity,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            check_id,
            severity,
            title: title.into(),
            detail: detail.into(),
            fix_hint: None,
            repairable: false,
            repair_outcome: None,
            tags: Vec::new(),
        }
    }

    /// Attach a manual-remediation hint (builder style).
    pub fn with_fix_hint(mut self, hint: impl Into<String>) -> Self {
        self.fix_hint = Some(hint.into());
        self
    }

    /// Mark this finding as mechanically repairable by the engine.
    #[must_use]
    pub const fn repairable(mut self) -> Self {
        self.repairable = true;
        self
    }

    /// Record the outcome of a repair attempt (builder style).
    #[must_use]
    pub fn with_repair(mut self, outcome: RepairOutcome) -> Self {
        self.repair_outcome = Some(outcome);
        self
    }

    /// Attach a machine-readable tag (builder style).
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Whether this finding carries the given tag.
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    #[must_use]
    pub fn is_problem(&self) -> bool {
        self.severity > Severity::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_orders_error_above_warning_above_info() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn ok_finding_is_not_a_problem() {
        let f = Finding::ok("core/x", "fine", "all good");
        assert!(!f.is_problem());
        assert!(!f.repairable);
    }

    #[test]
    fn builders_compose() {
        let f = Finding::problem("core/x", Severity::Error, "boom", "it broke")
            .with_fix_hint("turn it off and on")
            .repairable();
        assert!(f.is_problem());
        assert!(f.repairable);
        assert_eq!(f.fix_hint.as_deref(), Some("turn it off and on"));
    }

    #[test]
    fn tags_default_empty_and_compose() {
        let f = Finding::ok("core/x", "fine", "all good");
        assert!(f.tags.is_empty());
        assert!(!f.has_tag("provider-reachable"));

        let f = f.with_tag("provider-reachable").with_tag("second");
        assert!(f.has_tag("provider-reachable"));
        assert!(f.has_tag("second"));
        assert!(!f.has_tag("missing"));

        // Empty tags stay out of the serialized payload entirely.
        let bare = serde_json::to_value(Finding::ok("core/x", "fine", "all good")).unwrap();
        assert!(bare.get("tags").is_none());
        let tagged = serde_json::to_value(&f).unwrap();
        assert_eq!(
            tagged["tags"],
            serde_json::json!(["provider-reachable", "second"])
        );
    }
}
