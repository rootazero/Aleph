//! `VoiceModeLayer` — injects voice-mode guidelines when active (priority 1710).
//!
//! Voice-as-Context: when the gateway marks the session as spoken (the inbound
//! router writes it to [`session_mode`](crate::gateway::voice::session_mode),
//! the harness bridge reads it into [`ResolvedContext::voice_mode_active`]), this
//! layer reframes the whole reply contract for the ear instead of the eye. The
//! guidance is built from the resolved context rather than a fixed string so it
//! adapts to what the turn actually knows (today: active/inactive; the builder is
//! the single seam where future spoken-context signals attach).

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct VoiceModeLayer;

/// Build the spoken-reply guidelines section.
///
/// Kept as a builder (not a `const &str`) so the voice contract adapts to the
/// resolved context. The rules are deliberately language-neutral: the active
/// [`LanguageLayer`](crate::thinker::layers::LanguageLayer) already pins the
/// response language, so this layer only states *how* to speak it — never a
/// hardcoded English literal, which previously told a Chinese or Japanese voice
/// session to read numbers as English words.
fn build_voice_guidelines() -> String {
    // One owned String built once per voice turn; non-voice turns never call
    // this, keeping the prompt byte-identical when voice is off.
    let mut s = String::with_capacity(640);
    s.push_str(
        "## Voice Mode\n\
\n\
This channel has voice mode enabled — your reply is read aloud as speech. \
Write for the ear, not the eye:\n\
\n\
1. Speak conversationally. Drop markdown, code fences, tables, bullet symbols \
and emoji — they are read aloud literally and sound like noise.\n\
2. Render numbers, dates, currency and URLs as they are *spoken* in your reply \
language, never as written digits or raw symbols (say the words aloud, not \
\"3,500\", \"$20\", or \"https://example.com\").\n\
3. Keep replies short and front-load the answer — a listener cannot skim, and an \
over-long reply is cut off before it finishes. Use natural sentences and \
paragraphs, not lists.\n\
4. Narrate tool use briefly so silence is filled (e.g. \"Let me check that…\", \
\"Found it\").\n\
5. If something only works in writing — code, a long table, a precise URL — \
describe it in a sentence and offer to send the text version instead of reading \
it out.\n",
    );
    s
}

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
        let active = input.context.is_some_and(|ctx| ctx.voice_mode_active);
        if active {
            output.push_str(&build_voice_guidelines());
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
        // Functional enhancement: un-speakable content gets a verbal fallback.
        assert!(out.contains("offer to send the text version"));
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

    #[test]
    fn number_guidance_is_language_neutral() {
        // Regression: the old static prompt hardcoded the English literal
        // "three thousand five hundred", which mis-instructed non-English voice
        // sessions. The guidance must now state the spoken-form principle
        // without baking in an English number word.
        let out = render(&ctx_with_voice(true));
        assert!(!out.contains("three thousand five hundred"));
        assert!(out.to_lowercase().contains("spoken"));
        assert!(out.contains("reply language"));
    }
}
