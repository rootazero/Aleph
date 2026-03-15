//! CompactionConfig, ModelLimit, and TokenTracker tests

use crate::components::session_compactor::*;
use crate::components::types::ExecutionSession;

    // ========================================================================
    // CompactionConfig Tests
    // ========================================================================

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!(config.auto_compact);
        assert!(config.prune_enabled);
        assert_eq!(config.prune_minimum, 20_000);
        assert_eq!(config.prune_protect, 40_000);
        assert!(config.protected_tools.contains(&"skill".to_string()));
    }

    #[test]
    fn test_compaction_config_disabled() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: false,
            ..Default::default()
        };
        assert!(!config.auto_compact);
        assert!(!config.prune_enabled);
    }

    #[test]
    fn test_compaction_config_custom_protected_tools() {
        let config = CompactionConfig {
            protected_tools: vec!["skill".to_string(), "read".to_string(), "write".to_string()],
            ..Default::default()
        };
        assert_eq!(config.protected_tools.len(), 3);
        assert!(config.protected_tools.contains(&"skill".to_string()));
        assert!(config.protected_tools.contains(&"read".to_string()));
        assert!(config.protected_tools.contains(&"write".to_string()));
    }

    #[test]
    fn test_session_compactor_with_config() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: true,
            prune_minimum: 10_000,
            prune_protect: 20_000,
            protected_tools: vec!["custom_tool".to_string()],
        };
        let compactor = SessionCompactor::with_config(config);

        assert!(!compactor.config().auto_compact);
        assert!(compactor.config().prune_enabled);
        assert_eq!(compactor.config().prune_minimum, 10_000);
        assert_eq!(compactor.config().prune_protect, 20_000);
        assert!(compactor.config().protected_tools.contains(&"custom_tool".to_string()));
    }

    #[test]
    fn test_session_compactor_config_mut() {
        let mut compactor = SessionCompactor::new();

        compactor.config_mut().auto_compact = false;
        compactor.config_mut().prune_minimum = 15_000;

        assert!(!compactor.config().auto_compact);
        assert_eq!(compactor.config().prune_minimum, 15_000);
    }

    // ========================================================================
    // ModelLimit Tests
    // ========================================================================

    #[test]
    fn test_model_limit_default() {
        let limit = ModelLimit::default();

        assert_eq!(limit.context_limit, 128000);
        assert_eq!(limit.max_output_tokens, 4096);
        assert!((limit.reserve_ratio - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn test_model_limit_custom() {
        let limit = ModelLimit::new(200000, 8192, 0.3);

        assert_eq!(limit.context_limit, 200000);
        assert_eq!(limit.max_output_tokens, 8192);
        assert!((limit.reserve_ratio - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_model_limit_reserve_ratio_clamped() {
        let limit1 = ModelLimit::new(100000, 4096, 1.5);
        assert!((limit1.reserve_ratio - 1.0).abs() < f32::EPSILON);

        let limit2 = ModelLimit::new(100000, 4096, -0.5);
        assert!((limit2.reserve_ratio - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_compaction_threshold() {
        let limit = ModelLimit::new(100000, 4096, 0.2);
        // 100000 * (1 - 0.2) = 80000 (allow for floating point precision)
        let threshold1 = limit.compaction_threshold();
        assert!(
            (79990..=80010).contains(&threshold1),
            "Expected ~80000, got {}",
            threshold1
        );

        let limit2 = ModelLimit::new(200000, 4096, 0.1);
        // 200000 * (1 - 0.1) = 180000 (allow for floating point precision)
        let threshold2 = limit2.compaction_threshold();
        assert!(
            (179990..=180010).contains(&threshold2),
            "Expected ~180000, got {}",
            threshold2
        );
    }

    // ========================================================================
    // TokenTracker Tests
    // ========================================================================

    #[test]
    fn test_token_tracker_default() {
        let tracker = TokenTracker::new();

        // Check preset models
        let claude_opus = tracker.get_model_limit("claude-3-opus");
        assert_eq!(claude_opus.context_limit, 200000);

        let gpt4_turbo = tracker.get_model_limit("gpt-4-turbo");
        assert_eq!(gpt4_turbo.context_limit, 128000);

        let gemini_pro = tracker.get_model_limit("gemini-pro");
        assert_eq!(gemini_pro.context_limit, 32000);
    }

    #[test]
    fn test_token_tracker_unknown_model() {
        let tracker = TokenTracker::new();

        // Unknown model should return default
        let unknown = tracker.get_model_limit("unknown-model");
        assert_eq!(unknown.context_limit, 128000); // Default
    }

    #[test]
    fn test_token_tracker_prefix_match() {
        let tracker = TokenTracker::new();

        // Should match by prefix
        let claude_versioned = tracker.get_model_limit("claude-3-opus-20240229");
        assert_eq!(claude_versioned.context_limit, 200000);
    }

    #[test]
    fn test_token_estimation() {
        // Test basic estimation
        // "Hello" = 5 chars * 0.4 = 2 tokens (ceil)
        assert_eq!(TokenTracker::estimate_tokens("Hello"), 2);

        // Empty string = 0 tokens
        assert_eq!(TokenTracker::estimate_tokens(""), 0);

        // 100 chars * 0.4 = 40 tokens
        let text = "a".repeat(100);
        assert_eq!(TokenTracker::estimate_tokens(&text), 40);

        // 250 chars * 0.4 = 100 tokens
        let longer_text = "x".repeat(250);
        assert_eq!(TokenTracker::estimate_tokens(&longer_text), 100);
    }

    #[test]
    fn test_is_overflow() {
        let tracker = TokenTracker::new();

        // Create session with tokens below threshold
        let mut session = ExecutionSession::new().with_model("gemini-pro");
        session.total_tokens = 25000; // Below 32000 * 0.8 = 25600

        assert!(!tracker.is_overflow(&session));

        // Set tokens above threshold
        session.total_tokens = 26000; // Above 25600

        assert!(tracker.is_overflow(&session));
    }
