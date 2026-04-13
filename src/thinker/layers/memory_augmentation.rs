//! MemoryAugmentationLayer — inject pre-rendered memory XML into the system prompt (priority 1740)
//!
//! Sits between IdentityFilesLayer (1550) and LanguageLayer (1600).
//!
//! Reads `LayerInput::memory_user_message` (a pre-rendered XML string produced by
//! `MemoryContextProvider::build_memory_user_message`) and injects it verbatim
//! into the system prompt.

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
        if let Some(text) = &input.memory_user_message {
            if !text.trim().is_empty() {
                output.push_str(text);
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
