//! MemoryAugmentationLayer — inject pre-fetched LanceDB memory context (priority 1575)
//!
//! Sits between WorkspaceFilesLayer (1550) and LanguageLayer (1600).
//! The async retrieval happens before prompt assembly; this layer only
//! formats and injects the pre-fetched results.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryAugmentationLayer;

impl PromptLayer for MemoryAugmentationLayer {
    fn name(&self) -> &'static str {
        "memory_augmentation"
    }

    fn priority(&self) -> u32 {
        1575
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.memory_context {
            Some(ctx) if !ctx.is_empty() => ctx,
            _ => return,
        };

        output.push_str(&ctx.format_for_prompt());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::PromptLayer as _;
    use crate::thinker::memory_context::{MemoryContext, MemorySummary};
    use crate::memory::store::types::ScoredFact;
    use crate::memory::context::{FactType, MemoryFact};

    #[test]
    fn metadata() {
        let layer = MemoryAugmentationLayer;
        assert_eq!(layer.name(), "memory_augmentation");
        assert_eq!(layer.priority(), 1575);
        assert!(layer.paths().contains(&AssemblyPath::Basic));
        assert!(layer.paths().contains(&AssemblyPath::Soul));
    }

    #[test]
    fn supports_full_and_compact_not_minimal() {
        let layer = MemoryAugmentationLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn skips_when_no_memory_context() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_empty_context() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let ctx = MemoryContext::default();
        let input = LayerInput::basic(&config, &[]).with_memory_context(&ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn injects_facts_and_memories() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();

        let fact = MemoryFact::new("User prefers dark mode".to_string(), FactType::Preference, vec![]);
        let ctx = MemoryContext {
            facts: vec![ScoredFact {
                fact,
                score: 0.9,
            }],
            memory_summaries: vec![MemorySummary {
                date: "2026-03-05".to_string(),
                user_input: "How to configure embedding?".to_string(),
                ai_output: "Use aleph.toml...".to_string(),
                score: 0.8,
            }],
            daily_notes: vec![],
        };

        let input = LayerInput::basic(&config, &[]).with_memory_context(&ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Relevant Memory"));
        assert!(out.contains("**Facts:**"));
        assert!(out.contains("User prefers dark mode"));
        assert!(out.contains("**Past Conversations:**"));
        assert!(out.contains("[2026-03-05]"));
        assert!(out.contains("How to configure embedding?"));
    }
}
