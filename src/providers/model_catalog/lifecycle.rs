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
//!
//! # Retirement has a scope, and it is not always global
//!
//! The table shipped with two rows for a year, and the reason it stayed that
//! small was not that vendors stopped retiring things — it was that half the
//! retirements Aleph knew about were **not expressible**. `llama-3.3-70b-versatile`
//! is retired *on Groq* and served happily by Together, Cerebras and DeepInfra;
//! `deepseek-v3` is gone from DeepSeek's own API and still hosted everywhere the
//! open weights are. A model-id-only table forces a choice between recording a
//! true fact plus a false one (global row ⇒ `select_model` refuses an id that
//! works) and recording nothing (⇒ the guard stays inert). Neither is the answer.
//!
//! So a row carries an optional `provider` scope. `None` is the vendor's own
//! word — the model is gone wherever it is served. `Some(preset_id)` is one
//! host's word about its own catalog. Scoped rows are consulted first, so a host
//! can retire something ahead of its vendor without the two rows racing.
//!
//! The rule used when adding rows: **if the vendor's own catalog says
//! deprecated, the row is global; if only a reseller's catalog says so, the row
//! is scoped to that reseller.** That is why `minimax-m2.7` is absent despite
//! three resellers marking it deprecated — MiniMax's own docs still list it as a
//! current tier, and a global row would have made `select_model` refuse the
//! `minimax` preset's own aux model.

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

/// One row of [`LIFECYCLE_TABLE`].
#[derive(Debug, Clone, Copy)]
struct LifecycleRow {
    /// Preset id this row speaks for, or `None` when it is the vendor's own
    /// word and therefore true wherever the model is served.
    ///
    /// Matched **exactly** (case-insensitively) against the provider the caller
    /// supplies — no alias walking, no substring. The
    /// `lifecycle_scopes_name_a_real_preset` drift guard makes that safe by
    /// failing the build on a scope that names nothing.
    provider: Option<&'static str>,
    /// Canonicalised model-id prefix, same convention as `CAPABILITY_TABLE` /
    /// `PRICE_TABLE`: first match wins within a scope class, so specific
    /// prefixes precede broad ones.
    prefix: &'static str,
    life: ModelLifecycle,
}

/// Shorthand for the overwhelmingly common row shape.
const fn retired(
    provider: Option<&'static str>,
    prefix: &'static str,
    successor: &'static str,
    note: &'static str,
) -> LifecycleRow {
    LifecycleRow {
        provider,
        prefix,
        life: ModelLifecycle {
            status: ModelStatus::Deprecated,
            successor: Some(successor),
            note: Some(note),
        },
    }
}

