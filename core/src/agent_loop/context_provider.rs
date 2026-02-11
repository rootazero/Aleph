/// Context provider abstract interface
///
/// Implementations provide additional context to be injected into
/// the agent's prompt at message building time.
pub trait ContextProvider: Send + Sync {
    /// Get context content to inject
    ///
    /// Returns None if no context should be injected at this time.
    fn get_context(&self) -> Option<String>;

    /// Priority determines position in prompt
    ///
    /// Higher values appear first. Critical context should have
    /// highest priority (e.g., 1000+), normal context around 100.
    fn priority(&self) -> i32;

    /// Provider name for debugging and logging
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        context: String,
        priority: i32,
    }

    impl ContextProvider for MockProvider {
        fn get_context(&self) -> Option<String> {
            Some(self.context.clone())
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        fn name(&self) -> &str {
            "mock_provider"
        }
    }

    #[test]
    fn test_provider_trait() {
        let provider = MockProvider {
            context: "test context".to_string(),
            priority: 100,
        };

        assert_eq!(provider.get_context(), Some("test context".to_string()));
        assert_eq!(provider.priority(), 100);
        assert_eq!(provider.name(), "mock_provider");
    }
}
