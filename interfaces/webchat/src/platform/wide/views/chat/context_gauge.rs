//! Context-window occupancy gauge — a compact SVG ring showing how full the
//! model's context window is after the latest completed turn. Mirrors
//! hermes-desktop's `ContextGauge`, but the usage figures ride on data Aleph
//! already emits (`run_complete` summary → [`ChatState::context_usage`]), so no
//! backend protocol change is required.
//!
//! The gauge is purely presentational (R4): it reads the published snapshot —
//! both the occupancy and the per-model window are computed by core — and
//! self-hides until the first usage figure lands.

use super::state::ChatState;
use leptos::prelude::*;

/// Pick a token-tinted color for the ring by occupancy fraction.
fn gauge_color(frac: f64) -> &'static str {
    if frac >= 0.9 {
        "var(--color-danger)"
    } else if frac >= 0.7 {
        "var(--color-warning)"
    } else {
        "var(--color-primary)"
    }
}

/// SVG ring + percentage label. Renders nothing until `context_usage` is set.
#[component]
#[must_use]
pub fn ContextGauge() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.context_usage.get().is_some()>
            {move || {
                let usage = chat.context_usage.get()?;
                let window = usage.window_tokens.max(1);
                let frac = (f64::from(usage.used_tokens) / f64::from(window)).clamp(0.0, 1.0);
                let pct = (frac * 100.0).round() as u32;
                // r = 7 → circumference = 2·π·7 ≈ 43.98
                let circ = 43.98_f64;
                let dash = circ * frac;
                let gap = circ - dash;
                let color = gauge_color(frac);
                let (label, title) = if usage.is_estimate {
                    (
                        format!("≈{pct}%"),
                        format!(
                            "预估上下文占用 {pct}% · {} / {} tokens（首次对话后转为实测）",
                            usage.used_tokens, usage.window_tokens,
                        ),
                    )
                } else {
                    (
                        format!("{pct}%"),
                        format!(
                            "上下文占用 {pct}% · {} / {} tokens（本轮累计 {}）",
                            usage.used_tokens, usage.window_tokens, usage.total_tokens,
                        ),
                    )
                };
                Some(view! {
                    <div
                        class="flex items-center gap-1 text-text-tertiary select-none"
                        title=title
                    >
                        <svg width="18" height="18" viewBox="0 0 18 18" class="flex-shrink-0">
                            <circle
                                cx="9" cy="9" r="7" fill="none"
                                stroke="var(--color-surface-sunken)" stroke-width="2"
                            />
                            <circle
                                cx="9" cy="9" r="7" fill="none"
                                stroke=color stroke-width="2" stroke-linecap="round"
                                stroke-dasharray=format!("{dash:.2} {gap:.2}")
                                transform="rotate(-90 9 9)"
                            />
                        </svg>
                        <span class="text-[10px] tabular-nums">{label}</span>
                    </div>
                })
            }}
        </Show>
    }
}

/// Live last-call prompt-cache hit rate chip — `cache N%` beside the gauge.
/// Renders nothing until a provider call reports cache activity. Tints
/// warning below 50%: a sudden drop mid-session is the live symptom of a
/// stable-prefix bust (same thresholds as the TUI status bar, and the same
/// canonical `read / (input + read)` formula behind the number).
#[component]
#[must_use]
pub fn CacheStat() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.live_cache_pct.get().is_some()>
            {move || {
                let pct = chat.live_cache_pct.get()?;
                let color = if pct < 50 {
                    "var(--color-warning)"
                } else {
                    "var(--color-text-tertiary)"
                };
                Some(view! {
                    <div
                        class="flex items-center select-none"
                        title=format!(
                            "上一次调用的提示词缓存命中率 {pct}% · read / (input + read)（与 TUI 状态栏同口径）"
                        )
                    >
                        <span class="text-[10px] tabular-nums" style=format!("color: {color}")>
                            {format!("cache {pct}%")}
                        </span>
                    </div>
                })
            }}
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_color_tracks_thresholds() {
        assert_eq!(gauge_color(0.0), "var(--color-primary)");
        assert_eq!(gauge_color(0.69), "var(--color-primary)");
        assert_eq!(gauge_color(0.70), "var(--color-warning)");
        assert_eq!(gauge_color(0.89), "var(--color-warning)");
        assert_eq!(gauge_color(0.90), "var(--color-danger)");
        assert_eq!(gauge_color(1.0), "var(--color-danger)");
    }
}
