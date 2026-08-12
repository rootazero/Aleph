// Consciously-accepted clippy lints:
//   * module_inception   — emitted by the build-script-generated i18n module
//                         (`mod i18n { mod i18n }`); not hand-editable.
//   * type_complexity    — a Leptos signal/closure signature clearer inline.
//   * large_enum_variant — boxing the large variant would churn the view API
//                         for no real gain in a WASM single-thread context.
//   * new_ret_no_self    — emitted by the build-script-generated i18n module;
//                         not hand-editable.
#![allow(
    clippy::module_inception,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::new_ret_no_self,
    clippy::redundant_locals
)]

pub mod api;
pub mod app;
pub mod appearance;
pub mod canvas_engine;
pub mod components;
pub mod context;
// Source-level guard only — no production consumer, so it does not ship.
// Sibling of `platform::context_ownership`.
#[cfg(test)]
mod disposed_reads;
pub mod generation;
pub mod i18n;
pub mod models;
pub mod panic_overlay;
pub mod platform;
pub mod preset_providers;
pub mod state;

// Per-form-factor UI lives under `platform::{wide,phone,tablet}`. The wide
// (desktop/browser) screens physically moved to `platform::wide::views`; this
// re-export keeps every existing `crate::views::…` path resolving unchanged.
pub use platform::wide::views;

use wasm_bindgen::prelude::*;

/// Initialize the Leptos application
/// This function is automatically called when the WASM module is loaded
#[wasm_bindgen(start)]
pub fn main() {
    use leptos::prelude::*;

    // Panic hook: same console output as before (via console_error_panic_hook)
    // plus a DOM recovery overlay with a Reload button. See panic_overlay.rs.
    panic_overlay::install();

    // Replay persisted appearance prefs (mode / accent / font scale / roundness)
    // onto <html> before mount so the first paint already reflects user choices.
    appearance::init_appearance();

    // Mount the app to the body
    mount_to_body(app::App);
}
