// core/ui/control_plane/src/components/ui/tooltip.rs
use leptos::prelude::*;
use crate::components::sidebar::SystemAlert;

#[component]
pub fn Tooltip(
    text: &'static str,
    #[prop(into)] alert: MaybeProp<SystemAlert>,
    #[prop(default = "right")] position: &'static str,
) -> impl IntoView {
    view! {
        <div class="absolute left-full ml-2 px-3 py-2 bg-surface-raised border border-border rounded-lg opacity-0 group-hover:opacity-100 transition-opacity duration-200 pointer-events-none whitespace-nowrap z-50">
            <div class="text-sm font-medium text-text-primary">{text}</div>
            {move || alert.get().and_then(|a| a.message).map(|msg| view! {
                <div class="text-xs text-text-tertiary mt-1">{msg}</div>
            })}
        </div>
    }
}
