# Auto-Plan Trigger + Progress Narration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add one Stable prompt layer that teaches the model to autonomously decide when to use the `scratchpad` task list and to narrate progress (preamble + per-step recap) during interactive multi-step runs.

**Architecture:** A single new `PromptLayer` in `src/thinker/layers/multi_step_conduct.rs`, registered in `prompt_pipeline.rs`. Pure prompt guidance — zero harness change, zero deterministic judgment code, zero new dependencies. The streaming pipeline and `scratchpad` tool already exist; this layer only tells the model when/how to use them.

**Tech Stack:** Rust (crate `alephcore`), the `thinker` prompt-layer system (`PromptLayer` trait, `AssemblyPath`, `PromptMode`, `InteractionParadigm`/`Capability`).

## Global Constraints

- **One file + one registration.** Create `src/thinker/layers/multi_step_conduct.rs`; register in `src/thinker/layers/mod.rs` and `src/thinker/prompt_pipeline.rs`. No other files change.
- **Struct/layer identity:** `pub struct MultiStepConductLayer;`, `name()` = `"multi_step_conduct"`, `priority()` = `805`, `supports_mode` = `Full` only, `stability` = trait default (`Stable`, do **not** override), `paths()` = `[Basic, Hydration, Soul, Context, Cached]`.
- **Gating (whole layer, single gate):** `inject()` returns empty when `input.context` is `None` **or** when `ctx.environment_contract.active_capabilities` contains `Capability::SilentReply`. So Background/cron (SilentReply) render **nothing** → their prompt is byte-unchanged and `ALEPH_SILENT_COMPLETE` is untouched. Only interactive paradigms (WebRich/CLI/Messaging/Embedded without SilentReply) get the two sections.
- **R7/R9/R10:** intelligence lives in the prompt copy; the harness makes no completion/intent judgment and runs no extra LLM call. Layer lives in `src/thinker/`, never `src/harness/`.
- **Build policy (project cargo frugality):** the implementer **transcribes the exact code below, self-reviews, and commits — it does NOT run `cargo`/`cargo fmt`/`cargo test`.** The controller runs **one** targeted verification after the task (see Task 1 final step). Provided code is ≤100 cols and compiles as-is; any later `cargo fmt` reflow of the `use` import list is acceptable deferred churn (not required for this task).
- **Sorted-vec invariant:** `test_default_layers_sorted` asserts the registration `Vec` is in ascending `priority()` order (`<=`, ties allowed). The new `Box::new(MultiStepConductLayer)` (805) MUST be inserted between `OperationalGuidelinesLayer` (800) and `ProviderGuidanceLayer` (810).
- **Count invariant:** `test_default_layers_count` asserts the default pipeline has exactly 43 layers; this becomes 44.

---

## File Structure

- **Create** `src/thinker/layers/multi_step_conduct.rs` — the new layer + its unit tests. Single responsibility: inject multi-step planning + progress-narration guidance into interactive prompts.
- **Modify** `src/thinker/layers/mod.rs` — declare the module and re-export the layer (2 one-line edits).
- **Modify** `src/thinker/prompt_pipeline.rs` — import the layer (use-list), register it in the sorted `Vec`, bump the count assertion 43→44 (3 edits).

---

## Task 1: New `MultiStepConductLayer` (create + register + tests)

This is one cohesive deliverable: the layer is useless until registered, and registration trips two pipeline tests that must move with it. A reviewer reviews the prompt copy, the gate, and the wiring together.

**Files:**
- Create: `src/thinker/layers/multi_step_conduct.rs`
- Modify: `src/thinker/layers/mod.rs:59` and `src/thinker/layers/mod.rs:123`
- Modify: `src/thinker/prompt_pipeline.rs:11` (use-list), `:304-305` (registration), `:459` (count test)

