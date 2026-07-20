//! `CuratedMemoryLayer` — inject the frozen curated memory envelope (priority 60).
//!
//! Sits between `AgentRoleLayer` (55) and `ProfileLayer` (75) in the Stable zone.
//!
//! Reads `LayerInput::curated_memory_envelope` (a pre-rendered XML string
//! produced by `MemoryContextProvider::build_curated_message`) and injects
//! it verbatim. The envelope is captured once per session and reused, so
//! this layer is `LayerStability::Stable` to preserve the prompt prefix
//! cache across turns.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};

pub struct CuratedMemoryLayer;

impl PromptLayer for CuratedMemoryLayer {
    fn name(&self) -> &'static str {
        "curated_memory"
    }

    fn priority(&self) -> u32 {
        60
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Stable
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(text) = &input.curated_memory_envelope {
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
        let layer = CuratedMemoryLayer;
        assert_eq!(layer.name(), "curated_memory");
        assert_eq!(layer.priority(), 60);
        assert_eq!(layer.stability(), LayerStability::Stable);
        assert!(layer.paths().contains(&AssemblyPath::Soul));
        assert!(layer.paths().contains(&AssemblyPath::Basic));
    }

    #[test]
    fn skips_when_envelope_absent() {
        let layer = CuratedMemoryLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_envelope_blank() {
        let layer = CuratedMemoryLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_curated_envelope(Some("   ".to_string()));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn injects_envelope_verbatim() {
        let layer = CuratedMemoryLayer;
        let config = PromptConfig::default();
        let envelope =
            "<CuratedMemory>fact</CuratedMemory>\n<UserProfile>name</UserProfile>".to_string();
        let input = LayerInput::basic(&config, &[]).with_curated_envelope(Some(envelope.clone()));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert_eq!(out, envelope);
    }
}
