//! Model lifecycle: is this id still one the vendor will serve?
//!
//! The capability / price tables answer "how big is its window" and "what
//! does it cost"; neither can say "the vendor retired this three days ago".
//! Aleph had no answer at all, and the cost of that showed up in its own
//! record: `deepseek-chat` / `deepseek-reasoner` were retired on 2026-07-24
//! and the only available remedy was hand-editing the preset default. An
//! operator who had pinned the old id in `[providers.deepseek] models`, or an
//! LLM that called `select_model { model: "deepseek-chat" }` (the tool's own
//! doc comment still offered it as an example), got an opaque provider 400
//! with nothing pointing at the successor.
//!
//! Both references model this explicitly — opencode carries
//! `status: alpha | beta | deprecated | active` on every catalog model and
//! excludes deprecated ids from `model.available()`; kimi-cli drops retired
//! ids on each `/models` refresh and re-points the default. This module is the
//! Rust mapping: a **sibling table** to [`capabilities`](super::capabilities)
//! (the same pattern `pricing`'s `TIER_TABLE` uses beside `PRICE_TABLE`) that
//! holds only the models whose state is *not* the `Active` default, so the
//! common case stays a single miss and the capability literals are untouched.
//!
//! R7 stance: this is data. Nothing here silently reroutes a request. It makes
//! the retirement *visible* — to the picker, to `list_models`, to
//! `select_model`'s refusal message, and (most usefully) to the drift guard in
//! [`super::drift_tests`], which fails the build if a preset ever ships a deprecated
//! id as its default.

use serde::Serialize;

use super::alias::canonicalize_model_id;

/// Vendor lifecycle state of a model family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// Generally available. The default for every id absent from
    /// [`LIFECYCLE_TABLE`] that does not look like a preview id.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelLifecycle {
    pub status: ModelStatus,
    /// Model id the vendor points at instead. Only ever set for
    /// [`ModelStatus::Deprecated`]; it is what `select_model` quotes back and
    /// what an operator should put in their config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor: Option<&'static str>,
    /// One-line human note (retirement date, preview caveat).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
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
}

/// Suffixes that vendors use to mark an id as not-yet-stable. Matched on the
/// canonicalised id, so `gemini-3.1-pro-preview` and `…-preview-11-2025`
/// (date-stamped) both land on [`ModelStatus::Preview`]. This is a *lexical*
/// fact about vendor naming — the same class as stripping a trailing date
/// stamp — not an inference about the model.
const PREVIEW_MARKERS: &[&str] = &[
    "-preview",
    "-exp",
    "-experimental",
    "-beta",
    "-alpha",
    "-rc",
];

/// Models whose state differs from [`ModelLifecycle::ACTIVE`], keyed by the
/// same canonicalised-prefix convention as `CAPABILITY_TABLE` / `PRICE_TABLE`:
/// scanned in declaration order, first prefix match wins, so specific prefixes
/// precede broad ones.
///
/// **Only non-Active rows belong here.** An `Active` row would be pure noise —
/// the lookup already defaults to it.
const LIFECYCLE_TABLE: &[(&str, ModelLifecycle)] = &[
    // DeepSeek retired the two original chat/reasoner aliases on 2026-07-24 in
    // favour of the V4 split. Aleph's presets moved the same day; these rows
    // exist for configs and conversations that still name the old ids.
    (
        "deepseek-reasoner",
        ModelLifecycle {
            status: ModelStatus::Deprecated,
            successor: Some("deepseek-v4-pro"),
            note: Some("retired 2026-07-24; DeepSeek now splits reasoning into V4 Pro"),
        },
    ),
    (
        "deepseek-chat",
        ModelLifecycle {
            status: ModelStatus::Deprecated,
            successor: Some("deepseek-v4-flash"),
            note: Some("retired 2026-07-24; DeepSeek now serves V4 Flash"),
        },
    ),
];

/// Lifecycle state for a model id (raw or canonical).
///
/// Resolution order: the explicit [`LIFECYCLE_TABLE`] wins, then a vendor
/// preview marker in the id, then [`ModelLifecycle::ACTIVE`]. Never returns
/// `None` — "we have nothing recorded" and "generally available" are the same
/// answer for every consumer, and an `Option` would only push that collapse
/// out to five call sites.
#[must_use]
pub fn lifecycle_for(model: &str) -> ModelLifecycle {
    let canon = canonicalize_model_id(model);
    if let Some((_, life)) = LIFECYCLE_TABLE
        .iter()
        .find(|(prefix, _)| canon.starts_with(prefix))
    {
        return *life;
    }
    if PREVIEW_MARKERS.iter().any(|m| canon.ends_with(m)) {
        return ModelLifecycle {
            status: ModelStatus::Preview,
            successor: None,
            note: Some("vendor-labelled preview: id and behaviour may change without notice"),
        };
    }
    ModelLifecycle::ACTIVE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ids_are_active() {
        assert_eq!(lifecycle_for("claude-sonnet-5"), ModelLifecycle::ACTIVE);
        assert_eq!(
            lifecycle_for("some-private-relay-model"),
            ModelLifecycle::ACTIVE
        );
    }

    #[test]
    fn retired_deepseek_aliases_name_their_successor() {
        let chat = lifecycle_for("deepseek-chat");
        assert!(chat.is_deprecated());
        assert_eq!(chat.successor, Some("deepseek-v4-flash"));

        let reasoner = lifecycle_for("deepseek-reasoner");
        assert!(reasoner.is_deprecated());
        assert_eq!(reasoner.successor, Some("deepseek-v4-pro"));

        // The V4 ids that replaced them are not caught by the `deepseek-chat`
        // prefix — the successors must stay usable.
        assert!(!lifecycle_for("deepseek-v4-flash").is_deprecated());
        assert!(!lifecycle_for("deepseek-v4-pro").is_deprecated());
    }

    #[test]
    fn preview_markers_are_derived_from_the_id() {
        assert_eq!(
            lifecycle_for("gemini-3.1-pro-preview").status,
            ModelStatus::Preview
        );
        assert_eq!(
            lifecycle_for("gemini-3-flash-preview").status,
            ModelStatus::Preview
        );
        // Date-stamped preview ids canonicalise before the marker check.
        assert_eq!(
            lifecycle_for("gemini-3-flash-preview-20260114").status,
            ModelStatus::Preview
        );
        // A stable id must not be dragged in by a substring.
        assert_eq!(lifecycle_for("gpt-5.6").status, ModelStatus::Active);
    }

    #[test]
    fn lookup_accepts_vendor_tagged_and_hosted_ids() {
        // Canonicalisation runs first, so an aggregator-qualified retired id
        // is still recognised.
        assert!(lifecycle_for("deepseek/deepseek-chat").is_deprecated());
        assert!(lifecycle_for("deepseek-ai/DeepSeek-Chat").is_deprecated());
    }
}
