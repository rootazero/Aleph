//! How the search catalogue is split between the left panel and the "add a
//! provider" disclosure.
//!
//! The interaction is [`crate::components::preset_picker`], shared with the
//! chat and generation pages. This module owns only the **partition**: which
//! rows the left panel lists, which rows the picker offers, and what choosing
//! one selects. Writing those three in three places lets them drift, and a
//! row that is neither listed nor offered is unreachable without any code
//! failing.
//!
//! # Two differences from generation
//!
//! * **No `__preset__` prefix.** generation's configured editor and
//!   unconfigured setup form are two separate components, so it needs a
//!   prefix to route between them; search is one component covering both
//!   states.
//! * **The custom endpoint is a row in the offer, not a button under the
//!   list.** It is always offered and always last — see
//!   `the_custom_row_survives_any_query_and_stays_last`.
//!
//! # i18n stays out of here
//!
//! [`offerable`] takes a `copy: impl Fn(Subtitle) -> String` instead of
//! calling `t_string!` itself. The partition rule can therefore be unit-tested
//! without a locale, and rewording the copy cannot turn a partition test red.

use leptos::prelude::*;

use super::presentation::{presets, SearchPreset, NEUTRAL_ICON_COLOR};
use crate::api::{SearchBackendEntry, SearchConfig};
use crate::components::preset_picker::{PickerRow, PresetPicker};
use crate::components::provider_badge::BadgeState;
use crate::i18n::{t_string, use_i18n};

/// The id of the "custom endpoint" row in the picker.
///
/// Not any backend's name: choosing it selects a form, not a provider. The
/// `__` prefix keeps it apart from the protocol's backend names, the same
/// convention generation's `__preset__` uses.
pub(super) const CUSTOM_ROW_ID: &str = "__custom__";

/// Which copy a row's subtitle needs. The page resolves it through i18n; this
/// module stays pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Subtitle {
    NeedsApiKey,
    SelfHosted,
    NoKeyRequired,
    CustomEndpoint,
}

/// What the page should do once a row is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Chosen {
    /// Select this backend (configured → edit; unconfigured → the setup state
    /// of the same panel).
    Backend(String),
    /// Open the custom endpoint form.
    CustomForm,
}

/// Whether the operator already has a backend configured under this name.
pub(super) fn is_configured(backends: &[SearchBackendEntry], name: &str) -> bool {
    backends.iter().any(|b| b.name == name)
}

/// The rows the left panel lists: **every** configured backend, preset and
/// custom alike.
///
/// This is the substance of merging the two lists: presets used to go through
/// `PresetGrid` (all nine listed whether configured or not) and custom
/// backends through `CustomSearchProvidersList`, whose cards did not even look
/// alike. The left panel now answers one question only — which ones did you
/// configure.
pub(super) fn listed(cfg: &SearchConfig) -> Vec<SearchBackendEntry> {
    cfg.backends.clone()
}

/// Which copy a preset's subtitle needs.
fn subtitle_kind(preset: &SearchPreset) -> Subtitle {
    if preset.is_self_hosted {
        Subtitle::SelfHosted
    } else if preset.needs_api_key {
        Subtitle::NeedsApiKey
    } else {
        Subtitle::NoKeyRequired
    }
}

