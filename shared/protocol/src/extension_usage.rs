//! Wire shape for the per-extension usage summary carried on list rows.
//!
//! Lives here — not in `alephcore` and not duplicated in the Panel — because
//! both ends of the wire must agree on it and `aleph-panel` is forbidden to
//! depend on `alephcore`. One type means a rename is a compile error on both
//! sides; two hand-kept copies means a rename is a silently empty column.
//!
//! Attached to `mcp_config.list` server rows and `plugins.list` plugin rows.
//! Skills carry their own, older sidecar and report through
//! `skills.status`'s existing `usage` field instead.

use serde::{Deserialize, Serialize};

/// Compact "is anyone still calling this?" summary for one installed thing.
///
/// ## `calls: None` is a claim, not a missing value
///
/// `None` means this entry has **no tool-call channel that usage accounting can
/// observe** — a plugin that ships only hooks, for instance. It must render as
/// `—` and never as `0`: a renderer that collapses the two invites uninstalling
/// something that runs on every turn. `not_measurable_reason` is the sentence
/// to show next to that dash.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Calls that reached a tool. `None` = not measurable (see type docs).
    #[serde(default)]
    pub calls: Option<u64>,
    /// Why `calls` is `None`. Always `None` when `calls` is `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_measurable_reason: Option<String>,
    /// Subset of `calls` that returned an error.
    #[serde(default)]
    pub errors: u64,
    /// RFC3339 stamp of the most recent call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    /// Whole days since `last_used_at`. `None` when never used, not measurable,
    /// or the stamp could not be parsed — deliberately not a sentinel number,
    /// which would render as a measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_days: Option<i64>,
}

impl UsageSummary {
    /// `true` only when this entry is measurable and has genuinely never been
    /// called. The predicate every renderer should use for a "never used"
    /// badge, so none of them re-derives it from `calls == Some(0)` and gets
    /// the not-measurable case wrong.
    #[must_use]
    pub const fn never_used(&self) -> bool {
        matches!(self.calls, Some(0))
    }

    /// The count to print, or `None` for the `—` placeholder.
    #[must_use]
    pub const fn display_calls(&self) -> Option<u64> {
        self.calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_not_measurable_row_is_not_never_used() {
        let s = UsageSummary {
            calls: None,
            not_measurable_reason: Some("ships no tools".into()),
            ..Default::default()
        };
        assert!(!s.never_used(), "`—` must not be read as `never used`");
        assert_eq!(s.display_calls(), None);
    }

    #[test]
    fn a_zero_row_is_never_used() {
        let s = UsageSummary {
            calls: Some(0),
            ..Default::default()
        };
        assert!(s.never_used());
        assert_eq!(s.display_calls(), Some(0));
    }

    /// A row written by an older server (no usage fields at all) must still
    /// decode — the Panel ships independently of the daemon it talks to.
    #[test]
    fn an_empty_object_decodes_to_unknown() {
        let s: UsageSummary = serde_json::from_str("{}").unwrap();
        assert_eq!(s.calls, None);
        assert_eq!(s.errors, 0);
        assert!(!s.never_used());
    }
}