**Interfaces:**
- Consumes: `PromptLayer` trait + `LayerInput` / `AssemblyPath` from `crate::thinker::prompt_layer`; `PromptMode` from `crate::thinker::prompt_mode`; `Capability` from `crate::thinker::interaction`. Reads `input.context: Option<&ResolvedContext>` and `ctx.environment_contract.active_capabilities: HashSet<Capability>` (same field `ProtocolTokensLayer` reads).
- Produces: `pub struct MultiStepConductLayer;` re-exported as `crate::thinker::layers::MultiStepConductLayer`, registered in `PromptPipeline::default_layers()`.

- [ ] **Step 1: Create the layer file with implementation + unit tests**

Create `src/thinker/layers/multi_step_conduct.rs` with exactly this content:

```rust
//! `MultiStepConductLayer` — teaches the model to plan multi-step work and to
//! narrate progress in interactive conversations (priority 805).
//!
//! Closes two prompt gaps:
//!   1. Nothing told the model *when* to autonomously reach for the
//!      `scratchpad` tool. `ExecutionPlanLayer` only re-surfaces a plan that
//!      already exists; it never triggers plan *creation*. So a task list only
//!      appeared when the user hand-typed a trigger phrase.
//!   2. Across a long run of tool calls the model emitted no visible text, so
//!      the interactive panel showed only a "thinking" spinner. The streaming
//!      pipeline already forwards every assistant delta live — the model just
//!      was never told to speak between steps.
//!
//! Both fixes are pure prompt guidance (R7/R9): the harness makes no
//! completion judgment and runs no extra LLM call. R10-safe — this lives in
//! `src/thinker/layers/`, not `src/harness/`.
//!
//! Gating mirrors `ProtocolTokensLayer`'s inverse: the whole layer is withheld
//! whenever the `SilentReply` capability is active (Background / cron), where
//! silent completion is the point and the prompt must stay byte-identical.

use crate::thinker::interaction::Capability;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MultiStepConductLayer;

impl PromptLayer for MultiStepConductLayer {
    fn name(&self) -> &'static str {
        "multi_step_conduct"
    }

    fn priority(&self) -> u32 {
        805
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        // Ride every non-minimal path; the `inject()` guard keeps output empty
        // when no `ResolvedContext` is attached or SilentReply is active.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let Some(ctx) = input.context else {
            return;
        };

        // Single gate: interactive paradigms only. When SilentReply is active
        // (Background / cron) emit nothing so those prompts stay byte-identical
        // and `ALEPH_SILENT_COMPLETE` (taught by ProtocolTokensLayer) is the
        // only protocol in play there.
        if ctx
            .environment_contract
            .active_capabilities
            .contains(&Capability::SilentReply)
        {
            return;
        }

        // Section 1 — when to plan.
        output.push_str("## Planning Multi-Step Work\n\n");
        output.push_str(
            "When a request genuinely needs several ordered steps, spans multiple phases, or \
             asks for more than one distinct thing, plan before you act: use the `scratchpad` \
             tool to set an objective and lay out an execution list, then work it one item at a \
             time with `start_item` / `complete_item`.\n\n",
        );
        output.push_str(
            "Do not plan trivial work. A direct answer, a single tool call, or anything that \
             finishes in one or two steps needs no scratchpad — just do it. Decide from the shape \
             of the task; don't wait to be told. Stay flexible: drop the plan if the task turns \
             out simpler than expected, or start one mid-task if it grows.\n\n",
        );

        // Section 2 — narrate progress (interactive only, same gate as above).
        output.push_str("## Narrate Your Progress\n\n");
        output.push_str(
            "This is an interactive conversation and the user is watching. Don't work silently \
             across many tool calls. In your visible reply (not hidden thinking):\n",
        );
        output.push_str(
            "- Before an action (or a batch of related actions), post a one-line preamble of what \
             you're about to do — roughly 8-12 words, e.g. \"Next, I'll set up the data \
             model.\".\n",
        );
        output.push_str(
            "- After finishing each plan step, post a brief recap, e.g. \"Done: the data model is \
             in place.\", so the user can follow along.\n\n",
        );
        output.push_str(
            "Keep these to a sentence or two — enough to show momentum, not so much that it \
             clutters the conversation.\n\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::context::{ContextAggregator, ResolvedContext};
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::{LayerInput, LayerStability};
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
        MultiStepConductLayer.inject(&mut out, &input);
        out
    }

    #[test]
    fn name_matches_module() {
        assert_eq!(MultiStepConductLayer.name(), "multi_step_conduct");
    }

    #[test]
    fn priority_is_805() {
        assert_eq!(MultiStepConductLayer.priority(), 805);
    }

    #[test]
    fn stability_is_stable_by_default() {
        assert!(matches!(
            MultiStepConductLayer.stability(),
            LayerStability::Stable
        ));
    }

    #[test]
    fn excluded_from_minimal_mode() {
        assert!(!MultiStepConductLayer.supports_mode(PromptMode::Minimal));
        assert!(MultiStepConductLayer.supports_mode(PromptMode::Full));
    }

    #[test]
    fn no_context_emits_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        MultiStepConductLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn interactive_paradigm_emits_both_sections() {
        let out = render(&ctx_for(InteractionParadigm::WebRich));
        assert!(out.contains("## Planning Multi-Step Work"));
        assert!(out.contains("scratchpad"));
        assert!(out.contains("## Narrate Your Progress"));
    }

    #[test]
    fn silent_paradigm_emits_nothing() {
        // Background carries SilentReply → whole layer is withheld so the
        // background prompt stays byte-identical.
        let out = render(&ctx_for(InteractionParadigm::Background));
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 2: Declare and re-export the module in `mod.rs`**

In `src/thinker/layers/mod.rs`, make these two exact edits.

Edit A — module declaration. Replace:

```rust
mod operational_guidelines;
```

with:

```rust
mod multi_step_conduct;
mod operational_guidelines;
```

Edit B — re-export. Replace:

```rust
pub use operational_guidelines::OperationalGuidelinesLayer;
```

with:

```rust
pub use multi_step_conduct::MultiStepConductLayer;
pub use operational_guidelines::OperationalGuidelinesLayer;
```

- [ ] **Step 3: Import the layer in `prompt_pipeline.rs`**

In `src/thinker/prompt_pipeline.rs`, the `use super::layers::{ ... };` block spans lines 6-17. Replace these two lines (currently lines 11-12):

```rust
    McpInstructionsLayer, MemoryAugmentationLayer, MemoryProtocolLayer, OperationalGuidelinesLayer,
    ProfileLayer, ProtocolTokensLayer, ProviderGuidanceLayer, RoleLayer, RuntimeCapabilitiesLayer,
