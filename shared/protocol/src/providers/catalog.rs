//! Reference data attached to a provider/model row: capabilities, price,
//! lifecycle, and provenance.
//!
//! # Why these live in the protocol crate
//!
//! They are the four curated reference tables' **wire projection**. The tables
//! themselves stay in `alephcore` (`providers::model_catalog::{capabilities,
//! lifecycle}`, `pricing`) — this module owns only the shapes that cross the
//! socket, and `alephcore` re-exports them so the table literals and the wire
//! can never describe different structs.
//!
//! Before this module the Panel hand-copied `ModelLifecycle` and simply did not
//! declare `capabilities` / `cost` at all, so both were serialised by the
//! server on every `providers.catalog` call and silently dropped by serde on
//! arrival — a context window and a price the user was already paying to
//! transmit, and could not see.
//!
//! Declaring them only got them as far as the client. They still reached no
//! screen, and the reason was where they were attached: [`CatalogEntry`] is one
//! row per *provider*, so a window and a price there described `default_model` —
//! one member of the very list the row exists to let you choose from. The only
//! honest label would have been "the price of a model you are not picking", so
//! three rounds of pickers rendered neither. They are on [`RosterModel`] now,
//! one per offerable id, resolved through the same single join point as every
//! other piece of reference data.
//!
//! [`CatalogEntry`]: super::wire::CatalogEntry

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// What a model family can do.
///
/// `Copy` and const-constructible on purpose: `alephcore`'s capability table is
/// a `const` array of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Maximum total context window in tokens (input + output budget).
    pub context_window: u32,
    /// Maximum output tokens the model will emit in one response.
    pub max_output_tokens: u32,
    /// Accepts image input (multimodal vision).
    pub supports_vision: bool,
    /// Supports native tool / function calling.
    pub supports_tools: bool,
    /// Has an extended-thinking / reasoning mode.
    pub supports_reasoning: bool,
}

/// How a [`RateCard`]'s numbers were resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateBasis {
    /// The provider id itself resolves to a vendor that prices this model —
    /// the rate is the vendor's published price for its own endpoint.
    Direct,
    /// The provider id has no price section; rates were taken from the vendor
    /// named by the *model* id. Reseller margin is not modelled, so treat the
    /// figure as a floor rather than a quote.
    VendorInferred,
}

/// Per-million-token rate summary for the picker / catalog UI.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateCard {
    /// USD per million input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_mtok: Option<f64>,
    /// USD per million output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_mtok: Option<f64>,
    /// USD per million cached-prompt-read tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_per_mtok: Option<f64>,
    /// USD per million cached-prompt-write tokens (prompt-cache creation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_per_mtok: Option<f64>,
    /// USD per million reasoning tokens, when the vendor bills them separately
    /// (Gemini). `None` when reasoning is folded into the output rate — so a
    /// `None` here is not "free".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_per_mtok: Option<f64>,
    /// How these rates were resolved.
    pub basis: RateBasis,
}

impl ModelCapabilities {
    /// The context window as a short human string: `1M`, `200K`, `8192`.
    ///
    /// # Why the contract owns the wording
    ///
    /// Same reason [`ModelStatus::as_str`] does. Three pickers are about to
    /// print this number, in a terminal column, a table cell and an HTML span —
    /// three media, but one number, and "is 131072 shown as 128K or 131K"
    /// is not a per-medium decision. Placement stays with each face (R4);
    /// the value does not.
    #[must_use]
    pub fn context_window_short(&self) -> String {
        let w = self.context_window;
        if w >= 1_000_000 && w.is_multiple_of(1_000_000) {
            format!("{}M", w / 1_000_000)
        } else if w >= 1_000 {
            // Round to the nearest K rather than truncating: 131_072 is sold as
            // a 128K window, and printing 131K invents a number no vendor uses.
            format!("{}K", (w + 512) / 1024)
        } else {
            w.to_string()
        }
    }
}

