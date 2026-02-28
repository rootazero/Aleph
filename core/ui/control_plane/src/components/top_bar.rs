// core/ui/control_plane/src/components/top_bar.rs
//
// Top bar — logo, title, contextual actions (new chat button in Chat mode).
//
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use super::bottom_bar::PanelMode;
use super::theme_toggle::ThemeToggle;

#[component]
pub fn TopBar() -> impl IntoView {
    let location = use_location();
    let mode = Memo::new(move |_| PanelMode::from_path(&location.pathname.get()));

    view! {
        <header class="h-12 border-b border-border bg-sidebar flex items-center justify-between px-4 flex-shrink-0">
            // Left: Logo
            <div class="flex items-center gap-3">
                <div class="w-7 h-7 bg-primary rounded-lg flex items-center justify-center">
                    <span class="text-text-inverse font-bold text-base">"A"</span>
                </div>
                <h1 class="text-sm font-semibold tracking-tight">"Aleph"</h1>
            </div>

            // Right: contextual actions + theme
            <div class="flex items-center gap-2">
                <Show when=move || mode.get() == PanelMode::Chat>
                    <a
                        href="/"
                        class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
                               text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-colors"
                    >
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <line x1="12" y1="5" x2="12" y2="19" />
                            <line x1="5" y1="12" x2="19" y2="12" />
                        </svg>
                        "New Chat"
                    </a>
                </Show>
                <ThemeToggle />
            </div>
        </header>
    }
}
