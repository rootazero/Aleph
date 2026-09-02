//! Pure visual helpers for the background sub-agent tree — glyphs, heat colors,
//! sparklines, duration formatting. No Leptos; host-testable.

use aleph_protocol::subagent_tree::NodeLifecycle;

/// Status glyph for a node lifecycle (hermes-parity glyph set).
#[must_use]
pub const fn lifecycle_glyph(lc: NodeLifecycle) -> &'static str {
    match lc {
        NodeLifecycle::Running => "●",
        NodeLifecycle::Completed => "✓",
        NodeLifecycle::Failed => "✗",
        NodeLifecycle::TimedOut => "⌛",
        NodeLifecycle::Cancelled => "⊘",
    }
}

/// Tailwind text-color class for a lifecycle glyph.
#[must_use]
pub const fn lifecycle_color(lc: NodeLifecycle) -> &'static str {
    match lc {
        NodeLifecycle::Running => "text-blue-400",
        NodeLifecycle::Completed => "text-green-400",
        NodeLifecycle::Failed => "text-red-400",
        NodeLifecycle::TimedOut => "text-amber-400",
        NodeLifecycle::Cancelled => "text-gray-400",
    }
}

/// Map hotness (tools/sec) to an inline `rgb()` heat color: cold blue → hot red.
/// Saturates at ~6 tools/s. Drives the per-node activity bar so the eye finds
/// the hot branch (hermes heatmap parity).
#[must_use]
pub fn heat_color(hotness: f32) -> String {
    let t = (hotness / 6.0).clamp(0.0, 1.0);
    let r = (40.0 + t * 200.0) as u8;
    let g = (90.0 + (1.0 - (t - 0.5).abs() * 2.0).max(0.0) * 90.0) as u8;
    let b = (200.0 - t * 170.0) as u8;
    format!("rgb({r},{g},{b})")
}

/// Unicode sparkline of a depth distribution. `counts[i]` = nodes at depth `i`.
/// Empty / all-zero input → empty string.
#[must_use]
pub fn sparkline(counts: &[u32]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = counts.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return String::new();
    }
    counts
        .iter()
        .map(|&c| {
            if c == 0 {
                ' '
            } else {
                let idx =
                    ((f64::from(c) / f64::from(max)) * (BARS.len() - 1) as f64).round() as usize;
                BARS[idx.min(BARS.len() - 1)]
            }
        })
        .collect()
}

/// Token-count ladder: `950` / `2.3k` / `232k` / `1.2M`.
#[must_use]
pub fn fmt_tokens(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1000.0),
        10_000..=999_999 => format!("{}k", n / 1000),
        1_000_000..=9_999_999 => format!("{:.1}M", n as f64 / 1_000_000.0),
        _ => format!("{}M", n / 1_000_000),
    }
}

/// Human duration from milliseconds: `820ms` / `12s` / `1m 14s`.
#[must_use]
pub fn fmt_duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1000;
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m {}s", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_distinct_per_state() {
        assert_eq!(lifecycle_glyph(NodeLifecycle::Running), "●");
        assert_eq!(lifecycle_glyph(NodeLifecycle::Failed), "✗");
        assert_ne!(
            lifecycle_glyph(NodeLifecycle::Completed),
            lifecycle_glyph(NodeLifecycle::Cancelled)
        );
    }

    #[test]
    fn sparkline_scales_and_skips_zero() {
        assert_eq!(sparkline(&[]), "");
        assert_eq!(sparkline(&[0, 0]), "");
        let s = sparkline(&[1, 4]);
        assert_eq!(s.chars().count(), 2);
        // taller bar for the larger count
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars[1], '█');
    }

    #[test]
    fn fmt_tokens_ladder() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(2_340), "2.3k");
        assert_eq!(fmt_tokens(232_500), "232k");
        assert_eq!(fmt_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn fmt_duration_bands() {
        assert_eq!(fmt_duration(820), "820ms");
        assert_eq!(fmt_duration(12_000), "12s");
        assert_eq!(fmt_duration(74_000), "1m 14s");
    }

    #[test]
    fn heat_color_is_rgb_and_clamps() {
        assert!(heat_color(0.0).starts_with("rgb("));
        // saturates without panicking on huge input
        let _ = heat_color(9999.0);
    }
}
