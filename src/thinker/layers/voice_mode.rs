//! `VoiceModeLayer` — injects voice-mode guidelines when active (priority 1710).
//!
//! Voice-as-Context: when the gateway marks the session as spoken (the inbound
//! router writes it to [`voice_mode`](crate::gateway::voice::voice_mode),
//! the harness bridge reads it into [`ResolvedContext::voice`]), this layer
//! reframes the whole reply contract for the ear instead of the eye.
//!
//! The guidance is *built from* [`VoiceContext`] rather than a fixed string, so
//! it adapts to what the turn actually knows. Today two signals drive it: the
//! reply is spoken (always, when active), and whether the user's message arrived
//! as ASR-transcribed speech (adds a transcription-repair instruction). The
//! builder is the seam where further spoken-context signals attach.

use crate::thinker::context::VoiceContext;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct VoiceModeLayer;

/// Build the spoken-reply guidelines section for the given [`VoiceContext`].
///
/// Kept as a builder (not a `const &str`) so the voice contract adapts to the
/// resolved context. The rules are deliberately language-neutral: the active
/// [`LanguageLayer`](crate::thinker::layers::LanguageLayer) already pins the
/// response language, so this layer only states *how* to speak it — never a
/// hardcoded English literal, which previously told a Chinese or Japanese voice
/// session to read numbers as English words.
///
/// When the turn's input was ASR-transcribed ([`VoiceContext::SpokenTranscribed`]),
/// an extra rule invites the model to repair transcription artifacts — the user
/// spoke, so the text it sees is speech-to-text output that may be garbled. This
/// is the one idea worth porting from reference voice agents; Aleph already
/// computes the signal, this layer is where it finally changes behavior.
fn build_voice_guidelines(voice: VoiceContext) -> String {
    // One owned String built once per voice turn; non-voice turns never call
    // this, keeping the prompt byte-identical when voice is off.
    let mut s = String::with_capacity(512);
    // Lean framing, not a numbered how-to manual: a capable model writes for
    // the ear from the signal alone, so the old 5-rule list with digit/currency/
    // URL counter-examples was cut as a cage (§1.1 prune-the-prompt). Only the
    // runtime-driven transcription-repair rule below carries a non-inferable
    // signal (the input is ASR output).
    s.push_str(
        "## Voice Mode\n\
\n\
This channel has voice mode enabled — your reply is read aloud, so write for the ear: \
speak conversationally in your reply language, keep it short and front-loaded, and render \
numbers, dates, currency and URLs in their spoken form (not digits or raw symbols). Skip \
anything that only works in writing — markdown, code, tables — and offer to send the text \
version instead.\n",
    );
    if voice.is_transcribed() {
        // Input came from speech-to-text; the transcript may contain artifacts.
        s.push_str(
            "The user's message was transcribed from speech, so it may contain recognition \
errors or missing words. Infer the intended meaning and silently repair obvious transcription \
slips; if the intent is genuinely unclear, ask one short spoken clarifying question rather \
than guessing.\n",
        );
    }
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
        &[AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // Read voice state from the ResolvedContext — the per-request context
        // actually threaded through the production cached prompt path (the same
        // channel `StandingGoalLayer` / `ExecutionPlanLayer` use).
        let voice = input.context.map_or(VoiceContext::Off, |ctx| ctx.voice);
        if voice.is_active() {
            output.push_str(&build_voice_guidelines(voice));
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext, VoiceContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::security_context::SecurityContext;

    fn ctx_with_voice(voice: VoiceContext) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.voice = voice;
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
        let out = render(&ctx_with_voice(VoiceContext::Spoken));
        assert!(out.contains("## Voice Mode"));
        assert!(out.contains("voice mode enabled"));
        // Functional enhancement: un-speakable content gets a verbal fallback.
        assert!(out.contains("offer to send the text version"));
    }

    #[test]
    fn skips_when_voice_inactive() {
        let out = render(&ctx_with_voice(VoiceContext::Off));
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
        let out = render(&ctx_with_voice(VoiceContext::Spoken));
        assert!(!out.contains("three thousand five hundred"));
        assert!(out.to_lowercase().contains("spoken"));
        assert!(out.contains("reply language"));
    }

    #[test]
    fn spoken_only_omits_transcription_repair() {
        // A typed message on a voice-output channel: write-for-the-ear rules
        // apply, but there is no transcript to repair.
        let out = render(&ctx_with_voice(VoiceContext::Spoken));
        assert!(out.contains("## Voice Mode"));
        assert!(!out.contains("transcribed from speech"));
    }

    #[test]
    fn transcribed_input_adds_repair_guidance() {
        // The user *spoke*: the input is speech-to-text and may be garbled.
        // This is the wired-through signal that was previously discarded.
        let out = render(&ctx_with_voice(VoiceContext::SpokenTranscribed));
        assert!(out.contains("## Voice Mode"));
        assert!(out.contains("transcribed from speech"));
        assert!(out.contains("repair obvious transcription slips"));
    }

    #[test]
    fn spoken_and_transcribed_share_the_base_contract() {
        // Adding the transcription rule must not alter the base guidance —
        // spoken-only turns stay byte-identical to their previous output.
        let spoken = render(&ctx_with_voice(VoiceContext::Spoken));
        let transcribed = render(&ctx_with_voice(VoiceContext::SpokenTranscribed));
        assert!(transcribed.starts_with(spoken.trim_end()));
    }
}
