//! Pre-fetched memory context for prompt injection.
//!
//! Memory retrieval is async (embedding + LanceDB), but PromptLayer::inject()
//! is sync. This struct holds pre-fetched results to bridge that gap.

use crate::memory::store::types::ScoredFact;

/// Structured memory index content (e.g., from .aleph/MEMORY.md)
#[derive(Debug, Clone, Default)]
pub struct StructuredMemoryIndex {
    /// Raw content from MEMORY.md (truncated to 200 lines / 25KB)
    pub content: String,
    /// Whether the content was truncated
    pub truncated: bool,
}

/// Pre-fetched memory context ready for prompt injection.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 2 facts (compressed knowledge), sorted by relevance.
    pub facts: Vec<ScoredFact>,
    /// Layer 1 memory summaries (raw conversation excerpts).
    pub memory_summaries: Vec<MemorySummary>,
    /// Optional structured index (e.g., from MEMORY.md).
    pub structured_index: Option<StructuredMemoryIndex>,
}

/// A brief summary of a past conversation for prompt injection.
#[derive(Debug, Clone)]
pub struct MemorySummary {
    /// Date string (YYYY-MM-DD)
    pub date: String,
    /// User's question/input (truncated)
    pub user_input: String,
    /// AI's response (truncated)
    pub ai_output: String,
    /// Similarity score
    pub score: f32,
}

impl MemoryContext {
    /// Whether there is any content to inject.
    pub fn is_empty(&self) -> bool {
        let has_structured = self
            .structured_index
            .as_ref()
            .map_or(false, |s| !s.content.is_empty());
        !has_structured && self.facts.is_empty() && self.memory_summaries.is_empty()
    }

    /// Format into a prompt section string (hybrid: structured index + vector retrieval).
    pub fn format_for_prompt(&self) -> String {
        let has_structured = self
            .structured_index
            .as_ref()
            .map_or(false, |s| !s.content.is_empty());
        let has_vector = !self.facts.is_empty() || !self.memory_summaries.is_empty();

        if !has_structured && !has_vector {
            return String::new();
        }

        let mut output = String::from("## Memory Context\n\n");

        // Path 1: Structured index
        if let Some(ref index) = self.structured_index {
            if !index.content.is_empty() {
                output.push_str("### Index (structured)\n\n");
                output.push_str(&index.content);
                if index.truncated {
                    output.push_str("\n[... truncated ...]\n");
                }
                output.push_str("\n\n");
            }
        }

        // Path 2: Vector retrieval results
        if has_vector {
            output.push_str("### Relevant Memories (semantic)\n\n");
            for sf in &self.facts {
                output.push_str(&format!("- [{:.2}] {}\n", sf.score, sf.fact.content));
            }
            for ms in &self.memory_summaries {
                output.push_str(&format!(
                    "- [{:.2}] [{}] Q: {} → A: {}\n",
                    ms.score, ms.date, ms.user_input, ms.ai_output
                ));
            }
            output.push('\n');
        }

        // Taxonomy guidelines
        output.push_str("### Memory Guidelines\n\n");
        output.push_str("Memory categories: user (preferences), project (goals/status), feedback (corrections), reference (external pointers).\n");
        output.push_str("Save important context. Update stale memories. Delete outdated ones.\n\n");

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    #[test]
    fn test_empty_context() {
        let ctx = MemoryContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.format_for_prompt(), "");
    }

    #[test]
    fn test_mixed_context_format() {
        let fact = ScoredFact {
            fact: MemoryFact::new("Rust is great".to_string(), FactType::Preference, vec![]),
            score: 0.9,
        };
        let ctx = MemoryContext {
            facts: vec![fact],
            memory_summaries: vec![],
            structured_index: None,
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("## Memory Context"));
        assert!(prompt.contains("Rust is great"));
    }

    #[test]
    fn test_hybrid_format() {
        let fact = ScoredFact {
            fact: MemoryFact::new(
                "User prefers dark mode".to_string(),
                FactType::Preference,
                vec![],
            ),
            score: 0.92,
        };
        let ctx = MemoryContext {
            facts: vec![fact],
            memory_summaries: vec![],
            structured_index: Some(StructuredMemoryIndex {
                content: "- [Role](user/role.md) — data scientist".into(),
                truncated: false,
            }),
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("## Memory Context"));
        assert!(prompt.contains("### Index (structured)"));
        assert!(prompt.contains("data scientist"));
        assert!(prompt.contains("### Relevant Memories (semantic)"));
        assert!(prompt.contains("[0.92]"));
        assert!(prompt.contains("### Memory Guidelines"));
    }

    #[test]
    fn test_structured_only() {
        let ctx = MemoryContext {
            facts: vec![],
            memory_summaries: vec![],
            structured_index: Some(StructuredMemoryIndex {
                content: "- [Project](project/arch.md) — Rust core".into(),
                truncated: true,
            }),
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("### Index (structured)"));
        assert!(prompt.contains("[... truncated ...]"));
    }
}