/// The rows the picker offers for a query, best match first, with the custom
/// endpoint row always last.
///
/// An empty query returns **every** preset, in the protocol table's own order
/// — the contract [`PresetPicker`] declares, and the reason opening the
/// disclosure is still browsing.
pub(super) fn offerable(
    cfg: &SearchConfig,
    query: &str,
    copy: impl Fn(Subtitle) -> String,
) -> Vec<PickerRow> {
    let all: Vec<SearchPreset> = presets().collect();
    let mut rows: Vec<PickerRow> = aleph_protocol::providers::filter_rows(&all, query)
        .into_iter()
        .map(|preset| {
            let backend = cfg.backends.iter().find(|b| b.name == preset.name);
            PickerRow {
                configured: is_configured(&cfg.backends, preset.name),
                badge: BadgeState {
                    is_default: !cfg.default_provider.is_empty()
                        && cfg.default_provider == preset.name,
                    verified: backend.is_some_and(|b| b.verified),
                },
                id: preset.name.to_string(),
                name: preset.display_name.to_string(),
                subtitle: copy(subtitle_kind(&preset)),
                icon_color: preset.icon_color.to_string(),
                icon_glyph: None,
            }
        })
        .collect();

    rows.push(PickerRow {
        id: CUSTOM_ROW_ID.to_string(),
        name: copy(Subtitle::CustomEndpoint),
        subtitle: String::new(),
        icon_color: NEUTRAL_ICON_COLOR.to_string(),
        icon_glyph: Some("+".to_string()),
        configured: false,
        badge: BadgeState {
            is_default: false,
            verified: false,
        },
    });
    rows
}

/// What the page should do once `id` is chosen.
pub(super) fn chosen_target(id: &str) -> Chosen {
    if id == CUSTOM_ROW_ID {
        Chosen::CustomForm
    } else {
        Chosen::Backend(id.to_string())
    }
}