impl RateCard {
    /// Input and output USD per million tokens as one short string — `$3/$15`.
    ///
    /// `None` when neither rate is recorded, and a lone `—` in the half that is
    /// missing: an absent rate means *unpriced*, never free, and a renderer
    /// that prints `$0` for it is stating a price the catalogue never claimed.
    ///
    /// A [`RateBasis::VendorInferred`] card is prefixed `~`, because that card
    /// is the *vendor's* price for a model a reseller is hosting — a floor, not
    /// a quote. Dropping the marker here would drop it from every face at once,
    /// which is how a hint becomes a claim.
    #[must_use]
    pub fn io_per_mtok_short(&self) -> Option<String> {
        if self.input_per_mtok.is_none() && self.output_per_mtok.is_none() {
            return None;
        }
        let cell = |v: Option<f64>| v.map_or_else(|| "\u{2014}".to_string(), format_usd);
        let prefix = match self.basis {
            RateBasis::Direct => "",
            RateBasis::VendorInferred => "~",
        };
        Some(format!(
            "{prefix}{}/{}",
            cell(self.input_per_mtok),
            cell(self.output_per_mtok)
        ))
    }
}

/// USD with trailing zeros trimmed: `$3`, `$0.15`, `$1.25`.
fn format_usd(value: f64) -> String {
    let rendered = format!("{value:.2}");
    let trimmed = rendered.trim_end_matches('0').trim_end_matches('.');
    format!("${trimmed}")
}

/// Vendor lifecycle state of a model family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// Generally available.
    #[default]
    Active,
    /// Vendor-labelled preview / experimental / beta. Usable, but the id and
    /// its behaviour can change or disappear without a deprecation window.
    Preview,
    /// Retired, or announced for retirement. Requests may already be failing.
    Deprecated,
}

impl ModelStatus {
    /// Stable wire string for RPC / tool JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Preview => "preview",
            Self::Deprecated => "deprecated",
        }
    }
}

/// Lifecycle facts for one model family.
///
/// `successor` / `note` are `Cow` rather than `String` so `alephcore`'s
/// `LIFECYCLE_TABLE` can keep building these in a `const` array from string
/// literals, while a client deserialising the same JSON gets owned data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelLifecycle {
    #[serde(default)]
    pub status: ModelStatus,
    /// Model id the vendor points at instead. Only ever set for
    /// [`ModelStatus::Deprecated`]; it is what `select_model` quotes back and
    /// what an operator should put in their config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<Cow<'static, str>>,
    /// One-line human note (retirement date, preview caveat).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<Cow<'static, str>>,
}

impl ModelLifecycle {
    /// The default state: generally available, nothing to say about it.
    pub const ACTIVE: Self = Self {
        status: ModelStatus::Active,
        successor: None,
        note: None,
    };

    /// True when the id should not be handed to a provider any more.
    #[must_use]
    pub const fn is_deprecated(&self) -> bool {
        matches!(self.status, ModelStatus::Deprecated)
    }

    /// True when the vendor labels it preview / experimental.
    #[must_use]
    pub const fn is_preview(&self) -> bool {
        matches!(self.status, ModelStatus::Preview)
    }
}

/// Where a model id in a roster came from.
///
/// The operator deserves to know whether a row is something they configured or
/// something Aleph suggested, and a picker offering a raw id scraped off a live
/// `/models` endpoint should say so — those ids carry no curated window or
/// price, which is exactly when a blank capability column is honest rather than
/// broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// The provider preset's `default_model`.
    PresetDefault,
    /// One of the preset's curated `fallback_models`.
    PresetFallback,
    /// The preset's cheap `default_aux_model`.
    PresetAux,
    /// Listed by the operator in `[providers.<id>] models`.
    Configured,
    /// Returned by the provider's live `/models` endpoint.
    Discovered,
}

impl ModelSource {
    /// Stable wire string for RPC / tool JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PresetDefault => "preset_default",
            Self::PresetFallback => "preset_fallback",
            Self::PresetAux => "preset_aux",
            Self::Configured => "configured",
            Self::Discovered => "discovered",
        }
    }
}

/// One model as the provider's own `/models` endpoint reported it.
///
/// Discovery contributes **ids**; windows, prices and lifecycles stay in the
/// curated tables. The two extra fields here are the provider's own words, kept
/// because they are free and because a picker showing a raw scraped id with no
/// context window at all is less useful than one that can quote the vendor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// The id to send on the wire.
    pub id: String,
    /// Vendor-supplied label, when the listing carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Context window the provider advertises. Only some listings report it;
    /// `None` means "the provider did not say", not "small".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

