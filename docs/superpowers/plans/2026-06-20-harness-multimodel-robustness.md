# Harness Multi-Model Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the blunt "halt the run on a tool loop" behavior with progress-aware (distinctness) detection that steers misbehaving models via the existing veto→grace machinery, tuned per model family, so weak models recover instead of producing nothing.

**Architecture:** A per-model `ModelRobustnessProfile` is resolved per run at the orchestrator layer (`runner_impl`, where the active model is known) and threaded through `HarnessDeps` → `TurnVerifyContext` → `ToolLoopVerifier`. The verifier reads thresholds from the profile, uses tool-call **distinctness** (distinct `(name,args_hash)` / window) to tell fan-out from thrash, and emits `Veto` (steer) instead of `Halt` for the silent-thrash tier — reusing the harness's existing veto-cap→grace-turn salvage path for partial delivery. `Halt` is retained only for Tier-1 identical loops, and its grace turn's orphaned-tool-call bug is fixed so partial delivery actually works.

**Tech Stack:** Rust (tokio + serde), `alephcore` crate. Verifier seam in `src/verification/`; harness loop in `src/harness/`; per-run assembly in `src/orchestrator/harness_bridge/runner_impl.rs`.

## Global Constraints

- **R10 (thin harness)**: New cognition is forbidden in `src/harness/`. The detector stays in `src/verification/`; the profile is resolved at the orchestrator layer; the loop only *reads* thresholds (like it already reads `max_iterations`). No intent classification, no completion judgment, no recovery-strategy selection inside the loop.
- **R10 file budget**: Do NOT add files under `src/harness/`. New types go in `src/verification/`.
- **cargo 节制**: Do NOT run full test suites. At most ONE `cargo check -p alephcore --lib` per task to verify compilation. Unit tests run targeted: `cargo test -p alephcore --lib <module>`.
- **Commit format**: English, `<scope>: <description>` (e.g. `verification: add ModelRobustnessProfile`).
- **Immutability**: prefer returning new values; profile is `Copy`.
- **Serde only**, no second async runtime, MSRV 1.95.
- **Byte-compatibility**: the `conservative()` default profile MUST reproduce today's behavior (veto@5 identical, halt@8, 10 steers) so unchanged-model runs don't regress.
- **Window invariant**: `TOOL_HISTORY_WINDOW = 8` is the single source of truth (`src/verification/turn_verifier.rs:33`); thresholds clamp to it.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `src/verification/robustness_profile.rs` | `ModelRobustnessProfile` type, defaults, per-behavior table, clamp | **Create** |
| `src/verification/mod.rs` | re-export `ModelRobustnessProfile` | Modify |
| `src/verification/turn_verifier.rs` | add `robustness_profile` field to `TurnVerifyContext` | Modify |
| `src/verification/tool_loop_verifier.rs` | distinctness detection; Tier-2 → `Veto`; read profile from ctx | Modify |
| `src/harness/deps.rs` | add `robustness_profile: ModelRobustnessProfile` to `HarnessDeps` | Modify |
| `src/harness/agent/think.rs` | `run_verifiers` populates `robustness_profile` into ctx | Modify |
| `src/harness/agent.rs` | `MAX_VERIFIER_VETOS` → `deps.robustness_profile.steer_max` | Modify |
| `src/orchestrator/harness_bridge/runner_impl.rs` | resolve profile per run from active model behavior | Modify |
| HarnessDeps literal sites (subagent_spawner, agent.rs ctors, tests) | add new field | Modify |

---

## Task 1: `ModelRobustnessProfile` type + per-behavior table

**Files:**
- Create: `src/verification/robustness_profile.rs`
- Modify: `src/verification/mod.rs` (re-export)
- Test: inline `#[cfg(test)]` in `robustness_profile.rs`

**Interfaces:**
- Produces: `ModelRobustnessProfile { repeat_threshold: usize, halt_threshold: usize, steer_max: usize, novelty_min: f32, silence_required: bool }` (derives `Clone, Copy, Debug, PartialEq`); `ModelRobustnessProfile::conservative() -> Self`; `ModelRobustnessProfile::for_behavior(Option<&str>) -> Self`; `ModelRobustnessProfile::clamped(self) -> Self`; `impl Default`.