/// Models whose state differs from [`ModelLifecycle::ACTIVE`].
///
/// **Only non-Active rows belong here.** An `Active` row would be pure noise —
/// the lookup already defaults to it.
///
/// Sources are the vendor / host model catalogs cross-read during the 2026-08
/// refresh round; each row names the successor the catalog itself points at, so
/// a refusal message is always actionable.
const LIFECYCLE_TABLE: &[LifecycleRow] = &[
    // ── Vendor-wide (the vendor's own catalog marks the id deprecated) ──────
    //
    // DeepSeek retired the two original chat/reasoner aliases on 2026-07-24 in
    // favour of the V4 split. Aleph's presets moved the same day; these rows
    // exist for configs and conversations that still name the old ids.
    retired(
        None,
        "deepseek-reasoner",
        "deepseek-v4-pro",
        "retired 2026-07-24; DeepSeek now splits reasoning into V4 Pro",
    ),
    retired(
        None,
        "deepseek-chat",
        "deepseek-v4-flash",
        "retired 2026-07-24; DeepSeek now serves V4 Flash",
    ),
    // Anthropic's catalog carries Opus 4.8 as superseded by Opus 5. Aleph
    // shipped 4.8 in the `claude` preset's fallback chain — a retry that would
    // have been spent on a dead id.
    retired(
        None,
        "claude-opus-4-8",
        "claude-opus-5",
        "superseded by Claude Opus 5",
    ),
    // OpenAI: the 5.5 line (and its Pro tier) is replaced by 5.6. `-pro` must
    // precede the broad `gpt-5.5` row or it would never be reached.
    retired(None, "gpt-5.5-pro", "gpt-5.6", "superseded by GPT-5.6"),
    retired(None, "gpt-5.5", "gpt-5.6", "superseded by GPT-5.6"),
    // Cohere folded the three specialised Command A variants into one flagship.
    retired(
        None,
        "command-a-reasoning",
        "command-a-plus-05-2026",
        "folded into Command A Plus",
    ),
    retired(
        None,
        "command-a-vision",
        "command-a-plus-05-2026",
        "folded into Command A Plus",
    ),
    // Ordered after the two specialised rows: `command-a-` is their prefix too.
    retired(
        None,
        "command-a-03-2025",
        "command-a-plus-05-2026",
        "superseded by Command A Plus",
    ),
    // Z.ai's own catalog marks GLM-5.1 deprecated in favour of 5.2.
    retired(None, "glm-5.1", "glm-5.2", "superseded by GLM-5.2"),
    // Moonshot's roster moved past K2.5; five independent hosts (DeepInfra,
    // NVIDIA, Chutes, Venice, Ollama Cloud) mark it deprecated. K2.6 is still
    // actively served by most of them and stays Active.
    retired(None, "kimi-k2.5", "kimi-k2.6", "superseded by Kimi K2.6"),
    // Mistral consolidated the medium tier on 3.5.
    retired(
        None,
        "mistral-medium-2508",
        "mistral-medium-3-5",
        "superseded by Mistral Medium 3.5",
    ),
    retired(
        None,
        "devstral-medium",
        "mistral-medium-3-5",
        "superseded by Mistral Medium 3.5",
    ),
    // Google publishes shutdown dates. `gemini-3-pro-preview` must precede
    // nothing here (3.1 does not share its prefix), but the 2.0 / 1.5 rows are
    // family-wide and deliberately broad.
    retired(
        None,
        "gemini-3-pro-preview",
        "gemini-3.1-pro-preview",
        "Google shut down Gemini 3 Pro Preview on 2026-03-09",
    ),
    retired(
        None,
        "gemini-2.0",
        "gemini-2.5-flash",
        "Google shut down the Gemini 2.0 line on 2026-06-01",
    ),
    retired(
        None,
        "gemini-1.5",
        "gemini-2.5-flash",
        "Google shut down the Gemini 1.5 line on 2025-09-29",
    ),
    // ── Host-scoped (only this host's catalog retired it) ───────────────────
    //
    // Groq replaced both Llama tiers with the gpt-oss pair. The same Llama ids
    // are current on Together / Cerebras / DeepInfra / Hyperbolic, which is
    // exactly why these rows are scoped rather than global.
    retired(
        Some("groq"),
        "llama-3.3-70b-versatile",
        "openai/gpt-oss-120b",
        "Groq replaced its Llama tiers with gpt-oss",
    ),
    retired(
        Some("groq"),
        "llama-3.1-8b-instant",
        "openai/gpt-oss-20b",
        "Groq replaced its Llama tiers with gpt-oss",
    ),
    // DeepSeek's first-party API serves only the V4 split; the V3 open weights
    // remain available from open-weight hosts, so this is scoped to the vendor
    // endpoint and NOT to the hosts that still serve V3.
    retired(
        Some("deepseek"),
        "deepseek-v3",
        "deepseek-v4-pro",
        "DeepSeek's own API serves only the V4 split since 2026-07-24",
    ),
    // GitHub Copilot rotates its roster on its own schedule.
    retired(
        Some("github-copilot"),
        "gpt-5.4-mini",
        "gpt-5.6-luna",
        "rotated out of the Copilot roster",
    ),
    retired(
        Some("github-copilot"),
        "gpt-5.4",
        "gpt-5.6-terra",
        "rotated out of the Copilot roster",
    ),
    retired(
        Some("github-copilot"),
        "gemini-3.5-flash",
        "gemini-3.6-flash",
        "rotated out of the Copilot roster",
    ),
    retired(
        Some("github-copilot"),
        "gemini-2.5-pro",
        "gemini-3.1-pro-preview",
        "rotated out of the Copilot roster",
    ),
    retired(
        Some("github-copilot"),
        "gpt-4o",
        "gpt-5.6-sol",
        "rotated out of the Copilot roster",
    ),
];

