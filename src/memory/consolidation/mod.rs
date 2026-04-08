//! Consolidation module for user profile distillation
//!
//! Analyzes frequently accessed facts to extract stable user preferences,
//! habits, and characteristics, creating a consolidated user profile.

mod analyzer;
mod profile;
pub mod promotion_scorer;

#[cfg(test)]
mod tests;

pub use analyzer::{ConsolidationAnalyzer, ConsolidationConfig, FrequentFact};
pub use profile::{ConsolidatedFact, ProfileCategory, UserProfile};
pub use promotion_scorer::{PromotionScorer, PromotionThresholds};
