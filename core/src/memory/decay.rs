//! Memory Decay Mechanism
//!
//! Implements Ebbinghaus forgetting curve for memory lifecycle management.
//! Facts that haven't been accessed in a long time decay in strength,
//! while frequently accessed facts remain strong.

use crate::memory::context::{FactType, TemporalScope};
use serde::{Deserialize, Serialize};

/// Memory strength tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStrength {
    /// Number of times retrieved/hit
    pub access_count: u32,
    /// Last access timestamp (Unix seconds)
    pub last_accessed: i64,
    /// Creation timestamp (Unix seconds)
    pub creation_time: i64,
}

impl MemoryStrength {
    /// Create new strength record
    pub fn new(creation_time: i64) -> Self {
        Self {
            access_count: 0,
            last_accessed: creation_time,
            creation_time,
        }
    }

    /// Calculate current strength (Ebbinghaus curve simplified)
    ///
    /// Uses exponential decay with half-life:
    /// - strength = 0.5 ^ (days / half_life)
    /// - Access boost multiplier increases strength
    /// - Final strength capped at 1.0
    pub fn calculate_strength(&self, config: &DecayConfig, now: i64) -> f32 {
        let days_since_access = (now - self.last_accessed) as f32 / 86400.0;

        // Base decay: exponential decay curve
        // strength = 0.5 ^ (days / half_life)
        let base_decay = 0.5_f32.powf(days_since_access / config.half_life_days);

        // Access boost: each access adds boost, capped at 2.0
        let access_boost = (self.access_count as f32 * config.access_boost).min(2.0);

        // Final strength = base_decay * (1 + access_boost), capped at 1.0
        (base_decay * (1.0 + access_boost)).min(1.0)
    }

    /// Record an access (increment count, update timestamp)
    pub fn record_access(&mut self, now: i64) {
        self.access_count += 1;
        self.last_accessed = now;
    }

    /// Check if this memory should be considered for cleanup
    pub fn should_cleanup(&self, config: &DecayConfig, now: i64) -> bool {
        self.calculate_strength(config, now) < config.min_strength
    }

    /// Calculate strength with type-specific half-life
    pub fn calculate_strength_for_type(
        &self,
        config: &DecayConfig,
        now: i64,
        fact_type: &FactType,
    ) -> f32 {
        // Protected types never decay
        if config.is_protected(fact_type) {
            return 1.0;
        }

        let effective_half_life = config.effective_half_life(fact_type);
        let days_since_access = (now - self.last_accessed) as f32 / 86400.0;

        // Handle infinite half-life
        if effective_half_life.is_infinite() {
            return 1.0;
        }

        let base_decay = 0.5_f32.powf(days_since_access / effective_half_life);
        let access_boost = (self.access_count as f32 * config.access_boost).min(2.0);

        (base_decay * (1.0 + access_boost)).min(1.0)
    }
}

impl Default for MemoryStrength {
    fn default() -> Self {
        Self::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        )
    }
}

/// Decay configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    /// Half-life in days (default: 30)
    pub half_life_days: f32,
    /// Strength boost per access (default: 0.2)
    pub access_boost: f32,
    /// Minimum strength before cleanup (default: 0.1)
    pub min_strength: f32,
    /// Fact types that never decay
    pub protected_types: Vec<FactType>,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            half_life_days: 30.0,
            access_boost: 0.2,
            min_strength: 0.1,
            protected_types: vec![FactType::Personal],
        }
    }
}

impl DecayConfig {
    /// Get effective half-life for a fact type
    pub fn effective_half_life(&self, fact_type: &FactType) -> f32 {
        match fact_type {
            FactType::Preference => self.half_life_days * 2.0, // More durable
            FactType::Personal => f32::INFINITY,              // Never decay
            _ => self.half_life_days,
        }
    }

    /// Check if a fact type is protected from decay
    pub fn is_protected(&self, fact_type: &FactType) -> bool {
        self.protected_types.contains(fact_type)
    }

    /// Builder: set half-life
    pub fn with_half_life(mut self, days: f32) -> Self {
        self.half_life_days = days;
        self
    }

    /// Builder: set access boost
    pub fn with_access_boost(mut self, boost: f32) -> Self {
        self.access_boost = boost;
        self
    }