- [ ] **Step 1: Write the failing test** — create `src/verification/robustness_profile.rs` with ONLY the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_matches_legacy_behavior() {
        let p = ModelRobustnessProfile::conservative();
        assert_eq!(p.repeat_threshold, 5);
        assert_eq!(p.halt_threshold, 8); // TOOL_HISTORY_WINDOW
        assert_eq!(p.steer_max, 10);
        assert!(p.silence_required);
    }

    #[test]
    fn for_behavior_anthropic_is_loose() {
        let p = ModelRobustnessProfile::for_behavior(Some("anthropic"));
        assert!(p.steer_max >= ModelRobustnessProfile::conservative().steer_max);
        assert!(p.novelty_min <= ModelRobustnessProfile::conservative().novelty_min);
    }

    #[test]
    fn for_behavior_ollama_is_tight() {
        let p = ModelRobustnessProfile::for_behavior(Some("ollama"));
        assert!(p.repeat_threshold < ModelRobustnessProfile::conservative().repeat_threshold);
        assert!(p.steer_max < ModelRobustnessProfile::conservative().steer_max);
    }

    #[test]
    fn for_behavior_unknown_is_conservative() {
        assert_eq!(
            ModelRobustnessProfile::for_behavior(None),
            ModelRobustnessProfile::conservative()
        );
        assert_eq!(
            ModelRobustnessProfile::for_behavior(Some("mystery-model")),
            ModelRobustnessProfile::conservative()
        );
    }

    #[test]
    fn clamped_enforces_window_invariants() {
        let bad = ModelRobustnessProfile {
            repeat_threshold: 99,
            halt_threshold: 1,
            steer_max: 0,
            novelty_min: 5.0,
            silence_required: true,
        }
        .clamped();
        assert!(bad.repeat_threshold >= 2 && bad.repeat_threshold <= 8);
        assert!(bad.halt_threshold >= bad.repeat_threshold && bad.halt_threshold <= 8);
        assert!(bad.steer_max >= 1);
        assert!(bad.novelty_min >= 0.0 && bad.novelty_min <= 1.0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib robustness_profile`
Expected: FAIL — `cannot find type ModelRobustnessProfile` (module not yet declared / type missing).

- [ ] **Step 3: Write minimal implementation** — prepend the implementation above the test module in `src/verification/robustness_profile.rs`:

```rust
//! Per-model robustness profile — tunes the tool-loop watchdog thresholds to
//! the active model family. Resolved per run at the orchestrator layer (where
//! the model is known) and threaded into `TurnVerifyContext`. The harness loop
//! only *reads* it, never decides with it (R10-safe).

use crate::verification::turn_verifier::TOOL_HISTORY_WINDOW;

/// Tunable thresholds for `ToolLoopVerifier`, keyed off the active model's
/// behavior family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelRobustnessProfile {
    /// Tier-1 veto threshold: identical (name+args) consecutive calls.
    pub repeat_threshold: usize,
    /// Hard-halt threshold for the Tier-1 identical run (within the window).
    pub halt_threshold: usize,
    /// Max consecutive steers (vetoes) before the harness forces a wrap-up
    /// grace turn. Replaces the old global `MAX_VERIFIER_VETOS` const.
    pub steer_max: usize,
    /// Tier-2 fires only when window distinctness < this ratio (0.0..=1.0).
    /// Lower = more tolerant of fan-out before flagging a thrash.
    pub novelty_min: f32,
    /// Tier-2 requires the turn to carry no narration text.
    pub silence_required: bool,
}

impl ModelRobustnessProfile {
    /// Conservative default — byte-compatible with pre-change behavior.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            repeat_threshold: 5,
            halt_threshold: TOOL_HISTORY_WINDOW,
            steer_max: 10,
            novelty_min: 0.5,
            silence_required: true,
        }
    }

    /// Resolve a profile by model-behavior name (same name the prompt layer
    /// resolves via `protocol_to_behavior` / `model_behavior_override`).
    #[must_use]
    pub fn for_behavior(name: Option<&str>) -> Self {
        match name {
            // Strong instruction-followers: loose — they rarely loop.
            Some("anthropic") => Self {
                repeat_threshold: 5,
                halt_threshold: TOOL_HISTORY_WINDOW,
                steer_max: 12,
                novelty_min: 0.35,
                silence_required: true,
            },
            // Weak / local models: tight — steer earlier, fewer chances.
            Some("ollama") => Self {
                repeat_threshold: 3,
                halt_threshold: TOOL_HISTORY_WINDOW,
                steer_max: 6,
                novelty_min: 0.6,
                silence_required: false,
            },
            // openai / gemini / unknown: conservative default.
            _ => Self::conservative(),
        }
    }

    /// Clamp to the ring-buffer window invariants so a misconfigured profile
    /// can never silently disable detection.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.repeat_threshold = self.repeat_threshold.clamp(2, TOOL_HISTORY_WINDOW);
        self.halt_threshold = self
            .halt_threshold
            .clamp(self.repeat_threshold, TOOL_HISTORY_WINDOW);
        self.steer_max = self.steer_max.max(1);
        self.novelty_min = self.novelty_min.clamp(0.0, 1.0);
        self
    }
}

