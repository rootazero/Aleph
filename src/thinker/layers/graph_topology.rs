//! `GraphTopologyLayer` — emits `<loop_graph_context>` at priority 1754.
//!
//! A governed session is TOLD its place in the governance topology every
//! turn — who watches it, who owns its reference (and the proposal-note
//! path for changing it), which anchors ground it, and the human root
//! reference verbatim. The model never has to "remember" the graph.
//!
//! Cache discipline (the production trap this file must not repeat): the
//! rendered content comes from graph rows only — no clocks, no counters —
//! so an unchanged graph leaves the prompt byte-identical. Ungoverned
//! sessions (`None`) emit nothing: zero cost for everyone outside the
//! graph. Mirrors `StandingGoalLayer` (same paths incl. `Cached`; Dynamic
//! stability because the content is session-scoped state, while the
//! byte-stability requirement is carried by the deterministic render).

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct GraphTopologyLayer;

impl PromptLayer for GraphTopologyLayer {
    fn name(&self) -> &'static str {
        "graph_topology"
    }

    fn priority(&self) -> u32 {
        1754
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
        let Some(topology) = ctx.graph_topology.as_deref() else {
            return;
        };
        if topology.is_empty() {
            return;
        }
        output.push_str("<loop_graph_context>\n");
        output.push_str(topology);
        output.push_str("</loop_graph_context>\n\n");
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

    fn ctx_with_topology(t: Option<&str>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.graph_topology = t.map(|s| s.to_string());
        ctx
    }

    fn render(ctx: &ResolvedContext) -> String {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(ctx));
        let mut out = String::new();
        GraphTopologyLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn ungoverned_session_emits_nothing() {
        assert!(render(&ctx_with_topology(None)).is_empty());
        assert!(render(&ctx_with_topology(Some(""))).is_empty());
    }

    #[test]
    fn governed_session_renders_inside_tag() {
        let out = render(&ctx_with_topology(Some(
            "本会话是循环治理图中的节点 goal:s1。\n根参照 root:aleph: 用户时间 > token 成本\n",
        )));
        assert!(out.starts_with("<loop_graph_context>\n"));
        assert!(out.contains("root:aleph"));
        assert!(out.trim_end().ends_with("</loop_graph_context>"));
    }

    #[test]
    fn name_and_priority_slot_before_standing_goal() {
        assert_eq!(GraphTopologyLayer.name(), "graph_topology");
        assert_eq!(GraphTopologyLayer.priority(), 1754);
    }
}
