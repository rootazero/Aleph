/// Prompt augmentation module for injecting memory context into LLM prompts
///
/// This module formats retrieved memories and injects them into the system prompt
/// to provide the LLM with relevant historical context.
use crate::memory::context::MemoryEntry;
use chrono::{DateTime, Utc};

/// Prompt augmenter for injecting memory context into LLM prompts
pub struct PromptAugmenter {
    /// Maximum number of memories to include in prompt
    max_memories: usize,
    /// Include similarity scores in formatted output
    show_scores: bool,
}

impl PromptAugmenter {
    /// Create new prompt augmenter with default settings
    pub fn new() -> Self {
        Self {
            max_memories: 5,
            show_scores: false,
        }
    }

    /// Create prompt augmenter with custom settings
    pub fn with_config(max_memories: usize, show_scores: bool) -> Self {
        Self {
            max_memories,
            show_scores,
        }
    }

    /// Augment prompt with retrieved memories
    ///
    /// Takes base system prompt, retrieved memories, and current user input,
    /// then injects formatted memory context between system prompt and user input.
    ///
    /// # Arguments
    /// * `base_prompt` - Base system prompt (e.g., "You are a helpful assistant")
    /// * `memories` - Retrieved memories sorted by relevance
    /// * `current_input` - Current user input text
    ///
    /// # Returns
    /// * Augmented prompt string with memory context injected
    ///
    /// # Example Output Format
    /// ```text
    /// You are a helpful assistant.
    ///
    /// ## Context History
    /// The following are relevant past interactions in this context:
    ///
    /// ### [2025-12-24 10:30:15 UTC]
    /// User: What is the capital of France?
    /// Assistant: Paris is the capital of France.
    ///
    /// ---
    ///
    /// User: Tell me more about Paris
    /// ```
    pub fn augment_prompt(
        &self,
        base_prompt: &str,
        memories: &[MemoryEntry],
        current_input: &str,
    ) -> String {
        // If no memories, return base prompt + user input
        if memories.is_empty() {
            return format!("{}\n\nUser: {}", base_prompt, current_input);
        }

        // Limit number of memories
        let memories_to_include = memories.iter().take(self.max_memories);

        // Format memories
        let context_history = self.format_memories(memories_to_include);

        // Construct augmented prompt
        format!(
            "{}\n\n## Context History\nThe following are relevant past interactions in this context:\n\n{}\n\n---\n\nUser: {}",
            base_prompt,
            context_history,
            current_input
        )
    }

    /// Augment ONLY the user input with memory context (no system prompt, no "User:" prefix)
    ///
    /// This is the NEW method for the refactored architecture where:
    /// - System prompt is passed separately to the AI provider
    /// - User input should not contain "User:" prefix (it's added by the API)
    ///
    /// # Arguments
    /// * `memories` - Retrieved memories to include as context
    /// * `current_input` - Current user input text
    ///
    /// # Returns
    /// * User input string, optionally prefixed with memory context
    pub fn augment_user_input(&self, memories: &[MemoryEntry], current_input: &str) -> String {
        // If no memories, return just the user input (no prefix)
        if memories.is_empty() {
            return current_input.to_string();
        }

        // Limit number of memories
        let memories_to_include = memories.iter().take(self.max_memories);

        // Format memories
        let context_history = self.format_memories(memories_to_include);

        // Construct user input with memory context
        // Use a simple format that won't confuse the AI
        // The memory context is provided as background information, then the actual request follows
        format!(
            "Previous context for reference:\n{}\n\n---\n\n{}",
            context_history, current_input
        )
    }

    /// Format memories into human-readable context
    fn format_memories<'a, I>(&self, memories: I) -> String
    where
        I: Iterator<Item = &'a MemoryEntry>,
    {
        let mut formatted = String::new();

        for (idx, memory) in memories.enumerate() {
            if idx > 0 {
                formatted.push_str("\n\n");
            }

            // Format timestamp
            let timestamp = DateTime::<Utc>::from_timestamp(memory.context.timestamp, 0)
                .unwrap_or_else(Utc::now);
            let time_str = timestamp.format("%Y-%m-%d %H:%M:%S UTC");

            // Header with timestamp (and optional similarity score)
            if self.show_scores {
                if let Some(score) = memory.similarity_score {
                    formatted.push_str(&format!("### [{}] (Similarity: {:.2})\n", time_str, score));
                } else {
                    formatted.push_str(&format!("### [{}]\n", time_str));
                }
            } else {
                formatted.push_str(&format!("### [{}]\n", time_str));
            }

            // User input
            formatted.push_str(&format!("User: {}\n", memory.user_input.trim()));

            // AI output
            formatted.push_str(&format!("Assistant: {}", memory.ai_output.trim()));
        }

        formatted
    }

