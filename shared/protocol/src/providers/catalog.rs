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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterModel {
    /// The model id to send on the wire.
    pub id: String,
    /// Where this id came from.
    pub source: ModelSource,
    /// Vendor lifecycle for this id. Defaults to active.
    #[serde(default)]
    pub lifecycle: ModelLifecycle,
}

impl RosterModel {
    /// Convenience constructor for an active row.
    #[must_use]
    pub fn new(id: impl Into<String>, source: ModelSource) -> Self {
        Self {
            id: id.into(),
            source,
            lifecycle: ModelLifecycle::ACTIVE,
        }
    }
}
