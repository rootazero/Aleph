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
//! (`anthropic/`, `openai/`, etc.), collapses host paths and trailing date
//! stamps. The table is scanned in declaration order and the first prefix
//! match wins, so more-specific entries (e.g. `claude-opus-4`) come before
//! broader ones (`claude`). Operators bumping prices update one numeric
//! literal and recompile — there is no runtime config knob.
//!
//! The table is sectioned by **vendor**, but models are routinely served by
//! somebody other than their vendor. [`lookup_rates`] therefore tries the
//! provider id first and falls back to the vendor named by the *model* id,
//! tagging which one answered via [`RateBasis`]. Without that fallback every
//! aggregator, cloud reseller and private relay priced as `Unknown` — and
//! `cost_aware` routing sorts unpriced cloud candidates last, so the cheapest
//! tier on offer was reliably ranked worst.
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
use crate::providers::model_catalog::prefix_matches;

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
    /// Which table the rates came from — see [`RateBasis`]. `None` when
    /// [`status`] is `Unknown` (no rates were found at all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<RateBasis>,
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
            basis: None,
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
/// 2026-07; refresh by bumping the literals here and recompiling.
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
            Rates {
                // Mythos 5 shares Fable 5's rate card.
                model_prefix: "claude-mythos-5",
                input_per_mtok: Some(10.0),
                output_per_mtok: Some(50.0),
                cache_read_per_mtok: Some(1.0),
                cache_creation_per_mtok: Some(12.50),
                reasoning_per_mtok: None,
            },
            Rates {
                // Opus 5 — supersedes the (now retired) Opus 4.8 at the same
                // $5/$25 the 4.5+ line moved to.
                model_prefix: "claude-opus-5",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(25.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: Some(6.25),
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
                // Sonnet 5 (current default). Durable rate $3/$15 (a launch
                // promo of $2/$10 runs to 2026-08-31; we keep the durable rate
                // so estimates don't under-report once it ends).
                model_prefix: "claude-sonnet-5",
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.30),
                cache_creation_per_mtok: Some(3.75),
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
            // GPT-5 family (openclaw catalog). Dotted specifics precede the
            // broad `gpt-5` fallback. 5.6 is the current default.
            //
            // The 5.6 tiers are NOT flat-rated: Terra is half the flagship and
            // Luna a fifth of it. Capabilities can share one `gpt-5.6` row
            // because the shape is identical; rates cannot, and these two must
            // precede `gpt-5.6` or they would silently bill at 5x.
            Rates {
                model_prefix: "gpt-5.6-terra",
                input_per_mtok: Some(2.50),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.25),
                cache_creation_per_mtok: Some(3.125),
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "gpt-5.6-luna",
                input_per_mtok: Some(1.0),
                output_per_mtok: Some(6.0),
                cache_read_per_mtok: Some(0.10),
                cache_creation_per_mtok: Some(1.25),
                reasoning_per_mtok: None,
            },
            Rates {
                // Covers plain `gpt-5.6` and the `-sol` tier, which share it.
                model_prefix: "gpt-5.6",
                input_per_mtok: Some(5.0),
                output_per_mtok: Some(30.0),
                cache_read_per_mtok: Some(0.50),
                cache_creation_per_mtok: Some(6.25),
                reasoning_per_mtok: None,
            },
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
                // Gemini 3.1 Pro (current default). $2/$12 for ≤200K input; the
                // >200K step-up ($4/$18) now lives in `TIER_TABLE`. Must
                // precede broad `gemini` (flash).
                model_prefix: "gemini-3.1-pro",
                input_per_mtok: Some(2.0),
                output_per_mtok: Some(12.0),
                cache_read_per_mtok: Some(0.20),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(12.0),
            },
            // The Gemini 3.x **flash** tiers are ~20x the 2.0-flash rate the
            // broad `gemini` row below carries. Without these rows every 3.x
            // flash id — including the `gemini` preset's own aux model — priced
            // at $0.075/$0.30, which is not a rounding error: it is the figure
            // `cost_aware` sorts on and the one a run's cost estimate reports.
            // `-lite` must precede `-flash`, being a longer prefix of neither.
            Rates {
                model_prefix: "gemini-3.6-flash",
                input_per_mtok: Some(1.50),
                output_per_mtok: Some(7.50),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(7.50),
            },
            Rates {
                model_prefix: "gemini-3.5-flash-lite",
                input_per_mtok: Some(0.30),
                output_per_mtok: Some(2.50),
                cache_read_per_mtok: Some(0.03),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(2.50),
            },
            Rates {
                model_prefix: "gemini-3.5-flash",
                input_per_mtok: Some(1.50),
                output_per_mtok: Some(9.0),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(9.0),
            },
            Rates {
                // `gemini-3-flash-preview` (the aux model): Google has not
                // published a rate for the preview id separately, so the 3.x
                // flash tier rate stands in. That errs *high* rather than 20x
                // low, which is the safe direction for both a cost estimate and
                // for `cost_aware` (it can never rank an expensive model first).
                model_prefix: "gemini-3-flash",
                input_per_mtok: Some(1.50),
                output_per_mtok: Some(7.50),
                cache_read_per_mtok: Some(0.15),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: Some(7.50),
            },
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
        // V4 rates from the official pricing page (RMB → USD @ ~7.2). The
        // legacy deepseek-chat / deepseek-reasoner aliases retire 2026-07-24
        // and now bill at v4-flash rates. Prior figures were stale V3 and
        // mis-scaled (v4-pro was ~4x too high, v4-flash cache_read 10x).
        &[
            Rates {
                // ¥3 in / ¥6 out / ¥0.025 cache-hit.
                model_prefix: "deepseek-v4-pro",
                input_per_mtok: Some(0.435),
                output_per_mtok: Some(0.87),
                cache_read_per_mtok: Some(0.003_625),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // ¥1 in / ¥2 out / ¥0.02 cache-hit.
                model_prefix: "deepseek-v4-flash",
                input_per_mtok: Some(0.14),
                output_per_mtok: Some(0.28),
                cache_read_per_mtok: Some(0.0028),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // Thinking-mode alias of v4-flash; billed at v4-flash rates.
                model_prefix: "deepseek-reasoner",
                input_per_mtok: Some(0.14),
                output_per_mtok: Some(0.28),
                cache_read_per_mtok: Some(0.0028),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // Broad fallback; deepseek-chat is now the v4-flash
                // non-thinking alias → same v4-flash rates.
                model_prefix: "deepseek",
                input_per_mtok: Some(0.14),
                output_per_mtok: Some(0.28),
                cache_read_per_mtok: Some(0.0028),
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
                // grok-3-mini is the cheap tier, but it used to inherit the
                // broad `grok` row below — the *flagship* grok-3 rate — which
                // priced it ~5x its own primary. The cross-table drift guard
                // (`aux_model_is_not_pricier_than_the_default`) is what
                // surfaced it: xAI's preset named grok-3-mini as its cheap aux
                // model while the table billed it at $18/Mtok blended against
                // grok-4.3's $3.75. xAI's published mini rate is $0.30/$0.50.
                model_prefix: "grok-3-mini",
                input_per_mtok: Some(0.30),
                output_per_mtok: Some(0.50),
                cache_read_per_mtok: Some(0.075),
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
            // K3 flagship, published USD rates (platform.kimi.ai
            // /docs/pricing/chat-k3): $3 in / $15 out, $0.30 on a cache hit.
            // Must precede the broad `kimi` row, which would price it 5x low.
            //
            // No `TIER_TABLE` row on purpose: unlike Gemini 2.5/3.1 Pro and
            // Claude Sonnet, K3 publishes *no* long-context premium above
            // 200K — the rate is flat across the full 1,048,576-token window.
            //
            // Only the *open platform* meters tokens. The Kimi Code
            // subscription ids are excluded by `QUOTA_BILLED_MODELS` — and
            // they have to be excluded by name, because three of them
            // (`kimi-for-coding*`, `kimi-code`) would otherwise be caught by
            // the broad `kimi` row below.
            Rates {
                model_prefix: "kimi-k3",
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(15.0),
                cache_read_per_mtok: Some(0.30),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            // K2.5/K2.6/K2.7 published USD rates (platform.kimi.ai) differ from
            // the legacy family fallback below. K2.7-code shares K2.6's tier.
            Rates {
                model_prefix: "kimi-k2.7",
                input_per_mtok: Some(0.95),
                output_per_mtok: Some(4.0),
                cache_read_per_mtok: Some(0.19),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "kimi-k2.6",
                input_per_mtok: Some(0.95),
                output_per_mtok: Some(4.0),
                cache_read_per_mtok: Some(0.16),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "kimi-k2.5",
                input_per_mtok: Some(0.60),
                output_per_mtok: Some(3.0),
                cache_read_per_mtok: Some(0.10),
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
                // GLM-5.2 flagship (z.ai USD). Must precede glm-5 / glm-5-turbo.
                model_prefix: "glm-5.2",
                input_per_mtok: Some(1.40),
                output_per_mtok: Some(4.40),
                cache_read_per_mtok: Some(0.26),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // Correction: old row carried GLM-5-Turbo's rate; GLM-5.1 = 1.4/4.4.
                model_prefix: "glm-5.1",
                input_per_mtok: Some(1.40),
                output_per_mtok: Some(4.40),
                cache_read_per_mtok: Some(0.26),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // Precede broad glm-5 so Turbo isn't under-priced at the 5 rate.
                model_prefix: "glm-5-turbo",
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
        &[
            Rates {
                // M3 (current default) — without this row the default id
                // reports CostStatus::Unknown. Must precede minimax-m2.
                model_prefix: "minimax-m3",
                input_per_mtok: Some(0.60),
                output_per_mtok: Some(2.40),
                cache_read_per_mtok: Some(0.12),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "minimax-m2",
                input_per_mtok: Some(0.30),
                output_per_mtok: Some(1.20),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    // ── Baidu ERNIE (Qianfan) ────────────────────────────────────────────
    // Qianfan publishes in RMB; these are its own USD figures. ERNIE 5.0 (the
    // multimodal reasoning tier) costs more than 5.1, so 5.1 must precede it
    // only for readability — the prefixes do not overlap.
    (
        "baidu",
        &[
            Rates {
                model_prefix: "ernie-5.1",
                input_per_mtok: Some(0.591),
                output_per_mtok: Some(2.658),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "ernie-5.0",
                input_per_mtok: Some(0.886),
                output_per_mtok: Some(3.544),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    // ── Volcengine Doubao (Ark) ──────────────────────────────────────────
    // Previously ABSENT: `canonical_provider_id` had no doubao branch, so every
    // doubao run reported CostStatus::Unknown. Now wired (alias.rs) + priced.
    // RMB → USD @ ~7.2; best-effort (Doubao tiers shift often).
    (
        "doubao",
        &[
            Rates {
                // Seed 2.1 Pro and Seed Evolving share the top tier — 2x the
                // Turbo rate the family row below carries, so both need to
                // precede it.
                model_prefix: "doubao-seed-2-1-pro",
                input_per_mtok: Some(0.885),
                output_per_mtok: Some(4.427),
                cache_read_per_mtok: Some(0.177),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "doubao-seed-evolving",
                input_per_mtok: Some(0.885),
                output_per_mtok: Some(4.427),
                cache_read_per_mtok: Some(0.177),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // Doubao-Seed Turbo tier — the published Seed 2.1 Turbo rate,
                // and the family default for other Seed ids.
                model_prefix: "doubao-seed",
                input_per_mtok: Some(0.443),
                output_per_mtok: Some(2.214),
                cache_read_per_mtok: Some(0.0885),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                // Legacy non-Seed fallback (doubao-1.5-pro-256k etc.).
                model_prefix: "doubao",
                input_per_mtok: Some(0.11),
                output_per_mtok: Some(0.28),
                cache_read_per_mtok: None,
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        // StepFun Step-3.x Flash line. Source: openclaw `stepfun` plugin
        // catalog (2026-08). `cacheWrite: 0` there means "not billed
        // separately", recorded as None — not as free.
        "stepfun",
        &[
            Rates {
                model_prefix: "step-3.7-flash",
                input_per_mtok: Some(0.2),
                output_per_mtok: Some(1.15),
                cache_read_per_mtok: Some(0.04),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "step-3.5-flash",
                input_per_mtok: Some(0.1),
                output_per_mtok: Some(0.3),
                cache_read_per_mtok: Some(0.02),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        // Xiaomi MiMo v2.5. Source: openclaw `xiaomi` plugin catalog
        // (2026-08). The `-pro` row must precede the family row — `mimo-v2.5`
        // is a prefix of `mimo-v2.5-pro` and would otherwise underprice the
        // flagship 3x (the prefix-shadow guard enforces the order).
        "xiaomi",
        &[
            Rates {
                model_prefix: "mimo-v2.5-pro",
                input_per_mtok: Some(0.435),
                output_per_mtok: Some(0.87),
                cache_read_per_mtok: Some(0.0036),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
            Rates {
                model_prefix: "mimo-v2.5",
                input_per_mtok: Some(0.14),
                output_per_mtok: Some(0.28),
                cache_read_per_mtok: Some(0.0028),
                cache_creation_per_mtok: None,
                reasoning_per_mtok: None,
            },
        ],
    ),
    (
        // Meituan LongCat. Source: openclaw `longcat` plugin catalog
        // (2026-08). Cache write is billed at the input rate.
        "meituan",
        &[Rates {
            model_prefix: "longcat-2.0",
            input_per_mtok: Some(0.75),
            output_per_mtok: Some(2.95),
            cache_read_per_mtok: Some(0.015),
            cache_creation_per_mtok: Some(0.75),
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
        // Gemini 3.1 Pro (current `gemini` preset default, 1M window): the base
        // row's own comment recorded the >200K step-up ($4/$18) as "left to the
        // base rate — best-effort". That note predates nothing: it just never
        // got a tier row, so every long-context run on the *current default*
        // was billed at half price. Must precede `gemini-2.5-pro` only in
        // spirit (distinct prefixes); order here is newest-first for reading.
        "gemini-3.1-pro",
        &[PriceTier {
            min_input_tokens: 200_000,
            input_per_mtok: Some(4.0),
            output_per_mtok: Some(18.0),
            cache_read_per_mtok: None,
            cache_creation_per_mtok: None,
            reasoning_per_mtok: Some(18.0),
        }],
    ),
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
        // Sonnet 5 (current `claude` preset default) carries the 1M window, so
        // it takes the same >200K long-context premium the 4.x 1M beta did —
        // identical base rate ($3/$15), identical published multipliers (input
        // and cache 2x, output 1.5x). Without this row the flagship default was
        // the one model whose long-context runs were *never* tiered.
        //
        // Deliberately absent: `claude-opus-4-6/7/8` and `claude-fable-5` also
        // carry 1M windows, but their >200K rates are not published as a
        // multiple we can confirm. Extrapolating Sonnet's 2x/1.5x onto them
        // would be invented data; they stay flat-priced (a documented
        // under-estimate) until a vendor figure is in hand.
        "claude-sonnet-5",
        &[PriceTier {
            min_input_tokens: 200_000,
            input_per_mtok: Some(6.0),
            output_per_mtok: Some(22.50),
            cache_read_per_mtok: Some(0.60),
            cache_creation_per_mtok: Some(7.50),
            reasoning_per_mtok: None,
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

// `RateBasis` and `RateCard` live in `aleph_protocol::providers::catalog`: the
// Panel picker, the CLI and `list_models` all render them, and two of those
// crates cannot depend on `alephcore`. `RateCard` stays what it was — a
// serialisable projection of the matched `Rates` entry.
pub use aleph_protocol::providers::{RateBasis, RateCard};

/// A resolved price-table hit: the rates, the vendor section they came from
/// (needed for the parallel [`TIER_TABLE`] lookup) and how we got there.
#[derive(Debug, Clone, Copy)]
struct ResolvedRates {
    rates: &'static Rates,
    vendor_key: &'static str,
    basis: RateBasis,
}

/// Model ids whose endpoint bills a **plan quota**, not tokens.
///
/// [`PRICE_TABLE`] can say "this id costs $X"; it cannot say "this id has no
/// per-token price at all", and the difference is the whole point of this
/// list. Every entry is served only by Kimi Code (`api.kimi.com/coding`),
/// whose docs quote a *consumption multiplier* against a subscription — there
/// is no USD/Mtok figure to record, and inventing one is worse than
/// [`CostStatus::Unknown`].
///
/// `k3` / `k3-256k` / `k2p5` were already unpriced, but only because no prefix
/// happened to match them. `kimi-for-coding`, `kimi-for-coding-highspeed` and
/// `kimi-code` start with `kimi`, so they landed on the open platform's K2-era
/// family row and were quoted at $0.60/$2.50 — a rate the subscriber is not
/// being charged, on an endpoint that does not meter tokens.
///
/// Matched **exactly** against the canonicalised id, so it can never shadow an
/// open-platform model: `kimi-k3` and `kimi-k2.6` are genuinely per-token and
/// stay priced.
///
/// Deliberate consequence: these ids report [`CostStatus::Unknown`], which the
/// failover layer maps to the `u64::MAX` "unpriced cloud" sentinel, so
/// `cost_aware` ranks them last rather than ahead of a confirmable price. That
/// is the honest ordering for a cost we cannot quote, and it already applied to
/// this endpoint's default model (`k3`) before the list existed.
const QUOTA_BILLED_MODELS: &[&str] = &[
    "k3",
    "k3-256k",
    "kimi-for-coding",
    "kimi-for-coding-highspeed",
    "kimi-code",
    "k2p5",
];

/// Find the first [`Rates`] row in `vendor`'s section that prefix-matches an
/// already-canonicalised model id.
fn rates_in(vendor: &str, canonical_model: &str) -> Option<&'static Rates> {
    PRICE_TABLE
        .iter()
        .find(|(p, _)| *p == vendor)?
        .1
        .iter()
        .find(|r| prefix_matches(canonical_model, r.model_prefix))
}

/// Resolve the [`Rates`] entry for a `(provider, model)` pair, or `None` when
/// neither the provider nor the model names a vendor that prices it. Shared by
/// [`estimate`] and [`rate_card`] so both stay on one canonicalisation +
/// lookup path.
///
/// Two passes, in confidence order (see [`RateBasis`]):
/// 1. **Direct** — the provider id canonicalises to a vendor whose section
///    prices this model. Unchanged from the original behaviour.
/// 2. **Vendor-inferred** — otherwise, the *model* id names its vendor
///    ([`infer_vendor`]). This is what makes aggregators, Bedrock and private
///    relays priceable instead of permanently `Unknown`.
///
/// [`QUOTA_BILLED_MODELS`] short-circuits ahead of both, because the second
/// pass is the one that would otherwise re-price them: those ids name Kimi as
/// their vendor no matter which provider id is asked about.
fn lookup_rates(provider: &str, model: &str) -> Option<ResolvedRates> {
    let canonical = canonicalize_model(model);
    if QUOTA_BILLED_MODELS.contains(&canonical.as_str()) {
        return None;
    }

    let provider_key = canonical_provider(provider);
    if !provider_key.is_empty() {
        if let Some(rates) = rates_in(provider_key, &canonical) {
            return Some(ResolvedRates {
                rates,
                vendor_key: provider_key,
                basis: RateBasis::Direct,
            });
        }
    }

    let vendor = crate::providers::model_catalog::infer_vendor(model)?;
    let rates = rates_in(vendor, &canonical)?;
    Some(ResolvedRates {
        rates,
        vendor_key: vendor,
        basis: RateBasis::VendorInferred,
    })
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
        .find(|(p, prefix, _)| *p == provider_key && prefix_matches(canonical_model, prefix))
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
    lookup_rates(provider, model).map(|resolved| {
        let r = resolved.rates;
        RateCard {
            input_per_mtok: r.input_per_mtok,
            output_per_mtok: r.output_per_mtok,
            cache_read_per_mtok: r.cache_read_per_mtok,
            cache_creation_per_mtok: r.cache_creation_per_mtok,
            reasoning_per_mtok: r.reasoning_per_mtok,
            basis: resolved.basis,
        }
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
    let resolved = match lookup_rates(provider, model) {
        Some(r) => r,
        None => return CostEstimate::unknown(provider, model),
    };
    // Select the long-context tier (if any) from the prompt's input size —
    // the cached portions of the prompt count toward the threshold. Tiers are
    // keyed by the vendor section the rates actually came from, so a
    // vendor-inferred hit picks up that vendor's long-context premium too.
    let prompt_tokens = breakdown
        .input
        .saturating_add(breakdown.cache_read)
        .saturating_add(breakdown.cache_creation);
    let effective = effective_rates(
        resolved.vendor_key,
        &canonicalize_model(model),
        resolved.rates,
        prompt_tokens,
    );
    let (usd, status) = apply_rates(breakdown, &effective);
    CostEstimate {
        usd,
        status,
        provider: provider.to_string(),
        model: model.to_string(),
        basis: Some(resolved.basis),
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

    /// Prefix-shadow guard. Lookup is "first declaration whose prefix matches",
    /// so a broad row placed above a specific one silently kills it — the
    /// specific row still compiles, still reads correctly, and is simply never
    /// reached. Every `claude-opus-4-*` rate would vanish behind a stray
    /// `claude` row, and nothing but a hand-computed estimate would notice.
    ///
    /// Uses [`prefix_matches`] — the lookup's own predicate — so the guard also
    /// sees shadowing that only exists once separators are folded.
    #[test]
    fn no_price_row_is_shadowed_by_an_earlier_broader_prefix() {
        for (vendor, rows) in PRICE_TABLE {
            for (i, later) in rows.iter().enumerate() {
                for earlier in &rows[..i] {
                    assert!(
                        !prefix_matches(later.model_prefix, earlier.model_prefix),
                        "{vendor}: {:?} is unreachable — the earlier {:?} row \
                         already prefix-matches it. Move the specific row up.",
                        later.model_prefix,
                        earlier.model_prefix
                    );
                }
            }
        }
    }

    /// Rate rows are written in the vendor's own spelling of a generation
    /// separator, but a host may publish the other one — Copilot serves
    /// Anthropic models as `claude-opus-4.8`. The dotted id used to fall past
    /// the `claude-opus-4-8` row onto the broader `claude-opus-4` rate and bill
    /// every run on it at the wrong number, with nothing to notice but a
    /// hand-computed estimate.
    #[test]
    fn generation_separator_spelling_reaches_the_same_rate_row() {
        for (a, b) in [
            ("claude-opus-4.8", "claude-opus-4-8"),
            ("claude-haiku-4.5", "claude-haiku-4-5"),
        ] {
            assert_eq!(
                rate_card("anthropic", a),
                rate_card("anthropic", b),
                "{a} and {b} name the same model"
            );
        }
        // Mirror image, through the tier table as well as the rate table: the
        // dashed spelling of a dotted row must reach it.
        assert_eq!(
            rate_card("openai", "gpt-5-6"),
            rate_card("openai", "gpt-5.6")
        );
        // `PriceTier` is not `PartialEq` (nothing in production compares two),
        // so identity of the resolved static slice is the assertion.
        let dashed = lookup_tiers("google", &canonicalize_model("gemini-2-5-pro"));
        let dotted = lookup_tiers("google", &canonicalize_model("gemini-2.5-pro"));
        assert!(dotted.is_some(), "the dotted spelling is the table's own");
        assert!(
            matches!((dashed, dotted), (Some(a), Some(b)) if std::ptr::eq(a, b)),
            "gemini-2-5-pro missed the long-context tier its dotted twin gets"
        );
    }

    /// Same hazard on the tier axis, where the entries are `(vendor, prefix)`
    /// pairs in one flat list.
    #[test]
    fn no_tier_row_is_shadowed_by_an_earlier_broader_prefix() {
        for (i, (vendor, prefix, _)) in TIER_TABLE.iter().enumerate() {
            for (earlier_vendor, earlier_prefix, _) in &TIER_TABLE[..i] {
                if earlier_vendor != vendor {
                    continue;
                }
                assert!(
                    !prefix_matches(prefix, earlier_prefix),
                    "{vendor}: tier {prefix:?} is unreachable behind {earlier_prefix:?}"
                );
            }
        }
    }

    /// The long-context tier must apply to whatever the *current* flagships
    /// are, not to whichever generation happened to be current when the tier
    /// was written. `claude-sonnet-4` and `gemini-2.5-pro` had tiers while the
    /// 1M-window defaults that replaced them did not, so every long-context run
    /// on the current defaults billed at roughly half price.
    #[test]
    fn current_long_context_defaults_are_tiered() {
        // Input-only breakdowns: mixing output tokens in would let the output
        // rate dominate the short case and mask the input tier entirely.
        let long_prompt = TokenBreakdown {
            input: 400_000,
            ..Default::default()
        };
        let short_prompt = TokenBreakdown {
            input: 100_000,
            ..Default::default()
        };
        for (provider, model) in [
            ("anthropic", "claude-sonnet-5"),
            ("google", "gemini-3.1-pro-preview"),
        ] {
            let long = estimate(provider, model, &long_prompt);
            let short = estimate(provider, model, &short_prompt);
            let long_rate = long.usd / 400_000.0;
            let short_rate = short.usd / 100_000.0;
            assert!(
                long_rate > short_rate,
                "{provider}/{model}: a 400K prompt must bill above the base \
                 input rate (long={long:?}, short={short:?})"
            );
        }
    }

    /// Aggregators, clouds and relays resell other vendors' models under their
    /// own provider id. Before the vendor-inferred fallback they all priced as
    /// `Unknown`, which — combined with `unpriced_cost(Cloud) == u64::MAX` —
    /// sorted them last under `cost_aware` routing.
    #[test]
    fn resold_models_price_through_their_vendor() {
        for (provider, model) in [
            ("openrouter", "anthropic/claude-sonnet-5"),
            ("amazon-bedrock", "anthropic.claude-sonnet-5"),
            ("siliconflow", "deepseek-ai/DeepSeek-V3"),
            ("github-copilot", "gpt-4o"),
        ] {
            let card = rate_card(provider, model)
                .unwrap_or_else(|| panic!("{provider}/{model} must be priceable"));
            assert_eq!(
                card.basis,
                RateBasis::VendorInferred,
                "{provider}/{model} should be flagged as inferred, not quoted"
            );
        }
        // A vendor serving its own model stays `Direct` — the fallback must not
        // relabel first-party rates.
        assert_eq!(
            rate_card("anthropic", "claude-sonnet-5").unwrap().basis,
            RateBasis::Direct
        );

        // Deliberate limit, asserted so it stays deliberate: the fallback keys
        // on the model's *vendor*, and open-weight families have no vendor
        // price — Meta does not sell Llama inference, and each host prices it
        // differently (Groq / Together / Cerebras / Fireworks all differ). So a
        // hosted Llama stays unpriced rather than being assigned a fictional
        // "Meta rate". `unpriced_cost` still ranks it by endpoint tier.
        assert!(rate_card("groq", "llama-3.3-70b-versatile").is_none());
    }

    /// The open-weight stance, pinned across every family that now reaches a
    /// preset's advertised set.
    ///
    /// `gpt-oss` is the one that needs a guard rather than a comment: it infers
    /// vendor `openai` (OpenAI did release the weights), so the day somebody
    /// adds a broad `gpt` row to the OpenAI section, Groq-, Cerebras- and
    /// Baseten-hosted gpt-oss would silently start billing at OpenAI's hosted
    /// rate. Nemotron has the same shape via NVIDIA.
    #[test]
    fn open_weight_families_stay_unpriced() {
        for (provider, model) in [
            ("groq", "openai/gpt-oss-120b"),
            ("groq", "openai/gpt-oss-20b"),
            ("cerebras", "gpt-oss-120b"),
            ("baseten", "openai/gpt-oss-120b"),
            ("nvidia-nim", "nvidia/nemotron-3-ultra-550b-a55b"),
            ("together", "meta-llama/Llama-3.3-70B-Instruct-Turbo"),
        ] {
            assert!(
                rate_card(provider, model).is_none(),
                "{provider}/{model} must stay unpriced: no vendor sells this \
                 inference, and each host charges its own rate"
            );
        }
    }

    /// The Fireworks `p` separator is a pricing correctness issue, not just a
    /// tidiness one — both of its shipped ids used to fall past their own rate
    /// row into a much cheaper family fallback.
    #[test]
    fn fireworks_p_separator_ids_reach_their_own_rate() {
        let kimi = rate_card("fireworks", "accounts/fireworks/models/kimi-k2p6")
            .expect("kimi-k2p6 must price");
        assert_eq!(kimi.input_per_mtok, Some(0.95));
        let glm = rate_card("fireworks", "accounts/fireworks/routers/glm-5p2-fast")
            .expect("glm-5p2-fast must price");
        assert_eq!(glm.input_per_mtok, Some(1.40));
    }

    /// Gemini 3.x flash used to inherit the 2.0-flash rate from the broad
    /// `gemini` row — a 20x under-report on the tier the `gemini` preset's own
    /// aux model sits in.
    #[test]
    fn gemini_3x_flash_is_not_priced_as_2_0_flash() {
        for model in [
            "gemini-3.6-flash",
            "gemini-3.5-flash",
            "gemini-3-flash-preview",
        ] {
            let card = rate_card("gemini", model).unwrap_or_else(|| panic!("{model} must price"));
            assert!(
                card.input_per_mtok.unwrap_or(0.0) > 1.0,
                "{model} priced at {:?}/Mtok — that is the 2.0-flash rate",
                card.input_per_mtok
            );
        }
        // …and the lite tier is genuinely cheaper, so its longer prefix has to
        // win over `gemini-3.5-flash`.
        let lite = rate_card("gemini", "gemini-3.5-flash-lite").expect("lite must price");
        assert_eq!(lite.input_per_mtok, Some(0.30));
    }

    /// The three GPT-5.6 tiers share one capability row but not one rate card.
    #[test]
    fn gpt_5_6_tiers_price_separately() {
        let flagship = rate_card("openai", "gpt-5.6").expect("gpt-5.6");
        let terra = rate_card("openai", "gpt-5.6-terra").expect("terra");
        let luna = rate_card("openai", "gpt-5.6-luna").expect("luna");
        assert_eq!(flagship.input_per_mtok, Some(5.0));
        assert_eq!(terra.input_per_mtok, Some(2.50));
        assert_eq!(luna.input_per_mtok, Some(1.0));
        // `-sol` deliberately shares the flagship row.
        assert_eq!(
            rate_card("openai", "gpt-5.6-sol").unwrap().input_per_mtok,
            Some(5.0)
        );
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
            basis: Some(RateBasis::Direct),
        };
        let json = serde_json::to_string(&est).expect("serialize");
        let back: CostEstimate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(est, back);

        // Estimates persisted before `basis` existed must still load — the
        // field is `#[serde(default)]` precisely so old run summaries do.
        let legacy = r#"{"usd":1.23,"status":"complete","provider":"anthropic","model":"m"}"#;
        let parsed: CostEstimate = serde_json::from_str(legacy).expect("legacy deserialize");
        assert_eq!(parsed.basis, None);
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
        // deepseek-chat is now the v4-flash non-thinking alias:
        // 1M input @ $0.14 + 1M output @ $0.28 = $0.42.
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("deepseek", "deepseek-chat", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 0.42).abs() < 1e-6,
            "expected $0.42, got ${}",
            est.usd
        );
    }

    #[test]
    fn deepseek_v4_pro_more_specific_prefix_wins() {
        // v4-pro ($0.435 in + $0.87 out = $1.305 for 1M each) is a more
        // specific prefix than the broad `deepseek` fallback
        // ($0.14 + $0.28 = $0.42).
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let pro = estimate("deepseek", "deepseek-v4-pro", &breakdown);
        assert!(
            (pro.usd - 1.305).abs() < 1e-6,
            "expected $1.305, got ${}",
            pro.usd
        );
        let broad = estimate("deepseek", "deepseek-chat", &breakdown);
        assert!(
            broad.usd < pro.usd,
            "broad fallback must be cheaper than v4-pro"
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
        // Anthropic bills cache-creation; reasoning is folded into output.
        assert_eq!(card.cache_creation_per_mtok, Some(3.75));
        assert_eq!(card.reasoning_per_mtok, None);
    }

    #[test]
    fn rate_card_surfaces_separately_billed_reasoning() {
        // Gemini bills reasoning tokens at their own rate; the picker now sees
        // it rather than silently dropping the component.
        let card = rate_card("google", "gemini-2.0-flash").expect("priced");
        assert_eq!(card.reasoning_per_mtok, Some(0.30));
        // Gemini has no prompt-cache write surcharge in the table.
        assert_eq!(card.cache_creation_per_mtok, None);
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
            (glm51.usd - 1.40).abs() < 1e-6,
            "expected $1.40, got ${}",
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

    /// `kimi-k3` starts with `kimi`, so without its own row it silently
    /// inherited the $0.60/$2.50 family fallback — a 5x under-report on the
    /// flagship. The row must also not be shadowed by the K2.x rows above it.
    #[test]
    fn kimi_k3_priced_at_its_own_rate_not_the_family_fallback() {
        let card = rate_card("moonshot", "kimi-k3").expect("kimi-k3 priced");
        assert_eq!(card.input_per_mtok, Some(3.0));
        assert_eq!(card.output_per_mtok, Some(15.0));
        assert_eq!(card.cache_read_per_mtok, Some(0.30));

        // The K2 rows must not be dragged in by, nor drag in, the K3 prefix.
        assert_eq!(
            rate_card("moonshot", "kimi-k2.7-code")
                .unwrap()
                .input_per_mtok,
            Some(0.95)
        );

        let io_1m = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let k3 = estimate("moonshot", "kimi-k3", &io_1m);
        assert_eq!(k3.status, CostStatus::Complete);
        assert!(
            (k3.usd - 18.0).abs() < 1e-6,
            "expected $18.00, got ${}",
            k3.usd
        );
    }

    /// The Kimi Code subscription ids bill plan quota, not tokens. Reporting
    /// an invented per-token figure would be worse than saying "unknown", so
    /// every id that endpoint serves must stay unpriced.
    ///
    /// The `kimi-`prefixed three are the ones this gets silently wrong: they
    /// were quoted at the open platform's $0.60/$2.50 family rate, which is
    /// not a number anyone on this endpoint is billed. Asserted through both
    /// lookup passes — the vendor-inferred pass reaches the same table from the
    /// *model* id, so guarding only the provider-keyed pass guards nothing.
    #[test]
    fn kimi_code_subscription_ids_stay_unpriced() {
        let io = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        for model in [
            "k3",
            "k3-256k",
            "kimi-for-coding",
            "kimi-for-coding-highspeed",
            "kimi-code",
            "k2p5",
        ] {
            assert!(
                rate_card("kimi-for-coding", model).is_none(),
                "{model} carries a per-token rate on a quota-billed endpoint"
            );
            assert!(
                rate_card("some-relay", model).is_none(),
                "{model} re-priced through the vendor-inferred pass"
            );
            assert_eq!(
                estimate("kimi-for-coding", model, &io).status,
                CostStatus::Unknown,
                "{model} should report Unknown, not a fabricated figure"
            );
        }

        // Exact match, not a prefix: the open platform meters tokens and its
        // ids must keep their rates, including the family fallback row.
        assert!(rate_card("moonshot", "kimi-k3").is_some());
        assert!(rate_card("moonshot", "kimi-k2.6").is_some());
        assert!(rate_card("moonshot", "kimi-latest").is_some());
    }

    #[test]
    fn doubao_priced_after_vendor_wiring() {
        // Doubao previously reported CostStatus::Unknown (no canonical_provider_id
        // branch + no price table entry). Now both provider name and aliases
        // resolve to the doubao rate card.
        let input_1m = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        for provider in ["doubao", "volcengine", "ark"] {
            let est = estimate(provider, "doubao-seed-1-8-251228", &input_1m);
            assert_eq!(
                est.status,
                CostStatus::Complete,
                "provider {provider} should price the doubao Seed default"
            );
            assert!(est.usd > 0.0);
        }
    }

    #[test]
    fn minimax_m3_default_is_priced() {
        // With the default advanced to M3, its canonical id must hit a rate row
        // (previously only minimax-m2 existed -> M3 would be Unknown).
        let input_1m = TokenBreakdown {
            input: 1_000_000,
            ..Default::default()
        };
        let est = estimate("minimax", "MiniMax-M3", &input_1m);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 0.60).abs() < 1e-6,
            "expected $0.60, got ${}",
            est.usd
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
