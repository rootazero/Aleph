pub mod app;
pub mod api;
pub mod components;
pub mod context;
pub mod generation;
pub mod models;
pub mod preset_providers;
pub mod views;

use wasm_bindgen::prelude::*;

/// Initialize the Leptos application
/// This function is automatically called when the WASM module is loaded
#[wasm_bindgen(start)]
pub fn main() {
    use leptos::prelude::*;

    // Set up panic hook for better error messages
    console_error_panic_hook::set_once();

    // Initialize theme from localStorage or system preference
    init_theme();

    // Mount the app to the body
    mount_to_body(app::App);
}

/// Read theme preference from localStorage and apply dark/light class to <html>
fn init_theme() {
    let window = web_sys::window().expect("no window");
    let document = window.document().expect("no document");
    let html = document.document_element().expect("no html element");

    // Check localStorage for saved preference
    let storage = window.local_storage().ok().flatten();
    let saved_theme = storage
        .as_ref()
        .and_then(|s| s.get_item("aleph-theme").ok())
        .flatten();

    match saved_theme.as_deref() {
        Some("dark") => {
            let _ = html.class_list().add_1("dark");
        }
        Some("light") => {
            let _ = html.class_list().add_1("light");
        }
        _ => {
            // Follow system preference (CSS @media handles this)
        }
    }
}