/// The search catalogue's disclosure.
#[component]
pub(super) fn SearchPicker(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let copy = move |s: Subtitle| -> String {
        match s {
            Subtitle::NeedsApiKey => t_string!(i18n, settings.search.row_needs_key).to_string(),
            Subtitle::SelfHosted => t_string!(i18n, settings.search.self_hosted).to_string(),
            Subtitle::NoKeyRequired => t_string!(i18n, settings.search.no_api_key).to_string(),
            Subtitle::CustomEndpoint => {
                t_string!(i18n, settings.search.add_custom_provider).to_string()
            }
        }
    };
    let offer = move |query: &str| offerable(&config.get(), query, copy);
    let on_choose = move |id: String| match chosen_target(&id) {
        Chosen::Backend(name) => {
            show_add_form.set(false);
            selected.set(Some(name));
        }
        Chosen::CustomForm => {
            selected.set(None);
            show_add_form.set(true);
        }
    };

    view! { <PresetPicker offer=offer on_choose=on_choose open=open /> }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test-only copy resolver: maps `Subtitle` to stable sentinel strings so
    /// assertions do not depend on locale, and do not go red when the wording
    /// changes.
    fn copy(s: Subtitle) -> String {
        match s {
            Subtitle::NeedsApiKey => "needs-key".to_string(),
            Subtitle::SelfHosted => "self-hosted".to_string(),
            Subtitle::NoKeyRequired => "no-key".to_string(),
            Subtitle::CustomEndpoint => "custom".to_string(),
        }
    }

    fn backend(name: &str) -> SearchBackendEntry {
        SearchBackendEntry {
            name: name.to_string(),
            api_key: None,
            base_url: None,
            engine_id: None,
            engines: None,
            has_api_key: true,
            verified: true,
        }
    }

    fn cfg(default_provider: &str, backends: Vec<SearchBackendEntry>) -> SearchConfig {
        SearchConfig {
            enabled: true,
            default_provider: default_provider.to_string(),
            max_results: 5,
            timeout_seconds: 10,
            pii_enabled: false,
            pii_scrub_email: true,
            pii_scrub_phone: true,
            pii_scrub_ssn: true,
            pii_scrub_credit_card: true,
            backends,
        }
    }

    fn ids(rows: &[PickerRow]) -> Vec<String> {
        rows.iter().map(|r| r.id.clone()).collect()
    }

    /// The catalogue's nine rows plus the custom-endpoint row at the end. Order
    /// == the protocol table's order.
    const EVERY_ROW: &[&str] = &[
        "tavily",
        "brave",
        "google",
        "bing",
        "searxng",
        "exa",
        "firecrawl",
        "duckduckgo",
        "jina",
        CUSTOM_ROW_ID,
    ];

    #[test]
    fn an_empty_query_offers_every_backend_plus_the_custom_row() {
        let c = cfg("", vec![]);
        crate::components::preset_picker::contract::empty_query_offers_everything(
            |q| offerable(&c, q, copy),
            EVERY_ROW,
        );
    }

    #[test]
    fn a_configured_backend_is_listed_and_still_offered() {
        let c = cfg("tavily", vec![backend("tavily")]);
        assert_eq!(
            listed(&c).iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            ["tavily"]
        );
        crate::components::preset_picker::contract::configured_rows_stay_offered_and_marked(
            |q| offerable(&c, q, copy),
            "tavily",
        );
    }

    #[test]
    fn deleting_a_backend_returns_its_row_to_the_picker() {
        let after = cfg("", vec![]);
        crate::components::preset_picker::contract::deleted_row_returns_to_the_picker(
            |q| offerable(&after, q, copy),
            "tavily",
        );
    }

    #[test]
    fn a_query_narrows_the_offered_presets() {
        let c = cfg("", vec![]);
        let rows = offerable(&c, "brav", copy);
        assert_eq!(ids(&rows), ["brave", CUSTOM_ROW_ID]);
    }

    /// The custom-endpoint row is **always** in the offer, and always last.
    ///
    /// It is an action, not a catalogue row. Hiding it behind a query would
    /// mean an operator who searched for a vendor we do not carry gets an
    /// empty list and zero ways forward — criterion E.0 §14: the negative side
    /// of a gate must have an exit too.
    #[test]
    fn the_custom_row_survives_any_query_and_stays_last() {
        let c = cfg("", vec![]);
        for q in ["", "brav", "zzzz-no-such-vendor"] {
            let rows = offerable(&c, q, copy);
            assert_eq!(
                rows.last().map(|r| r.id.as_str()),
                Some(CUSTOM_ROW_ID),
                "query {q:?} left the operator with no way to add a custom endpoint"
            );
        }
    }

    #[test]
    fn the_custom_row_is_never_marked_configured() {
        let c = cfg("tavily", vec![backend("tavily")]);
        let rows = offerable(&c, "", copy);
        let custom = rows.iter().find(|r| r.id == CUSTOM_ROW_ID).unwrap();
        assert!(!custom.configured, "the custom row is an action, not a backend");
    }

    /// A backend not in the preset table (the operator's own) must also show
    /// up in the configured list — that is the whole point of merging the two
    /// lists.
    #[test]
    fn a_custom_backend_is_listed_next_to_the_presets() {
        let c = cfg("tavily", vec![backend("tavily"), backend("my-searx")]);
        assert_eq!(
            listed(&c).iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            ["tavily", "my-searx"]
        );
    }

    #[test]
    fn subtitles_come_from_the_protocols_requirement_flags() {
        let c = cfg("", vec![]);
        let rows = offerable(&c, "", copy);
        let sub = |id: &str| {
            rows.iter().find(|r| r.id == id).unwrap().subtitle.clone()
        };
        assert_eq!(sub("tavily"), "needs-key", "tavily needs an API key");
        assert_eq!(sub("searxng"), "self-hosted", "searxng is run by the operator");
        assert_eq!(sub("duckduckgo"), "no-key", "duckduckgo needs no credential");
    }

    #[test]
    fn choosing_the_custom_row_opens_the_add_form() {
        assert_eq!(chosen_target(CUSTOM_ROW_ID), Chosen::CustomForm);
    }

    /// search does not need a `__preset__` prefix: its detail panel is the
    /// same component covering both the "configured" and "unconfigured" states
    /// (`detail_panel.rs`'s form-sync Effect falls back to the preset's
    /// base_url when `find_backend` comes up empty).
    #[test]
    fn choosing_any_backend_row_selects_it_by_name() {
        assert_eq!(chosen_target("tavily"), Chosen::Backend("tavily".into()));
        assert_eq!(chosen_target("my-searx"), Chosen::Backend("my-searx".into()));
    }
}
