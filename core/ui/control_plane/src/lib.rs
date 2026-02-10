pub mod app;
pub mod api;
pub mod components;
pub mod context;
pub mod generation;
pub mod models;
pub mod preset_providers;
pub mod views;
pub mod mock_data;

use wasm_bindgen::prelude::*;

/// Initialize the Leptos application
/// This function is automatically called when the WASM module is loaded
#[wasm_bindgen(start)]
pub fn main() {
    use leptos::prelude::*;

    // Set up panic hook for better error messages
    console_error_panic_hook::set_once();

    // Mount the app to the body
    mount_to_body(app::App);
}