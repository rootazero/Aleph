//! `ChainContextLayer` — surfaces subagent call-chain position to the LLM.
//!
//! `HarnessDeps.chain_context` is propagated through every subagent spawn
//! (see `agents::subagent_spawner`), but until now it was invisible to
//! the model itself: a depth-3 subagent had no idea it was nested. This
//! layer renders a small section when the chain is non-root so the LLM
//! can reason about delegation budget ("I'm depth 2 of max 5 — should I
//! spawn another worker or finish the job myself?").
//!
//! Root chains (`depth == 0`) intentionally emit nothing — the root agent
//! is the default assumption and noise here would just bloat every prompt
//! a top-level conversation produces.
//!
//! # Why there is no chain id in here
//!
//! The section used to end with ``- Chain id: `ch-…` (use this when reporting
//! back to the parent).`` Both of R9's rules reject it:
//!
//! 1. It is not a runtime fact the model can act on — it asks the model to
//!    perform an action the system gives it no interface for. `chain_id` has
//!    exactly two consumers in the whole repo (`agents::runtime` uses it as a
//!    transcript filename and to rebuild `parent_chain`); **no tool schema
//!    accepts one**, and a subagent's structured output has no such field. It
//!    does not even identify the child: `ChainContext::child` copies the
//!    parent's id verbatim.
//! 2. No single tool owns the sentence, so there is nothing to move it to.
//!
//! Deleting it was not about the ~45 bytes. A subagent is built through
//! `AssemblyPath::Basic`, which produces one unsplit system string, and the
//! Anthropic adapter puts its lone cache breakpoint over that whole block. A
//! fresh `chain_id` per parent run therefore re-keyed the entire prefix, so
//! every spawn re-paid `tools` (~82 KB of builtin descriptions alone) plus the
//! full system prompt at the 1.25× cache-creation rate with `cache_read`
//! pinned at zero, and OpenAI's `derive_prompt_cache_key` landed each spawn in
//! a different backend bucket. It was the only per-spawn-varying byte on that
//! path, so removing it is what makes repeated spawns of the same role share a
//! warm prefix. The symptom was visible only on the bill.
//!
//! `prompt_contract::basic_path_prefix_is_stable_across_spawns` pins this.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Priority 1702 sits early in the per-request Dynamic zone, just before
/// `AgentCatalogLayer` (1704). Late enough that identity/profile have already
/// established who the agent is, early enough that any later guidance can
/// reference "as noted in your delegation chain above".
const CHAIN_CONTEXT_PRIORITY: u32 = 1702;

pub struct ChainContextLayer;

