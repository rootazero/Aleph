//! ToolRuntimeStateLayer — emits `<tool_runtime_state>` XML at priority 502.
//!
//! Sits immediately after `ToolsLayer` (500) and `HydratedToolsLayer` (501).
//! Surfaces per-tool runtime state (depth limits, sandbox availability,
//! "unavailable: reason" hints) so the LLM has live context that the
//! static JSON schema can't carry. R9 in action: intelligence in the
//! prompt, not in the wire-format schema.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ToolRuntimeStateLayer;

impl PromptLayer for ToolRuntimeStateLayer {
    fn name(&self) -> &'static str {
        "tool_runtime_state"
    }

    fn priority(&self) -> u32 {
        502
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        if ctx.runtime_state_blocks.is_empty() {
            return;
        }
        output.push_str("<tool_runtime_state>\n");
        for fragment in &ctx.runtime_state_blocks {
            output.push_str(&fragment.render_xml());
        }
        output.push_str("</tool_runtime_state>\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_layer::LayerInput;
    use crate::thinker::security_context::SecurityContext;
    use crate::tools::runtime_state::{RuntimeStateFragment, ToolStatus};

    fn make_ctx_with_blocks(blocks: Vec<RuntimeStateFragment>) -> ResolvedContext {
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.runtime_state_blocks = blocks;
        ctx
    }

    #[test]
    fn layer_priority_is_502() {
        assert_eq!(ToolRuntimeStateLayer.priority(), 502);
    }

    #[test]
    fn layer_name_matches_module() {
        assert_eq!(ToolRuntimeStateLayer.name(), "tool_runtime_state");
    }

    #[test]
    fn empty_blocks_emit_nothing() {
        let ctx = make_ctx_with_blocks(vec![]);
        let config = crate::thinker::prompt_builder::PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        ToolRuntimeStateLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn available_block_renders() {
        let blocks = vec![RuntimeStateFragment::available(
            "delegate_task",
            vec!["depth 2 of 4".into()],
        )];
        let ctx = make_ctx_with_blocks(blocks);
        let config = crate::thinker::prompt_builder::PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        ToolRuntimeStateLayer.inject(&mut out, &input);
        assert!(out.starts_with("<tool_runtime_state>"));
        assert!(out.contains("<tool name=\"delegate_task\">"));
        assert!(out.contains("<hint>depth 2 of 4</hint>"));
        assert!(out.contains("</tool_runtime_state>"));
    }

    #[test]
    fn unavailable_block_renders_with_status_attr() {
        let blocks = vec![RuntimeStateFragment {
            tool_name: "send_telegram".into(),
            status: ToolStatus::Unavailable {
                reason: "bridge offline".into(),
            },
            hints: vec![],
        }];
        let ctx = make_ctx_with_blocks(blocks);
        let config = crate::thinker::prompt_builder::PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        ToolRuntimeStateLayer.inject(&mut out, &input);
        assert!(out.contains("status=\"unavailable\""));
        assert!(out.contains("<hint>bridge offline</hint>"));
    }

    #[test]
    fn multiple_blocks_render_in_order() {
        let blocks = vec![
            RuntimeStateFragment::available("a", vec!["one".into()]),
            RuntimeStateFragment::available("b", vec!["two".into()]),
        ];
        let ctx = make_ctx_with_blocks(blocks);
        let config = crate::thinker::prompt_builder::PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        ToolRuntimeStateLayer.inject(&mut out, &input);
        let a_pos = out.find("name=\"a\"").expect("a present");
        let b_pos = out.find("name=\"b\"").expect("b present");
        assert!(a_pos < b_pos, "expected a before b in output: {out}");
    }
}
