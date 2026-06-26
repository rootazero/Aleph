//! Mobile-only Settings landing — iOS grouped-list of `SETTINGS_GROUPS`.
//!
//! Rendered alongside the desktop Quick Setup checklist (`settings/mod.rs`);
//! visibility is CSS-gated (`max-sm:block` here / `max-sm:hidden` on Quick
//! Setup) so both mount unconditionally (no Leptos reactive-scope teardown).
//! Zero new data: every cell is driven straight from `SETTINGS_GROUPS`.

use crate::components::settings_sidebar::SETTINGS_GROUPS;
use crate::i18n::use_i18n;
use leptos::prelude::*;
use leptos_router::components::A;

/// Number of groups rendered as iOS sections (data-source sanity).
#[must_use]
pub fn landing_group_count() -> usize {
    SETTINGS_GROUPS.len()
}

/// Total cells (= leaf settings entries) across all groups.
#[must_use]
pub fn landing_tab_count() -> usize {
    SETTINGS_GROUPS.iter().map(|g| g.tabs.len()).sum()
}

#[component]
#[must_use]
pub fn MobileSettingsLanding() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        // Mobile-only: hidden ≥640px; the desktop Quick Setup covers wide.
        <div class="hidden max-sm:block px-4 pb-8 aleph-content-top space-y-6">
            {SETTINGS_GROUPS.iter().map(|group| {
                let group_label = group.i18n_label(i18n);
                view! {
                    <section class="space-y-2">
                        <h2 class="px-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group_label}
                        </h2>
                        <div class="rounded-xl overflow-hidden border border-border bg-surface-raised divide-y divide-border">
                            {group.tabs.iter().map(|tab| {
                                let path = tab.path();
                                let label = tab.i18n_label(i18n);
                                let icon_svg = tab.icon_svg();
                                view! {
                                    <A
                                        href=path
                                        attr:class="flex items-center gap-3 px-4 py-3 active:bg-surface-sunken transition-colors"
                                    >
                                        <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                                             stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                             stroke-linejoin="round"
                                             class="text-text-tertiary flex-shrink-0"
                                             inner_html=icon_svg
                                        />
                                        <span class="flex-1 text-sm text-text-primary">{label}</span>
                                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
                                             stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                             stroke-linejoin="round"
                                             class="text-text-tertiary flex-shrink-0"
                                        >
                                            <polyline points="9 18 15 12 9 6" />
                                        </svg>
                                    </A>
                                }
                            }).collect_view()}
                        </div>
                    </section>
                }
            }).collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_renders_all_six_groups() {
        assert_eq!(
            landing_group_count(),
            6,
            "iOS landing must mirror all 6 SETTINGS_GROUPS"
        );
    }

    #[test]
    fn landing_cell_count_matches_metadata() {
        // 3 Basic + 8 AI + 1 Channels + 4 Extensions + 4 Advanced + 1 Network = 21.
        assert_eq!(
            landing_tab_count(),
            21,
            "landing must surface every settings leaf as a cell"
        );
    }
}
