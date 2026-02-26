//! Prompt Hooks
//!
//! Trait-based hook system for modifying prompts before and after assembly.
//! Extensions and plugins can implement PromptHook to customize prompt
//! generation without modifying the core builder.

use crate::error::Result;
use crate::thinker::prompt_builder::PromptConfig;

/// Hook for modifying prompt generation.
///
/// Implement this trait to intercept prompt building at two points:
/// - `before_prompt_build`: Modify the PromptConfig before assembly
/// - `after_prompt_build`: Modify the final prompt string after assembly
pub trait PromptHook: Send + Sync {
    /// Called before the system prompt is assembled.
    /// Modify the config to influence what sections are included.
    fn before_prompt_build(&self, _config: &mut PromptConfig) -> Result<()> {
        Ok(())
    }

    /// Called after the system prompt is assembled.
    /// Modify the final prompt text.
    fn after_prompt_build(&self, _prompt: &mut String) -> Result<()> {
        Ok(())
    }

    /// Name of this hook (for logging/debugging).
    fn name(&self) -> &str {
        "unnamed_hook"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBeforeHook;
    impl PromptHook for TestBeforeHook {
        fn before_prompt_build(&self, config: &mut PromptConfig) -> Result<()> {
            config.language = Some("French".to_string());
            Ok(())
        }
        fn name(&self) -> &str {
            "test_before"
        }
    }

    struct TestAfterHook;
    impl PromptHook for TestAfterHook {
        fn after_prompt_build(&self, prompt: &mut String) -> Result<()> {
            prompt.push_str("\n## Hook Injected\nThis was added by a hook.\n");
            Ok(())
        }
        fn name(&self) -> &str {
            "test_after"
        }
    }

    struct NoOpHook;
    impl PromptHook for NoOpHook {}

    #[test]
    fn test_before_hook_modifies_config() {
        let hook = TestBeforeHook;
        let mut config = PromptConfig::default();
        assert!(config.language.is_none());
        hook.before_prompt_build(&mut config).unwrap();
        assert_eq!(config.language.as_deref(), Some("French"));
    }

    #[test]
    fn test_after_hook_modifies_prompt() {
        let hook = TestAfterHook;
        let mut prompt = "## Existing Content\n".to_string();
        hook.after_prompt_build(&mut prompt).unwrap();
        assert!(prompt.contains("Hook Injected"));
    }

    #[test]
    fn test_noop_hook_does_nothing() {
        let hook = NoOpHook;
        let mut config = PromptConfig::default();
        hook.before_prompt_build(&mut config).unwrap();
        // Config should be unchanged - no panic is good

        let mut prompt = "test".to_string();
        hook.after_prompt_build(&mut prompt).unwrap();
        assert_eq!(prompt, "test");
    }

    #[test]
    fn test_hook_name() {
        let hook = TestBeforeHook;
        assert_eq!(hook.name(), "test_before");

        let noop = NoOpHook;
        assert_eq!(noop.name(), "unnamed_hook");
    }
}
