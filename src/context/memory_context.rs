//! Memory context data for prompt building.

/// Aggregated memory context retrieved for the current turn.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    pub facts: Vec<MemoryFact>,
    pub past_conversations: Vec<ConversationSnippet>,
}

/// A single recalled fact from long-term memory.
#[derive(Debug, Clone)]
pub struct MemoryFact {
    pub content: String,
    pub category: String,
    pub relevance_score: f32,
}

/// A snippet from a past conversation session.
#[derive(Debug, Clone)]
pub struct ConversationSnippet {
    pub date: String,
    pub user_input: String,
    pub ai_output: String,
}

impl MemoryContext {
    /// Returns true if no facts or past conversations are present.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.past_conversations.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_memory_context_is_empty() {
        let ctx = MemoryContext::default();
        assert!(ctx.is_empty());
    }

    #[test]
    fn memory_context_with_facts_is_not_empty() {
        let ctx = MemoryContext {
            facts: vec![MemoryFact {
                content: "user prefers dark mode".to_string(),
                category: "preference".to_string(),
                relevance_score: 0.9,
            }],
            past_conversations: Vec::new(),
        };
        assert!(!ctx.is_empty());
    }

    #[test]
    fn memory_context_with_conversations_is_not_empty() {
        let ctx = MemoryContext {
            facts: Vec::new(),
            past_conversations: vec![ConversationSnippet {
                date: "2026-03-30".to_string(),
                user_input: "hello".to_string(),
                ai_output: "hi there".to_string(),
            }],
        };
        assert!(!ctx.is_empty());
    }
}
