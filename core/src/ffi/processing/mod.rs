//! Processing methods for AetherCore
//!
//! This module contains AI processing methods: process, cancel, generate_topic_title, extract_text
//!
//! # Architecture
//!
//! Uses `IntentRouter` + `AgentLoop` with observe-think-act cycle:
//! - L0-L2: Fast routing via IntentRouter (slash commands, patterns, context)
//! - Agent Loop: LLM-based thinking for complex tasks

mod agent_loop;
mod core;
mod direct_route;
mod memory;
mod orchestration;
mod progress_callback;
mod skill;
mod types;

// Re-export public types
pub use types::ProcessOptions;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_options_default() {
        let opts = ProcessOptions::default();
        assert!(opts.app_context.is_none());
        assert!(opts.window_title.is_none());
        assert!(opts.topic_id.is_none());
        assert!(opts.stream); // Streaming enabled by default
        assert!(opts.attachments.is_none());
    }

    #[test]
    fn test_process_options_new() {
        let opts = ProcessOptions::new();
        assert!(opts.app_context.is_none());
        assert!(opts.stream);
    }

    #[test]
    fn test_process_options_builder() {
        let opts = ProcessOptions::new()
            .with_app_context("com.example.app".to_string())
            .with_window_title("Test Window".to_string())
            .with_stream(false);

        assert_eq!(opts.app_context, Some("com.example.app".to_string()));
        assert_eq!(opts.window_title, Some("Test Window".to_string()));
        assert!(!opts.stream);
    }
}