/// One offerable model in a provider's roster.
///
/// # Why this is a struct and not a bare `String`
///
/// The roster used to be `Vec<String>`. The moment a list of records is
/// projected down to a list of scalars, every field beside the key disappears
/// for every renderer at once — and the loss happens in the producer, so each
/// renderer looks individually correct. Provenance and retirement are exactly
/// what a picker needs to avoid offering an id that now 400s, or to explain why
/// a freshly discovered id shows no context window.
///
/// # Why the window and the price are *here* and not on the entry
///
/// They used to be on [`CatalogEntry`][crate::providers::CatalogEntry], where
/// they described one model — `default_model` — on a row whose entire job is
/// letting you pick a *different* one. That is why three rounds shipped them on
/// the wire and no face ever rendered them: the only honest label would have
/// been "the window of a model you are not choosing". Per-row they are the two
/// facts that decide the pick, so they belong on the row being picked, resolved
/// through the same single join point (`ModelRecord::resolve`) as every other
/// piece of reference data.
///
/// `None` on either is *not* "zero": it is "no curated row for this family",
/// which is the normal state for an id scraped off a live `/models` endpoint.
/// A renderer must leave the cell blank rather than print `0`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RosterModel {
    /// The model id to send on the wire.
    pub id: String,
    /// Where this id came from.
    pub source: ModelSource,
    /// Vendor lifecycle for this id. Defaults to active.
    #[serde(default)]
    pub lifecycle: ModelLifecycle,
    /// What this id can do. `None` when the family is not in the curated
    /// capability table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    /// Per-million-token rates for this id. `None` when unpriced — which is
    /// not the same as free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<RateCard>,
}

impl RosterModel {
    /// Convenience constructor for an active row with no curated reference
    /// data. Callers that *have* the reference data assign the fields.
    #[must_use]
    pub fn new(id: impl Into<String>, source: ModelSource) -> Self {
        Self {
            id: id.into(),
            source,
            lifecycle: ModelLifecycle::ACTIVE,
            capabilities: None,
            cost: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn caps(context_window: u32) -> ModelCapabilities {
        ModelCapabilities {
            context_window,
            max_output_tokens: 4096,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        }
    }

    const fn card(input: Option<f64>, output: Option<f64>, basis: RateBasis) -> RateCard {
        RateCard {
            input_per_mtok: input,
            output_per_mtok: output,
            cache_read_per_mtok: None,
            cache_creation_per_mtok: None,
            reasoning_per_mtok: None,
            basis,
        }
    }

    #[test]
    fn a_window_is_named_the_way_the_vendor_sells_it() {
        // 131_072 is sold as a 128K window. Truncating to thousands would print
        // 131K, a number no vendor's docs contain.
        assert_eq!(caps(131_072).context_window_short(), "128K");
        assert_eq!(caps(200_000).context_window_short(), "195K");
        assert_eq!(caps(1_000_000).context_window_short(), "1M");
        assert_eq!(caps(8_192).context_window_short(), "8K");
        assert_eq!(caps(512).context_window_short(), "512");
    }

    #[test]
    fn trailing_zeros_do_not_survive() {
        assert_eq!(
            card(Some(3.0), Some(15.0), RateBasis::Direct)
                .io_per_mtok_short()
                .unwrap(),
            "$3/$15"
        );
        assert_eq!(
            card(Some(0.15), Some(1.25), RateBasis::Direct)
                .io_per_mtok_short()
                .unwrap(),
            "$0.15/$1.25"
        );
    }

    /// The rule the whole type exists to protect: absent is not zero.
    #[test]
    fn an_unrecorded_rate_never_renders_as_free() {
        let none = card(None, None, RateBasis::Direct);
        assert!(
            none.io_per_mtok_short().is_none(),
            "a card with no rates must decline to render, not print $0"
        );

        let half = card(Some(3.0), None, RateBasis::Direct)
            .io_per_mtok_short()
            .unwrap();
        assert_eq!(half, "$3/\u{2014}");
        assert!(
            !half.contains("$0"),
            "the missing half must read as unknown, not as free"
        );
    }

    /// A reseller's price is the vendor's, so it is a floor. The marker has to
    /// live here or it lives in none of the three faces.
    #[test]
    fn an_inferred_card_says_so() {
        assert_eq!(
            card(Some(3.0), Some(15.0), RateBasis::VendorInferred)
                .io_per_mtok_short()
                .unwrap(),
            "~$3/$15"
        );
    }

    #[test]
    fn a_bare_roster_row_carries_no_reference_data() {
        // `None` here is "no curated row", which is the normal state for an id
        // scraped off a live `/models` endpoint — so the constructor must not
        // invent zeroes for it.
        let m = RosterModel::new("some-relay-model", ModelSource::Discovered);
        assert!(m.capabilities.is_none());
        assert!(m.cost.is_none());
    }
}
