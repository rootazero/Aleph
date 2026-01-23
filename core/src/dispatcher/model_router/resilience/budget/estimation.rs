//! Cost Estimation
//!
//! Pre-call cost estimation for budget checks.

use crate::dispatcher::model_router::{CallRecord, CostTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Model Pricing
// =============================================================================

/// Model pricing data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Price per million input tokens (USD)
    pub input_price_per_1m: f64,
    /// Price per million output tokens (USD)
    pub output_price_per_1m: f64,
    /// Price per million cached input tokens (if supported)
    pub cached_input_price_per_1m: Option<f64>,
}

impl Default for ModelPricing {
    fn default() -> Self {
        // Default to Medium tier pricing
        Self {
            input_price_per_1m: 3.0,
            output_price_per_1m: 15.0,
            cached_input_price_per_1m: None,
        }
    }
}

impl ModelPricing {
    /// Create pricing from cost tier
    pub fn from_cost_tier(tier: CostTier) -> Self {
        match tier {
            CostTier::Free => Self {
                input_price_per_1m: 0.0,
                output_price_per_1m: 0.0,
                cached_input_price_per_1m: Some(0.0),
            },
            CostTier::Low => Self {
                input_price_per_1m: 0.25,
                output_price_per_1m: 1.25,
                cached_input_price_per_1m: Some(0.025),
            },
            CostTier::Medium => Self {
                input_price_per_1m: 3.0,
                output_price_per_1m: 15.0,
                cached_input_price_per_1m: Some(0.3),
            },
            CostTier::High => Self {
                input_price_per_1m: 15.0,
                output_price_per_1m: 75.0,
                cached_input_price_per_1m: Some(1.5),
            },
        }
    }

    /// Calculate cost for given token counts
    pub fn calculate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input_price_per_1m;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output_price_per_1m;
        input_cost + output_cost
    }
}

// =============================================================================
// Cost Estimate
// =============================================================================

/// Pre-call cost estimation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Model ID
    pub model_id: String,
    /// Input tokens
    pub input_tokens: u32,
    /// Estimated output tokens
    pub estimated_output_tokens: u32,
    /// Base cost estimate (USD)
    pub base_cost_usd: f64,
    /// Cost with safety margin (USD)
    pub with_margin_usd: f64,
    /// Source of pricing data
    pub pricing_source: PricingSource,
}

impl CostEstimate {
    /// Get the conservative estimate (with margin)
    pub fn cost(&self) -> f64 {
        self.with_margin_usd
    }
}

/// Source of pricing data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingSource {
    /// From static configuration (ModelProfile)
    Profile,
    /// Learned from actual usage
    Learned,
    /// From provider API
    ProviderApi,
    /// Default tier-based estimate
    Default,
}

// =============================================================================
// Cost Estimator
// =============================================================================

/// Cost estimator for pre-call budget checks
#[derive(Debug, Clone)]
pub struct CostEstimator {
    /// Model pricing data
    pricing: HashMap<String, ModelPricing>,
    /// Safety margin for estimation (e.g., 1.2 = 20% buffer)
    safety_margin: f64,
    /// Default output token estimate when not provided
    default_output_estimate: u32,
}

impl Default for CostEstimator {
    fn default() -> Self {
        Self {
            pricing: HashMap::new(),
            safety_margin: 1.2,
            default_output_estimate: 500,
        }
    }
}

impl CostEstimator {
    /// Create a new cost estimator
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set safety margin
    pub fn with_safety_margin(mut self, margin: f64) -> Self {
        self.safety_margin = margin.max(1.0);
        self
    }

    /// Builder: set default output estimate
    pub fn with_default_output_estimate(mut self, tokens: u32) -> Self {
        self.default_output_estimate = tokens;
        self
    }

    /// Set pricing for a model
    pub fn set_pricing(&mut self, model_id: impl Into<String>, pricing: ModelPricing) {
        self.pricing.insert(model_id.into(), pricing);
    }

    /// Set pricing from cost tier
    pub fn set_pricing_from_tier(&mut self, model_id: impl Into<String>, tier: CostTier) {
        self.pricing
            .insert(model_id.into(), ModelPricing::from_cost_tier(tier));
    }

    /// Get pricing for a model
    pub fn get_pricing(&self, model_id: &str) -> Option<&ModelPricing> {
        self.pricing.get(model_id)
    }

    /// Estimate cost before making a call
    pub fn estimate(
        &self,
        model_id: &str,
        input_tokens: u32,
        estimated_output_tokens: Option<u32>,
    ) -> CostEstimate {
        let output_tokens = estimated_output_tokens.unwrap_or(self.default_output_estimate);

        let (pricing, source) = self.pricing.get(model_id).map_or_else(
            || (ModelPricing::default(), PricingSource::Default),
            |p| (p.clone(), PricingSource::Profile),
        );

        let base_cost = pricing.calculate_cost(input_tokens, output_tokens);
        let with_margin = base_cost * self.safety_margin;

        CostEstimate {
            model_id: model_id.to_string(),
            input_tokens,
            estimated_output_tokens: output_tokens,
            base_cost_usd: base_cost,
            with_margin_usd: with_margin,
            pricing_source: source,
        }
    }

    /// Update pricing from actual costs (learning)
    pub fn learn_from_actual(&mut self, record: &CallRecord) {
        if let Some(actual_cost) = record.cost_usd {
            let total_tokens = record.input_tokens + record.output_tokens;
            if total_tokens == 0 {
                return;
            }

            // Calculate actual per-token costs
            // This is a simplification - in practice we'd need input/output breakdown
            let pricing = self.pricing.entry(record.model_id.clone()).or_default();

            // Use exponential moving average to update
            let alpha = 0.1; // Learning rate

            // Estimate split based on typical ratios
            let input_ratio = record.input_tokens as f64 / total_tokens as f64;
            let output_ratio = record.output_tokens as f64 / total_tokens as f64;

            if record.input_tokens > 0 {
                let input_cost_est = actual_cost * input_ratio;
                let actual_input_per_1m =
                    (input_cost_est / record.input_tokens as f64) * 1_000_000.0;
                pricing.input_price_per_1m =
                    pricing.input_price_per_1m * (1.0 - alpha) + actual_input_per_1m * alpha;
            }

            if record.output_tokens > 0 {
                let output_cost_est = actual_cost * output_ratio;
                let actual_output_per_1m =
                    (output_cost_est / record.output_tokens as f64) * 1_000_000.0;
                pricing.output_price_per_1m =
                    pricing.output_price_per_1m * (1.0 - alpha) + actual_output_per_1m * alpha;
            }
        }
    }
}
