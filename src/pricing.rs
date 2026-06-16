//! Per-run cost estimation against a static price table.
//!
//! Hermes-agent surfaces `estimated_cost_usd` + `cost_status` on every turn
//! summary. Aleph mirrors that signal with an opt-in module: callers supply
//! the cumulative [`TokenBreakdown`] for the run, plus the provider + model
//! identifiers; the table returns a USD figure annotated with how confident
//! the estimate is. Unknown models degrade to [`CostStatus::Unknown`] without
//! poisoning the rest of the outcome — pricing is best-effort, never a gate.
//!
//! The price table is intentionally inline (no network lookup, no config
//! file). Prices drift; we accept that and let operators upgrade Aleph to
//! pick up new entries. The alternative — pulling live rates — would import
//! a HTTP dependency for a low-signal feature, violating R3 (Core
//! Minimalism).
//!
//! # Lookup semantics
//!
//! Model matching is **prefix-based on the canonicalised model id**.
//! `canonicalize_model` lowercases the input, strips leading provider tags
//! (`anthropic/`, `openai/`, etc.) and trailing date stamps. The table is
//! scanned in declaration order and the first prefix match wins, so
//! more-specific entries (e.g. `claude-opus-4`) come before broader ones
//! (`claude`). Operators bumping prices update one numeric literal and
//! recompile — there is no runtime config knob.
//!
//! # Long-context tiers
//!
//! Some vendors charge a premium once a prompt crosses an input-token
//! threshold (Gemini 2.5 Pro and Claude Sonnet's 1M-context beta both step
//! up at 200K). [`PRICE_TABLE`] holds the base rate; the parallel
//! [`TIER_TABLE`] holds the >threshold overrides. [`estimate`] picks the
//! tier from the prompt's input size, so long-context runs are no longer
//! billed at the (≈2x cheaper) base rate. Flat-priced models skip the tier
//! lookup entirely and stay byte-identical.

use serde::{Deserialize, Serialize};

use crate::orchestrator::dispatch::TokenBreakdown;

/// Estimated USD cost for a single agent run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostEstimate {
    /// Cost in USD. `0.0` when [`status`] is `Unknown`.
    pub usd: f64,
    /// Confidence band — see [`CostStatus`].
    pub status: CostStatus,
    /// Provider identifier the table was queried with (e.g. `"anthropic"`).
    pub provider: String,
    /// Model identifier the table was queried with (e.g. `"claude-sonnet-4-6"`).
    pub model: String,
}

