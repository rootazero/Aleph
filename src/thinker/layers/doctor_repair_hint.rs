//! `DoctorRepairHintLayer` — webchat-only "press f to repair" hint (priority 1715).
//!
//! Closes the detect→repair loop for the `/doctor` slash command: when the
//! model runs the read-only `doctor` tool and finds unresolved problems, this
//! layer asks it to end the reply by reminding the user they can press `f` to
//! start automatic repair. Gated to the WebRich paradigm because `f` is a
//! webchat-panel hotkey (`interfaces/webchat/.../state/hotkey.rs`) — CLI /
//! Telegram have no such key, so the hint must never reach them (R1/R4). The
//! model decides *whether* to surface it (only on unresolved problems) and
//! phrases it (R9); this layer only supplies the affordance.

use crate::thinker::interaction::InteractionParadigm;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct DoctorRepairHintLayer;

impl PromptLayer for DoctorRepairHintLayer {
    fn name(&self) -> &'static str {
        "doctor_repair_hint"
    }
    fn priority(&self) -> u32 {
        // Dynamic layers MUST live in the `>= 1700` per-request suffix zone so
        // the cacheable Stable prefix (priorities `< 1700`) is never split by a
        // dynamic layer — see `PromptPipeline::default_layers` and the
        // `stable_layers_come_before_dynamic` invariant. Sits with the other
        // paradigm-gated layer (`VoiceModeLayer` @1710).
        1715
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // `f` is a webchat-panel hotkey only — gate on WebRich so CLI /
        // Telegram / background runs never see a "press f" instruction.
        let is_web_panel = input
            .context
            .is_some_and(|ctx| ctx.environment_contract.paradigm == InteractionParadigm::WebRich);
        if is_web_panel {
            output.push_str(
                "## Self-Repair (web panel)\n\
\n\
After running the `doctor` tool in read-only mode (`fix=false`) and finding \
unresolved problems, end your reply by reminding the user they can press the \
`f` key to start automatic repair. If no problems remain, do not mention `f`.\n",
            );
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

    fn ctx_for(paradigm: InteractionParadigm) -> ResolvedContext {
        ContextAggregator::resolve(
            &InteractionManifest::new(paradigm),
            &SecurityContext::permissive(),
            &[],
        )
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        DoctorRepairHintLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn metadata() {
        let layer = DoctorRepairHintLayer;
        assert_eq!(layer.name(), "doctor_repair_hint");
        assert_eq!(layer.priority(), 1715);
        assert!(matches!(layer.stability(), LayerStability::Dynamic));
        // Guards the "dead on the cached path" regression class: a layer that
        // drops `Cached` from `paths()` silently vanishes in production while
        // every other test stays green (see RoleLayer/CitationStandardsLayer).
        assert!(layer.paths().contains(&AssemblyPath::Cached));
    }

    #[test]
    fn injects_for_web_panel() {
        let out = render(&ctx_for(InteractionParadigm::WebRich));
        assert!(out.contains("## Self-Repair"));
        assert!(out.contains("`f`"));
        assert!(out.contains("doctor"));
    }

    #[test]
    fn skips_non_web_paradigms() {
        for p in [
            InteractionParadigm::CLI,
            InteractionParadigm::Messaging,
            InteractionParadigm::Background,
            InteractionParadigm::Embedded,
        ] {
            assert!(
                render(&ctx_for(p)).is_empty(),
                "{p:?} must not see the press-f hint (R1/R4)"
            );
        }
    }

    #[test]
    fn skips_when_no_context() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        DoctorRepairHintLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
