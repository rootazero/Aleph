// Provider pricing for /usage cost estimates.
//
// Prices are USD per million tokens (input, output). Sourced 2026-05-21.
// Hardcoded by design — see docs/superpowers/specs/2026-05-21-repl-agent-control-panel-design.md
// "Pricing config file" is explicit out-of-scope for this cycle.
//
// When a provider is missing, `estimate_cost` returns None and callers should
// render "n/a (no pricing for <name>)" rather than fabricate a number.

/// Per-million-token prices in USD (input, output).
struct PricingEntry {
    /// Substring match (case-insensitive) against the model name.
    /// Listed in order; first match wins.
    name_contains: &'static str,
    input_per_million: f64,
    output_per_million: f64,
}

const PRICING_TABLE: &[PricingEntry] = &[
    // Anthropic — Claude 4.x family (2026 prices)
    PricingEntry {
        name_contains: "claude-opus-4",
        input_per_million: 15.0,
        output_per_million: 75.0,
    },
    PricingEntry {
        name_contains: "claude-sonnet-4",
        input_per_million: 3.0,
        output_per_million: 15.0,
    },
    PricingEntry {
        name_contains: "claude-haiku-4",
        input_per_million: 0.80,
        output_per_million: 4.0,
    },
    // OpenAI
    PricingEntry {
        name_contains: "gpt-4o-mini",
        input_per_million: 0.15,
        output_per_million: 0.60,
    },
    PricingEntry {
        name_contains: "gpt-4o",
        input_per_million: 2.50,
        output_per_million: 10.0,
    },
    PricingEntry {
        name_contains: "o3",
        input_per_million: 2.0,
        output_per_million: 8.0,
    },
    // Kimi (Moonshot) — kimi-for-coding tier
    PricingEntry {
        name_contains: "kimi",
        input_per_million: 0.15,
        output_per_million: 2.50,
    },
    // T8Star (Anthropic-protocol relay)
    PricingEntry {
        name_contains: "t8star",
        input_per_million: 0.50,
        output_per_million: 1.50,
    },
];

/// Cost breakdown in USD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostBreakdown {
    pub input_usd: f64,
    pub output_usd: f64,
    pub total_usd: f64,
}

/// Estimate USD cost for a model + token counts. Returns None if the model
/// name does not match any known pricing entry.
pub fn estimate_cost(
    model_name: &str,
    input_tokens: u64,
    output_tokens: u64,
) -> Option<CostBreakdown> {
    let needle = model_name.to_ascii_lowercase();
    let entry = PRICING_TABLE
        .iter()
        .find(|e| needle.contains(e.name_contains))?;

    let input_usd = (input_tokens as f64) * entry.input_per_million / 1_000_000.0;
    let output_usd = (output_tokens as f64) * entry.output_per_million / 1_000_000.0;
    Some(CostBreakdown {
        input_usd,
        output_usd,
        total_usd: input_usd + output_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_sonnet_4_matches_with_case_variations() {
        let cost = estimate_cost("claude-sonnet-4-6", 1_000_000, 500_000).unwrap();
        assert!((cost.input_usd - 3.0).abs() < 1e-9);
        assert!((cost.output_usd - 7.5).abs() < 1e-9);
        assert!((cost.total_usd - 10.5).abs() < 1e-9);

        // Case-insensitive
        let upper = estimate_cost("CLAUDE-SONNET-4-6", 1_000_000, 500_000).unwrap();
        assert_eq!(upper, cost);
    }

    #[test]
    fn kimi_for_coding_matches() {
        let cost = estimate_cost("kimi-for-coding", 100_000, 50_000).unwrap();
        assert!((cost.input_usd - 0.015).abs() < 1e-9);
        assert!((cost.output_usd - 0.125).abs() < 1e-9);
    }

    #[test]
    fn unknown_provider_returns_none() {
        assert!(estimate_cost("some-random-llm-7b", 1000, 100).is_none());
        assert!(estimate_cost("", 1000, 100).is_none());
    }

    #[test]
    fn zero_tokens_zero_cost() {
        let cost = estimate_cost("claude-opus-4-7", 0, 0).unwrap();
        assert_eq!(cost.input_usd, 0.0);
        assert_eq!(cost.output_usd, 0.0);
        assert_eq!(cost.total_usd, 0.0);
    }

    #[test]
    fn longer_match_wins_via_ordering() {
        // gpt-4o-mini appears before gpt-4o so the mini match wins for the mini model
        let mini = estimate_cost("gpt-4o-mini-2026", 1_000_000, 1_000_000).unwrap();
        let full = estimate_cost("gpt-4o-2026", 1_000_000, 1_000_000).unwrap();
        assert!(mini.total_usd < full.total_usd);
    }
}
