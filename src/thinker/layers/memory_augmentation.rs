//! MemoryAugmentationLayer — inject pre-rendered memory XML into the system prompt (priority 1740)
//!
//! Sits between IdentityFilesLayer (1550) and LanguageLayer (1600).
//!
//! Spec 3 migration: the layer now reads `LayerInput::memory_user_message` (a
//! pre-rendered XML string produced by
//! `MemoryContextProvider::build_memory_user_message`) instead of the legacy
//! `LayerInput::memory_context` field.  The legacy field is kept for backward
//! compatibility during the transition and will be removed in Task 5.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryAugmentationLayer;

impl PromptLayer for MemoryAugmentationLayer {
    fn name(&self) -> &'static str {
        "memory_augmentation"
    }

    fn priority(&self) -> u32 {
        1740
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
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
        // Spec 3: prefer the new pre-rendered XML message over the legacy MemoryContext.
        if let Some(text) = &input.memory_user_message {
            if !text.trim().is_empty() {
                output.push_str(text);
            }
            return;
        }

        // Legacy fallback — will be removed in Task 5.
        #[allow(deprecated)]
        if let Some(ctx) = input.memory_context {
            if !ctx.is_empty() {
                output.push_str(&ctx.format_for_prompt());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata() {
        let layer = MemoryAugmentationLayer;
        assert_eq!(layer.name(), "memory_augmentation");
        assert_eq!(layer.priority(), 1740);
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
    fn skips_when_no_memory() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_memory_user_message_is_empty() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_memory_user_message("   ".to_string());
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn injects_memory_user_message_verbatim() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let xml = "<memory><fact>User prefers dark mode</fact></memory>".to_string();
        let input = LayerInput::basic(&config, &[]).with_memory_user_message(xml.clone());
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert_eq!(out, xml);
    }

    #[test]
    fn memory_user_message_takes_precedence_over_legacy_context() {
        use crate::memory::context::{MemoryFact, NoteType};
        use crate::memory::store::types::ScoredFact;
        use crate::thinker::memory_context::{MemoryContext, MemorySummary};

        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();

        // Build a non-empty legacy context that would inject different text.
        let fact = MemoryFact::new(
            "should not appear".to_string(),
            NoteType::Preference,
            vec![],
        );
        #[allow(deprecated)]
        let ctx = MemoryContext {
            facts: vec![ScoredFact { fact, score: 0.9 }],
            memory_summaries: vec![MemorySummary {
                date: "2026-03-05".to_string(),
                user_input: "irrelevant".to_string(),
                ai_output: "irrelevant".to_string(),
                score: 0.8,
            }],
            structured_index: None,
        };

        let xml = "<memory><fact>new path</fact></memory>".to_string();
        let input = LayerInput::basic(&config, &[])
            .with_memory_user_message(xml.clone())
            .with_memory_context(&ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        // Only the new-path XML should appear, not the legacy context text.
        assert_eq!(out, xml);
        assert!(!out.contains("should not appear"));
    }

    #[cfg(test)]
    mod spec3_tests {
        use super::*;
        use crate::thinker::prompt_builder::PromptConfig;

        /// Task 4 spec: when `memory_user_message` is None and no legacy context,
        /// the layer must produce no output.
        #[test]
        fn no_injection_when_both_fields_absent() {
            let layer = MemoryAugmentationLayer;
            let config = PromptConfig::default();
            let input = LayerInput::basic(&config, &[]);
            let mut out = String::new();
            layer.inject(&mut out, &input);
            assert!(out.is_empty(), "expected empty output, got: {out:?}");
        }

        /// Task 4 spec: non-empty `memory_user_message` is injected verbatim.
        #[test]
        fn injects_new_path_xml() {
            let layer = MemoryAugmentationLayer;
            let config = PromptConfig::default();
            let xml =
                "<memory><fact>User prefers Rust</fact></memory>".to_string();
            let input =
                LayerInput::basic(&config, &[]).with_memory_user_message(xml.clone());
            let mut out = String::new();
            layer.inject(&mut out, &input);
            assert_eq!(out, xml);
        }
    }
}