```

with these three lines:

```rust
    McpInstructionsLayer, MemoryAugmentationLayer, MemoryProtocolLayer, MultiStepConductLayer,
    OperationalGuidelinesLayer, ProfileLayer, ProtocolTokensLayer, ProviderGuidanceLayer,
    RoleLayer, RuntimeCapabilitiesLayer,
```

- [ ] **Step 4: Register the layer in the sorted vec (between 800 and 810)**

In `src/thinker/prompt_pipeline.rs` (around line 304), replace:

```rust
            Box::new(OperationalGuidelinesLayer),
            Box::new(ProviderGuidanceLayer),
```

with:

```rust
            Box::new(OperationalGuidelinesLayer),
            Box::new(MultiStepConductLayer),
            Box::new(ProviderGuidanceLayer),
```

This keeps the vec in ascending priority order (800, 805, 810) so `test_default_layers_sorted` still passes.

- [ ] **Step 5: Bump the layer-count assertion 43 → 44**

In `src/thinker/prompt_pipeline.rs` (around line 459), replace:

```rust
        assert_eq!(pipeline.layer_count(), 43);
```

with:

```rust
        // → 44 (MultiStepConductLayer @805 Stable — autonomous scratchpad
        // planning + interactive progress narration, 2026-06-28).
        assert_eq!(pipeline.layer_count(), 44);
