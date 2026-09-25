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
    /// An uncalibrated estimate (the message rows), not a measurement; a
    /// view marks it so. Estimated rows never make the total unknown — they
    /// share what the provider's count leaves after the measured rows.
    pub estimated: bool,
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
            estimated: false,
        })
        .collect();
    let tool_bytes = b.tool_bytes();
    if !b.tools.is_empty() {
        rows.push(ContextRow {
            label: format!(
                "Tools ({} schema{})",
                b.tools.len(),
                if b.tools.len() == 1 { "" } else { "s" }
            ),
            tokens: tool_bytes / 4,
            bytes: Some(tool_bytes),
            estimated: false,
        });
    }
    let measured: u64 = rows.iter().map(|r| r.tokens).sum();
    // The conversation half, one row per kind that has tokens: what the
    // tool results, the replayed reasoning and the rest of the history each
    // cost in the last prompt sent — the server's uncalibrated estimate.
    let mut estimates: Vec<ContextRow> = b
        .messages
        .map(|m| {
            [
                ("Messages: tool results", m.tool_results),
                ("Messages: reasoning", m.reasoning),
                ("Messages: other", m.other),
            ]
            .into_iter()
            .filter(|(_, tokens)| *tokens > 0)
            .map(|(label, tokens)| ContextRow {
                label: label.into(),
                tokens,
                bytes: None,
                estimated: true,
            })
            .collect()
        })
        .unwrap_or_default();
    let estimated: u64 = estimates.iter().map(|r| r.tokens).sum();
    let ours = measured + estimated;
    let within =
        |a: u64, b: u64| (a as f64 - b as f64).abs() <= (b as f64 * PROVIDER_TOLERANCE).max(32.0);
    let total = match b.provider_reported {
        Some(u) => {
            let reported = u.input + u.cache_read + u.cache_creation;
            if within(reported, ours) {
                Some(ours)
            } else if reported >= measured {
                // The provider wins. The estimate rows share what it leaves
                // after the measured rows, scaled down when they overshoot
                // it — an estimate cannot outvote a count.
                fit_estimates(&mut estimates, reported - measured);
                Some(reported)
            } else if within(reported, measured) {
                fit_estimates(&mut estimates, 0);
                Some(measured)
            } else {
                // The provider counted LESS than our MEASURED parts, beyond
                // tolerance: the two measurements contradict each other.
                // Trusting `reported` would show rows that outweigh their own
                // total (the lie this function exists to prevent); trusting
                // `ours` would silently overrule the provider, the
                // authoritative account of what was actually billed. The
                // honest answer is "we don't know" — the same answer given
                // when the provider reports nothing at all. Only measured rows
                // can contradict: an overshooting estimate is scaled above.
                None
            }
        }
        None => None, // unknown after compaction: render `?`, not our sum
    };
    rows.extend(estimates);
    let ours: u64 = rows.iter().map(|r| r.tokens).sum();
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
            estimated: false,
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

/// Scale `rows` down proportionally so they sum to at most `budget`, dropping
/// any that reach zero. Rows already within budget are left as they are.
fn fit_estimates(rows: &mut Vec<ContextRow>, budget: u64) {
    let sum: u64 = rows.iter().map(|r| r.tokens).sum();
    if sum <= budget {
        return;
    }
    let mut given = 0u64;
    for r in rows.iter_mut() {
        r.tokens = (u128::from(r.tokens) * u128::from(budget) / u128::from(sum)) as u64;
        given += r.tokens;
    }
    // Floor division leaves < rows.len() tokens over; the largest row takes them.
    if let Some(largest) = rows.iter_mut().max_by_key(|r| r.tokens) {
        largest.tokens += budget - given;
    }
    rows.retain(|r| r.tokens > 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::context_breakdown::{
        LayerSizeView, MessageTokens, ToolSchemaSize, UsageTokens,
    };
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
            messages: Some(MessageTokens {
                tool_results: 300,
                reasoning: 100,
                other: 100,
            }),
            tool_output: None,
            provider_reported: provider,
            context_window: Some(200_000),
            dynamic_bytes_sent: None,
        }
    }
    /// A kind with no tokens gets no row: the overlay is a table of what
    /// occupies the window, and a zero row is a line with nothing in it.
    #[test]
    fn an_empty_message_kind_gets_no_row() {
        let mut b = breakdown(None);
        b.messages = Some(MessageTokens {
            tool_results: 40,
            reasoning: 0,
            other: 10,
        });
        let labels: Vec<String> = reconcile(&b).rows.into_iter().map(|r| r.label).collect();
        assert!(
            labels.contains(&"Messages: tool results".to_string()),
            "{labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l == "Messages: reasoning"),
            "{labels:?}"
        );
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
    fn usage(input: u64) -> Option<UsageTokens> {
        Some(UsageTokens {
            input,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        })
    }

    /// F1: the message rows are an uncalibrated estimate, and when they
    /// overshoot the provider's count (measured 1100 < gauge 1300 < measured +
    /// estimate 1600) they are scaled into what the count leaves — the total
    /// and the percent bar stay, and the rows add up to the total. Before,
    /// this common case read as a contradiction and showed no total at all.
    ///
    /// Mutation-checked: running the contradiction check over the estimate
    /// rows too (`reported >= ours`), or skipping the scaling, turns this red.
    #[test]
    fn an_overshooting_estimate_is_scaled_into_the_providers_count() {
        let r = reconcile(&breakdown(usage(1300)));
        assert_eq!(r.total, Some(1300));
        assert!(r.percent.is_some());
        assert_eq!(r.rows.iter().map(|x| x.tokens).sum::<u64>(), 1300);
        assert_eq!(r.other, 0);
        let estimates: Vec<(&str, u64)> = r
            .rows
            .iter()
            .filter(|x| x.estimated)
            .map(|x| (x.label.as_str(), x.tokens))
            .collect();
        assert_eq!(
            estimates,
            vec![
                ("Messages: tool results", 120),
                ("Messages: reasoning", 40),
                ("Messages: other", 40),
            ],
            "300:100:100 scaled into the 200 the count leaves"
        );
    }

    /// Only the MEASURED rows can contradict the provider: a count below them
    /// beyond tolerance is still "unknown".
    #[test]
    fn a_count_below_the_measured_rows_is_still_unknown() {
        let r = reconcile(&breakdown(usage(1000)));
        assert_eq!(r.total, None);
        assert_eq!(r.percent, None);
    }

    /// A count within tolerance of the measured rows alone leaves the estimate
    /// nothing: its rows go, and the measured rows are the total.
    #[test]
    fn a_count_that_only_covers_the_measured_rows_drops_the_estimate() {
        let r = reconcile(&breakdown(usage(1080)));
        assert_eq!(r.total, Some(1100));
        assert!(r.rows.iter().all(|x| !x.estimated), "{:?}", r.rows);
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
            5,
            "System/Tools and the three message rows survive untouched"
        );
        assert_eq!(
            r.rows[0],
            ContextRow {
                label: "System".into(),
                tokens: 1000,
                bytes: Some(4000),
                estimated: false,
            }
        );
        assert_eq!(
            r.rows[1],
            ContextRow {
                label: "Tools (1 schema)".into(),
                tokens: 100,
                bytes: Some(400),
                estimated: false,
            }
        );
        assert_eq!(
            r.rows[2..]
                .iter()
                .map(|row| (row.label.as_str(), row.tokens, row.bytes))
                .collect::<Vec<_>>(),
            vec![
                ("Messages: tool results", 300, None),
                ("Messages: reasoning", 100, None),
                ("Messages: other", 100, None),
            ]
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