impl PromptLayer for ChainContextLayer {
    fn name(&self) -> &'static str {
        "chain_context"
    }

    fn priority(&self) -> u32 {
        CHAIN_CONTEXT_PRIORITY
    }

    fn stability(&self) -> LayerStability {
        // Dynamic: depth changes per spawn, so this section can never live
        // in the cached stable prefix.
        LayerStability::Dynamic
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // Full only — Compact/Minimal strip orchestration metadata to
        // keep token budget tight (matches the pipeline policy applied
        // to other delegation-related layers like agent_catalog).
        matches!(mode, PromptMode::Full)
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let chain = match input.chain_context {
            Some(c) => c,
            None => return,
        };
        // Root agents already know they're the root; emit nothing rather
        // than telling every top-level prompt "depth=0/5".
        if chain.is_root() {
            return;
        }

        let remaining = chain.max_depth.saturating_sub(chain.depth);
        output.push_str("## Delegation Chain\n\n");
        output.push_str(&format!(
            "- You are running as a **sub-agent at depth {}** of an active delegation chain.\n",
            chain.depth
        ));
        // NO chain id here. It told the model to "use this when reporting back
        // to the parent" while no tool schema anywhere accepts a chain id, so
        // the instruction was unexecutable — and it was also the ONLY
        // per-spawn-varying byte on the whole Basic path, i.e. the single
        // string that made every subagent request a cache miss. See the module
        // doc.
        if remaining == 0 {
            output.push_str(
                "- **Depth budget exhausted** — you cannot spawn further child agents. \
                 Complete the task yourself or hand structured findings back to the parent.\n",
            );
        } else if remaining == 1 {
            output.push_str(
                "- You may spawn **at most 1 more** nested child before the chain limit is reached.\n",
            );
        } else {
            output.push_str(&format!(
                "- You may spawn up to **{remaining} more** nested children before the chain limit \
                 ({max}) is reached.\n",
                remaining = remaining,
                max = chain.max_depth
            ));
        }
        output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::chain_context::ChainContext;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata() {
        let layer = ChainContextLayer;
        assert_eq!(layer.name(), "chain_context");
        assert_eq!(layer.priority(), 1702);
        assert_eq!(layer.stability(), LayerStability::Dynamic);
        assert!(layer.paths().contains(&AssemblyPath::Basic));
    }

    #[test]
    fn supports_full_only() {
        let layer = ChainContextLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn skips_when_no_chain_attached() {
        let layer = ChainContextLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_for_root_chain() {
        let layer = ChainContextLayer;
        let config = PromptConfig::default();
        let root = ChainContext::new();
        assert!(root.is_root());
        let input = LayerInput::basic(&config, &[]).with_chain_context(&root);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty(), "root chain should produce no section");
    }

    #[test]
    fn emits_for_single_handoff() {
        let layer = ChainContextLayer;
        let config = PromptConfig::default();
        let root = ChainContext::new();
        let child = root.child().unwrap();
        let input = LayerInput::basic(&config, &[]).with_chain_context(&child);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Delegation Chain"));
        assert!(out.contains("depth 1"));
        // Default max_depth is 5; depth=1 should leave 4 remaining.
        assert!(out.contains("up to **4 more**"));
    }

    /// The rendered section must contain nothing that varies between two spawns
    /// with the same shape.
    ///
    /// This layer is the only one on `AssemblyPath::Basic` that ever held a
    /// per-spawn value, and Basic produces one unsplit system block under a
    /// single cache breakpoint — so one varying byte here costs the whole
    /// tools+system prefix, every spawn, with no error and no failing test.
    /// Asserts byte equality rather than "does not contain the id" so a future
    /// timestamp/uuid/counter is caught too.
    #[test]
    fn the_section_is_byte_identical_across_two_chains() {
        let layer = ChainContextLayer;
        let config = PromptConfig::default();
        let a = ChainContext::new().child().unwrap();
        let b = ChainContext::new().child().unwrap();
        assert_ne!(a.chain_id, b.chain_id, "the two chains must really differ");

        let mut out_a = String::new();
        layer.inject(
            &mut out_a,
            &LayerInput::basic(&config, &[]).with_chain_context(&a),
        );
        let mut out_b = String::new();
        layer.inject(
            &mut out_b,
            &LayerInput::basic(&config, &[]).with_chain_context(&b),
        );

        assert_eq!(
            out_a, out_b,
            "a per-spawn byte in this layer silently re-keys every subagent's \
             prompt cache; the symptom shows up only on the bill"
        );
    }

    #[test]
    fn warns_when_depth_budget_exhausted() {
        let layer = ChainContextLayer;
        let config = PromptConfig::default();
        // max_depth=2 → at depth 2 the budget is fully used.
        let root = ChainContext {
            max_depth: 2,
            ..ChainContext::new()
        };
        let c1 = root.child().unwrap();
        let c2 = c1.child().unwrap();
        let input = LayerInput::basic(&config, &[]).with_chain_context(&c2);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("Depth budget exhausted"));
        assert!(out.contains("depth 2"));
    }

    #[test]
    fn singular_remaining_phrasing_when_one_left() {
        let layer = ChainContextLayer;
        let config = PromptConfig::default();
        let root = ChainContext {
            max_depth: 2,
            ..ChainContext::new()
        };
        let c1 = root.child().unwrap();
        let input = LayerInput::basic(&config, &[]).with_chain_context(&c1);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("at most 1 more"));
        assert!(!out.contains("up to **1 more**"));
    }
}