impl Default for ModelRobustnessProfile {
    fn default() -> Self {
        Self::conservative()
    }
}
```

Then declare + re-export in `src/verification/mod.rs` (add next to the other `pub mod`/`pub use` lines):

```rust
pub mod robustness_profile;
pub use robustness_profile::ModelRobustnessProfile;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib robustness_profile`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add src/verification/robustness_profile.rs src/verification/mod.rs
git commit -m "verification: add ModelRobustnessProfile (per-model loop thresholds)"
```

---

## Task 2: Thread `robustness_profile` through HarnessDeps → TurnVerifyContext

**Files:**
- Modify: `src/verification/turn_verifier.rs` (add field to `TurnVerifyContext`)
- Modify: `src/harness/deps.rs` (add field to `HarnessDeps`)
- Modify: `src/harness/agent/think.rs:1697-1724` (`run_verifiers` populates ctx)
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs` (resolve per run, set on deps)
- Modify (add field to literal): `src/agents/subagent_spawner/mod.rs:338`, `src/harness/agent.rs:1122,1330,1402,1468`, `src/harness/tests/chain.rs:80`, `src/harness/tests/think.rs:241,300`

**Interfaces:**
- Consumes: `ModelRobustnessProfile` (Task 1).
- Produces: `TurnVerifyContext.robustness_profile: ModelRobustnessProfile`; `HarnessDeps.robustness_profile: ModelRobustnessProfile`.

- [ ] **Step 1: Write the failing test** — add to `src/verification/tool_loop_verifier.rs` test module a compile-level test that the context carries a profile (this fails to compile until the field exists):

```rust
#[cfg(test)]
mod profile_wiring_tests {
    use super::*;
    use crate::verification::turn_verifier::{TurnVerifyContext, ToolCallSummary};
    use crate::verification::ModelRobustnessProfile;

