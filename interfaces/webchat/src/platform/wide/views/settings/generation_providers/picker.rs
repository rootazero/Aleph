//! How the generation catalogue is split between the left panel and the
//! "add a provider" disclosure.
//!
//! The interaction is [`crate::components::preset_picker`], shared with the
//! chat providers page. This module owns the partition — which rows the panel
//! lists, which rows the picker offers, and what choosing one selects. All
//! three live together because they are one decision: written in three places
//! they drift, and a row that is neither listed nor offered is unreachable
//! without any code failing.
//!
//! # Why this catalogue is hideable and the embedding one is not
//!
//! 44 presets across five modality tabs put 14 cards in front of an operator
//! who wanted the one or two they had set up. They are safe to collapse because
//! generation providers are **additive**: `handle_create` touches no defaults,
//! `handle_set_default` is a separate explicit act, and `handle_delete` refuses
//! to remove a provider that is serving as one — so nothing on a row is a cost
//! you pay by choosing it. The embedding page's five rows carry `model · <n>d`,
//! and that number decides whether the vectors already in the memory store
//! survive the switch; those rows exist to be compared with each other and
//! stay on screen.
//!
//! # Category first, then the query
//!
//! [`offerable`] narrows to the selected modality before ranking, so a query
//! can never pull a video preset into the image tab. The picker re-offers when
//! the tab changes because its `offer` closure reads the category signal.

use leptos::prelude::*;

use crate::api::GenerationProviderEntry;
use crate::components::preset_picker::{PickerRow, PresetPicker};
use crate::components::provider_badge::BadgeState;
use crate::generation::GenerationType;
use crate::preset_providers::{PresetCatalog, PresetProvider};

/// True when the operator has a config section for this preset id.
pub(super) fn is_configured(providers: &[GenerationProviderEntry], id: &str) -> bool {
    providers.iter().any(|p| p.name == id)
}

/// The presets the left panel lists: this category's, configured only.
///
/// Configured rows stay offerable in the picker as well — marked, opening
/// their editor rather than a blank setup form. Dropping them from the picker
/// would teach a reader searching for one that Aleph does not support it, and
/// would break the delete round-trip: a deleted provider has to be findable
/// again to be set up again.
pub(super) fn listed(
    catalog: &PresetCatalog,
    providers: &[GenerationProviderEntry],
    category: GenerationType,
) -> Vec<PresetProvider> {
    catalog
        .by_category(category)
        .into_iter()
        .filter(|p| is_configured(providers, &p.id))
        .collect()
}

/// The rows the picker offers for a query, best match first.
///
/// An empty query returns every preset in the category, in the catalogue's own
/// order — the contract [`PresetPicker`] states, and the reason opening the
/// disclosure is still browsing.
pub(super) fn offerable(
    catalog: &PresetCatalog,
    providers: &[GenerationProviderEntry],
    category: GenerationType,
    query: &str,
) -> Vec<PickerRow> {
    catalog
        .by_category_matching(category, query)
        .into_iter()
        .map(|preset| {
            let entry = providers.iter().find(|e| e.name == preset.id);
            PickerRow {
                configured: entry.is_some(),
                badge: BadgeState {
                    is_default: entry.is_some_and(|e| !e.is_default_for.is_empty()),
                    verified: entry.is_some_and(|e| e.config.verified),
                },
                id: preset.id,
                name: preset.name,
                subtitle: preset.default_model,
                icon_color: preset.color,
                icon_glyph: Some(preset.icon),
            }
        })
        .collect()
}

/// What selecting `id` puts in the page's `selected_provider_id`.
///
/// A configured preset opens its detail editor under its own id; an
/// unconfigured one opens the setup form under the `__preset__` key the right
/// pane routes on. Identical to what clicking a card in the panel does — the
/// picker selects, it never writes config.
pub(super) fn chosen_target(providers: &[GenerationProviderEntry], id: &str) -> String {
    if is_configured(providers, id) {
        id.to_string()
    } else {
        format!("__preset__{id}")
    }
}

