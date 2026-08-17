//! Which chat-catalogue rows the "add a provider" disclosure offers, and what
//! choosing one selects.
//!
//! The interaction — disclosure state, search box, ↑/↓/Enter/Esc, scrolling the
//! lit row into view — is [`crate::components::preset_picker`], shared with the
//! generation page. This module owns only the half that is specific to
//! `providers.catalog`.
//!
//! # Why the catalogue moved behind a button
//!
//! `providers.catalog` ships 56 presets. Rendering all of them as cards made
//! the left panel a scroll well in which the rows an operator actually works
//! with — the ones they have configured — were the hardest to find. The panel
//! now lists sign-in providers and configured providers only; everything else
//! is one click and one keystroke away here.
//!
//! # Why this is not a regression in discoverability
//!
//! [`pickable`] with an empty query returns **every** offerable row in the
//! server's curated order (default first, verified first) — the shared ranker
//! documents that "no filter" is behaviourally identical to not filtering at
//! all. So opening this is still browsing; typing is an accelerator, not a toll
//! gate. A catalogue that only appeared *after* you typed would require knowing
//! a vendor's name before you could learn Aleph supports it.

use leptos::prelude::*;

use aleph_protocol::providers::search::filter_catalog;

use crate::api::{AuthKind, CatalogEntry, ProviderInfo};
use crate::components::preset_picker::{PickerRow, PresetPicker};
use crate::components::provider_badge::BadgeState;

use super::list::{configured_key, is_configured};

/// The rows the picker offers, in the order it shows them.
///
/// Two exclusions, each because the row already has a door of its own:
///
/// * `protocol == "moa"` is a virtual multiplexer over other providers'
///   credentials with no config section, so "adding" it would open an editor
///   that can only write nonsense — the same exclusion, for the same reason, as
///   `list::editable`.
/// * `auth_kind == OAuth` rows stay in the always-visible subscription section
///   of the left panel. They are three to five rows that need no key, and they
///   are on screen without opening anything; offering them here as well would
///   be a second entrance to a room whose door is already open.
///
/// Configured rows are deliberately **kept**. Hiding a provider from search
/// because you already set it up teaches the reader that Aleph does not support
/// it; the row is marked instead, and picking it opens its editor rather than a
/// blank setup form. It also keeps the delete round-trip symmetric — a deleted
/// provider empties its `models` ladder, which drops it from the configured
/// section, and it must still be findable here to be set up again.
///
/// `filter_catalog`, not the flat `filter_rows`: a chat provider can be found
/// by a model in its roster (`sonnet` → Anthropic), and the narrowed roster
/// that comes back is why the picker takes an `offer` closure rather than a
/// pre-flattened row list.
pub(super) fn pickable(catalog: &[CatalogEntry], query: &str) -> Vec<CatalogEntry> {
    filter_catalog(catalog, query)
        .into_iter()
        .filter(|e| e.protocol != "moa" && e.auth_kind != AuthKind::OAuth)
        .collect()
}

/// What selecting `id` puts in the `selected` signal.
///
/// A configured row opens its editor under the key `providers.list` reports —
/// which is not necessarily the catalogue id, because a preset can be
/// configured under an alias (`kimi` for `moonshot`). An unconfigured row opens
/// the preset setup form. Choosing never writes config either way.
pub(super) fn chosen_target(
    catalog: &[CatalogEntry],
    providers: &[ProviderInfo],
    id: &str,
) -> String {
    catalog
        .iter()
        .find(|e| e.id == id)
        .and_then(|e| configured_key(e, providers))
        .unwrap_or_else(|| format!("__preset__{id}"))
}

/// Flatten a catalogue row into what the picker draws.
fn as_row(entry: &CatalogEntry) -> PickerRow {
    PickerRow {
        id: entry.id.clone(),
        name: entry.display_name.clone(),
        subtitle: if entry.default_model.is_empty() {
            entry
                .notes
                .clone()
                .unwrap_or_else(|| entry.base_url.clone())
        } else {
            entry.default_model.clone()
        },
        icon_color: entry.color.clone(),
        // Chat rows have no glyph — the card falls back to the first character
        // of the display name, which is what the left-panel rows already show.
        icon_glyph: None,
        configured: is_configured(entry),
        badge: BadgeState {
            is_default: entry.is_default,
            verified: entry.verified,
        },
    }
}

