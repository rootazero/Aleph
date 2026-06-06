//! Context-window occupancy gauge — a compact SVG ring showing how full the
//! model's context window is after the latest completed turn. Mirrors
//! hermes-desktop's `ContextGauge`, but the usage figures ride on data Aleph
//! already emits (`run_complete` summary → [`ChatState::context_usage`]), so no
//! backend protocol change is required.
//!
//! The gauge is purely presentational: it reads the published snapshot and
//! self-hides until the first usage figure lands. The only logic it owns is the
//! window-size heuristic ([`context_window_for`]) — kept here (next to its
//! tests) because the panel is an I/O-only interface (R4) and cannot reach
//! core's model catalogue.

use super::state::ChatState;
use leptos::prelude::*;

/// Best-effort context-window size (tokens) for a model id.
///
/// Keyed on well-known family substrings. A display gauge tolerates
/// approximation — correctness never depends on this value, so an unknown model
/// falls back to a conservative 128k rather than failing. Order matters:
/// most-specific families first.
pub fn context_window_for(model: &str) -> u32 {
    let m = model.to_ascii_lowercase();
    if m.contains("gpt-3.5") {
        16_385
    } else if m.contains("gpt-4o")
        || m.contains("gpt-4.1")
        || m.contains("gpt-4-turbo")
        || m.contains("o1")
        || m.contains("o3")
        || m.contains("o4")
    {
        128_000
    } else if m.contains("claude") {
        // Claude 3.x / 4.x default surface (1M is a beta opt-in, not assumed).
        200_000
    } else if m.contains("kimi") || m.contains("moonshot") {
        200_000
    } else if m.contains("gemini") {
        1_000_000
    } else if m.contains("qwen") {
        131_072
    } else if m.contains("glm") {
        128_000
    } else if m.contains("deepseek") {
        65_536
    } else {
        128_000
    }
}

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
                let title = format!(
                    "上下文占用 {pct}% · {} / {} tokens（本轮累计 {}）",
                    usage.used_tokens, usage.window_tokens, usage.total_tokens,
                );
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
                        <span class="text-[10px] tabular-nums">{format!("{pct}%")}</span>
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
    fn known_families_resolve_expected_windows() {
        assert_eq!(context_window_for("claude-opus-4-8"), 200_000);
        assert_eq!(context_window_for("kimi-k2"), 200_000);
        assert_eq!(context_window_for("gpt-4o-mini"), 128_000);
        assert_eq!(context_window_for("gpt-3.5-turbo"), 16_385);
        assert_eq!(context_window_for("gemini-2.5-pro"), 1_000_000);
        assert_eq!(context_window_for("qwen2.5-72b"), 131_072);
    }

    #[test]
    fn unknown_model_falls_back_conservatively() {
        assert_eq!(context_window_for("some-future-model"), 128_000);
        assert_eq!(context_window_for(""), 128_000);
    }

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