/// The generation catalogue's picker, scoped to the selected modality tab.
#[component]
pub(super) fn CategoryPicker(
    catalog: ReadSignal<PresetCatalog>,
    providers: ReadSignal<Vec<GenerationProviderEntry>>,
    category: ReadSignal<GenerationType>,
    selected: WriteSignal<Option<String>>,
    show_add_form: WriteSignal<bool>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let offer =
        move |query: &str| offerable(&catalog.get(), &providers.get(), category.get(), query);
    let on_choose = move |id: String| {
        selected.set(Some(chosen_target(&providers.get_untracked(), &id)));
        show_add_form.set(false);
    };

    view! { <PresetPicker offer=offer on_choose=on_choose open=open /> }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::providers::GenerationPresetRow;

    fn preset(id: &str, name: &str, modality: &str) -> GenerationPresetRow {
        GenerationPresetRow {
            id: id.to_string(),
            display_name: name.to_string(),
            provider_type: "openai".to_string(),
            default_model: format!("{id}-model"),
            base_url: None,
            modalities: vec![modality.to_string()],
            notes: None,
            homepage: None,
            signup_url: None,
        }
    }

    fn catalog() -> PresetCatalog {
        PresetCatalog::from_rows(vec![
            preset("openai-dalle", "OpenAI DALL-E", "image"),
            preset("stability-ai", "Stability AI", "image"),
            preset("google-veo", "Google Veo", "video"),
        ])
    }

    /// A row as `generation_providers.list` would report it, built through
    /// serde rather than a struct literal so the fields this partition does not
    /// care about cannot go stale as the config grows. Only the ones without a
    /// serde default are spelled out.
    fn configured(name: &str) -> GenerationProviderEntry {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "is_default_for": [],
            "config": {
                "provider_type": "openai",
                "enabled": true,
                "color": "#10a37f",
                "capabilities": ["image"],
                "timeout_seconds": 60,
            },
        }))
        .expect("every field without a serde default is supplied above")
    }

    fn ids(rows: &[PickerRow]) -> Vec<String> {
        rows.iter().map(|r| r.id.clone()).collect()
    }

    #[test]
    fn an_empty_query_offers_every_preset_in_the_category() {
        // The whole argument for putting the catalogue behind a button: opening
        // it is still browsing.
        crate::components::preset_picker::contract::empty_query_offers_everything(
            |q| offerable(&catalog(), &[], GenerationType::Image, q),
            &["openai-dalle", "stability-ai"],
        );
    }

    #[test]
    fn a_query_never_reaches_outside_the_selected_tab() {
        // `google-veo` matches "go" but is a video preset; surfacing it in the
        // image tab would open a setup form the tab cannot route.
        assert!(ids(&offerable(&catalog(), &[], GenerationType::Image, "go")).is_empty());
    }

    #[test]
    fn a_keyword_narrows_the_offered_rows() {
        assert_eq!(
            ids(&offerable(&catalog(), &[], GenerationType::Image, "stab")),
            ["stability-ai"]
        );
    }

    #[test]
    fn the_panel_lists_only_what_is_configured() {
        let providers = vec![configured("stability-ai")];
        let rows = listed(&catalog(), &providers, GenerationType::Image);
        assert_eq!(
            rows.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            ["stability-ai"]
        );
    }

    #[test]
    fn nothing_configured_means_an_empty_panel_and_a_full_picker() {
        // The state a fresh install lands in — and why the page seeds the
        // disclosure open rather than rendering one lone button.
        assert!(listed(&catalog(), &[], GenerationType::Image).is_empty());
        assert_eq!(
            offerable(&catalog(), &[], GenerationType::Image, "").len(),
            2
        );
    }

    #[test]
    fn a_configured_preset_is_listed_and_still_offered() {
        let providers = vec![configured("stability-ai")];
        assert_eq!(
            listed(&catalog(), &providers, GenerationType::Image).len(),
            1
        );
        crate::components::preset_picker::contract::configured_rows_stay_offered_and_marked(
            |q| offerable(&catalog(), &providers, GenerationType::Image, q),
            "stability-ai",
        );
    }

    #[test]
    fn deleting_a_provider_returns_its_row_to_the_panels_picker_only() {
        let before = vec![configured("stability-ai")];
        assert_eq!(listed(&catalog(), &before, GenerationType::Image).len(), 1);
        // Delete empties the config list; the preset must remain offerable.
        assert!(listed(&catalog(), &[], GenerationType::Image).is_empty());
        crate::components::preset_picker::contract::deleted_row_returns_to_the_picker(
            |q| offerable(&catalog(), &[], GenerationType::Image, q),
            "stability-ai",
        );
    }

    #[test]
    fn choosing_an_unconfigured_preset_opens_its_setup_form() {
        assert_eq!(chosen_target(&[], "openai-dalle"), "__preset__openai-dalle");
    }

    #[test]
    fn choosing_a_configured_preset_opens_its_editor() {
        let providers = vec![configured("openai-dalle")];
        assert_eq!(chosen_target(&providers, "openai-dalle"), "openai-dalle");
    }

    #[test]
    fn an_offered_row_carries_everything_the_card_used_to_show() {
        // The card the picker replaces rendered a category glyph, the name, the
        // default model and the Default/Verified badges. Dropping any of them
        // would make the disclosure a worse view of the same rows.
        let rows = offerable(&catalog(), &[], GenerationType::Image, "dalle");
        let row = &rows[0];
        assert_eq!(row.name, "OpenAI DALL-E");
        assert_eq!(row.subtitle, "openai-dalle-model");
        assert!(row.icon_glyph.is_some());
        assert!(!row.icon_color.is_empty());
    }
}