impl CostEstimate {
    /// Construct the "no entry found" estimate. Used by [`estimate`] when
    /// either provider or model misses the table.
    pub fn unknown(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            usd: 0.0,
            status: CostStatus::Unknown,
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Confidence band attached to every [`CostEstimate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CostStatus {
    /// All non-zero token components hit a populated rate.
    Complete,
    /// Some components (e.g. `cache_creation`) lacked a rate; the figure is a
    /// lower bound — the unknown components were billed as zero.
    PartialMissingPrice,
    /// Provider or model not in the table.
    Unknown,
}

/// Per-million-token USD rates for one model. `None` means "no rate
/// recorded" — the component is treated as zero by [`apply_rates`] and the
/// estimate status downgrades to `PartialMissingPrice`.
#[derive(Debug, Clone, Copy)]
struct Rates {
    /// Token id prefix this entry covers. Lowercased; matched
    /// prefix-style on the canonicalised model id.
    model_prefix: &'static str,
    input_per_mtok: Option<f64>,
    output_per_mtok: Option<f64>,
    /// Anthropic prompt-cache hit rate (much cheaper than input).
    cache_read_per_mtok: Option<f64>,
    /// Anthropic prompt-cache write rate (slight premium over input).
    cache_creation_per_mtok: Option<f64>,
    /// Reasoning tokens — only Gemini bills these separately. Anthropic
    /// already folds reasoning into `output`; `OpenAI` o-series likewise.
    reasoning_per_mtok: Option<f64>,
}

/// One long-context pricing tier on the **input-token axis**.
///
/// Several vendors charge a premium once a request's prompt crosses a
/// threshold (Gemini 2.5 Pro at 200K, Claude Sonnet's 1M-context beta at
/// 200K, …). The flat [`Rates`] entry encodes the base (below-threshold)
/// rate; a [`PriceTier`] overrides it once `min_input_tokens` is reached.
/// Components left `None` inherit the base [`Rates`] value, so a tier never
/// silently drops a rate the base had.
#[derive(Debug, Clone, Copy)]
struct PriceTier {
    /// Tier applies when the prompt's input-token count is `>=` this value.
    min_input_tokens: u32,
    input_per_mtok: Option<f64>,
    output_per_mtok: Option<f64>,
    cache_read_per_mtok: Option<f64>,
    cache_creation_per_mtok: Option<f64>,
    reasoning_per_mtok: Option<f64>,
}

/// Static price table. Entries are scanned in declaration order; the first
/// `model_prefix` that prefix-matches wins. Order more-specific prefixes
/// before broader catch-alls. Sources are vendor pricing pages as of
/// 2026-06; refresh by bumping the literals here and recompiling.
const PRICE_TABLE: &[(&str, &[Rates])] = &[
    (
        "anthropic",
        &[
            Rates {
                // Generation-5 flagship.
                model_prefix: "claude-fable-5",
                input_per_mtok: Some(10.0),
                output_per_mtok: Some(50.0),
                cache_read_per_mtok: Some(1.0),
                cache_creation_per_mtok: Some(12.50),
                reasoning_per_mtok: None,
            },
            // Opus 4.5+ dropped to $5/$25; the broad `claude-opus-4`
            // fallback below keeps the 4.0/4.1-era $15/$75.
            Rates {
                model_prefix: "claude-opus-4-8",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(25.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: Some(6.25),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-opus-4-7",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(25.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: Some(6.25),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-opus-4-6",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(25.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: Some(6.25),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-opus-4-5",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(25.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: Some(6.25),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-opus-4",
                input_per_mtok: Some(15.0),
                output_per_mtok: Some(75.0),
                cache_read_per_mtok: Some(1.50),
                cache_creation_per_mtok: Some(18.75),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-sonnet-4",
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.30),
                cache_creation_per_mtok: Some(3.75),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-haiku-4-5",
                input_per_mtok: Some(1.0),
                output_per_mtok: Some(5.0),
                cache_read_per_mtok: Some(0.10),
                cache_creation_per_mtok: Some(1.25),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "claude-haiku-4",
                input_per_mtok: Some(0.80),
                output_per_mtok: Some(4.0),
                cache_read_per_mtok: Some(0.08),
                cache_creation_per_mtok: Some(1.0),
                reasoning_per_mtok: None,
            },
            Rates {
                // Older 3.x rates as fallback for the family.
                model_prefix: "claude-3",
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.30),
                cache_creation_per_mtok: Some(3.75),
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        "openai",
        &[
            // GPT-5 family (openclaw catalog, 2026-03 snapshot). Dotted
            // specifics precede the broad `gpt-5` fallback.
            Rates {
                model_prefix: "gpt-5.5",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(30.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-5.4-mini",
                input_per_mtok: Some(0.75),
                output_per_mtok: Some(4.50),
                cache_read_per_mtok: Some(0.075),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-5.4-nano",
                input_per_mtok: Some(0.20),
                output_per_mtok: Some(1.25),
                cache_read_per_mtok: Some(0.02),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-5.4",
                input_per_mtok: Some(2.50),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.25),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // gpt-5.3-codex and gpt-5.3-chat-latest share one rate.
                model_prefix: "gpt-5.3",
                input_per_mtok: Some(1.75),
                output_per_mtok: Some(14.0),
                cache_read_per_mtok: Some(0.175),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-5",
                input_per_mtok: Some(2.50),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.25),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "o4-mini",
                input_per_mtok: Some(1.10),
                output_per_mtok: Some(4.40),
                cache_read_per_mtok: Some(0.28),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "o3-mini",
                input_per_mtok: Some(1.10),
                output_per_mtok: Some(4.40),
                cache_read_per_mtok: Some(0.55),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "o3-pro",
                input_per_mtok: Some(20.0),
                output_per_mtok: Some(80.0),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "o3-deep-research",
                input_per_mtok: Some(10.0),
                output_per_mtok: Some(40.0),
                cache_read_per_mtok: Some(2.50),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // o3 base was missing entirely — estimates degraded to
                // Unknown despite the capability table knowing the family.
                model_prefix: "o3",
                input_per_mtok: Some(2.0),
                output_per_mtok: Some(8.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "o1-mini",
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(12.0),
                cache_read_per_mtok: Some(1.50),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "o1",
                input_per_mtok: Some(15.0),
                output_per_mtok: Some(60.0),
                cache_read_per_mtok: Some(7.50),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-4o-mini",
                input_per_mtok: Some(0.15),
                output_per_mtok: Some(0.60),
                cache_read_per_mtok: Some(0.075),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-4o",
                input_per_mtok: Some(2.50),
                output_per_mtok: Some(10.0),
                cache_read_per_mtok: Some(1.25),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-4",
                input_per_mtok: Some(30.0),
                output_per_mtok: Some(60.0),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        "google",
        &[
            Rates {
                model_prefix: "gemini-2.5-pro",
                input_per_mtok: Some(1.25),
                output_per_mtok: Some(10.0),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(10.0),
            },
            Rates {
                model_prefix: "gemini-2.0-flash",
                input_per_mtok: Some(0.075),
                output_per_mtok: Some(0.30),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(0.30),
            },
            Rates {
                model_prefix: "gemini",
                input_per_mtok: Some(0.075),
                output_per_mtok: Some(0.30),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(0.30),
            },
        ],
    ),
    (
        "deepseek",
        &[
            Rates {
                model_prefix: "deepseek-v4-pro",
                input_per_mtok: Some(1.74),
                output_per_mtok: Some(3.48),
                cache_read_per_mtok: Some(0.145),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "deepseek-v4-flash",
                input_per_mtok: Some(0.14),
                output_per_mtok: Some(0.28),
                cache_read_per_mtok: Some(0.028),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "deepseek-reasoner",
                input_per_mtok: Some(0.55),
                output_per_mtok: Some(2.19),
                cache_read_per_mtok: Some(0.14),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // deepseek-chat (V3) and the family fallback.
                model_prefix: "deepseek",
                input_per_mtok: Some(0.27),
                output_per_mtok: Some(1.10),
                cache_read_per_mtok: Some(0.07),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        "xai",
        &[
            // Grok 4.x generations: dotted/suffixed specifics precede the
            // `grok-4` base, which precedes the grok-3-era broad fallback.
            Rates {
                model_prefix: "grok-4.3",
                input_per_mtok: Some(1.25),
                output_per_mtok: Some(2.50),
                cache_read_per_mtok: Some(0.20),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "grok-4-fast",
                input_per_mtok: Some(0.20),
                output_per_mtok: Some(0.50),
                cache_read_per_mtok: Some(0.05),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "grok",
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.75),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        "mistral",
        &[
            Rates {
                model_prefix: "mistral-large",
                input_per_mtok: Some(2.0),
                output_per_mtok: Some(6.0),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "mistral",
                input_per_mtok: Some(0.20),
                output_per_mtok: Some(0.60),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        "moonshot",
        &[
            // K2.5/K2.6 published rates differ from the legacy family
            // fallback below.
            Rates {
                model_prefix: "kimi-k2.6",
                input_per_mtok: Some(0.95),
                output_per_mtok: Some(4.0),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "kimi-k2.5",
                input_per_mtok: Some(0.38),
                output_per_mtok: Some(1.72),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "kimi",
                input_per_mtok: Some(0.60),
                output_per_mtok: Some(2.50),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "moonshot",
                input_per_mtok: Some(0.60),
                output_per_mtok: Some(2.50),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    // ── Zhipu / Z.AI ─────────────────────────────────────────────────────
    // Previously absent even though the alias layer and capability table
    // already knew the vendor — estimates silently degraded to Unknown.
    (
        "zai",
        &[
            Rates {
                model_prefix: "glm-5.1",
                input_per_mtok: Some(1.20),
                output_per_mtok: Some(4.0),
                cache_read_per_mtok: Some(0.24),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "glm-5",
                input_per_mtok: Some(1.0),
                output_per_mtok: Some(3.20),
                cache_read_per_mtok: Some(0.20),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // GLM-4.x family fallback.
                model_prefix: "glm",
                input_per_mtok: Some(0.60),
                output_per_mtok: Some(2.20),
                cache_read_per_mtok: Some(0.11),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    // ── Alibaba / Qwen (DashScope) ───────────────────────────────────────
    (
        "qwen",
        &[
            Rates {
                model_prefix: "qwen3.6-flash",
                input_per_mtok: Some(0.029),
                output_per_mtok: Some(0.287),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "qwen3.6-plus",
                input_per_mtok: Some(0.115),
                output_per_mtok: Some(0.688),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "qwen3-max",
                input_per_mtok: Some(0.359),
                output_per_mtok: Some(1.434),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    // ── MiniMax ──────────────────────────────────────────────────────────
    // M2 family flat pricing (MiniMax open-platform pricing, 2026-06). The
    // `minimax` provider preset previously had no reachable rate, so its runs
    // always reported `CostStatus::Unknown`; `canonical_provider_id("minimax")`
    // now resolves, so this section is consulted.
    (
        "minimax",
        &[Rates {
            model_prefix: "minimax-m2",
            input_per_mtok: Some(0.30),
            output_per_mtok: Some(1.20),
            cache_read_per_mtok: None,
            cache_creation_per_mtok: None,
            reasoning_per_mtok: None,
        }],
    ),
];

/// Long-context pricing tiers, parallel to [`PRICE_TABLE`] and keyed the same
/// way `(provider, model_prefix)`. Kept separate (mirroring
/// [`crate::providers::metadata`]'s sibling-table pattern) so the common
/// flat-priced models in [`PRICE_TABLE`] stay untouched — only the handful of
/// models with a published >threshold premium appear here. Tiers within an
/// entry are sorted ascending by `min_input_tokens`; the highest threshold the
/// prompt has crossed wins. Sources are vendor pricing pages as of 2026-05.
const TIER_TABLE: &[(&str, &str, &[PriceTier])] = &[
    (
        "google",
        // Gemini 2.5 Pro: prompts over 200K input tokens bill at ~2x.
        "gemini-2.5-pro",
        &[PriceTier {
            min_input_tokens: 200_000,
            input_per_mtok: Some(2.50),
            output_per_mtok: Some(15.0),
            cache_read_per_mtok: None,
            cache_creation_per_mtok: None,
            reasoning_per_mtok: Some(15.0),
        }],
    ),
    (
        "anthropic",
        // Claude Sonnet 4.x 1M-context beta: prompts over 200K input tokens
        // bill input/cache at 2x and output at 1.5x.
        "claude-sonnet-4",
        &[PriceTier {
            min_input_tokens: 200_000,
            input_per_mtok: Some(6.0),
            output_per_mtok: Some(22.50),
            cache_read_per_mtok: Some(0.60),
            cache_creation_per_mtok: Some(7.50),
            reasoning_per_mtok: None,
        }],
    ),
];

/// Lower-case the provider id and accept a few common synonyms so callers
/// can pass the raw provider name from `ProviderConfig` without tagging.
///
/// Delegates to the shared
/// [`crate::providers::model_catalog::canonical_provider_id`]; the empty
/// string preserves this module's "unknown provider" sentinel contract.
fn canonical_provider(provider: &str) -> &'static str {
    crate::providers::model_catalog::canonical_provider_id(provider).unwrap_or("")
}

/// Strip provider tags / date stamps / aliases from a model id so the
/// prefix match is stable. Lower-cases for case-insensitive lookup.
///
/// Thin alias for the shared
/// [`crate::providers::model_catalog::canonicalize_model_id`].
fn canonicalize_model(model: &str) -> String {
    crate::providers::model_catalog::canonicalize_model_id(model)
}

/// Per-million-token rate summary for the picker / catalog UI. A serialisable
/// projection of the matched [`Rates`] entry — the input/output/cache-read
/// rates a user cares about when choosing a model. `None`-valued components
/// in the table serialise as `null`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateCard {
    /// USD per million input tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_per_mtok: Option<f64>,
    /// USD per million output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_per_mtok: Option<f64>,
    /// USD per million cached-prompt-read tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_per_mtok: Option<f64>,
}

/// Resolve the [`Rates`] entry for a `(provider, model)` pair, or `None` when
/// the provider/model is not in the table. Shared by [`estimate`] and
/// [`rate_card`] so both stay on one canonicalisation + lookup path.
fn lookup_rates(provider: &str, model: &str) -> Option<&'static Rates> {
    let provider_key = canonical_provider(provider);
    if provider_key.is_empty() {
        return None;
    }
    let canonical = canonicalize_model(model);
    let entries = PRICE_TABLE.iter().find(|(p, _)| *p == provider_key)?.1;
    entries
        .iter()
        .find(|r| canonical.starts_with(r.model_prefix))
}

/// Resolve the [`PriceTier`] slice for an already-canonicalised
/// `(provider_key, model)` pair, or `None` when the model is flat-priced
/// (the common case). Same prefix-match semantics as [`lookup_rates`].
fn lookup_tiers(provider_key: &str, canonical_model: &str) -> Option<&'static [PriceTier]> {
    if provider_key.is_empty() {
        return None;
    }
    TIER_TABLE
        .iter()
        .find(|(p, prefix, _)| *p == provider_key && canonical_model.starts_with(*prefix))
        .map(|(_, _, tiers)| *tiers)
}

/// Return the rates in effect for a prompt of `prompt_tokens` input tokens:
/// the flat `base` rates with any crossed [`PriceTier`] applied on top.
///
/// Flat-priced models (no [`TIER_TABLE`] entry) and prompts below the first
/// threshold return `*base` unchanged, keeping their estimates byte-identical
/// to the pre-tier behaviour. A crossed tier overrides each component it
/// specifies; `None` components fall back to the base rate via `.or`, so a
/// tier can never downgrade a priced component to "missing".
fn effective_rates(
    provider_key: &str,
    canonical_model: &str,
    base: &Rates,
    prompt_tokens: u32,
) -> Rates {
    let Some(tiers) = lookup_tiers(provider_key, canonical_model) else {
        return *base;
    };
    let crossed = tiers
        .iter()
        .filter(|t| prompt_tokens >= t.min_input_tokens)
        .max_by_key(|t| t.min_input_tokens);
    match crossed {
        Some(t) => Rates {
            model_prefix: base.model_prefix,
            input_per_mtok: t.input_per_mtok.or(base.input_per_mtok),
            output_per_mtok: t.output_per_mtok.or(base.output_per_mtok),
            cache_read_per_mtok: t.cache_read_per_mtok.or(base.cache_read_per_mtok),
            cache_creation_per_mtok: t.cache_creation_per_mtok.or(base.cache_creation_per_mtok),
            reasoning_per_mtok: t.reasoning_per_mtok.or(base.reasoning_per_mtok),
        },
        None => *base,
    }
}

/// Return the per-million-token [`RateCard`] for a `(provider, model)` pair,
/// or `None` when the model is not priced. Powers the model picker's
/// cost-at-a-glance column (`providers.catalog`).
#[must_use]
pub fn rate_card(provider: &str, model: &str) -> Option<RateCard> {
    lookup_rates(provider, model).map(|r| RateCard {
        input_per_mtok: r.input_per_mtok,
        output_per_mtok: r.output_per_mtok,
        cache_read_per_mtok: r.cache_read_per_mtok,
    })
}

/// Apply rates to one [`TokenBreakdown`] and return the per-component cost
/// in USD along with a [`CostStatus`].
fn apply_rates(b: &TokenBreakdown, r: &Rates) -> (f64, CostStatus) {
    let mut usd = 0.0;
    let mut missing = false;
    let mut bill = |tokens: u32, rate: Option<f64>| match rate {
        Some(per_mtok) if tokens > 0 => {
            usd += (f64::from(tokens) / 1_000_000.0) * per_mtok;
        }
        None if tokens > 0 => missing = true,
        _ => {}
    };
    bill(b.input, r.input_per_mtok);
    bill(b.output, r.output_per_mtok);
    bill(b.cache_read, r.cache_read_per_mtok);
    bill(b.cache_creation, r.cache_creation_per_mtok);
    bill(b.reasoning, r.reasoning_per_mtok);
    let status = if missing {
        CostStatus::PartialMissingPrice
    } else {
        CostStatus::Complete
    };
    (usd, status)
}

/// Estimate the cost of a run given its accumulated token breakdown.
///
/// Returns `CostStatus::Unknown` when either provider or model is not in
/// the table — callers should treat that as "no estimate available".
#[must_use]
pub fn estimate(provider: &str, model: &str, breakdown: &TokenBreakdown) -> CostEstimate {
    let base = match lookup_rates(provider, model) {
        Some(r) => r,
        None => return CostEstimate::unknown(provider, model),
    };
    // Select the long-context tier (if any) from the prompt's input size —
    // the cached portions of the prompt count toward the threshold.
    let prompt_tokens = breakdown
        .input
        .saturating_add(breakdown.cache_read)
        .saturating_add(breakdown.cache_creation);
    let effective = effective_rates(
        canonical_provider(provider),
        &canonicalize_model(model),
        base,
        prompt_tokens,
    );
    let (usd, status) = apply_rates(breakdown, &effective);
    CostEstimate {
        usd,
        status,
        provider: provider.to_string(),
        model: model.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anti-drift guard: every vendor key in [`PRICE_TABLE`] / [`TIER_TABLE`]
    /// must be reachable through [`canonical_provider`]. A price section whose
    /// key no provider alias resolves to is dead — its rates can never be
    /// looked up. This replaces the references' "single vendor table can't
    /// drift" property with an explicit Rust assertion, since Aleph keeps the
    /// vendor knowledge in several parallel tables.
    #[test]
    fn every_price_table_vendor_is_resolvable() {
        for (vendor, _) in PRICE_TABLE {
            assert_eq!(
                canonical_provider(vendor),
                *vendor,
                "PRICE_TABLE vendor {vendor:?} is not a fixed point of \
                 canonical_provider — its rates are unreachable (drift)"
            );
        }
        for (vendor, _, _) in TIER_TABLE {
            assert_eq!(
                canonical_provider(vendor),
                *vendor,
                "TIER_TABLE vendor {vendor:?} is unreachable via canonical_provider"
            );
        }
    }

    #[test]
    fn minimax_pricing_reachable_after_vendor_wiring() {
        // The `minimax` preset previously priced as Unknown because
        // canonical_provider_id had no minimax branch. Now it resolves.
        let card = rate_card("minimax", "MiniMax-M2.5").expect("minimax rate card");
        assert_eq!(card.input_per_mtok, Some(0.30));
        assert_eq!(card.output_per_mtok, Some(1.20));
    }

    #[test]
    fn unknown_provider_returns_zero_usd_unknown_status() {
        let breakdown = TokenBreakdown::default();
        let est = estimate("nonexistent-provider", "nonexistent-model", &breakdown);
        assert_eq!(est.usd, 0.0);
        assert_eq!(est.status, CostStatus::Unknown);
        assert_eq!(est.provider, "nonexistent-provider");
        assert_eq!(est.model, "nonexistent-model");
    }

    #[test]
    fn unknown_model_under_known_provider_is_unknown() {
        let breakdown = TokenBreakdown {
            input: 100,
            output: 200,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-2-imaginary", &breakdown);
        assert_eq!(est.status, CostStatus::Unknown);
        assert_eq!(est.usd, 0.0);
    }

    #[test]
    fn cost_estimate_roundtrips_through_serde() {
        let est = CostEstimate {
            usd: 1.23,
            status: CostStatus::Complete,
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };
        let json = serde_json::to_string(&est).expect("serialize");
        let back: CostEstimate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(est, back);
    }

    #[test]
    fn anthropic_sonnet_complete_estimate() {
        // 1M input is well past the 200K long-context threshold, so the tier
        // rates apply: 1M input @ $6 + 1M output @ $22.50 = $28.50.
        // (See `claude_sonnet_below_threshold_uses_base_rate` for base rates.)
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 28.5).abs() < 1e-6,
            "expected $28.50 (long-context tier), got ${}",
            est.usd
        );
    }

    #[test]
    fn anthropic_cache_components_billed_separately() {
        // 1M cache_read + 1M cache_creation = a 2M-token prompt, past the
        // 200K threshold → tier cache rates: 1M @ $0.60 + 1M @ $7.50 = $8.10.
        let breakdown = TokenBreakdown {
            cache_read: 1_000_000,
            cache_creation: 1_000_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 8.10).abs() < 1e-6,
            "expected $8.10 (long-context tier), got ${}",
            est.usd
        );
    }

    #[test]
    fn openai_o1_missing_cache_creation_is_partial_when_used() {
        // o1 has no cache_creation rate — a run with non-zero
        // cache_creation tokens drops to PartialMissingPrice.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            cache_creation: 100,
            ..Default::default()
        };
        let est = estimate("openai", "o1", &breakdown);
        assert_eq!(est.status, CostStatus::PartialMissingPrice);
        // Input still billed: 1M * $15 = $15.00
        assert!(
            (est.usd - 15.0).abs() < 1e-6,
            "expected $15.00, got ${}",
            est.usd
        );
    }

    #[test]
    fn openai_o1_missing_cache_creation_is_complete_when_unused() {
        // Same model, zero cache_creation usage — Complete since the
        // missing rate was not actually consulted.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        let est = estimate("openai", "o1", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
    }

    #[test]
    fn gemini_reasoning_tokens_billed_at_output_rate() {
        // 1M reasoning tokens @ $0.30 (gemini-2.0-flash) = $0.30
        let breakdown = TokenBreakdown {
            reasoning: 1_000_000,
            ..Default::default()
        };
        let est = estimate("google", "gemini-2.0-flash", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 0.30).abs() < 1e-6,
            "expected $0.30, got ${}",
            est.usd
        );
    }

    #[test]
    fn canonical_model_strips_provider_tag_and_date() {
        // 8-digit YYYYMMDD trailing stamp IS removed.
        assert_eq!(
            canonicalize_model("anthropic/claude-sonnet-4-6-20250520"),
            "claude-sonnet-4-6"
        );
        // 8-digit date stamp removed even without provider tag.
        assert_eq!(canonicalize_model("gpt-4o-20241120"), "gpt-4o");
        // A dash-separated date with non-8-digit tail is left intact —
        // we don't try to parse arbitrary date formats.
        assert_eq!(
            canonicalize_model("openai/gpt-4o-2024-11-20"),
            "gpt-4o-2024-11-20"
        );
    }

    #[test]
    fn provider_synonyms_resolve() {
        // Synonym handling — "claude" stand-in for anthropic etc.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        let claude = estimate("claude", "claude-sonnet-4-6", &breakdown);
        let anth = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        assert_eq!(claude.status, anth.status);
        assert!((claude.usd - anth.usd).abs() < 1e-9);
    }

    #[test]
    fn deepseek_chat_is_priced() {
        // 1M input @ $0.27 + 1M output @ $1.10 = $1.37.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("deepseek", "deepseek-chat", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 1.37).abs() < 1e-6,
            "expected $1.37, got ${}",
            est.usd
        );
    }

    #[test]
    fn deepseek_reasoner_more_specific_prefix_wins() {
        // 1M output @ $2.19 (reasoner) not $1.10 (chat fallback).
        let breakdown = TokenBreakdown {
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("deepseek", "deepseek-reasoner", &breakdown);
        assert!(
            (est.usd - 2.19).abs() < 1e-6,
            "expected $2.19, got ${}",
            est.usd
        );
    }

    #[test]
    fn xai_grok_priced_via_inferred_provider_synonym() {
        // Provider passed as "grok" resolves to the xai table entry.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        let est = estimate("grok", "grok-4", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 3.0).abs() < 1e-6,
            "expected $3.00, got ${}",
            est.usd
        );
    }

    #[test]
    fn rate_card_returns_per_mtok_summary() {
        let card = rate_card("anthropic", "claude-sonnet-4-6").expect("priced");
        assert_eq!(card.input_per_mtok, Some(3.0));
        assert_eq!(card.output_per_mtok, Some(15.0));
        assert_eq!(card.cache_read_per_mtok, Some(0.30));
    }

    #[test]
    fn rate_card_unknown_model_is_none() {
        assert!(rate_card("anthropic", "claude-imaginary-99").is_none());
        assert!(rate_card("nonexistent", "whatever").is_none());
    }

    #[test]
    fn gemini_pro_below_threshold_uses_base_rate() {
        // 100K input < 200K tier threshold → base $1.25.
        let breakdown = TokenBreakdown {
            input: 100_000,
            ..Default::default()
        };
        let est = estimate("google", "gemini-2.5-pro", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        // 100K * $1.25/M = $0.125
        assert!(
            (est.usd - 0.125).abs() < 1e-6,
            "expected $0.125 (base), got ${}",
            est.usd
        );
    }

    #[test]
    fn gemini_pro_above_threshold_uses_tier_rate() {
        // 250K input >= 200K threshold → tier input $2.50, output $15.
        let breakdown = TokenBreakdown {
            input: 250_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("google", "gemini-2.5-pro", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        // 250K * $2.50/M + 1M * $15/M = 0.625 + 15.0 = 15.625
        assert!(
            (est.usd - 15.625).abs() < 1e-6,
            "expected $15.625 (tier), got ${}",
            est.usd
        );
    }

    #[test]
    fn gemini_pro_reasoning_follows_tier() {
        // Above threshold, reasoning tokens bill at the tier rate ($15), not
        // the base ($10).
        let breakdown = TokenBreakdown {
            input: 300_000,
            reasoning: 1_000_000,
            ..Default::default()
        };
        let est = estimate("google", "gemini-2.5-pro", &breakdown);
        // 300K * $2.50/M + 1M * $15/M = 0.75 + 15.0 = 15.75
        assert!(
            (est.usd - 15.75).abs() < 1e-6,
            "expected $15.75 (tier reasoning), got ${}",
            est.usd
        );
    }

    #[test]
    fn claude_sonnet_long_context_tier_doubles_input() {
        // 200K input (exactly at threshold) + 1M output → tier rates:
        // input $6, output $22.50.
        let breakdown = TokenBreakdown {
            input: 200_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        // 200K * $6/M + 1M * $22.50/M = 1.2 + 22.5 = 23.7
        assert!(
            (est.usd - 23.7).abs() < 1e-6,
            "expected $23.70 (long-context tier), got ${}",
            est.usd
        );
    }

    #[test]
    fn claude_sonnet_cached_prompt_counts_toward_threshold() {
        // Prompt size = input + cache_read + cache_creation. A prompt that is
        // 50K fresh + 160K cached (210K total) crosses the 200K threshold even
        // though `input` alone is below it.
        let breakdown = TokenBreakdown {
            input: 50_000,
            cache_read: 160_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        // input 50K * $6/M + cache_read 160K * $0.60/M = 0.3 + 0.096 = 0.396
        assert!(
            (est.usd - 0.396).abs() < 1e-6,
            "expected $0.396 (tier via cached prompt), got ${}",
            est.usd
        );
    }

    #[test]
    fn claude_sonnet_below_threshold_uses_base_rate() {
        // Below the 200K threshold the long-context tier must not perturb the
        // estimate — base rates ($3 input / $15 output) apply.
        let breakdown = TokenBreakdown {
            input: 100_000,
            output: 100_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        // 100K * $3/M + 100K * $15/M = 0.3 + 1.5 = 1.8
        assert!(
            (est.usd - 1.8).abs() < 1e-6,
            "expected $1.80 (base), got ${}",
            est.usd
        );
    }

    #[test]
    fn flat_priced_model_ignores_tier_table() {
        // Opus has no tier entry — a huge prompt still uses flat rates.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-opus-4-1", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        // 1M * $15/M + 1M * $75/M = 90.0 (flat opus rates, unchanged)
        assert!(
            (est.usd - 90.0).abs() < 1e-6,
            "expected $90.00 (flat opus), got ${}",
            est.usd
        );
    }

    #[test]
    fn current_generation_anthropic_priced() {
        // Fable 5: 1M in @ $10 + 1M out @ $50 = $60.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let fable = estimate("anthropic", "claude-fable-5", &breakdown);
        assert_eq!(fable.status, CostStatus::Complete);
        assert!(
            (fable.usd - 60.0).abs() < 1e-6,
            "expected $60.00, got ${}",
            fable.usd
        );

        // Opus 4.8 dropped to $5/$25; the 4.0/4.1-era fallback stays $15/$75
        // (covered by flat_priced_model_ignores_tier_table).
        let opus48 = estimate("anthropic", "claude-opus-4-8", &breakdown);
        assert!(
            (opus48.usd - 30.0).abs() < 1e-6,
            "expected $30.00, got ${}",
            opus48.usd
        );

        let haiku = rate_card("anthropic", "claude-haiku-4-5-20251001").expect("priced");
        assert_eq!(haiku.input_per_mtok, Some(1.0));
        assert_eq!(haiku.output_per_mtok, Some(5.0));
        assert_eq!(haiku.cache_read_per_mtok, Some(0.10));
    }

    #[test]
    fn gpt5_family_and_o_series_priced() {
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        // gpt-5.5: $5 + $30 = $35.
        let gpt55 = estimate("openai", "gpt-5.5", &breakdown);
        assert_eq!(gpt55.status, CostStatus::Complete);
        assert!(
            (gpt55.usd - 35.0).abs() < 1e-6,
            "expected $35.00, got ${}",
            gpt55.usd
        );

        // Specific dotted prefixes win over the broad gpt-5 fallback.
        let nano = rate_card("openai", "gpt-5.4-nano").expect("priced");
        assert_eq!(nano.input_per_mtok, Some(0.20));
        let broad = rate_card("openai", "gpt-5-experimental").expect("priced via fallback");
        assert_eq!(broad.input_per_mtok, Some(2.50));

        // o3 base was previously Unknown; o3-mini keeps its own rate.
        let o3 = estimate(
            "openai",
            "o3",
            &TokenBreakdown {
                input: 1_000_000,
                ..Default::default()
            },
        );
        assert_eq!(o3.status, CostStatus::Complete);
        assert!(
            (o3.usd - 2.0).abs() < 1e-6,
            "expected $2.00, got ${}",
            o3.usd
        );
        assert_eq!(
            rate_card("openai", "o3-mini").unwrap().input_per_mtok,
            Some(1.10)
        );
        assert_eq!(
            rate_card("openai", "o4-mini").unwrap().cache_read_per_mtok,
            Some(0.28)
        );
    }

    #[test]
    fn zai_and_qwen_providers_priced() {
        // Zhipu aliases resolve through canonical_provider_id ("zhipu"/"zai"
        // /"glm") — previously these all returned Unknown.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        let glm51 = estimate("zhipu", "glm-5.1", &breakdown);
        assert_eq!(glm51.status, CostStatus::Complete);
        assert!(
            (glm51.usd - 1.20).abs() < 1e-6,
            "expected $1.20, got ${}",
            glm51.usd
        );
        // GLM-4.x falls back to the family rate.
        let glm46 = estimate(
            "zai",
            "glm-4.6",
            &TokenBreakdown {
                output: 1_000_000,
                ..Default::default()
            },
        );
        assert!(
            (glm46.usd - 2.20).abs() < 1e-6,
            "expected $2.20, got ${}",
            glm46.usd
        );

        // DashScope provider alias resolves to the qwen table.
        let qwen = estimate("dashscope", "qwen3-max", &breakdown);
        assert_eq!(qwen.status, CostStatus::Complete);
        assert!(
            (qwen.usd - 0.359).abs() < 1e-6,
            "expected $0.359, got ${}",
            qwen.usd
        );
    }

    #[test]
    fn grok4x_deepseek_v4_kimi_k2_priced() {
        let input_1m = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        // grok-4.3 has the new cheap rate; legacy grok-4 keeps $3 via the
        // broad fallback (covered by the synonym test above).
        let grok43 = estimate("xai", "grok-4.3", &input_1m);
        assert!(
            (grok43.usd - 1.25).abs() < 1e-6,
            "expected $1.25, got ${}",
            grok43.usd
        );
        assert_eq!(
            rate_card("xai", "grok-4-fast").unwrap().input_per_mtok,
            Some(0.20)
        );

        // deepseek-v4-flash: $0.14 + $0.28 = $0.42.
        let ds = estimate(
            "deepseek",
            "deepseek-v4-flash",
            &TokenBreakdown {
                input: 1_000_000,
                output: 1_000_000,
                ..Default::default()
            },
        );
        assert_eq!(ds.status, CostStatus::Complete);
        assert!(
            (ds.usd - 0.42).abs() < 1e-6,
            "expected $0.42, got ${}",
            ds.usd
        );

        let k26 = estimate("moonshot", "kimi-k2.6", &input_1m);
        assert!(
            (k26.usd - 0.95).abs() < 1e-6,
            "expected $0.95, got ${}",
            k26.usd
        );
    }

    #[test]
    fn lookup_tiers_only_matches_tiered_models() {
        assert!(lookup_tiers("google", "gemini-2.5-pro").is_some());
        assert!(lookup_tiers("anthropic", "claude-sonnet-4-6").is_some());
        // Flat-priced families resolve to no tier.
        assert!(lookup_tiers("anthropic", "claude-opus-4-1").is_none());
        assert!(lookup_tiers("openai", "gpt-4o").is_none());
        // Empty provider key never matches.
        assert!(lookup_tiers("", "gemini-2.5-pro").is_none());
    }
}
