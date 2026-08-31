//! The one list in the left column: configured backends, preset and custom
//! alike.
//!
//! This used to be two lists. Presets went through `PresetGrid` (all nine
//! cards drawn whether configured or not), custom backends through
//! `CustomSearchProvidersList` (a grey icon, a different subtitle), and the
//! two kinds of card did not even look alike. So "which search providers do
//! I have configured" had two answers in the left panel, depending on
//! whether the one you configured happened to be in the protocol table --
//! and that has nothing to do with the operator.

use leptos::prelude::*;

use super::presentation::{find_preset, NEUTRAL_ICON_COLOR};
use crate::api::SearchConfig;
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::i18n::{t, use_i18n};

/// A backend name's icon color: presets carry a brand color, everything else
/// gets the neutral one.
fn icon_color_for(name: &str) -> &'static str {
    find_preset(name).map_or(NEUTRAL_ICON_COLOR, |p| p.icon_color)
}

/// A backend name's display name: presets carry a vendor name, everything
/// else is its own id.
fn display_name_for(name: &str) -> String {
    find_preset(name).map_or_else(|| name.to_string(), |p| p.display_name.to_string())
}

/// The card list of configured backends.
#[component]
pub(super) fn ConfiguredList(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || {
            let cfg = config.get();
            let rows = super::picker::listed(&cfg);
            if rows.is_empty() {
                // No heading on the empty state: an empty "Search Providers"
                // sub-heading reads like a load failure.
                return view! { <div></div> }.into_any();
            }
            let default_provider = cfg.default_provider.clone();
            view! {
                <div>
                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                        {t!(i18n, settings.search.providers_section)}
                    </h2>
                    <div class="grid grid-cols-1 gap-2">
                        {rows.into_iter().map(|backend| {
                            let name = backend.name.clone();
                            let name_sel = name.clone();
                            let name_click = name.clone();
                            let is_default = !default_provider.is_empty()
                                && default_provider == name;
                            let verified = backend.verified;
                            view! {
                                <ProviderRowCard
                                    name=display_name_for(&name)
                                    icon_color=icon_color_for(&name).to_string()
                                    subtitle=name.clone()
                                    is_selected=move || {
                                        selected.get().as_deref() == Some(name_sel.as_str())
                                    }
                                    is_configured=move || true
                                    dot=move || if verified { RowDot::Verified } else { RowDot::None }
                                    badge=move || view! {
                                        <ProviderBadges state=BadgeState { is_default, verified } />
                                    }.into_any()
                                    on_click=move || {
                                        show_add_form.set(false);
                                        selected.set(Some(name_click.clone()));
                                    }
                                />
                            }
                        }).collect_view()}
                    </div>
                </div>
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that isn't in the preset table still needs an icon color it
    /// can paint with -- a row with no color renders as a blank tile.
    /// Criterion E.0 §17: a piece of display state must be traceable to the
    /// line that renders it.
    #[test]
    fn a_custom_backend_gets_the_neutral_icon_colour() {
        assert_eq!(icon_color_for("my-searx"), NEUTRAL_ICON_COLOR);
    }

    #[test]
    fn a_preset_backend_keeps_its_brand_colour() {
        assert_eq!(icon_color_for("brave"), "#FB542B");
    }

    /// A preset row shows the vendor's name; a custom row shows its own id --
    /// a custom backend has no display_name, so the id is the only honest
    /// answer.
    #[test]
    fn display_name_falls_back_to_the_backend_id() {
        assert_eq!(display_name_for("brave"), "Brave");
        assert_eq!(display_name_for("my-searx"), "my-searx");
    }
}
