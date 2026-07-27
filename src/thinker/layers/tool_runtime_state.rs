//! `ToolRuntimeStateLayer` — emits `<tool_runtime_state>` XML at priority 1703.
//!
//! Surfaces per-tool runtime state (depth limits, sandbox availability,
//! "unavailable: reason" hints) so the LLM has live context that the
//! static JSON schema can't carry. R9 in action: intelligence in the
//! prompt, not in the wire-format schema.
//!
//! # Why this is Dynamic, and why it was a cache bug
//!
//! Its input is a snapshot of the tool catalog's `ToolHealthCache` — a mutable,
//! 30s-TTL structure. This layer nonetheless sat at priority 502 and declared no
//! `stability()`, so it inherited the trait default of `Stable` and rode inside
//! the **cached prefix**. One MCP health probe flipping a tool from healthy to
//! unhealthy therefore rewrote a byte of the cached prefix mid-session, silently
//! invalidating the whole conversation's prompt cache — and the Anthropic
//! adapter's own comment asserted the opposite was true.
//!
//! It is per-request state, so it belongs in the Dynamic suffix zone
//! (priority >= 1700) alongside the other live-state layers, where changing
//! bytes cost only themselves. The pipeline's `stable_layers_come_before_dynamic`
//! invariant is what pins that zoning.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct ToolRuntimeStateLayer;

impl PromptLayer for ToolRuntimeStateLayer {
    fn name(&self) -> &'static str {
        "tool_runtime_state"
    }

    fn priority(&self) -> u32 {
        1703
    }

    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        mode != PromptMode::Minimal
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
        );
        ctx.runtime_state_blocks = blocks;
        ctx
    }

    /// Live tool-health state must sit in the Dynamic suffix zone (>= 1700).
    /// At 502 with the default `Stable` it rode the CACHED prefix, so a 30s-TTL
    /// health probe flipping a tool rewrote the cached prefix mid-session and
    /// invalidated the whole conversation's prompt cache.
    #[test]
    fn layer_is_dynamic_and_lives_in_the_per_request_zone() {
        assert_eq!(ToolRuntimeStateLayer.priority(), 1703);
        assert!(ToolRuntimeStateLayer.priority() >= 1700);
        assert_eq!(ToolRuntimeStateLayer.stability(), LayerStability::Dynamic);
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
