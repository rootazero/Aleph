//! `TimerLoopLayer` — emits `<timer_loop>` at priority 1753 (Dynamic).
//!
//! Re-surfaces the session's active timer loop into the system prompt every
//! turn while it runs — the clock-gated sibling of `StandingGoalLayer`
//! (1755). Between ticks the model had no idea a watch was running in this
//! session (the tick reminder exists only inside tick turns), so on an
//! ordinary user turn it could neither report on the loop nor avoid starting
//! a duplicate. Pure scaffolding: the content is the user's own watch prompt
//! plus the loop's own status, rendered verbatim. No judgment, no LLM call.
//! `None` emits nothing, leaving the prompt byte-identical for sessions with
//! no loop.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct TimerLoopLayer;

impl PromptLayer for TimerLoopLayer {
    fn name(&self) -> &'static str {
        "timer_loop"
    }

    fn priority(&self) -> u32 {
        1753
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        mode != PromptMode::Minimal
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };
        let Some(timer_loop) = ctx.timer_loop.as_deref() else {
            return;
        };
        if timer_loop.is_empty() {
            return;
        }
        output.push_str("<timer_loop>\n");
        // Escaped at the seam: this body carries the user's own `loop(prompt=…)`
        // text, so an unescaped `</timer_loop>` in it would close this element and
        // let the payload forge top-level prompt sections (e.g. a fake
        // `Approval mode: full` bullet). `xml_util` is the single source of truth
        // for that defense — see its module docs.
        output.push_str(&crate::thinker::xml_util::escape_xml(timer_loop));
        output.push_str("\n</timer_loop>\n\n");
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

    fn ctx_with_loop(summary: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.timer_loop = summary.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        TimerLoopLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn no_loop_emits_nothing() {
        assert!(render(&ctx_with_loop(None)).is_empty());
    }

    #[test]
    fn loop_summary_renders_inside_tag() {
        let out = render(&ctx_with_loop(Some(
            "check deploy (Loop active, cadence: every 5m, ticks: 2/20)",
        )));
        assert!(out.starts_with("<timer_loop>\n"));
        assert!(out.contains("check deploy"));
        assert!(out.trim_end().ends_with("</timer_loop>"));
    }

    #[test]
    fn empty_summary_emits_nothing() {
        assert!(render(&ctx_with_loop(Some(""))).is_empty());
    }

    #[test]
    fn name_and_priority() {
        assert_eq!(TimerLoopLayer.name(), "timer_loop");
        assert_eq!(TimerLoopLayer.priority(), 1753);
    }
}
