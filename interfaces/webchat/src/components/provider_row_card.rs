//! Shared provider-row card for Settings provider lists.
//!
//! Five sites across `views/settings/{providers,embedding_providers,reranking_providers}/`
//! render structurally-identical "card buttons" in left-panel lists. Each card has:
//! - a coloured icon tile with the first character of the provider name
//! - the provider name + an optional badge (e.g. "Default", "Verified", "Active")
//! - a subtitle line (description, model, or `model · dims`)
//! - a small verified dot on the icon corner (only some lists)
//! - selection / configured / unconfigured visual states driven by the parent signal
//!
//! Reactive slots accept `impl Fn() -> T + 'static + Send` closures so each site can
//! decide whether the predicate is static (`move || true`) or signal-driven
//! (`move || providers.get().iter().any(...)`).
//!
//! Also supports OAuth-style rows (e.g. the `SubscriptionLoginSection` row in
//! `providers/list.rs`) via the optional `large_icon` prop (`w-10 h-10` icon
//! tile) and the optional `trailing` slot (a right-pushed element such as a
//! chevron). Prefer these props over writing a new inline row.

use leptos::prelude::*;

/// Verified-dot variants rendered on the icon corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDot {
    /// No dot rendered.
    None,
    /// Green success dot — provider is verified.
    Verified,
    /// Greyed dot — placeholder used by the Custom providers list when a row
    /// always shows SOME dot regardless of verified state.
    Inactive,
}

#[component]
pub fn ProviderRowCard(
    /// Display name (capitalisation is caller's responsibility).
    name: String,
    /// CSS color value for the icon-tile background.
    icon_color: String,
    /// Secondary descriptor line — description, model, or `model · dims`.
    subtitle: String,
    /// Selected predicate — drives the primary-subtle border + bg.
    is_selected: impl Fn() -> bool + 'static + Send,
    /// Configured predicate — when not selected, configured rows get bg-surface-raised,
    /// unconfigured rows get bg-surface-sunken.
    is_configured: impl Fn() -> bool + 'static + Send,
    /// Verified-indicator dot on the icon corner.
    dot: impl Fn() -> RowDot + 'static + Send,
    /// Right-side badge slot. Caller returns an `AnyView` (use `.into_any()`).
    /// Use `move || view! { <span></span> }.into_any()` to render no badge.
    badge: impl Fn() -> AnyView + 'static + Send,
    /// Click handler.
    on_click: impl Fn() + 'static + Send,
    /// Optional trailing element rendered after the name/subtitle block,
    /// pushed to the right (e.g. a chevron for OAuth subscription rows).
    #[prop(optional, into)]
    trailing: Option<ViewFn>,
    /// When true the icon tile uses `w-10 h-10` instead of `w-8 h-8`
    /// (used by OAuth subscription rows).
    #[prop(optional)]
    large_icon: bool,
) -> impl IntoView {
    let first_char = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let name_for_view = name.clone();
    view! {
        <button
            on:click=move |_| on_click()
            class=move || {
                let base = "text-left p-3 rounded-lg border transition-all";
                if is_selected() {
                    format!("{} bg-primary-subtle border-primary", base)
                } else if is_configured() {
                    format!("{} bg-surface-raised border-border hover:border-primary/40", base)
                } else {
                    format!("{} bg-surface-sunken border-border hover:border-border-strong", base)
                }
            }
        >
            <div class="flex items-center gap-3">
                <div class="relative shrink-0">
                    <div
                        class=format!(
                            "{} rounded-lg flex items-center justify-center text-white text-sm font-bold",
                            if large_icon { "w-10 h-10" } else { "w-8 h-8" }
                        )
                        style=format!("background-color: {}", icon_color)
                    >
                        {first_char}
                    </div>
                    {move || match dot() {
                        RowDot::Verified => view! {
                            <span class="absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-success border-2 border-surface-raised" />
                        }.into_any(),
                        RowDot::Inactive => view! {
                            <span class="absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-text-tertiary/30 border-2 border-surface-raised" />
                        }.into_any(),
                        RowDot::None => view! { <span /> }.into_any(),
                    }}
                </div>
                <div class="min-w-0">
                    <div class="flex items-center gap-2">
                        <span class="font-medium text-text-primary text-sm truncate">
                            {name_for_view}
                        </span>
                        {move || badge()}
                    </div>
                    <div class="text-xs text-text-tertiary truncate">
                        {subtitle}
                    </div>
                </div>
                {trailing.map(|t| view! { <div class="ml-auto shrink-0">{t.run()}</div> })}
            </div>
        </button>
    }
}
