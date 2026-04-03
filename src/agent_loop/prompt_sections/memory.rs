//! Memory section — context from long-term memory.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};
use crate::context::MemoryContext;

pub fn render(ctx: &MemoryContext) -> PromptSection {
    if ctx.is_empty() {
        return PromptSection {
            name: "memory".into(),
            stability: Stability::Dynamic,
            priority: 1700,
            protected: false,
            content: String::new(),
        };
    }

    let mut parts = Vec::new();

    if !ctx.facts.is_empty() {
        let lines: Vec<String> = ctx
            .facts
            .iter()
            .map(|f| format!("- [{}] {}", f.category, f.content))
            .collect();
        parts.push(format!("**Relevant Facts:**\n{}", lines.join("\n")));
    }

    if !ctx.past_conversations.is_empty() {
        let lines: Vec<String> = ctx
            .past_conversations
            .iter()
            .map(|c| format!("- [{}] {} → {}", c.date, c.user_input, c.ai_output))
            .collect();
        parts.push(format!("**Past Conversations:**\n{}", lines.join("\n")));
    }

    PromptSection {
        name: "memory".into(),
        stability: Stability::Dynamic,
        priority: 1700,
        protected: false,
        content: format!("# Context from Memory\n\n{}", parts.join("\n\n")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ConversationSnippet, MemoryFact};

    #[test]
    fn empty_context_produces_empty_content() {
        let ctx = MemoryContext::default();
        let section = render(&ctx);
        assert!(section.content.is_empty());
        assert_eq!(section.name, "memory");
        assert_eq!(section.priority, 1700);
    }

    #[test]
    fn renders_facts() {
        let ctx = MemoryContext {
            facts: vec![MemoryFact {
                content: "User prefers dark mode".into(),
                category: "preference".into(),
                relevance_score: 0.9,
            }],
            past_conversations: Vec::new(),
        };
        let section = render(&ctx);
        assert!(section
            .content
            .contains("[preference] User prefers dark mode"));
        assert!(section.content.contains("Relevant Facts"));
    }

    #[test]
    fn renders_past_conversations() {
        let ctx = MemoryContext {
            facts: Vec::new(),
            past_conversations: vec![ConversationSnippet {
                date: "2026-03-30".into(),
                user_input: "hello".into(),
                ai_output: "hi there".into(),
            }],
        };
        let section = render(&ctx);
        assert!(section.content.contains("[2026-03-30] hello → hi there"));
        assert!(section.content.contains("Past Conversations"));
    }

    #[test]
    fn renders_both_facts_and_conversations() {
        let ctx = MemoryContext {
            facts: vec![MemoryFact {
                content: "fact".into(),
                category: "env".into(),
                relevance_score: 0.5,
            }],
            past_conversations: vec![ConversationSnippet {
                date: "2026-01-01".into(),
                user_input: "q".into(),
                ai_output: "a".into(),
            }],
        };
        let section = render(&ctx);
        assert!(section.content.contains("Relevant Facts"));
        assert!(section.content.contains("Past Conversations"));
    }
}
