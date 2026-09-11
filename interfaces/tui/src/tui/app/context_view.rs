//! The `/context` overlay's view model: one `context.breakdown` response,
//! joined with the live gauge and reconciled into rows.
//!
//! # Why the join happens here and not on the server
//!
//! `context.breakdown` measures the PROMPT — every figure recorded at the
//! moment the bytes were produced. It cannot report the provider's own token
//! count for the same turn, because only the provider's response carries that,
//! so it always sends `provider_reported: None`. `reconcile` treats that
//! `None` as "unknown" and returns `total: None` / `percent: None` rather than
//! dressing our own sum as the provider's — which means a client that pipes
//! the response straight through gets rows with no total and no bar.
//!
//! The gateway's handler, the wire type and `reconcile` itself all say so in
//! their doc comments; this module is the first client to exist, and the join
//! they describe is [`ContextView::new`].

use aleph_protocol::context_breakdown::{ContextBreakdown, UsageTokens};
use shared_ui_logic::transcript::{reconcile, ContextRows};

/// One `/context` snapshot: what was measured, plus where it came from.
///
/// A snapshot, not a subscription — the numbers are the last prompt's, and
/// they do not change while the overlay is open. Re-running `/context` after a
/// turn is how you get the next one.
#[derive(Debug, Clone)]
pub struct ContextView {
    /// Monotonic turn counter of the measured prompt. Shown because a
    /// breakdown from turn 3 read during turn 9 describes a prompt nine turns
    /// of history ago, and nothing else on screen would say so.
    pub turn: u64,
    /// Reconciled rows, total, window and the `Other` remainder.
    pub rows: ContextRows,
    /// Σ bytes of the layers the server marked `stable` — the prefix-cache
    /// floor, which the system-prompt budget trim never touches.
    pub stable_bytes: u64,
    /// Σ bytes of the `dynamic` layers **as assembled**, before that trim.
    pub dynamic_assembled_bytes: u64,
    /// Size of the dynamic half **as actually sent**. `None` means the turn
    /// built no system prompt at all — never "nothing was trimmed".
    pub dynamic_sent_bytes: Option<u64>,
    /// First visible row, for a layer list taller than the overlay.
    pub scroll: usize,
}

impl ContextView {
    /// Reconcile `breakdown` against the live context gauge.
    ///
    /// `gauge` is `(used, window)` from the latest `ContextGauge` event —
    /// `None` until this session's first LLM call reports one, which is also
    /// the state that makes the overlay render `?` instead of a total.
    ///
    /// # Why the whole occupancy goes into `input`
    ///
    /// `UsageTokens` has four fields because that is the shape a provider
    /// reports; the gauge is one number, the same one the status bar paints as
    /// `ctx N%`. `reconcile` sums `input + cache_read + cache_creation`, so
    /// putting the occupancy in `input` is what makes that sum equal the gauge.
    /// Splitting it across the cache fields would invent a cache breakdown this
    /// client does not have.
    ///
    /// # Why the window is overwritten too
    ///
    /// The server resolves its own window from the model catalogue plus the
    /// operator's override, and the gauge's denominator is the one the run
    /// actually used. They are meant to be the same number — and if they ever
    /// are not, the screen must not show `/context` disagreeing with the status
    /// bar one row below it about how big this window is (判据 §12: one
    /// derivation). The server's answer stays as the fallback for a session
    /// that has not run yet.
    #[must_use]
    pub fn new(breakdown: &ContextBreakdown, gauge: Option<(u32, u32)>) -> Self {
        let mut joined = breakdown.clone();
        if let Some((used, window)) = gauge {
            joined.provider_reported = Some(UsageTokens {
                input: u64::from(used),
                ..UsageTokens::default()
            });
            joined.context_window = Some(window);
        }
        let (stable_bytes, dynamic_assembled_bytes) =
            joined
                .layers
                .iter()
                .fold((0u64, 0u64), |(stable, dynamic), layer| {
                    match layer.zone.as_str() {
                        // The server spells the two zones; an unrecognised third one
                        // counts as neither rather than silently joining a half it was
                        // not measured for.
                        "stable" => (stable.saturating_add(layer.bytes), dynamic),
                        "dynamic" => (stable, dynamic.saturating_add(layer.bytes)),
                        _ => (stable, dynamic),
                    }
                });
        Self {
            turn: joined.turn,
            rows: reconcile(&joined),
            stable_bytes,
            dynamic_assembled_bytes,
            dynamic_sent_bytes: joined.dynamic_bytes_sent,
            scroll: 0,
        }
    }

    /// Bytes the system prompt actually occupied: the protected stable prefix
    /// plus the dynamic half **as sent**, falling back to the assembled size
    /// when the record carries no sent measurement.
    #[must_use]
    pub fn prompt_bytes_sent(&self) -> u64 {
        self.stable_bytes.saturating_add(
            self.dynamic_sent_bytes
                .unwrap_or(self.dynamic_assembled_bytes),
        )
    }

