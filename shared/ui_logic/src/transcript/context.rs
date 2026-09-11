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

/// ⚠️ **The caller must fill `b.provider_reported` before calling this.** The
/// server's `context.breakdown` always sends it as `None` — it measures the
/// prompt, and only the provider's response carries the provider's count — and
/// with `None` this function returns `total: None` and `percent: None` by
/// design ("unknown", never our own sum dressed as the provider's). So a client
/// that pipes the RPC response straight in gets rows with no total and no
/// percent bar. Fill it from the live `ContextGauge` first.
///
/// ⚠️ The layer rows describe the prompt BEFORE the system-prompt budget trim.
/// `b.dynamic_bytes_sent` is the authoritative size of the dynamic half as
/// sent; this function does not adjust the rows by it, so a view that wants the
/// sent size must read that field.
#[must_use]
pub fn reconcile(b: &ContextBreakdown) -> ContextRows {
    let mut rows: Vec<ContextRow> = b
        .layers
        .iter()
        .map(|l| ContextRow {
            label: l.name.clone(),
            tokens: l.tokens,
            bytes: Some(l.bytes),
        })
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
        rows.push(ContextRow {
            label: "Messages".into(),
            tokens: m,
            bytes: None,
        });
    }
    let ours: u64 = rows.iter().map(|r| r.tokens).sum();
    let total = match b.provider_reported {
        Some(u) => {
            let reported = u.input + u.cache_read + u.cache_creation;
            let diff = (reported as f64 - ours as f64).abs();
            let disagrees = diff > (ours as f64 * PROVIDER_TOLERANCE).max(32.0);
            match (disagrees, reported >= ours) {
                (false, _) => Some(ours),
                (true, true) => Some(reported),
                // The provider counted LESS than our measured parts,
                // beyond tolerance: the two measurements contradict each
                // other. Trusting `reported` would show rows that outweigh
                // their own total (the lie this function exists to
                // prevent); trusting `ours` would silently overrule the
                // provider, the authoritative account of what was actually
                // billed. The honest answer is "we don't know" — the same
                // answer given when the provider reports nothing at all.
                // Parts BELOW the total (the normal case) still keep their
                // `Other` remainder; only parts ABOVE it is unresolvable.
                (true, false) => None,
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
        rows.push(ContextRow {
            label: "Other".into(),
            tokens: other,
            bytes: None,
        });
    }
    ContextRows {
        rows,
        total,
        window: b.context_window,
        percent,
        other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::context_breakdown::{LayerSizeView, ToolSchemaSize, UsageTokens};
    fn breakdown(provider: Option<UsageTokens>) -> ContextBreakdown {
        ContextBreakdown {
            session_key: "k".into(),
            turn: 1,
            layers: vec![LayerSizeView {
                name: "System".into(),
                bytes: 4000,
                tokens: 1000,
                zone: "stable".into(),
            }],
            tools: vec![ToolSchemaSize {
                name: "grep".into(),
                schema_bytes: 400,
                description_bytes: 0,
            }],
            messages_tokens: Some(500),
            provider_reported: provider,
            context_window: Some(200_000),
            dynamic_bytes_sent: None,
        }
    }
    #[test]
    fn provider_wins_when_it_disagrees_and_the_remainder_is_an_explicit_other_row() {
        let r = reconcile(&breakdown(Some(UsageTokens {
            input: 2000,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        })));
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
        let r = reconcile(&breakdown(Some(UsageTokens {
            input: 1600,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        })));
        assert_eq!(r.total, Some(1600));
        assert_eq!(r.other, 0);
    }
    #[test]
    fn a_provider_total_smaller_than_the_parts_beyond_tolerance_means_unknown() {
        // A downward disagreement past tolerance means our two
        // measurements CONTRADICT each other — trusting the reported
        // total would show rows that outweigh it (the exact lie this
        // function exists to prevent); trusting our sum would silently
        // overrule the provider, which is the authoritative account of
        // what was actually billed. The honest answer is "we don't know":
        // `total`/`percent` go to `None`, exactly as when the provider
        // reports nothing at all, and no `Other` row is emitted. Every
        // itemized row is kept exactly as measured — reconciliation only
        // withholds the total/percent judgment, never the parts.
        let b = breakdown(Some(UsageTokens {
            input: 100,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        }));
        let r = reconcile(&b);
        assert_eq!(r.total, None);
        assert_eq!(r.percent, None);
        assert_eq!(r.other, 0);
        assert!(r.rows.iter().all(|x| x.label != "Other"));
        assert_eq!(
            r.rows.len(),
            3,
            "System/Tools/Messages rows survive untouched"
        );
        assert_eq!(
            r.rows[0],
            ContextRow {
                label: "System".into(),
                tokens: 1000,
                bytes: Some(4000)
            }
        );
        assert_eq!(
            r.rows[1],
            ContextRow {
                label: "Tools (1 schemas)".into(),
                tokens: 100,
                bytes: Some(400)
            }
        );
        assert_eq!(
            r.rows[2],
            ContextRow {
                label: "Messages".into(),
                tokens: 500,
                bytes: None
            }
        );
    }
    #[test]
    fn a_downward_disagreement_exactly_at_the_tolerance_boundary_keeps_our_total() {
        // ours = 1600, so the threshold is max(1600*0.001, 32.0) = 32.0.
        // A diff of EXACTLY 32 must not count as disagreeing (`diff >
        // threshold` is strict) — this is still the ordinary within-
        // tolerance case, not the new unknown-total case.
        let r = reconcile(&breakdown(Some(UsageTokens {
            input: 1568,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        })));
        assert_eq!(r.total, Some(1600));
        assert_eq!(r.other, 0);
    }
    #[test]
    fn no_context_window_known_means_no_percent_even_with_a_known_total() {
        let mut b = breakdown(Some(UsageTokens {
            input: 1600,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        }));
        b.context_window = None;
        let r = reconcile(&b);
        assert_eq!(r.total, Some(1600));
        assert_eq!(r.percent, None);
    }
}
