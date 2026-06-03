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
    /// Some components (e.g. cache_creation) lacked a rate; the figure is a
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
    /// already folds reasoning into `output`; OpenAI o-series likewise.
    reasoning_per_mtok: Option<f64>,
}

/// Static price table. Entries are scanned in declaration order; the first
/// `model_prefix` that prefix-matches wins. Order more-specific prefixes
/// before broader catch-alls. Sources are vendor pricing pages as of
/// 2026-05; refresh by bumping the literals here and recompiling.
const PRICE_TABLE: &[(&str, &[Rates])] = &[
    (
        "anthropic",
        &[
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
            Rates {
                model_prefix: "o3-mini",
                input_per_mtok: Some(1.10),
                output_per_mtok: Some(4.40),
                cache_read_per_mtok: Some(0.55),
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
        &[Rates {
            model_prefix: "grok",
            input_per_mtok: Some(3.0),
            output_per_mtok: Some(15.0),
            cache_read_per_mtok: Some(0.75),
            cache_creation_per_mtok: None,
            reasoning_per_mtok: None,
        }],
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

/// Return the per-million-token [`RateCard`] for a `(provider, model)` pair,
/// or `None` when the model is not priced. Powers the model picker's
/// cost-at-a-glance column (`providers.catalog`).
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
            usd += (tokens as f64 / 1_000_000.0) * per_mtok;
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
pub fn estimate(provider: &str, model: &str, breakdown: &TokenBreakdown) -> CostEstimate {
    let rate = match lookup_rates(provider, model) {
        Some(r) => r,
        None => return CostEstimate::unknown(provider, model),
    };
    let (usd, status) = apply_rates(breakdown, rate);
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
        // 1M input + 1M output @ $3 + $15 = $18.00
        let breakdown = TokenBreakdown {
            input: 1_000_000,
            output: 1_000_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 18.0).abs() < 1e-6,
            "expected $18.00, got ${}",
            est.usd
        );
    }

    #[test]
    fn anthropic_cache_components_billed_separately() {
        // 1M cache_read @ $0.30 + 1M cache_creation @ $3.75 = $4.05
        let breakdown = TokenBreakdown {
            cache_read: 1_000_000,
            cache_creation: 1_000_000,
            ..Default::default()
        };
        let est = estimate("anthropic", "claude-sonnet-4-6", &breakdown);
        assert_eq!(est.status, CostStatus::Complete);
        assert!(
            (est.usd - 4.05).abs() < 1e-6,
            "expected $4.05, got ${}",
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
}