    /// How many bytes the system-prompt budget cut from the dynamic half, or
    /// `None` when nothing was cut, nothing was measured, or the sent size is
    /// *larger* than the rows (a post-pipeline block no layer owns).
    ///
    /// Deliberately not a signed delta: "trimmed" and "welded" are different
    /// facts and the caller says which one it is painting.
    #[must_use]
    pub fn trimmed_bytes(&self) -> Option<u64> {
        let sent = self.dynamic_sent_bytes?;
        self.dynamic_assembled_bytes
            .checked_sub(sent)
            .filter(|d| *d > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::context_breakdown::{LayerSizeView, ToolSchemaSize};

    fn layer(name: &str, bytes: u64, tokens: u64, zone: &str) -> LayerSizeView {
        LayerSizeView {
            name: name.into(),
            bytes,
            tokens,
            zone: zone.into(),
        }
    }

    fn breakdown() -> ContextBreakdown {
        ContextBreakdown {
            session_key: "agent:main".into(),
            turn: 7,
            layers: vec![
                layer("identity", 4_000, 1_000, "stable"),
                layer("memory", 8_000, 2_000, "dynamic"),
            ],
            tools: vec![ToolSchemaSize {
                name: "grep".into(),
                schema_bytes: 400,
                description_bytes: 400,
            }],
            messages_tokens: None,
            provider_reported: None,
            context_window: Some(200_000),
            dynamic_bytes_sent: Some(6_000),
        }
    }

    /// The one thing the server cannot answer. With no gauge the total stays
    /// unknown — the state the overlay paints as `?`.
    ///
    /// Reddens if the join ever substitutes our own sum for the provider's
    /// count, which is the failure `reconcile` exists to prevent and the one a
    /// `total == 0` assertion would sail straight past.
    #[test]
    fn without_a_gauge_the_total_is_unknown_rather_than_our_own_sum() {
        let v = ContextView::new(&breakdown(), None);
        assert_eq!(v.rows.total, None);
        assert_eq!(v.rows.percent, None);
        assert!(
            v.rows.rows.iter().any(|r| r.tokens > 0),
            "the measured rows are still known; only the total is not"
        );
        assert_eq!(v.rows.other, 0, "no total means no remainder to attribute");
    }

    /// The gauge is what turns the rows into a percentage, and the percentage
    /// must be the gauge's own — the number the status bar is painting one row
    /// below the overlay.
    #[test]
    fn the_gauge_supplies_the_total_and_the_denominator() {
        let v = ContextView::new(&breakdown(), Some((50_000, 200_000)));
        assert_eq!(v.rows.total, Some(50_000));
        assert_eq!(v.rows.window, Some(200_000));
        let pct = v.rows.percent.expect("a gauge means a percent");
        assert!((pct - 25.0).abs() < 0.01, "expected 25%, got {pct}");
    }

    /// A gauge window that disagrees with the server's wins, because the gauge
    /// is what the status bar divides by. Reddens if the overwrite is dropped
    /// and the overlay starts quoting a second window.
    #[test]
    fn the_live_window_outranks_the_catalogues() {
        let mut b = breakdown();
        b.context_window = Some(200_000);
        let v = ContextView::new(&b, Some((50_000, 1_000_000)));
        assert_eq!(v.rows.window, Some(1_000_000));
        let pct = v.rows.percent.expect("percent");
        assert!((pct - 5.0).abs() < 0.01, "expected 5%, got {pct}");
    }

    /// A session that has never run has no gauge, and the server's window is
    /// then the only one there is.
    #[test]
    fn with_no_gauge_the_servers_window_still_reaches_the_view() {
        let v = ContextView::new(&breakdown(), None);
        assert_eq!(v.rows.window, Some(200_000));
    }

    /// The zones are split by what the server labelled them, and the trim is
    /// the difference between the dynamic rows and what was sent.
    #[test]
    fn the_trim_is_the_gap_between_what_was_assembled_and_what_was_sent() {
        let v = ContextView::new(&breakdown(), None);
        assert_eq!(v.stable_bytes, 4_000);
        assert_eq!(v.dynamic_assembled_bytes, 8_000);
        assert_eq!(v.trimmed_bytes(), Some(2_000));
        assert_eq!(v.prompt_bytes_sent(), 10_000, "stable floor + dynamic sent");
    }

    /// `dynamic_bytes_sent: None` means "no prompt was measured", not "nothing
    /// was trimmed" — so it must not produce a trim figure, and the sent size
    /// falls back to the assembled one.
    #[test]
    fn an_unmeasured_prompt_reports_no_trim_rather_than_a_zero_one() {
        let mut b = breakdown();
        b.dynamic_bytes_sent = None;
        let v = ContextView::new(&b, None);
        assert_eq!(v.trimmed_bytes(), None);
        assert_eq!(v.prompt_bytes_sent(), 12_000);
    }

    /// A dynamic half LARGER than its rows is a post-pipeline block no layer
    /// owns — the other direction, and not a negative trim.
    #[test]
    fn a_welded_block_is_not_reported_as_a_trim() {
        let mut b = breakdown();
        b.dynamic_bytes_sent = Some(9_000);
        let v = ContextView::new(&b, None);
        assert_eq!(v.trimmed_bytes(), None);
        assert_eq!(v.prompt_bytes_sent(), 13_000);
    }
}