/// Lifecycle state for a model id (raw or canonical), as served by `provider`.
///
/// `provider` is the preset / config id the model would be requested from.
/// `None` means the caller has no provider context — only vendor-wide rows
/// apply, because "retired on Groq" says nothing about an unqualified id.
///
/// Resolution order: host-scoped [`LIFECYCLE_TABLE`] rows for this provider,
/// then vendor-wide rows, then a preview marker in the id, then
/// [`ModelLifecycle::ACTIVE`]. Never returns `None` — "we have nothing
/// recorded" and "generally available" are the same answer for every consumer,
/// and an `Option` would only push that collapse out to five call sites.
#[must_use]
pub fn lifecycle_for(provider: Option<&str>, model: &str) -> ModelLifecycle {
    let canon = canonicalize_model_id(model);
    // A host's word about its own catalog outranks the vendor's, so a provider
    // that retires something early is not shadowed by a broader global row.
    let scoped = provider
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .and_then(|p| {
            LIFECYCLE_TABLE.iter().find(|row| {
                row.provider
                    .is_some_and(|scope| scope.eq_ignore_ascii_case(p))
                    && canon.starts_with(row.prefix)
            })
        });
    if let Some(row) = scoped {
        return row.life;
    }
    if let Some(row) = LIFECYCLE_TABLE
        .iter()
        .find(|row| row.provider.is_none() && canon.starts_with(row.prefix))
    {
        return row.life;
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

/// Every distinct host scope named by [`LIFECYCLE_TABLE`]. Consumed only by the
/// drift guard that checks each one still names a real preset — a scope that
/// names nothing is a row that can never fire, and the exact-match rule above
/// makes that failure silent.
#[cfg(test)]
pub(super) fn declared_provider_scopes() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = LIFECYCLE_TABLE.iter().filter_map(|r| r.provider).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ids_are_active() {
        assert_eq!(
            lifecycle_for(None, "claude-sonnet-5"),
            ModelLifecycle::ACTIVE
        );
        assert_eq!(
            lifecycle_for(None, "some-private-relay-model"),
            ModelLifecycle::ACTIVE
        );
    }

    #[test]
    fn retired_deepseek_aliases_name_their_successor() {
        let chat = lifecycle_for(None, "deepseek-chat");
        assert!(chat.is_deprecated());
        assert_eq!(chat.successor, Some("deepseek-v4-flash"));

        let reasoner = lifecycle_for(None, "deepseek-reasoner");
        assert!(reasoner.is_deprecated());
        assert_eq!(reasoner.successor, Some("deepseek-v4-pro"));

        // The V4 ids that replaced them are not caught by the `deepseek-chat`
        // prefix — the successors must stay usable.
        assert!(!lifecycle_for(None, "deepseek-v4-flash").is_deprecated());
        assert!(!lifecycle_for(None, "deepseek-v4-pro").is_deprecated());
    }

    #[test]
    fn preview_markers_are_derived_from_the_id() {
        assert_eq!(
            lifecycle_for(None, "gemini-3.1-pro-preview").status,
            ModelStatus::Preview
        );
        assert_eq!(
            lifecycle_for(None, "gemini-3-flash-preview").status,
            ModelStatus::Preview
        );
        // Date-stamped preview ids canonicalise before the marker check.
        assert_eq!(
            lifecycle_for(None, "gemini-3-flash-preview-20260114").status,
            ModelStatus::Preview
        );
        // A stable id must not be dragged in by a substring.
        assert_eq!(lifecycle_for(None, "gpt-5.6").status, ModelStatus::Active);
    }

    #[test]
    fn lookup_accepts_vendor_tagged_and_hosted_ids() {
        // Canonicalisation runs first, so an aggregator-qualified retired id
        // is still recognised.
        assert!(lifecycle_for(None, "deepseek/deepseek-chat").is_deprecated());
        assert!(lifecycle_for(None, "deepseek-ai/DeepSeek-Chat").is_deprecated());
    }

    #[test]
    fn host_scoped_retirement_does_not_leak_to_other_hosts() {
        // The whole reason the scope exists: Groq dropped both Llama tiers,
        // Together / Cerebras / DeepInfra still serve them.
        let on_groq = lifecycle_for(Some("groq"), "llama-3.3-70b-versatile");
        assert!(on_groq.is_deprecated());
        assert_eq!(on_groq.successor, Some("openai/gpt-oss-120b"));

        assert!(!lifecycle_for(Some("together"), "llama-3.3-70b-versatile").is_deprecated());
        assert!(!lifecycle_for(Some("cerebras"), "llama-3.3-70b").is_deprecated());
        // No provider context ⇒ vendor-wide rows only. Refusing an id we cannot
        // attribute to the retiring host would be a false refusal.
        assert!(!lifecycle_for(None, "llama-3.3-70b-versatile").is_deprecated());
    }

    #[test]
    fn vendor_wide_rows_apply_to_every_host() {
        for provider in [None, Some("anthropic"), Some("openrouter"), Some("groq")] {
            assert!(
                lifecycle_for(provider, "claude-opus-4-8").is_deprecated(),
                "opus-4-8 must read as retired from {provider:?}"
            );
        }
        // …and the successor is usable everywhere.
        assert!(!lifecycle_for(Some("anthropic"), "claude-opus-5").is_deprecated());
    }

    #[test]
    fn host_scope_outranks_a_broader_vendor_row() {
        // Copilot retired plain `gpt-4o`; nobody else has, so the same id off
        // Copilot stays active. This also pins the scoped-before-global order.
        assert!(lifecycle_for(Some("github-copilot"), "gpt-4o").is_deprecated());
        assert!(!lifecycle_for(Some("openai"), "gpt-4o").is_deprecated());
    }

    #[test]
    fn deepseek_v3_is_retired_on_the_vendor_endpoint_only() {
        // DeepSeek's own API dropped V3; the open weights are still hosted, and
        // a global row here would refuse ids that work on those hosts.
        assert!(lifecycle_for(Some("deepseek"), "deepseek-v3").is_deprecated());
        assert!(!lifecycle_for(Some("chutes"), "deepseek-ai/DeepSeek-V3-0324").is_deprecated());
        assert!(!lifecycle_for(Some("siliconflow"), "deepseek-ai/DeepSeek-V3").is_deprecated());
    }

    #[test]
    fn reseller_opinion_alone_does_not_retire_a_vendor_model() {
        // Three resellers mark MiniMax M2.7 deprecated; MiniMax still lists it.
        // A global row would have made `select_model` refuse the `minimax`
        // preset's own aux model.
        assert!(!lifecycle_for(Some("minimax"), "MiniMax-M2.7").is_deprecated());
        assert!(!lifecycle_for(None, "MiniMax-M2.7").is_deprecated());
    }

    #[test]
    fn specialised_command_a_rows_precede_the_dated_flagship_row() {
        // `command-a-` is a prefix of all three; declaration order decides.
        assert_eq!(
            lifecycle_for(None, "command-a-reasoning-08-2025").successor,
            Some("command-a-plus-05-2026")
        );
        assert_eq!(
            lifecycle_for(None, "command-a-03-2025").successor,
            Some("command-a-plus-05-2026")
        );
        // The successor itself must not be caught by any of them.
        assert!(!lifecycle_for(None, "command-a-plus-05-2026").is_deprecated());
    }
}
