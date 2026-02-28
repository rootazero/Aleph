// core/ui/control_plane/src/components/chat_sidebar.rs
//
// Chat mode sidebar — session list grouped by project (placeholder until session API wired).
//
use leptos::prelude::*;

#[component]
pub fn ChatSidebar() -> impl IntoView {
    view! {
        <div class="flex flex-col h-full">
            // Search
            <div class="p-3">
                <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-text-tertiary flex-shrink-0">
                        <circle cx="11" cy="11" r="8" />
                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                    </svg>
                    <span class="text-text-tertiary">"Search chats..."</span>
                </div>
            </div>

            // Session list (placeholder)
            <div class="flex-1 overflow-y-auto px-3 py-2 space-y-1">
                <p class="text-xs text-text-tertiary px-3 py-4 text-center">
                    "Start a new conversation"
                </p>
            </div>
        </div>
    }
}
