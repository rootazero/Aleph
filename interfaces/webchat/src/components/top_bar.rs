//
// Top bar — logo, theme toggle. Agent selection is handled by the chat sidebar.
//
use super::theme_toggle::ThemeToggle;
use leptos::prelude::*;

#[component]
pub fn TopBar() -> impl IntoView {
    view! {
        <header class="h-12 border-b border-border bg-sidebar flex items-center justify-between px-4 flex-shrink-0">
            // Left: Logo
            <div class="flex items-center gap-3">
                <div class="w-7 h-7 bg-primary rounded-lg flex items-center justify-center">
                    <span class="text-text-inverse font-bold text-base">"A"</span>
                </div>
                <h1 class="text-sm font-semibold tracking-tight">"Aleph"</h1>
            </div>

            // Right: theme toggle
            <div class="flex items-center gap-2">
                <ThemeToggle />
            </div>
        </header>
    }
}
