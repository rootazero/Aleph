//! `RuntimeContextLayer` — micro-environmental awareness (priority 1720)
//!
//! Sits at 1720 to deconflict from `VoiceModeLayer` (1710): both rode 1710,
//! which left their relative order resolved only by registration sequence
//! (a latent ordering hazard). The harness wiring (`prompt_build.rs`) and the
//! build-path test already document this layer as 1720 — the code now agrees.
//!
//! Renders the **codex-aligned `<environment_context>` XML block** whose
//! shape mirrors codex's `FileSystemContext::render()` from
//! `codex/codex-rs/core/src/context/environment_context.rs`: a single tag,
//! with body sub-elements for cwd / repo / git / model / time, optionally
//! nested `<parent kind="…">id</parent>` for sub-agent runs. The XML
//! boundary buys two things the pipe-separated markdown line that used to
//! ship here could not: (a) a tag-delimited region downstream tooling can
//! match on (`<environment_context>` start/end, same role
//! `markdown_excerpt::split_wikilinks` plays for note excerpts), and (b) a
//! hard character boundary so the model cannot be tricked into "closing"
//! the section and forging a sibling at top prompt scope.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct RuntimeContextLayer;

impl PromptLayer for RuntimeContextLayer {
    fn name(&self) -> &'static str {
        "runtime_context"
    }
    fn priority(&self) -> u32 {
        1720
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // Ride every non-minimal path; `ResolvedContext` is threaded on the
        // Basic / Cached production routes and the inject() guard keeps output
        // empty when it (or its `runtime_context`) is absent.
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        let Some(ref runtime_ctx) = ctx.runtime_context else {
            return;
        };
        // Belt-and-braces: skip emitting the XML when there is nothing inside
        // it worth rendering (internal-tooling path that called `collect()`
        // with no model handed in). Cheap, and keeps the prompt byte-identical
        // for paths that have always been empty.
        if !runtime_ctx.has_dynamic_facts() {
            return;
        }

        // Parent binding comes from `ResolvedContext` via the envelope's
        // `parent` field. The mapping is established by the harness bridge
        // (`prompt_build::resolve_prompt_context`) — we only render. A `None`
        // here drops the `<parent>` element entirely; this is the common path
        // (primary user-facing session) so the prompt stays byte-identical
        // for the 99 % case.
        let parent = ctx.envelope_parent.as_ref().map(|p| (p.kind.as_str(), p.id.as_str()));
        let block = runtime_ctx.to_environment_context_block(parent);
        output.push_str(&block);
        output.push_str("\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_runtime_context_no_context() {
        let layer = RuntimeContextLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools); // no context
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn priority_is_1720_deconflicted_from_voice_mode() {
        // Regression: this layer used to return 1710, colliding with
        // `VoiceModeLayer` (also 1710). 1720 is an otherwise-empty slot
        // (next neighbour is 1730), so the assembled prompt order is
        // unchanged but the relative ordering is now priority-explicit.
        assert_eq!(RuntimeContextLayer.priority(), 1720);
        assert_ne!(
            RuntimeContextLayer.priority(),
            crate::thinker::layers::VoiceModeLayer.priority(),
        );
    }

    #[test]
    fn test_runtime_context_paths() {
        let paths = RuntimeContextLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn graceful_noop_on_basic_path_without_context() {
        let layer = RuntimeContextLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
