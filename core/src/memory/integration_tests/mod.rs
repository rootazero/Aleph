//! Integration tests for Memory System Evolution
//!
//! These tests verify that memory system components can be instantiated
//! and configured correctly.
//!
//! Note: Most tests are marked as #[ignore] because they require model downloads.
//!
//! Run with: cargo test --lib memory::integration_tests -- --ignored

pub mod workspace_isolation;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod integration_tests {
    use crate::memory::{
        context_comptroller::{ComptrollerConfig, RetentionMode},
        ripple::RippleConfig,
    };

    #[tokio::test]
    async fn test_comptroller_config() {
        // Test that ComptrollerConfig can be created
        let config = ComptrollerConfig {
            similarity_threshold: 0.95,
            token_budget: 1000,
            fold_threshold: 0.2,
            retention_mode: RetentionMode::Hybrid,
        };

        assert_eq!(config.similarity_threshold, 0.95);
        assert_eq!(config.token_budget, 1000);
        println!("ComptrollerConfig created: {:?}", config);
    }

    #[tokio::test]
    async fn test_ripple_config() {
        // Test RippleTask configuration
        let config = RippleConfig {
            max_hops: 3,
            max_facts_per_hop: 5,
            similarity_threshold: 0.7,
        };

        assert_eq!(config.max_hops, 3);
        assert_eq!(config.max_facts_per_hop, 5);
        assert_eq!(config.similarity_threshold, 0.7);
        println!("RippleConfig created: {:?}", config);
    }

    #[tokio::test]
    async fn test_retention_modes() {
        // Test that all retention modes are available
        let modes = vec![
            RetentionMode::PreferTranscript,
            RetentionMode::PreferFact,
            RetentionMode::Hybrid,
        ];

        assert_eq!(modes.len(), 3, "Should have 3 retention modes");
        println!("Available retention modes: {:?}", modes);
    }

    #[tokio::test]
    async fn test_default_config() {
        // Test default configuration
        let config = ComptrollerConfig::default();

        assert_eq!(config.similarity_threshold, 0.95);
        assert_eq!(config.token_budget, 100000);
        assert_eq!(config.fold_threshold, 0.2);
        println!("Default config: {:?}", config);
    }
}
