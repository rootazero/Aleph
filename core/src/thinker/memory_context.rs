//! Pre-fetched memory context for prompt injection.
//!
//! Memory retrieval is async (embedding + LanceDB), but PromptLayer::inject()
//! is sync. This struct holds pre-fetched results to bridge that gap.

use crate::memory::store::types::ScoredFact;

/// Pre-fetched memory context ready for prompt injection.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 2 facts (compressed knowledge), sorted by relevance.
    pub facts: Vec<ScoredFact>,
    /// Layer 1 memory summaries (raw conversation excerpts).
    pub memory_summaries: Vec<MemorySummary>,
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
        self.facts.is_empty() && self.memory_summaries.is_empty()
    }

    /// Format into a prompt section string.
    pub fn format_for_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut output = String::from("## Relevant Memory\n\n");

        if !self.facts.is_empty() {
            output.push_str("**Facts:**\n");
            for sf in &self.facts {
                output.push_str(&format!("- {}\n", sf.fact.content));
            }
            output.push('\n');
        }

        if !self.memory_summaries.is_empty() {
            output.push_str("**Past Conversations:**\n");
            for ms in &self.memory_summaries {
                output.push_str(&format!(
                    "- [{}] Q: {} A: {}\n",
                    ms.date, ms.user_input, ms.ai_output
                ));
            }
            output.push('\n');
        }

        output
    }
}