    /// Get compact summary of memory count for logging
    pub fn get_memory_summary(&self, memories: &[MemoryEntry]) -> String {
        let count = memories.len().min(self.max_memories);
        if count == 0 {
            "No relevant memories".to_string()
        } else if count == 1 {
            "1 relevant memory".to_string()
        } else {
            format!("{} relevant memories", count)
        }
    }
}

impl Default for PromptAugmenter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::ContextAnchor;

    fn create_test_memory(
        user_input: &str,
        ai_output: &str,
        timestamp: i64,
        similarity: Option<f32>,
    ) -> MemoryEntry {
        let context = ContextAnchor::with_timestamp(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
            timestamp,
        );

        let mut entry = MemoryEntry::new(
            "test-id".to_string(),
            context,
            user_input.to_string(),
            ai_output.to_string(),
        );

        if let Some(score) = similarity {
            entry = entry.with_score(score);
        }

        entry
    }

    #[test]
    fn test_augmenter_creation() {
        let augmenter = PromptAugmenter::new();
        assert_eq!(augmenter.max_memories, 5);
        assert!(!augmenter.show_scores);
    }

    #[test]
    fn test_augmenter_with_config() {
        let augmenter = PromptAugmenter::with_config(3, true);
        assert_eq!(augmenter.max_memories, 3);
        assert!(augmenter.show_scores);
    }

    #[test]
    fn test_augment_prompt_no_memories() {
        let augmenter = PromptAugmenter::new();
        let result = augmenter.augment_prompt("You are an AI assistant", &[], "Hello");
        assert!(result.contains("You are an AI assistant"));
        assert!(result.contains("User: Hello"));
        assert!(!result.contains("Context History"));
    }

    #[test]
    fn test_augment_prompt_with_single_memory() {
        let augmenter = PromptAugmenter::new();
        let memory = create_test_memory(
            "What is Paris?",
            "Paris is the capital of France.",
            1703419815, // 2023-12-24 10:30:15 UTC
            Some(0.85),
        );

        let result = augmenter.augment_prompt(
            "You are a helpful assistant",
            &[memory],
            "Tell me more about Paris",
        );

        assert!(result.contains("You are a helpful assistant"));
        assert!(result.contains("Context History"));
        assert!(result.contains("What is Paris?"));
        assert!(result.contains("Paris is the capital of France"));
        assert!(result.contains("Tell me more about Paris"));
        // Similarity score should not be shown by default
        assert!(!result.contains("Similarity"));
    }

    #[test]
    fn test_augment_prompt_with_multiple_memories() {
        let augmenter = PromptAugmenter::new();
        let memories = vec![
            create_test_memory(
                "What is Python?",
                "Python is a programming language.",
                1703419815,
                Some(0.9),
            ),
            create_test_memory(
                "What is Rust?",
                "Rust is a systems programming language.",
                1703419915,
                Some(0.85),
            ),
        ];

        let result = augmenter.augment_prompt(
            "You are a code expert",
            &memories,
            "Compare Python and Rust",
        );

        assert!(result.contains("You are a code expert"));
        assert!(result.contains("What is Python?"));
        assert!(result.contains("What is Rust?"));
        assert!(result.contains("Compare Python and Rust"));
    }

    #[test]
    fn test_augment_prompt_respects_max_memories() {
        let augmenter = PromptAugmenter::with_config(2, false);

        let memories = vec![
            create_test_memory("Q1", "A1", 1703419815, Some(0.9)),
            create_test_memory("Q2", "A2", 1703419915, Some(0.85)),
            create_test_memory("Q3", "A3", 1703420015, Some(0.8)),
            create_test_memory("Q4", "A4", 1703420115, Some(0.75)),
        ];

        let result = augmenter.augment_prompt("System prompt", &memories, "Current query");

        // Should only include first 2 memories
        assert!(result.contains("Q1"));
        assert!(result.contains("Q2"));
        assert!(!result.contains("Q3"));
        assert!(!result.contains("Q4"));
    }

    #[test]
    fn test_augment_prompt_with_scores() {
        let augmenter = PromptAugmenter::with_config(5, true);
        let memory = create_test_memory("Test question", "Test answer", 1703419815, Some(0.92));

        let result = augmenter.augment_prompt("System prompt", &[memory], "New question");

        // Should show similarity score
        assert!(result.contains("Similarity: 0.92"));
    }

    #[test]
    fn test_format_memories_basic() {
        let augmenter = PromptAugmenter::new();
        let memory = create_test_memory(
            "What is the capital of France?",
            "The capital of France is Paris.",
            1703419815,
            None,
        );

        let formatted = augmenter.format_memories(vec![&memory].into_iter());

        assert!(formatted.contains("2023-12-24"));
        assert!(formatted.contains("User: What is the capital of France?"));
        assert!(formatted.contains("Assistant: The capital of France is Paris."));
    }

    #[test]
    fn test_format_memories_multiple() {
        let augmenter = PromptAugmenter::new();
        let memories = [
            create_test_memory("Q1", "A1", 1703419815, Some(0.9)),
            create_test_memory("Q2", "A2", 1703419915, Some(0.85)),
        ];

        let formatted = augmenter.format_memories(memories.iter());

        assert!(formatted.contains("Q1"));
        assert!(formatted.contains("A1"));
        assert!(formatted.contains("Q2"));
        assert!(formatted.contains("A2"));
        // Should have separator between memories
        assert!(formatted.matches("###").count() >= 2);
    }

    #[test]
    fn test_format_memories_with_scores() {
        let augmenter = PromptAugmenter::with_config(5, true);
        let memory = create_test_memory("Test", "Answer", 1703419815, Some(0.87));

        let formatted = augmenter.format_memories(vec![&memory].into_iter());

        assert!(formatted.contains("Similarity: 0.87"));
    }

    #[test]
    fn test_format_memories_trims_whitespace() {
        let augmenter = PromptAugmenter::new();
        let memory = create_test_memory(
            "  Question with spaces  ",
            "  Answer with spaces  ",
            1703419815,
            None,
        );

        let formatted = augmenter.format_memories(vec![&memory].into_iter());

        // Trimmed versions should appear
        assert!(formatted.contains("User: Question with spaces\n"));
        assert!(formatted.contains("Assistant: Answer with spaces"));
    }

    #[test]
    fn test_get_memory_summary_empty() {
        let augmenter = PromptAugmenter::new();
        let summary = augmenter.get_memory_summary(&[]);
        assert_eq!(summary, "No relevant memories");
    }

    #[test]
    fn test_get_memory_summary_single() {
        let augmenter = PromptAugmenter::new();
        let memory = create_test_memory("Q", "A", 1703419815, None);
        let summary = augmenter.get_memory_summary(&[memory]);
        assert_eq!(summary, "1 relevant memory");
    }

    #[test]
    fn test_get_memory_summary_multiple() {
        let augmenter = PromptAugmenter::new();
        let memories = vec![
            create_test_memory("Q1", "A1", 1703419815, None),
            create_test_memory("Q2", "A2", 1703419915, None),
            create_test_memory("Q3", "A3", 1703420015, None),
        ];
        let summary = augmenter.get_memory_summary(&memories);
        assert_eq!(summary, "3 relevant memories");
    }

    #[test]
    fn test_get_memory_summary_respects_max() {
        let augmenter = PromptAugmenter::with_config(2, false);
        let memories = vec![
            create_test_memory("Q1", "A1", 1703419815, None),
            create_test_memory("Q2", "A2", 1703419915, None),
            create_test_memory("Q3", "A3", 1703420015, None),
        ];
        let summary = augmenter.get_memory_summary(&memories);
        // Should report only max_memories count
        assert_eq!(summary, "2 relevant memories");
    }

    #[test]
    fn test_augment_prompt_preserves_structure() {
        let augmenter = PromptAugmenter::new();
        let memory = create_test_memory("Old Q", "Old A", 1703419815, Some(0.8));

        let result = augmenter.augment_prompt("System: Be helpful", &[memory], "New question");

        // Check structure: system prompt -> context -> separator -> user input
        let parts: Vec<&str> = result.split("\n\n").collect();
        assert!(parts.len() >= 4); // At least: system, context header, context, user
        assert!(parts[0].contains("System: Be helpful"));
        assert!(result.ends_with("User: New question"));
    }
}
