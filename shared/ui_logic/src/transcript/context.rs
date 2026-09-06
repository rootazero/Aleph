//! Turn a `ContextBreakdown` into the rows a `/context` view paints, with
//! provider reconciliation and an explicit `Other` remainder — never an
//! inflated estimate (pi-cc-extensions `context.ts` H.4, but Aleph's layer
//! bytes are MEASURED, not chars/4).

use aleph_protocol::context_breakdown::ContextBreakdown;

/// Provider count vs. our total: beyond this fraction the provider wins.
pub const PROVIDER_TOLERANCE: f64 = 0.001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    pub label: String,
    pub tokens: u64,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextRows {
    pub rows: Vec<ContextRow>,
    /// Best-known occupancy: provider-reported when present, else our sum.
    pub total: Option<u64>,
    pub window: Option<u32>,
    pub percent: Option<f32>,
    /// `total - Σrows`, ≥ 0, shown as its own row.
    pub other: u64,
}

#[must_use]
pub fn reconcile(b: &ContextBreakdown) -> ContextRows {
    let mut rows: Vec<ContextRow> = b
        .layers
        .iter()
        .map(|l| ContextRow { label: l.name.clone(), tokens: l.tokens, bytes: Some(l.bytes) })
        .collect();
    let tool_bytes = b.tool_bytes();
    if !b.tools.is_empty() {
        rows.push(ContextRow {
            label: format!("Tools ({} schemas)", b.tools.len()),
            tokens: tool_bytes / 4,
            bytes: Some(tool_bytes),
        });
    }
    if let Some(m) = b.messages_tokens {
        rows.push(ContextRow { label: "Messages".into(), tokens: m, bytes: None });
    }
    let ours: u64 = rows.iter().map(|r| r.tokens).sum();
    let total = match b.provider_reported {
        Some(u) => {
            let reported = u.input + u.cache_read + u.cache_creation;
            let diff = (reported as f64 - ours as f64).abs();
            if diff > (ours as f64 * PROVIDER_TOLERANCE).max(32.0) {
                Some(reported)
            } else {
                Some(ours)
            }
        }
        None => None, // unknown after compaction: render `?`, not our sum
    };
    let other = total.map_or(0, |t| t.saturating_sub(ours));
    let percent = match (total, b.context_window) {
        (Some(t), Some(w)) if w > 0 => Some((t as f32 / w as f32) * 100.0),
        _ => None,
    };
    if other > 0 {
        rows.push(ContextRow { label: "Other".into(), tokens: other, bytes: None });
    }
    ContextRows { rows, total, window: b.context_window, percent, other }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::context_breakdown::{LayerSizeView, ToolSchemaSize, UsageTokens};
    fn breakdown(provider: Option<UsageTokens>) -> ContextBreakdown {
        ContextBreakdown {
            session_key: "k".into(),
            turn: 1,
            layers: vec![LayerSizeView { name: "System".into(), bytes: 4000, tokens: 1000, zone: "stable".into() }],
            tools: vec![ToolSchemaSize { name: "grep".into(), schema_bytes: 400, description_bytes: 0 }],
            messages_tokens: Some(500),
            provider_reported: provider,
            context_window: Some(200_000),
        }
    }
    #[test]
    fn provider_wins_when_it_disagrees_and_the_remainder_is_an_explicit_other_row() {
        let r = reconcile(&breakdown(Some(UsageTokens { input: 2000, output: 0, cache_read: 0, cache_creation: 0 })));
        assert_eq!(r.total, Some(2000));
        assert_eq!(r.other, 400); // 2000 - (1000 + 100 + 500)
        assert_eq!(r.rows.last().unwrap().label, "Other");
        assert!((r.percent.unwrap() - 1.0).abs() < 0.01);
    }
    #[test]
    fn no_provider_usage_means_unknown_not_our_sum() {
        let r = reconcile(&breakdown(None));
        assert_eq!(r.total, None);
        assert_eq!(r.percent, None);
        assert_eq!(r.other, 0);
        assert!(r.rows.iter().all(|x| x.label != "Other"));
    }
    #[test]
    fn a_provider_count_within_tolerance_keeps_our_rows_unchanged() {
        let r = reconcile(&breakdown(Some(UsageTokens { input: 1600, output: 0, cache_read: 0, cache_creation: 0 })));
        assert_eq!(r.total, Some(1600));
        assert_eq!(r.other, 0);
    }
    #[test]
    fn a_provider_total_smaller_than_the_parts_saturates_other_to_zero_never_negative() {
        // Fail-closed in the other direction: when the provider reports
        // LESS than our measured sum, `Other` must not go negative — it
        // saturates to 0 rather than lying with a negative remainder. The
        // displayed rows will sum above `total` in this case; that is the
        // provider disagreeing downward, not a bug in the reconciliation.
        let r = reconcile(&breakdown(Some(UsageTokens { input: 100, output: 0, cache_read: 0, cache_creation: 0 })));
        assert_eq!(r.total, Some(100));
        assert_eq!(r.other, 0);
        assert!(r.rows.iter().all(|x| x.label != "Other"));
    }
    #[test]
    fn no_context_window_known_means_no_percent_even_with_a_known_total() {
        let mut b = breakdown(Some(UsageTokens { input: 1600, output: 0, cache_read: 0, cache_creation: 0 }));
        b.context_window = None;
        let r = reconcile(&b);
        assert_eq!(r.total, Some(1600));
        assert_eq!(r.percent, None);
    }
}