/// The chat catalogue's picker.
///
/// `open` is owned by the parent because the first-load seed — expand when the
/// operator has configured nothing — has to reach it from where the catalogue
/// fetch lands.
#[component]
pub(super) fn CatalogPicker(
    catalog: RwSignal<Vec<CatalogEntry>>,
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let offer = move |query: &str| pickable(&catalog.get(), query).iter().map(as_row).collect();
    let on_choose = move |id: String| {
        let target = chosen_target(&catalog.get_untracked(), &providers.get_untracked(), &id);
        selected.set(Some(target));
    };

    view! { <PresetPicker offer=offer on_choose=on_choose open=open /> }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, protocol: &str, auth: AuthKind) -> CatalogEntry {
        CatalogEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            default_model: String::new(),
            base_url: String::new(),
            protocol: protocol.to_string(),
            color: "#808080".to_string(),
            homepage: None,
            notes: None,
            signup_url: None,
            aliases: Vec::new(),
            modalities: Vec::new(),
            models: Vec::new(),
            has_api_key: false,
            verified: false,
            enabled: true,
            is_default: false,
            auth_kind: auth,
            endpoint: "cloud".to_string(),
            requires_explicit_model: false,
            discoverable: false,
            roster: Vec::new(),
        }
    }

    fn key_entry(id: &str) -> CatalogEntry {
        entry(id, "openai", AuthKind::ApiKey)
    }

    fn ids(rows: &[CatalogEntry]) -> Vec<String> {
        rows.iter().map(|e| e.id.clone()).collect()
    }

    /// A row as `providers.list` would report it. Built through serde rather
    /// than a struct literal so the wire defaults are the ones under test —
    /// `ProviderInfo` has no `Default`, and hand-writing all sixteen fields
    /// here would go stale the next time one is added.
    fn configured(name: &str) -> ProviderInfo {
        serde_json::from_value(serde_json::json!({ "name": name }))
            .expect("every field but `name` has a serde default")
    }

    #[test]
    fn choosing_an_unconfigured_row_opens_its_setup_form() {
        let catalog = vec![key_entry("groq")];
        assert_eq!(
            chosen_target(&catalog, &[], "groq"),
            "__preset__groq",
            "nothing is configured, so there is no editor to open"
        );
    }

    #[test]
    fn choosing_a_configured_row_opens_its_editor() {
        let catalog = vec![key_entry("groq")];
        assert_eq!(
            chosen_target(&catalog, &[configured("groq")], "groq"),
            "groq"
        );
    }

    #[test]
    fn choosing_a_row_configured_under_an_alias_opens_that_alias_editor() {
        // `moonshot` set up as `kimi`: the config key is the alias, and
        // resolving to the catalogue id instead would open a blank setup form
        // for a provider that is already configured.
        let mut moonshot = key_entry("moonshot");
        moonshot.aliases = vec!["kimi".to_string()];
        assert_eq!(
            chosen_target(&[moonshot], &[configured("kimi")], "moonshot"),
            "kimi"
        );
    }

    #[test]
    fn choosing_a_row_the_catalogue_no_longer_has_still_opens_a_setup_form() {
        // The catalogue can be refetched between the keypress and the read.
        // Falling through to the preset form is the honest answer; panicking or
        // selecting nothing would strand the operator on a dead click.
        assert_eq!(chosen_target(&[], &[], "groq"), "__preset__groq");
    }

    #[test]
    fn empty_query_offers_every_row_in_the_servers_order() {
        // The whole argument for putting the catalogue behind a button: opening
        // it is still browsing. If this ever filters on an empty query, the
        // page stops being able to tell you which providers exist.
        let catalog = vec![
            key_entry("openai"),
            key_entry("moonshot"),
            key_entry("groq"),
        ];
        assert_eq!(ids(&pickable(&catalog, "")), ["openai", "moonshot", "groq"]);
        assert_eq!(
            ids(&pickable(&catalog, "   ")),
            ["openai", "moonshot", "groq"]
        );
    }

    #[test]
    fn a_keyword_narrows_the_list() {
        let catalog = vec![
            key_entry("openai"),
            key_entry("moonshot"),
            key_entry("groq"),
        ];
        assert_eq!(ids(&pickable(&catalog, "moon")), ["moonshot"]);
    }

    #[test]
    fn an_alias_finds_its_row() {
        // `kimi` → `moonshot` is the case the shared ranker exists for; the
        // picker must not lose it by filtering on ids alone.
        let mut moonshot = key_entry("moonshot");
        moonshot.aliases = vec!["kimi".to_string()];
        let catalog = vec![key_entry("openai"), moonshot];
        assert_eq!(ids(&pickable(&catalog, "kimi")), ["moonshot"]);
    }

    #[test]
    fn the_moa_pseudo_row_is_never_offered() {
        let catalog = vec![key_entry("openai"), entry("moa", "moa", AuthKind::ApiKey)];
        assert_eq!(ids(&pickable(&catalog, "")), ["openai"]);
        // Not merely absent from the unfiltered list — unreachable by name too.
        assert!(pickable(&catalog, "moa").is_empty());
    }

    #[test]
    fn subscription_rows_are_not_offered_because_their_section_is_always_visible() {
        let catalog = vec![
            key_entry("openai"),
            entry("chatgpt", "codex", AuthKind::OAuth),
        ];
        assert_eq!(ids(&pickable(&catalog, "")), ["openai"]);
        assert!(pickable(&catalog, "chatgpt").is_empty());
    }

    #[test]
    fn an_already_configured_row_is_still_offered() {
        // "I set this up months ago and searched for it" must not answer the
        // same way as "Aleph does not have this provider".
        let mut configured = key_entry("moonshot");
        configured.models = vec!["kimi-k2".to_string()];
        let catalog = vec![key_entry("openai"), configured];
        assert_eq!(ids(&pickable(&catalog, "moonshot")), ["moonshot"]);
    }

    #[test]
    fn deleting_a_provider_returns_its_row_to_the_picker() {
        // Deletion empties the operator's ladder, which is exactly what drops
        // the row out of the configured section. The picker offers rows
        // regardless of that state, so the round trip closes: configure →
        // delete → find it again → configure again.
        let mut configured = key_entry("moonshot");
        configured.models = vec!["kimi-k2".to_string()];
        assert!(is_configured(&configured));

        let deleted = key_entry("moonshot");
        assert!(!is_configured(&deleted));
        assert_eq!(ids(&pickable(&[deleted], "moonshot")), ["moonshot"]);
    }
}