    fn ctx_with<'a>(
        calls: &'a [ToolCallSummary],
        profile: ModelRobustnessProfile,
        text: Option<&'a str>,
    ) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 0,
            tool_calls_made: 0,
            final_text: text,
            recent_tool_calls: calls,
            stop_reason: None,
            session_id: None,
            robustness_profile: profile,
        }
    }

    #[test]
    fn context_carries_profile() {
        let calls: Vec<ToolCallSummary> = Vec::new();
        let c = ctx_with(&calls, ModelRobustnessProfile::conservative(), None);
        assert_eq!(c.robustness_profile.repeat_threshold, 5);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib tool_loop_verifier`
Expected: FAIL — compile error: `TurnVerifyContext` has no field `robustness_profile`.

- [ ] **Step 3: Add the field to `TurnVerifyContext`** — in `src/verification/turn_verifier.rs`, inside `pub struct TurnVerifyContext<'a>`, after the `session_id` field (line ~65):

```rust
    /// Per-model robustness thresholds resolved for THIS run at the
    /// orchestrator layer. The verifier reads its thresholds from here so a
    /// shared verifier instance can be tuned per run/model without per-run
    /// reconstruction. Defaults to `conservative()` in contexts that don't
    /// resolve a model (tests / rollback).
    pub robustness_profile: crate::verification::ModelRobustnessProfile,
```

- [ ] **Step 4: Add the field to `HarnessDeps`** — in `src/harness/deps.rs`, inside `pub struct HarnessDeps`, after the `verifier_chain` field (line ~43):

```rust
    /// Per-model robustness profile for the active run's model. Resolved by
    /// `harness_bridge::runner_impl` (where the model is known) and read by
    /// `run_verifiers` when building `TurnVerifyContext`. Defaults to
    /// `conservative()` at construction sites that don't resolve a model.
    pub robustness_profile: crate::verification::ModelRobustnessProfile,
```

- [ ] **Step 5: Populate ctx in `run_verifiers`** — in `src/harness/agent/think.rs`, the `run_verifiers` function (around line 1711) builds `TurnVerifyContext`. Add the field:

```rust
        let ctx = TurnVerifyContext {
            iterations,
            tool_calls_made,
            final_text: if final_text.is_empty() {
                None
            } else {
                Some(final_text)
            },
            recent_tool_calls: &snapshot,
            stop_reason,
            session_id: Some(session_key),
            robustness_profile: self.deps.robustness_profile,
        };
```

- [ ] **Step 6: Resolve the profile per run in `runner_impl`** — in `src/orchestrator/harness_bridge/runner_impl.rs`, in the `HarnessDeps { ... }` literal (around line 252, next to `verifier_chain: self.verifier_chain.clone(),`), add:

```rust
            // Per-model loop-watchdog thresholds. Resolve from the active
            // provider's behavior family (same key the prompt layer uses).
            robustness_profile: crate::verification::ModelRobustnessProfile::for_behavior(
                llm.model_behavior_override()
                    .or_else(|| crate::providers::model_behaviors::protocol_to_behavior(llm.protocol())),
            )
            .clamped(),
```

> NOTE for the implementer: confirm `llm` (the `Arc<dyn AiProvider>` for this run) exposes `model_behavior_override()` and `protocol()` in this scope (it is used as `llm.name()` at line 116). If `model_behavior_override` returns `Option<&str>` and `protocol()` returns `&str`, the above compiles as written. If the provider handle differs, resolve `behavior_name: Option<&str>` the same way `inner.rs` does for the "Model behavior resolved" log and pass it in.

- [ ] **Step 7: Add the field to every other `HarnessDeps` literal** — each of these sites constructs `HarnessDeps { ... }` and must add one line. Use `conservative()` (these are non-model-resolving contexts: subagent spawn inherits, tests):

Sites (from `grep -n 'HarnessDeps {'`):
- `src/agents/subagent_spawner/mod.rs:338`
- `src/harness/agent.rs:1122`, `:1330`, `:1402`, `:1468`
- `src/harness/tests/chain.rs:80`
- `src/harness/tests/think.rs:241`, `:300`

Add to each literal:

```rust
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
```

(In test modules where `crate::` resolves awkwardly, use `ModelRobustnessProfile::conservative()` with an appropriate `use`.)

> For `subagent_spawner`: a subagent could later inherit the parent's profile, but that's out of scope (YAGNI). `conservative()` is correct for now.

- [ ] **Step 8: Run test + compile check**

Run: `cargo test -p alephcore --lib tool_loop_verifier` then `cargo check -p alephcore --lib`
Expected: PASS + clean compile (all `HarnessDeps`/`TurnVerifyContext` literals now have the field).

- [ ] **Step 9: Commit**

```bash
git add src/verification/turn_verifier.rs src/harness/deps.rs src/harness/agent/think.rs src/orchestrator/harness_bridge/runner_impl.rs src/agents/subagent_spawner/mod.rs src/harness/agent.rs src/harness/tests/
git commit -m "harness: thread per-run ModelRobustnessProfile into TurnVerifyContext"
```

---

## Task 3: Distinctness detection + Tier-2 emits `Veto` (not `Halt`)

**Files:**
- Modify: `src/verification/tool_loop_verifier.rs` (rewrite `verify` to read profile + distinctness)
- Test: inline `#[cfg(test)]` in `tool_loop_verifier.rs`

**Interfaces:**
- Consumes: `TurnVerifyContext.robustness_profile` (Task 2), existing `trailing_repeat_run`, `trailing_same_name_run`, `TOOL_HISTORY_WINDOW`.
- Produces: new behavior — Tier-2 returns `Veto` instead of `Halt`; new `fn distinct_count(&[ToolCallSummary]) -> usize`.

- [ ] **Step 1: Write the failing tests** — add to the `tool_loop_verifier.rs` test module. These encode the four behaviors (helper `ctx_with` from Task 2 Step 1 is reused; ensure it's in scope or duplicate it):

```rust
#[cfg(test)]
mod distinctness_tests {
    use super::*;
    use crate::verification::turn_verifier::{TurnVerifyContext, ToolCallSummary};
    use crate::verification::ModelRobustnessProfile;
    use tokio_util::sync::CancellationToken;

    fn call(name: &str, args: u64) -> ToolCallSummary {
        ToolCallSummary { name: name.to_string(), args_hash: args }
    }

    fn ctx<'a>(
        calls: &'a [ToolCallSummary],
        profile: ModelRobustnessProfile,
        text: Option<&'a str>,
    ) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 0,
            tool_calls_made: 0,
            final_text: text,
            recent_tool_calls: calls,
            stop_reason: None,
            session_id: None,
            robustness_profile: profile,
        }
    }

    // THE NEWS CASE: 8 web_fetch, all distinct URLs → fan-out → Continue.
    #[tokio::test]
    async fn high_distinctness_fanout_passes() {
        let calls: Vec<_> = (0..8).map(|i| call("web_fetch", i)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v.verify(
            &ctx(&calls, ModelRobustnessProfile::conservative(), None),
            &CancellationToken::new(),
        ).await;
        assert!(verdict.is_continue(), "distinct fan-out must not trip: {verdict:?}");
    }

    // THRASH: 3 files cycling, 8 same-name silent → Tier-2 → VETO (was Halt).
    #[tokio::test]
    async fn low_distinctness_thrash_steers_not_halts() {
        let calls: Vec<_> = (0..8).map(|i| call("file_read", (i % 3) as u64)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v.verify(
            &ctx(&calls, ModelRobustnessProfile::conservative(), None),
            &CancellationToken::new(),
        ).await;
        assert!(verdict.is_veto(), "silent thrash must steer (Veto), not Halt: {verdict:?}");
    }

    // THRASH WITH NARRATION + silence_required → Continue (legit exploration).
    #[tokio::test]
    async fn thrash_with_narration_passes_when_silence_required() {
        let calls: Vec<_> = (0..8).map(|i| call("file_read", (i % 3) as u64)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v.verify(
            &ctx(&calls, ModelRobustnessProfile::conservative(), Some("Comparing the three files...")),
            &CancellationToken::new(),
        ).await;
        assert!(verdict.is_continue(), "narrated exploration must pass: {verdict:?}");
    }

    // TIER-1 identical run at repeat_threshold → Veto.
    #[tokio::test]
    async fn identical_run_vetoes() {
        let calls: Vec<_> = (0..5).map(|_| call("grep", 42)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v.verify(
            &ctx(&calls, ModelRobustnessProfile::conservative(), None),
            &CancellationToken::new(),
        ).await;
        assert!(verdict.is_veto(), "identical run should veto: {verdict:?}");
    }

    // TIER-1 identical run at full window → Halt (dead loop).
    #[tokio::test]
    async fn identical_run_full_window_halts() {
        let calls: Vec<_> = (0..8).map(|_| call("grep", 42)).collect();
        let v = ToolLoopVerifier::new();
        let verdict = v.verify(
            &ctx(&calls, ModelRobustnessProfile::conservative(), None),
            &CancellationToken::new(),
        ).await;
        assert!(verdict.is_halt(), "identical full-window run should halt: {verdict:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tool_loop_verifier::distinctness_tests`
Expected: FAIL — `high_distinctness_fanout_passes` currently Halts (Tier-2), `low_distinctness_thrash_steers_not_halts` currently Halts (expects Veto). (Identical-run tests may already pass.)

- [ ] **Step 3: Add `distinct_count` helper** — in `src/verification/tool_loop_verifier.rs`, near `trailing_same_name_run` (~line 127):

```rust
/// Number of distinct `(name, args_hash)` pairs in the window. A low ratio
/// of distinct/total means the model is revisiting a small set of calls
/// (thrash); a high ratio means genuine fan-out / exploration.
fn distinct_count(calls: &[ToolCallSummary]) -> usize {
    let mut seen: Vec<(&str, u64)> = Vec::with_capacity(calls.len());
    for c in calls {
        let key = (c.name.as_str(), c.args_hash);
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen.len()
}
```

- [ ] **Step 4: Rewrite `verify` to read the profile + distinctness** — replace the body of `impl TurnVerifier for ToolLoopVerifier`'s `verify` (currently lines ~141-216) with:

```rust
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        // Death-loop detection is a *mid-turn* concern (see original rationale).
        if ctx.stop_reason.is_some() {
            return VerifierVerdict::Continue;
        }
        let profile = ctx.robustness_profile;
        if ctx.recent_tool_calls.len() < profile.repeat_threshold {
            return VerifierVerdict::Continue;
        }
        let run = trailing_repeat_run(ctx.recent_tool_calls);

        // Tier 1 — identical (name + args_hash) consecutive calls. Fires
        // regardless of narration: an exact-repeat is never productive.
        if run >= profile.repeat_threshold {
            let tool = &ctx.recent_tool_calls[ctx.recent_tool_calls.len() - 1].name;
            if run >= profile.halt_threshold && profile.halt_threshold > profile.repeat_threshold {
                return VerifierVerdict::Halt {
                    reason: format!(
                        "tool '{tool}' invoked {run} consecutive times with identical arguments \
                         despite repeated feedback — terminating an unproductive loop",
                    ),
                    class: ErrorClass::Recoverable,
                };
            }
            return VerifierVerdict::Veto {
                reason: format!(
                    "tool '{tool}' invoked {run} consecutive times with identical arguments — \
                     try a different approach or summarize what you've found",
                ),
                class: ErrorClass::Recoverable,
            };
        }

        // Tier 2 — same tool NAME fills the window but arguments revisit a
        // SMALL set (low distinctness) AND no narration. This is a thrash
        // (e.g. re-reading 3 files round and round). UNLIKE the old code this
        // now emits a `Veto` (steer), not a terminal `Halt`: the harness
        // injects feedback and, only if the model ignores `steer_max` of them,
        // its veto-cap path fires a wrap-up grace turn. High-distinctness
        // fan-out (e.g. 8 distinct web_fetch URLs) has distinctness == 1.0,
        // which is >= novelty_min, so it never reaches here.
        let same_name_run = trailing_same_name_run(ctx.recent_tool_calls);
        let has_text = ctx.final_text.is_some_and(|t| !t.trim().is_empty());
        let distinct = distinct_count(ctx.recent_tool_calls);
        let distinctness = distinct as f32 / ctx.recent_tool_calls.len() as f32;
        let silent_ok = !profile.silence_required || !has_text;
        if same_name_run >= TOOL_HISTORY_WINDOW
            && distinctness < profile.novelty_min
            && silent_ok
        {
            let tool = &ctx.recent_tool_calls[ctx.recent_tool_calls.len() - 1].name;
            return VerifierVerdict::Veto {
                reason: format!(
                    "tool '{tool}' invoked {same_name_run} times cycling a small set of arguments \
                     ({distinct} distinct) with no narration — stop and summarize, or take a \
                     different approach",
                ),
                class: ErrorClass::Recoverable,
            };
        }

        VerifierVerdict::Continue
    }
```

> The `ToolLoopVerifier` struct's `repeat_threshold`/`halt_threshold` fields and `with_threshold`/`with_halt_threshold`/`threshold`/`halt_threshold` methods are now unused by `verify` (thresholds come from the profile). Leave `new()` (still called at `orchestrator_init.rs:150`). Remove the now-dead `repeat_threshold`/`halt_threshold` fields and their setter/getter methods IF the compiler flags them as dead; otherwise keep `new()` as a unit struct. Adjust `new()` to `Self {}` if you remove the fields, and update any test that called `with_threshold`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tool_loop_verifier`
Expected: PASS (all distinctness tests + any retained legacy tests). Fix legacy tests that asserted the old Tier-2 `Halt` — they should now assert `Veto`.

- [ ] **Step 6: Compile check**

Run: `cargo check -p alephcore --lib`
Expected: clean (resolve any dead-code warnings per the note in Step 4).

- [ ] **Step 7: Commit**

```bash
git add src/verification/tool_loop_verifier.rs
git commit -m "verification: distinctness-based loop detection; Tier-2 steers instead of halting"
```

---

## Task 4: Make the veto cap per-model (`MAX_VERIFIER_VETOS` → `steer_max`)

**Files:**
- Modify: `src/harness/agent.rs:353` (the const), `:598-626` (the cap check)
- Test: `src/harness/tests/...` (extend an existing veto-cap test, or add one)

**Interfaces:**
- Consumes: `self.deps.robustness_profile.steer_max` (Task 2).
- Produces: the veto cap is now the per-run profile's `steer_max` instead of a global const.

- [ ] **Step 1: Write/extend the failing test** — locate the existing veto-cap test referenced by `src/harness/tests/task10_wiring/mod.rs:741` ("MAX_VERIFIER_VETOS=10 safety cap"). Add a sibling test that sets a small `steer_max` via the deps profile and asserts the grace/HitLimit fires after exactly that many vetoes. Sketch (adapt to the harness test harness in that file):

```rust
#[tokio::test]
async fn veto_cap_follows_profile_steer_max() {
    // Build harness deps with robustness_profile.steer_max = 2 and a fake
    // provider that loops a vetoable identical tool call forever.
    // Assert the run terminates (HitLimit / grace) after ~2 vetoes, NOT 10.
    // (Use the same deps/provider builders this test module already uses;
    //  set `robustness_profile: ModelRobustnessProfile { steer_max: 2,
    //  ..ModelRobustnessProfile::conservative() }`.)
}
```

> The implementer fills the body using the existing test scaffolding in `task10_wiring` (fake provider + deps builder). The assertion: with `steer_max = 2`, termination happens after 2 vetoes.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib veto_cap_follows_profile_steer_max`
Expected: FAIL — terminates at 10 (the const), not 2.

- [ ] **Step 3: Replace the const usage with the profile** — in `src/harness/agent.rs`, at the cap check (~line 600), change:

```rust
                        if verifier_veto_count >= Self::MAX_VERIFIER_VETOS {
```
to:
```rust
                        if verifier_veto_count >= self.deps.robustness_profile.steer_max {
```

And update the warn log field (line ~603) from `max_vetos = Self::MAX_VERIFIER_VETOS,` to `max_vetos = self.deps.robustness_profile.steer_max,`.

Keep the `MAX_VERIFIER_VETOS` const if other code references it; otherwise delete it (line ~353) to avoid a dead-const warning. (`verifier_veto_count` already resets to 0 on a non-vetoed turn at ~line 625 — that episode-reset semantics is correct and unchanged.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib veto_cap_follows_profile_steer_max`
Expected: PASS (terminates after 2).

- [ ] **Step 5: Compile check + commit**

Run: `cargo check -p alephcore --lib`
```bash
git add src/harness/agent.rs src/harness/tests/
git commit -m "harness: veto cap follows per-model steer_max instead of global const"
```

---

## Task 5: Fix the grace-turn orphaned-tool-call 400 (Tier-1 Halt path)

**Files:**
- Investigate + Modify: `src/harness/agent/think.rs` (`close_unexecuted_tool_uses` ~1668-1694, the Halt branch ~1087-1142, `fire_boundary_grace_turn`)
- Test: `src/harness/tests/...` (new regression test)

**Interfaces:**
- Consumes: the Halt branch + `close_unexecuted_tool_uses` + grace-turn payload build.
- Produces: a valid tool_use↔tool_result pairing in the grace turn's request payload for a parallel batch (no Anthropic 400).

> This task is **investigation-first** (systematic-debugging): the Halt branch already calls `close_unexecuted_tool_uses` (emits a synthetic `ToolError` per `response.tool_calls[].id`), yet a parallel-batch Halt was observed to 400 on orphaned tool_call_ids. Reproduce first, then fix the actual gap.

- [ ] **Step 1: Write the failing regression test** — add a harness-level test that drives a turn emitting a parallel batch of identical tool_use blocks (to trip Tier-1 Halt at the window), then captures the request payload the grace turn builds, and asserts every `tool_use` id has a matching `tool_result`/`tool_error`:

```rust
#[tokio::test]
async fn grace_turn_payload_has_no_orphaned_tool_calls() {
    // Drive the harness so a turn emits N identical tool_use blocks that
    // trip the Tier-1 Halt (repeat_threshold..=halt_threshold). Capture the
    // grace turn's RequestPayload (via the fake provider recording calls).
    // Assert: for every tool_use block id in the assistant message history,
    // there is a matching tool_result/tool_error block. No orphan → no 400.
    //
    // Use the task10_wiring fake-provider scaffolding; the fake provider's
    // grace-turn call is the LAST recorded RequestPayload. Walk its messages.
}
```

- [ ] **Step 2: Run + reproduce**

Run: `cargo test -p alephcore --lib grace_turn_payload_has_no_orphaned_tool_calls`
Expected: FAIL — reproduces the orphan (some `tool_use` id lacks a matching result in the grace payload). Capture WHICH ids are orphaned and WHY (e.g. a partially-executed mixed batch where some calls ran via Act before the Halt and weren't in `response.tool_calls`, or the synthetic `ToolError` events not yet persisted when `fire_boundary_grace_turn` rebuilds the prompt from the session log).

- [ ] **Step 3: Implement the minimal fix** — based on the reproduction, the most likely fix is one (or both) of:

(a) Ensure `close_unexecuted_tool_uses` covers **every** emitted `tool_use` id on the halting turn, not only `response.tool_calls` that map 1:1 — if any were partially executed, their results already exist; the gap is only the *unexecuted* remainder. Build the id set from the full set of emitted tool_use blocks minus those that already emitted a `ToolResult`/`ToolError`.

(b) Ensure the synthetic `ToolError` events are **flushed/persisted before** `fire_boundary_grace_turn` rebuilds the prompt from the session log (await ordering: emit-and-await all `close_unexecuted_tool_uses` events *before* calling `fire_boundary_grace_turn`). The current code already awaits `close_unexecuted_tool_uses` before `fire_boundary_grace_turn` — verify the grace turn reads the same session state and doesn't snapshot an earlier log.

Apply the minimal change the reproduction proves necessary. Example for (a) — replace the id collection in the Halt branch:

```rust
            // Every tool_use emitted this turn that has NOT already produced a
            // result must get a synthetic one, or the grace prompt build drops
            // the orphan and the provider 400s.
            let pending_tool_use_ids: Vec<String> = response
                .tool_calls
                .iter()
                .map(|c| c.id.clone())
                .collect();
            self.close_unexecuted_tool_uses(
                session_id,
                turn_id,
                &pending_tool_use_ids,
                "tool-loop halt",
            )
            .await;
```

(If the reproduction shows the grace turn itself emits NEW tool_use that then orphan, ensure the grace turn is built tool-less — confirm `fire_boundary_grace_turn` passes `None` tools, per the earlier finding.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib grace_turn_payload_has_no_orphaned_tool_calls`
Expected: PASS — every tool_use has a matching result in the grace payload.

- [ ] **Step 5: Add a fail-soft fallback** — if the grace turn's provider call still errors, the run must deliver the last assistant text rather than nothing. Locate `fire_boundary_grace_turn`'s error handling; ensure on grace-LLM error it logs WARN and leaves the prior assistant text as the deliverable (do not swallow into empty). Add a one-line test asserting a grace-turn provider error still yields non-empty final output if prior text exists.

- [ ] **Step 6: Compile check + commit**

Run: `cargo check -p alephcore --lib`
```bash
git add src/harness/agent/think.rs src/harness/tests/
git commit -m "harness: fix orphaned tool_call ids in tool-loop grace turn (partial delivery)"
```

---

## Task 6: Integration test — weak-model fan-out → steer → partial delivery

**Files:**
- Test: `src/harness/tests/...` (new integration test, alongside `task10_wiring`)

**Interfaces:**
- Consumes: all prior tasks (profile threading, distinctness, steer cap, grace fix).

- [ ] **Step 1: Write the integration test** — a fake provider script that (1) emits a high-distinctness fan-out batch, then (2) emits a silent low-distinctness thrash repeatedly, run under a tight profile (`for_behavior(Some("ollama"))` or `steer_max: 2`). Assert:

```rust
#[tokio::test]
async fn weak_model_fanout_then_thrash_steers_and_delivers_partial() {
    // Profile: tight (steer_max small, silence_required false).
    // Provider script:
    //   turn 1: 5 distinct web_fetch (fan-out)        -> NOT vetoed (Continue/Act)
    //   turns 2..: silent cycling 3-file thrash        -> Veto (steer) each
    //   after steer_max vetoes: grace turn produces text
    // Assertions:
    //   - turn 1 fan-out executed (no veto on it)
    //   - thrash produced Veto verdicts (steering), not an immediate Halt
    //   - run terminated via the veto-cap grace path
    //   - final delivered output is NON-EMPTY (partial delivery worked)
}
```

- [ ] **Step 2: Run to verify it fails (before wiring is complete) / passes (after)**

Run: `cargo test -p alephcore --lib weak_model_fanout_then_thrash_steers_and_delivers_partial`
Expected: PASS once Tasks 1-5 are in. (If authored before, it fails on the fan-out being vetoed or empty delivery.)

- [ ] **Step 3: Final compile check + commit**

Run: `cargo check -p alephcore --lib`
```bash
git add src/harness/tests/
git commit -m "harness: integration test for fan-out/thrash steering + partial delivery"
```

---

## Self-Review (run before handing off to execution)

**Spec coverage:**
- §4.1 distinctness detection → Task 3 ✓
- §4.2 Tier-2 → Veto, reuse veto-cap→grace → Task 3 + Task 4 ✓
- §4.3 ModelRobustnessProfile (built-in table) → Task 1 ✓; per-run threading → Task 2 ✓
- §4.4 grace-turn 400 fix + fail-soft fallback → Task 5 ✓
- §7 tests: distinctness/profile/escalation/grace-regression/integration → Tasks 1,3,4,5,6 ✓
- §10 acceptance (fan-out passes / thrash steers / partial delivery / per-model thresholds / no harness cognition) → Tasks 3,4,5,6 ✓

**Open implementer confirmations (flagged inline, not placeholders):**
- Task 2 Step 6: exact `AiProvider` accessor for behavior name in `runner_impl` scope (`model_behavior_override()` / `protocol()`).
- Task 3 Step 4: whether to delete now-unused `ToolLoopVerifier` threshold fields/methods (compiler-driven).
- Task 5: the precise orphan cause is reproduction-driven (Step 2 gathers evidence before the Step 3 fix) — this is intentional systematic-debugging, not a placeholder.

**Type consistency:** `ModelRobustnessProfile` fields (`repeat_threshold`, `halt_threshold`, `steer_max`, `novelty_min`, `silence_required`) are used identically in Tasks 1/2/3/4. `TurnVerifyContext.robustness_profile` and `HarnessDeps.robustness_profile` are the same type. ✓
