//! `MobileTopBar` — the reusable mobile chrome band mounted atop every tab.
//!
//! Three slots: `left` (defaults to a hamburger that opens the nav drawer),
//! `title` (always a plain `String` signal rendered centered — the component
//! has ZERO agent / `MemoryState` dependency, so any tab can mount it), and
//! `right` (defaults to the `NotificationBell` trigger). Chat overrides `left`
//! with its agent pill; other tabs pass `title = label_of(mode)` and leave
//! `left` / `right` unset. Safe-area + z-band live in the `.mobile-top-bar`
//! design-system class so no tab re-derives them.

use crate::components::notification_bell::NotificationBell;
use crate::state::viewport::ViewportState;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn MobileTopBar(
    /// Center title — a plain string signal, no agent context required.
    title: Signal<String>,
    /// Left slot. `None` → auto hamburger that opens the nav drawer.
    #[prop(optional)]
    left: Option<Children>,
    /// Right slot. `None` → auto `NotificationBell` trigger.
    #[prop(optional)]
    right: Option<Children>,
) -> impl IntoView {
    let drawer_open = expect_context::<ViewportState>().drawer_open;

    let left_slot = match left {
        Some(children) => children().into_any(),
        None => view! {
            <button
                type="button"
                class="aleph-no-drag flex h-8 w-8 items-center justify-center \
                       rounded-full text-text-secondary hover:text-text-primary \
                       hover:bg-surface-raised transition-colors"
                on:click=move |_| drawer_open.set(true)
                aria-label="Open navigation"
            >
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="1.8"
                     stroke-linecap="round" stroke-linejoin="round">
                    <line x1="3" y1="6" x2="21" y2="6" />
                    <line x1="3" y1="12" x2="21" y2="12" />
                    <line x1="3" y1="18" x2="21" y2="18" />
                </svg>
            </button>
        }
        .into_any(),
    };

    let right_slot = match right {
        Some(children) => children().into_any(),
        None => view! { <NotificationBell /> }.into_any(),
    };

    view! {
        <div class="mobile-top-bar hidden max-sm:flex items-center justify-between \
                    px-3 pb-2">
            {left_slot}
            <span class="flex-1 min-w-0 text-center text-sm font-semibold \
                         text-text-primary truncate px-2">
                {move || title.get()}
            </span>
            {right_slot}
        </div>
    }
}
