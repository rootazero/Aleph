//! `VoiceModeLayer` — injects voice mode guidelines when active (priority 1710)

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
        // Read from the ResolvedContext — the per-request context actually
        // threaded through the production cached prompt path (the same channel
        // `StandingGoalLayer` / `ExecutionPlanLayer` use). The legacy
        // `input.inbound` is never populated in production (no
        // `PromptBuilder.inbound` field, no production caller), so reading it
        // left this layer permanently dead.
        let active = input
            .context
            .is_some_and(|ctx| ctx.voice_mode_active);
        if active {
            output.push_str(VOICE_MODE_PROMPT);
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_with_voice(active: bool) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.voice_mode_active = active;
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        VoiceModeLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn metadata() {
        let layer = VoiceModeLayer;
        assert_eq!(layer.name(), "voice_mode");
        assert_eq!(layer.priority(), 1710);
        assert!(matches!(layer.stability(), LayerStability::Dynamic));
    }

    #[test]
    fn injects_when_voice_active() {
        let out = render(&ctx_with_voice(true));
        assert!(out.contains("## Voice Mode"));
        assert!(out.contains("voice mode enabled"));
    }

    #[test]
    fn skips_when_voice_inactive() {
        let out = render(&ctx_with_voice(false));
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_no_context() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