```

- [ ] **Step 6: Self-review, then commit (no cargo per build policy)**

Self-review checklist before committing:
- The new file matches the code block verbatim (struct name, `priority()` = 805, single SilentReply gate, both sections only after the gate).
- `mod.rs` has both the `mod` declaration and the `pub use`.
- `prompt_pipeline.rs`: import present, `Box::new(MultiStepConductLayer)` sits between `OperationalGuidelinesLayer` and `ProviderGuidanceLayer`, count is 44.
- No other files touched.

Commit:

```bash
git add src/thinker/layers/multi_step_conduct.rs src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs
git commit -m "thinker: add MultiStepConductLayer for auto-plan + progress narration"
```

- [ ] **Step 7: [CONTROLLER ONLY] targeted verification**

The controller (not the implementer) runs exactly one targeted test invocation:

```bash
cargo test -p alephcore --lib -- multi_step_conduct test_default_layers full_mode_includes_all_layers
```

Expected: PASS — the 7 `multi_step_conduct::tests::*`, `test_default_layers_count`, `test_default_layers_sorted`, and `full_mode_includes_all_layers` all green. If anything fails, dispatch a fix subagent with the failure output; do not add unrelated changes.

---

## Acceptance Gate (controller, post-implementation — not a code task)

Authoritative runtime QA, mirroring the prior Todo-panel SDD. After the unit gate is green, the controller (with user approval to restart the daemon) rebuilds `aleph-server` (re-embed nothing new — this is a core change, so `cargo build -p alephcore --bin aleph-server`), restarts the `:18790` daemon, and via chrome-devtools-mcp on `http://127.0.0.1:18790/`:

1. Send a genuinely multi-step request **without any trigger phrase** → confirm the model autonomously creates a scratchpad plan (Todo panel appears) AND emits preamble + per-step recap bubbles in the chat (not 50+ silent "thinking" turns).
2. Send a simple single-step question → confirm NO plan/panel is created (no over-triggering).
3. (Optional) Confirm a Background/cron path is unaffected — covered by the `silent_paradigm_emits_nothing` unit test, so runtime check is optional.

Not pushed/deployed unless the user directs it.

---

## Self-Review (writing-plans)

**1. Spec coverage:**
- Spec §4.1 layer contract (file, name, priority 805, Stable, Full, paths, single `!SilentReply` gate) → Global Constraints + Task 1 Steps 1-5. ✅
- Spec §4.2 prompt copy (section ① plan-trigger with explicit "don't plan trivial" negative; section ② preamble 8-12 words + per-step recap, visible reply not thinking) → Step 1 code. ✅
- Spec §6 success criteria → Acceptance Gate items 1-3 + unit tests. ✅
- Spec §7 tests (interactive both sections / Background empty / no-context empty / Minimal empty / priority+name+stability / pipeline count+sorted) → Step 1 tests + Step 7. ✅
- Spec §8 footprint (1 new file + registration) → File Structure. ✅
- Spec §9 non-goals (no harness/pipeline/scratchpad/panel change; no deterministic scoring; Background unchanged; no other surfaces) → Global Constraints + gate. ✅

**2. Placeholder scan:** No TBD/TODO; every code/edit step shows full content. ✅

**3. Type consistency:** `MultiStepConductLayer` / `multi_step_conduct` / `805` used identically across the file, `mod.rs`, `prompt_pipeline.rs` import, vec, and tests. Field path `ctx.environment_contract.active_capabilities.contains(&Capability::SilentReply)` matches `ProtocolTokensLayer`. Test helpers (`ContextAggregator::resolve`, `SecurityContext::permissive`, `LayerInput::basic(...).with_resolved_context_opt`) match `execution_plan.rs`'s proven pattern. ✅
