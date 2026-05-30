pub mod api;
pub mod app;
pub mod appearance;
pub mod canvas_engine;
pub mod components;
pub mod context;
pub mod generation;
pub mod i18n;
pub mod models;
pub mod panic_overlay;
pub mod preset_data;
pub mod preset_providers;
pub mod state;
pub mod views;

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
