//! Unified provider status badges
//!
//! Renders the "Default" and "Verified" pills for a provider row/detail.
//! Both badges can be shown simultaneously.

use crate::i18n::*;
use leptos::prelude::*;

/// Pure, testable badge state for a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BadgeState {
    pub is_default: bool,
    pub verified: bool,
}

impl BadgeState {
    /// True if any badge would render.
    pub fn any(self) -> bool {
        self.is_default || self.verified
    }
}

/// Renders provider status badges. Both "Default" and "Verified" may show at once.
#[component]
pub fn ProviderBadges(state: BadgeState) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <span class="flex items-center gap-1">
            {state.is_default.then(|| view! {
                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-primary-subtle text-primary">
                    {t!(i18n, settings.providers.badge_default)}
                </span>
            })}
            {state.verified.then(|| view! {
                <span class="px-2.5 py-1 rounded-full text-xs font-medium bg-success-subtle text-success">
                    {t!(i18n, settings.providers.badge_verified)}
                </span>
            })}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_renders_nothing() {
        let s = BadgeState::default();
        assert!(!s.any());
        assert!(!s.is_default);
        assert!(!s.verified);
    }

    #[test]
    fn default_and_verified_coexist() {
        let s = BadgeState { is_default: true, verified: true };
        assert!(s.any());
        assert!(s.is_default);
        assert!(s.verified);
    }

    #[test]
    fn single_badge_is_any() {
        assert!(BadgeState { is_default: true, verified: false }.any());
        assert!(BadgeState { is_default: false, verified: true }.any());
    }
}
