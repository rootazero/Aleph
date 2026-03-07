//! Pre-fetched memory context for prompt injection.
//!
//! Memory retrieval is async (embedding + LanceDB), but PromptLayer::inject()
//! is sync. This struct holds pre-fetched results to bridge that gap.

use crate::memory::store::types::ScoredFact;

/// Daily memory entry from workspace/memory/YYYY-MM-DD.md files.
#[derive(Debug, Clone)]
pub struct DailyMemory {
    pub date: String,
    pub content: String,
}

/// Pre-fetched memory context ready for prompt injection.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 2 facts (compressed knowledge), sorted by relevance.
    pub facts: Vec<ScoredFact>,
    /// Layer 1 memory summaries (raw conversation excerpts).
    pub memory_summaries: Vec<MemorySummary>,
    /// Daily notes from workspace/memory/YYYY-MM-DD.md files.
    pub daily_notes: Vec<DailyMemory>,
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
        self.facts.is_empty() && self.memory_summaries.is_empty() && self.daily_notes.is_empty()
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

        if !self.daily_notes.is_empty() {
            output.push_str("**Recent Notes:**\n");
            for note in &self.daily_notes {
                output.push_str(&format!("### {}\n{}\n\n", note.date, note.content));
            }
        }

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
    fn test_daily_notes_not_empty() {
        let ctx = MemoryContext {
            daily_notes: vec![DailyMemory {
                date: "2026-03-07".to_string(),
                content: "Test note".to_string(),
            }],
            ..Default::default()
        };
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_daily_notes_format() {
        let ctx = MemoryContext {
            daily_notes: vec![
                DailyMemory {
                    date: "2026-03-07".to_string(),
                    content: "Morning standup notes".to_string(),
                },
                DailyMemory {
                    date: "2026-03-06".to_string(),
                    content: "Debug session log".to_string(),
                },
            ],
            ..Default::default()
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("Recent Notes"));
        assert!(prompt.contains("2026-03-07"));
        assert!(prompt.contains("Morning standup notes"));
        assert!(prompt.contains("2026-03-06"));
        assert!(prompt.contains("Debug session log"));
    }

    #[test]
    fn test_mixed_context_format() {
        let fact = ScoredFact {
            fact: MemoryFact::new(
                "Rust is great".to_string(),
                FactType::Preference,
                vec![],
            ),
            score: 0.9,
        };
        let ctx = MemoryContext {
            facts: vec![fact],
            memory_summaries: vec![],
            daily_notes: vec![DailyMemory {
                date: "2026-03-07".to_string(),
                content: "Daily note".to_string(),
            }],
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("Facts"));
        assert!(prompt.contains("Recent Notes"));
    }
}