    /// Builder: set min strength
    pub fn with_min_strength(mut self, min: f32) -> Self {
        self.min_strength = min;
        self
    }

    /// Builder: add protected type
    pub fn with_protected_type(mut self, fact_type: FactType) -> Self {
        if !self.protected_types.contains(&fact_type) {
            self.protected_types.push(fact_type);
        }
        self
    }

    /// Get effective half-life considering temporal scope
    pub fn effective_half_life_with_scope(
        &self,
        fact_type: &FactType,
        temporal_scope: &TemporalScope,
    ) -> f32 {
        let base = self.effective_half_life(fact_type);

        if base.is_infinite() {
            return base;
        }

        match temporal_scope {
            TemporalScope::Ephemeral => base * 0.5,  // Decays 2x faster
            TemporalScope::Permanent => base * 3.0,  // Lasts 3x longer
            TemporalScope::Contextual => base,       // Normal decay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decay_config_default() {
        let config = DecayConfig::default();
        assert!((config.half_life_days - 30.0).abs() < 0.01);
        assert!((config.access_boost - 0.2).abs() < 0.01);
        assert!((config.min_strength - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_strength_calculation_no_decay() {
        let config = DecayConfig::default();
        let now = 1000000;

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: now,
            creation_time: now,
        };

        let score = strength.calculate_strength(&config, now);
        assert!((score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_strength_calculation_with_decay() {
        let config = DecayConfig::default();
        let now = 1000000;
        let thirty_days_ago = now - (30 * 86400);

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: thirty_days_ago,
            creation_time: thirty_days_ago,
        };

        let score = strength.calculate_strength(&config, now);
        // After one half-life, score should be ~0.5
        assert!((score - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_strength_with_access_boost() {
        let config = DecayConfig::default();
        let now = 1000000;
        let thirty_days_ago = now - (30 * 86400);

        let strength = MemoryStrength {
            access_count: 5, // 5 accesses = 1.0 boost
            last_accessed: thirty_days_ago,
            creation_time: thirty_days_ago,
        };

        let score = strength.calculate_strength(&config, now);
        // 0.5 * (1 + 1.0) = 1.0
        assert!((score - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_preference_has_longer_half_life() {
        let config = DecayConfig::default();
        let half_life = config.effective_half_life(&FactType::Preference);
        assert!((half_life - 60.0).abs() < 0.01);
    }

    #[test]
    fn test_personal_never_decays() {
        let config = DecayConfig::default();
        assert!(config.is_protected(&FactType::Personal));
    }

    #[test]
    fn test_record_access() {
        let mut strength = MemoryStrength::new(1000000);
        assert_eq!(strength.access_count, 0);

        strength.record_access(1000100);
        assert_eq!(strength.access_count, 1);
        assert_eq!(strength.last_accessed, 1000100);
    }

    #[test]
    fn test_should_cleanup() {
        let config = DecayConfig::default();
        let now = 1000000;
        let very_old = now - (365 * 86400); // 1 year ago

        let old_strength = MemoryStrength {
            access_count: 0,
            last_accessed: very_old,
            creation_time: very_old,
        };

        // After 1 year with no access, should be very weak
        assert!(old_strength.should_cleanup(&config, now));
    }

    #[test]
    fn test_ephemeral_decays_faster() {
        let config = DecayConfig::default();
        let now = 1000000;
        let fifteen_days_ago = now - (15 * 86400);

        let strength = MemoryStrength {
            access_count: 0,
            last_accessed: fifteen_days_ago,
            creation_time: fifteen_days_ago,
        };

        // Normal type: ~0.71 after 15 days (half of half-life)
        let normal_score = strength.calculate_strength_for_type(&config, now, &FactType::Other);

        // Check that ephemeral scope gives faster decay
        let ephemeral_half_life = config.effective_half_life_with_scope(&FactType::Other, &TemporalScope::Ephemeral);
        assert!(ephemeral_half_life < config.half_life_days);

        // Ephemeral: 15 days (half-life with 0.5x multiplier = 15 days half-life)
        // So after 15 days, score should be ~0.5
        assert!(normal_score > 0.5); // Normal should be above 0.5 after half the half-life
    }
}
