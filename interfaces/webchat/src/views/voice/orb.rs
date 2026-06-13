//! The orb. Rendering kernel = layered divs + CSS (Task 1 classes); swap to
//! Canvas/shader later without touching callers — props are the contract.

use leptos::prelude::*;

use super::machine::VoicePhase;

/// Immersive voice orb. A purely presentational rendering kernel: it maps the
/// session [`VoicePhase`] (+ a transient error flash) to a Task 1 state class
/// and feeds the mic/playback level into the `--voice-level` custom property.
/// Callers depend only on the props, so the internals can later become a
/// `<canvas>`/shader without touching them.
#[component]
#[must_use]
pub(crate) fn VoiceOrb(
    /// Current session phase — selects flow speed / error tint.
    #[prop(into)]
    phase: Signal<VoicePhase>,
    /// Mic/playback level 0..1 — drives scale and glow via `--voice-level`.
    #[prop(into)]
    level: Signal<f64>,
    /// True briefly after an error to flash the danger tint.
    #[prop(into, default = Signal::derive(|| false))]
    error_flash: Signal<bool>,
) -> impl IntoView {
    let class = move || {
        let state = if error_flash.get() {
            "voice-orb--error"
        } else {
            match phase.get() {
                VoicePhase::Listening => "voice-orb--listening",
                VoicePhase::Processing => "voice-orb--processing",
                VoicePhase::Speaking => "voice-orb--speaking",
            }
        };
        format!("voice-orb {state}")
    };
    let style = move || format!("--voice-level: {:.3}", level.get().clamp(0.0, 1.0));
    view! {
        <div class=class style=style>
            <div class="voice-orb-flow"></div>
            <div class="voice-orb-sheen"></div>
        </div>
    }
}
