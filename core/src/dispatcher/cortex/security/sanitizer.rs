//! SecurityPipeline - Input sanitization infrastructure
//!
//! Provides the core types and traits for building security sanitization
//! pipelines. Rules are applied in priority order to detect and handle
//! potentially malicious input.

use crate::sync_primitives::{Arc, RwLock};

/// Trust level assigned to the input source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustLevel {
    /// Unknown or untrusted source (default)
    #[default]
    Untrusted,
    /// Verified trusted source
    Trusted,
    /// Administrative privileges
    Admin,
}

/// Locale hint for language-specific processing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    /// English (US)
    #[default]
    EnUS,
    /// Chinese (Simplified)
    ZhCN,
    /// Other locales
    Other,
}

/// Context passed to sanitizer rules
#[derive(Debug, Clone, Default)]
pub struct SanitizeContext {
    /// Session identifier for logging/correlation
    pub session_id: Option<String>,
    /// Locale hint for language-specific patterns
    pub locale: Locale,
    /// Trust level of the input source
    pub user_trust_level: TrustLevel,
}

/// Action taken by a sanitizer rule
#[derive(Debug, Clone, PartialEq)]
pub enum SanitizeAction {
    /// Input passed without modification
    Pass,
    /// Sensitive content was masked (count of items masked)
    Masked(usize),
    /// Special characters were escaped (count of escapes)
    Escaped(usize),
    /// Confidence penalty applied (penalty value 0.0-1.0)
    ConfidencePenalty(f32),
    /// Input was blocked entirely (reason)
    Blocked(String),
}

/// Result of applying a sanitizer rule
#[derive(Debug, Clone)]
pub struct SanitizeResult {
    /// Sanitized output text
    pub output: String,
    /// Whether the rule was triggered
    pub triggered: bool,
    /// Action taken by the rule
    pub action: SanitizeAction,
    /// Optional details about what was detected
    pub details: Option<String>,
}

impl SanitizeResult {
    /// Create a pass result (no changes)
    pub fn pass(input: &str) -> Self {
        Self {
            output: input.to_string(),
            triggered: false,
            action: SanitizeAction::Pass,
            details: None,
        }
    }

    /// Create a masked result
    pub fn masked(output: String, count: usize, details: Option<String>) -> Self {
        Self {
            output,
            triggered: true,
            action: SanitizeAction::Masked(count),
            details,
        }
    }

    /// Create an escaped result
    pub fn escaped(output: String, count: usize, details: Option<String>) -> Self {
        Self {
            output,
            triggered: true,
            action: SanitizeAction::Escaped(count),
            details,
        }
    }

    /// Create a confidence penalty result
    pub fn penalty(input: &str, penalty: f32, details: Option<String>) -> Self {
        Self {
            output: input.to_string(),
            triggered: true,
            action: SanitizeAction::ConfidencePenalty(penalty),
            details,
        }
    }

    /// Create a blocked result
    pub fn blocked(reason: String) -> Self {
        Self {
            output: String::new(),
            triggered: true,
            action: SanitizeAction::Blocked(reason.clone()),
            details: Some(reason),
        }
    }
}

/// Configuration for security pipeline features
#[derive(Debug, Clone, Default)]
pub struct SecurityConfig {
    /// Master switch for the entire pipeline
    pub enabled: bool,
    /// Enable tag injection detection
    pub tag_injection_enabled: bool,
    /// Enable PII masking
    pub pii_masking_enabled: bool,
    /// Enable instruction override detection
    pub instruction_override_enabled: bool,
}

impl SecurityConfig {
    /// Create a config with all features enabled
    pub fn default_enabled() -> Self {
        Self {
            enabled: true,
            tag_injection_enabled: true,
            pii_masking_enabled: true,
            instruction_override_enabled: true,
        }
    }
}

/// Trait for implementing sanitization rules
///
/// Rules are applied in priority order (lower number = higher priority).
/// Each rule can inspect and optionally transform the input.
pub trait SanitizerRule: Send + Sync {
    /// Name of the rule for logging/metrics
    fn name(&self) -> &str;

    /// Priority for rule ordering (lower = runs first)
    fn priority(&self) -> u32;

