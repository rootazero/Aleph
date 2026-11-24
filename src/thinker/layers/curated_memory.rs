//! `CuratedMemoryLayer` — inject the frozen curated memory envelope (priority 60).
//!
//! Sits between `AgentRoleLayer` (55) and `ProfileLayer` (75) in the Stable zone.
//!
//! Reads `LayerInput::curated_memory_envelope` (a pre-rendered XML string
//! produced by `MemoryContextProvider::build_curated_message`) and injects
//! its CONTENT verbatim. The only thing the layer adds is the blank-line
//! separator that keeps the next layer from gluing onto the envelope's last
//! line (same house pattern as `memory_protocol`), so the envelope text
//! itself is never rewritten. The envelope is captured once per session and
//! reused, so this layer is `LayerStability::Stable` to preserve the prompt
//! prefix cache across turns.

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
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(text) = &input.curated_memory_envelope {
            if !text.trim().is_empty() {
                output.push_str(text);
                if !text.ends_with("\n\n") {
                    if text.ends_with('\n') {
                        output.push('\n');
                    } else {
                        output.push_str("\n\n");
                    }
                }
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
    fn injects_envelope_content_verbatim_then_one_separator() {
        let layer = CuratedMemoryLayer;
        let config = PromptConfig::default();
        let envelope =
            "<CuratedMemory>fact</CuratedMemory>\n<UserProfile>name</UserProfile>".to_string();
        let input = LayerInput::basic(&config, &[]).with_curated_envelope(Some(envelope.clone()));
        let mut out = String::new();
        layer.inject(&mut out, &input);
        // The envelope content is verbatim; the layer only appends the
        // blank-line separator the next layer needs.
        assert_eq!(out, format!("{envelope}\n\n"));
        // An envelope that already carries the separator is not extended.
        let mut out = String::new();
        layer.inject(
            &mut out,
            &LayerInput::basic(&config, &[]).with_curated_envelope(Some(format!("{envelope}\n\n"))),
        );
        assert_eq!(out, format!("{envelope}\n\n"));
    }
}
