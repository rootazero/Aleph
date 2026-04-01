//! VoiceModeLayer — injects voice mode guidelines when active (priority 1710)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct VoiceModeLayer;

const VOICE_MODE_PROMPT: &str = r#"## Voice Mode

Current Channel has voice mode enabled. Your replies will be converted to speech. Guidelines:

1. Narrate your actions briefly before and after tool use (e.g., "Let me check that...", "Found it")
2. Use conversational, spoken-language style — avoid markdown, code blocks, tables
3. Organize long replies in natural paragraphs, keep each concise
4. Express numbers and URLs in spoken form ("about three thousand five hundred" not "3,500")
"#;

impl PromptLayer for VoiceModeLayer {
    fn name(&self) -> &'static str {
        "voice_mode"
    }
    fn priority(&self) -> u32 {
        1710
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let active = input
            .inbound
            .map(|ctx| ctx.voice_mode_active)
            .unwrap_or(false);
        if active {
            output.push_str(VOICE_MODE_PROMPT);
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::inbound_context::InboundContext;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;

    #[test]
    fn metadata() {
        let layer = VoiceModeLayer;
        assert_eq!(layer.name(), "voice_mode");
        assert_eq!(layer.priority(), 1710);
        assert!(matches!(layer.stability(), LayerStability::Dynamic));
    }

    #[test]
    fn injects_when_voice_active() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let inbound = InboundContext {
            voice_mode_active: true,
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]).with_inbound(&inbound);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Voice Mode"));
        assert!(out.contains("voice mode enabled"));
    }

    #[test]
    fn skips_when_voice_inactive() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let inbound = InboundContext {
            voice_mode_active: false,
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]).with_inbound(&inbound);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_no_inbound() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