    /// Apply the rule to input text
    fn sanitize(&self, input: &str, ctx: &SanitizeContext) -> SanitizeResult;

    /// Check if this rule is enabled given the config
    fn is_enabled(&self, config: &SecurityConfig) -> bool {
        config.enabled
    }
}

/// Result of processing through the entire pipeline
#[derive(Debug)]
pub struct PipelineResult {
    /// Final sanitized text
    pub text: String,
    /// Actions taken by each triggered rule (rule_name, action)
    pub actions: Vec<(String, SanitizeAction)>,
    /// Whether any rule blocked the input
    pub blocked: bool,
    /// Reason if blocked
    pub block_reason: Option<String>,
}

/// Security sanitization pipeline
///
/// Applies multiple rules in priority order to sanitize input text.
/// Rules can mask sensitive data, escape special characters, apply
/// confidence penalties, or block input entirely.
pub struct SecurityPipeline {
    rules: Vec<Box<dyn SanitizerRule>>,
    config: Arc<RwLock<SecurityConfig>>,
}

impl SecurityPipeline {
    /// Create a new pipeline with the given configuration
    pub fn new(config: SecurityConfig) -> Self {
        Self {
            rules: Vec::new(),
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// Add a rule to the pipeline
    ///
    /// Rules are automatically sorted by priority when processing.
    pub fn add_rule(&mut self, rule: Box<dyn SanitizerRule>) {
        self.rules.push(rule);
        // Sort by priority (lower number = higher priority)
        self.rules.sort_by_key(|r| r.priority());
    }

    /// Process input through all enabled rules
    ///
    /// Returns immediately if any rule blocks the input.
    pub fn process(&self, input: &str, ctx: &SanitizeContext) -> PipelineResult {
        let config = self.config.read().unwrap();

        // If pipeline is disabled, pass through unchanged
        if !config.enabled {
            return PipelineResult {
                text: input.to_string(),
                actions: Vec::new(),
                blocked: false,
                block_reason: None,
            };
        }

        let mut current_text = input.to_string();
        let mut actions = Vec::new();

        for rule in &self.rules {
            if !rule.is_enabled(&config) {
                continue;
            }

            let result = rule.sanitize(&current_text, ctx);

            if result.triggered {
                actions.push((rule.name().to_string(), result.action.clone()));

                // Check if blocked
                if let SanitizeAction::Blocked(reason) = &result.action {
                    return PipelineResult {
                        text: String::new(),
                        actions,
                        blocked: true,
                        block_reason: Some(reason.clone()),
                    };
                }

                // Update text for next rule
                current_text = result.output;
            }
        }

        PipelineResult {
            text: current_text,
            actions,
            blocked: false,
            block_reason: None,
        }
    }

    /// Update the pipeline configuration
    pub fn update_config(&self, config: SecurityConfig) {
        let mut current = self.config.write().unwrap();
        *current = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_creation() {
        let pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
        let ctx = SanitizeContext::default();

        // Empty pipeline should pass through unchanged
        let result = pipeline.process("hello world", &ctx);

        assert!(!result.blocked);
        assert_eq!(result.text, "hello world");
        assert!(result.actions.is_empty());
    }

    #[test]
    fn test_pipeline_rule_order() {
        // Create rules with different priorities
        struct LowPriorityRule;
        impl SanitizerRule for LowPriorityRule {
            fn name(&self) -> &str {
                "low_priority"
            }
            fn priority(&self) -> u32 {
                100
            }
            fn sanitize(&self, input: &str, _ctx: &SanitizeContext) -> SanitizeResult {
                // Append marker to show this rule ran
                SanitizeResult::masked(format!("{}_low", input), 1, None)
            }
        }

        struct HighPriorityRule;
        impl SanitizerRule for HighPriorityRule {
            fn name(&self) -> &str {
                "high_priority"
            }
            fn priority(&self) -> u32 {
                10
            }
            fn sanitize(&self, input: &str, _ctx: &SanitizeContext) -> SanitizeResult {
                // Append marker to show this rule ran
                SanitizeResult::masked(format!("{}_high", input), 1, None)
            }
        }

        let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());

        // Add low priority first, then high priority
        pipeline.add_rule(Box::new(LowPriorityRule));
        pipeline.add_rule(Box::new(HighPriorityRule));

        let ctx = SanitizeContext::default();
        let result = pipeline.process("test", &ctx);

        // High priority should run first, then low priority
        // Result should be "test_high_low"
        assert_eq!(result.text, "test_high_low");
        assert_eq!(result.actions.len(), 2);
        assert_eq!(result.actions[0].0, "high_priority");
        assert_eq!(result.actions[1].0, "low_priority");
    }

    #[test]
    fn test_sanitize_result_constructors() {
        let pass = SanitizeResult::pass("input");
        assert!(!pass.triggered);
        assert_eq!(pass.output, "input");
        assert_eq!(pass.action, SanitizeAction::Pass);

        let masked = SanitizeResult::masked("***".to_string(), 3, Some("masked SSN".to_string()));
        assert!(masked.triggered);
        assert_eq!(masked.action, SanitizeAction::Masked(3));

        let escaped = SanitizeResult::escaped("&lt;".to_string(), 1, None);
        assert!(escaped.triggered);
        assert_eq!(escaped.action, SanitizeAction::Escaped(1));

        let penalty = SanitizeResult::penalty("input", 0.5, Some("suspicious".to_string()));
        assert!(penalty.triggered);
        assert_eq!(penalty.action, SanitizeAction::ConfidencePenalty(0.5));

        let blocked = SanitizeResult::blocked("injection detected".to_string());
        assert!(blocked.triggered);
        assert!(matches!(blocked.action, SanitizeAction::Blocked(_)));
    }

    #[test]
    fn test_pipeline_disabled() {
        struct BlockingRule;
        impl SanitizerRule for BlockingRule {
            fn name(&self) -> &str {
                "blocker"
            }
            fn priority(&self) -> u32 {
                1
            }
            fn sanitize(&self, _input: &str, _ctx: &SanitizeContext) -> SanitizeResult {
                SanitizeResult::blocked("always blocks".to_string())
            }
        }

        let mut pipeline = SecurityPipeline::new(SecurityConfig::default());
        pipeline.add_rule(Box::new(BlockingRule));

        let ctx = SanitizeContext::default();

        // With disabled config (default), should pass through
        let result = pipeline.process("test", &ctx);
        assert!(!result.blocked);
        assert_eq!(result.text, "test");
    }

    #[test]
    fn test_pipeline_blocking() {
        struct BlockingRule;
        impl SanitizerRule for BlockingRule {
            fn name(&self) -> &str {
                "blocker"
            }
            fn priority(&self) -> u32 {
                1
            }
            fn sanitize(&self, _input: &str, _ctx: &SanitizeContext) -> SanitizeResult {
                SanitizeResult::blocked("malicious content".to_string())
            }
        }

        let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
        pipeline.add_rule(Box::new(BlockingRule));

        let ctx = SanitizeContext::default();
        let result = pipeline.process("test", &ctx);

        assert!(result.blocked);
        assert_eq!(result.block_reason, Some("malicious content".to_string()));
        assert!(result.text.is_empty());
    }

    #[test]
    fn test_config_update() {
        let pipeline = SecurityPipeline::new(SecurityConfig::default());

        // Initially disabled
        {
            let config = pipeline.config.read().unwrap();
            assert!(!config.enabled);
        }

        // Update to enabled
        pipeline.update_config(SecurityConfig::default_enabled());

        {
            let config = pipeline.config.read().unwrap();
            assert!(config.enabled);
            assert!(config.tag_injection_enabled);
            assert!(config.pii_masking_enabled);
        }
    }

    #[test]
    fn test_trust_level_default() {
        assert_eq!(TrustLevel::default(), TrustLevel::Untrusted);
    }

    #[test]
    fn test_locale_default() {
        assert_eq!(Locale::default(), Locale::EnUS);
    }

    #[test]
    fn test_sanitize_context_default() {
        let ctx = SanitizeContext::default();
        assert!(ctx.session_id.is_none());
        assert_eq!(ctx.locale, Locale::EnUS);
        assert_eq!(ctx.user_trust_level, TrustLevel::Untrusted);
    }
}
