# Workspace Panel Step Narration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the WebChat workspace panel show the model's step-by-step natural-language narration (like Claude Code's `⏺` lines) instead of a bare list of tool calls.

**Architecture:** The render link (model text → `TextEmitted` → `text_emitted` event → `ChatMessage.content` → `StepCard` narration) already exists end-to-end. The narration is empty only because three prompt/guard rules suppress it. Fix = (①) flip guidelines rule 17 from "don't narrate" to "narrate each step substantively"; (②) remove the `has_text` escape in `tool_loop_verifier` so encouraged narration can no longer mask an identical-call death loop (safe because the loop is keyed on identical `name`+`args_hash`); (③) restyle the panel's `StepCard` so narration leads with markdown; (④) soften one Google-family directive that opposed narration.

**Tech Stack:** Rust (`alephcore` lib — thinker prompt layers + verification), Leptos/WASM (webchat panel).

**Spec:** `docs/superpowers/specs/2026-06-06-workspace-panel-narration-design.md`

---

## File Structure

- `src/verification/tool_loop_verifier.rs` — remove the `has_text` early-return; update the module doc's detection-rule bullets. (Block ②)
- `src/verification/tests/tool_loop_verifier.rs` — invert the "thinking text rescues the loop" test; update module doc line. (Block ②)
- `src/thinker/layers/guidelines.rs` — rewrite rule 17 string + update its unit-test assertion/comment. (Block ①)
- `src/thinker/layers/provider_guidance.rs` — soften the "Actions and results beat narration" clause. (Block ④)
- `interfaces/webchat/src/components/workspace_panel.rs` — render `StepCard` narration via `MarkdownRenderer`, promoted to `text-sm text-text-primary`. (Block ③)

Tasks 1–2 are pure Rust lib (`cargo test -p alephcore --lib`). Task 3 is Rust lib. Task 4 is WASM (build-only check). Task 5 is whole-system verification.

---

## Task 1: Harden `tool_loop_verifier` — narration no longer masks an identical-call loop (Block ②)

**Files:**
- Modify: `src/verification/tool_loop_verifier.rs:132-138` (remove `has_text` block) and module doc `:9-15`
- Test: `src/verification/tests/tool_loop_verifier.rs:55-85`

- [ ] **Step 1: Invert the "thinking text allows" test to expect a veto**

In `src/verification/tests/tool_loop_verifier.rs`, replace the whole `at_threshold_with_thinking_text_allows` test (lines 55-69) with this — same setup, but now narration must NOT rescue 5 identical calls:

```rust
#[tokio::test]
async fn thinking_text_does_not_rescue_identical_loop() {
    // Narration is now ENCOURAGED (guidelines rule 17), so it can no longer be
    // the signal that suppresses death-loop detection. Five identical
    // (name, args_hash) calls is a loop whether or not the model narrates —
    // the args_hash equality already excludes legitimate varied exploration.
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 5];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: Some("hmm, let me reconsider"),
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}
```

- [ ] **Step 2: Rename the now-redundant whitespace test for clarity**

Still in the test file, replace the `whitespace_only_text_treated_as_no_text` test (lines 71-85) with a version whose name reflects the new semantics (any text, identical loop → still vetoes):

```rust
#[tokio::test]
async fn text_present_still_vetoes_identical_loop() {
    // After removing the has_text escape, the presence of *any* final_text
    // (whitespace or substantive) does not change the verdict for an identical
    // (name, args_hash) run — it vetoes on repetition alone.
    let v = ToolLoopVerifier::new().with_threshold(5);
    let history = vec![make("read", 1); 5];
    let ctx = TurnVerifyContext {
        iterations: 5,
        tool_calls_made: 5,
        final_text: Some("   \n\t  "),
        recent_tool_calls: &history,
        stop_reason: None,
        session_id: None,
    };
    let cancel = CancellationToken::new();
    assert!(v.verify(&ctx, &cancel).await.is_veto());
}
```

Also update the module doc on line 1 from:

```rust
//! `ToolLoopVerifier` threshold + thinking-text guard semantics.
```

to:

```rust
//! `ToolLoopVerifier` threshold + identical-call repetition semantics.
```

- [ ] **Step 3: Run the inverted test to verify it FAILS**

Run: `cargo test -p alephcore --lib thinking_text_does_not_rescue_identical_loop -- --nocapture`
Expected: FAIL — current code hits the `has_text` early-return and returns `Continue`, so `is_veto()` is false (`assertion failed`).

- [ ] **Step 4: Remove the `has_text` early-return**

In `src/verification/tool_loop_verifier.rs`, delete this block (lines 132-138):

```rust
        let has_text = ctx
            .final_text
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        if has_text {
            return VerifierVerdict::Continue;
        }
```

The code now flows directly from the `recent_tool_calls.len() < self.repeat_threshold` guard (line 129-131) into `let run = trailing_repeat_run(ctx.recent_tool_calls);` (line 139).

- [ ] **Step 5: Update the module doc detection-rule bullets**

In the same file, in the `//!` header, remove the now-false `final_text` bullet. Change lines 12-15 from:

```rust
//!   - `ctx.recent_tool_calls.len() >= threshold`
//!   - the trailing `threshold` entries all have the same `name` and
//!     `args_hash`
//!   - the current turn's `final_text` is empty/None
```

to:

```rust
//!   - `ctx.recent_tool_calls.len() >= threshold`
//!   - the trailing `threshold` entries all have the same `name` and
//!     `args_hash` (identical, redundant calls — varied args reset the run)
```

And change lines 1-4 from:

```rust
//! `ToolLoopVerifier` — structural watchdog that vetoes when the
//! model has issued N consecutive identical tool calls without
//! producing thinking text in between (closes master roadmap § 1.4
//! P1: "stop hook 仅在模型停手触发；tool_use 死循环不覆盖").
```

to:

```rust
//! `ToolLoopVerifier` — structural watchdog that vetoes when the
//! model has issued N consecutive identical tool calls (same `name` +
//! `args_hash`), regardless of any narration text on the turn (closes
//! master roadmap § 1.4 P1: "stop hook 仅在模型停手触发；tool_use 死循环不覆盖").
```

- [ ] **Step 6: Run the full verifier test suite to verify all PASS**

Run: `cargo test -p alephcore --lib tool_loop -- --nocapture`
Expected: PASS — all tests green, including `thinking_text_does_not_rescue_identical_loop`, `text_present_still_vetoes_identical_loop`, and the unchanged `different_args_hash_breaks_repetition` (varied args still `Continue`).

- [ ] **Step 7: Commit**

```bash
git add src/verification/tool_loop_verifier.rs src/verification/tests/tool_loop_verifier.rs
git commit -m "verification: drop has_text escape so narration can't mask an identical-call loop"
```

---

## Task 2: Rewrite guidelines rule 17 — narrate each step substantively (Block ①)

**Files:**
- Modify: `src/thinker/layers/guidelines.rs:50` (the rule 17 string) and the unit test `:113-116`

- [ ] **Step 1: Update the unit-test assertion to expect the NEW rule 17 wording**

In `src/thinker/layers/guidelines.rs`, replace the rule-17 test block (lines 113-116) with:

```rust
        // Rule 17: narrate each step — before a tool call write one short line
        // of intent (what + why), summarize key results, recap at the end.
        // Encourages the Claude-Code "⏺" cadence; the anti-pattern is empty
        // "now I'll…" announcements that never deliver (see rule 15). The
        // structural loop guard no longer depends on the ABSENCE of narration
        // (see tool_loop_verifier), so encouraging narration is safe.
        assert!(out.contains("17. Narrate each step as you work"));
```

- [ ] **Step 2: Run the test to verify it FAILS**

Run: `cargo test -p alephcore --lib test_guidelines_content -- --nocapture`
Expected: FAIL — the live string still says "17. Narrate only when it earns its place", so `out.contains("17. Narrate each step as you work")` is false.

- [ ] **Step 3: Replace the rule 17 string**

In `src/thinker/layers/guidelines.rs`, replace the entire line 50 (the `output.push_str("17. Narrate only when it earns its place …")` statement) with:

```rust
        output.push_str("17. Narrate each step as you work — before a tool call (or a batch of them) write ONE short natural-language line stating what you're about to do and why; when a key result lands, summarize it in a sentence; give a brief recap as you conclude. This running commentary is what makes your work readable as it streams (a line of intent → the action → what you learned). Keep every line substantive — it must carry a finding, a decision, or a reason. The anti-pattern is the EMPTY announcement: \"now I'll write the report\" with no report ever produced is noise that masquerades as progress (see rule 15) — state real intent and let your VERY NEXT action deliver on it.\n\n");
```

- [ ] **Step 4: Run the test to verify it PASSES**

Run: `cargo test -p alephcore --lib test_guidelines_content -- --nocapture`
Expected: PASS — the new string contains "17. Narrate each step as you work".

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/guidelines.rs
git commit -m "thinker: flip guidelines rule 17 to prescribe substantive per-step narration"
```

---

## Task 3: Soften the Google-family anti-narration directive (Block ④)

**Files:**
- Modify: `src/thinker/layers/provider_guidance.rs:179-180`

- [ ] **Step 1: Replace the Conciseness clause**

In `src/thinker/layers/provider_guidance.rs`, inside `GOOGLE_OPERATIONAL_DIRECTIVES`, replace the Conciseness bullet (lines 179-180) — currently:

```rust
- **Conciseness**: keep explanatory text brief — a few sentences, not paragraphs. Actions and \
results beat narration.\n\
```

with (keeps "brief", drops the "don't narrate" implication so it no longer contradicts guidelines rule 17):

```rust
- **Conciseness**: keep explanatory text brief — a few sentences, not paragraphs. Still narrate \
each step in one short line (what you're doing and why); just keep it tight.\n\
```

- [ ] **Step 2: Verify the layer still compiles and its tests pass**

Run: `cargo test -p alephcore --lib provider_guidance -- --nocapture`
Expected: PASS (no test asserts the old clause text; this confirms no regression).

- [ ] **Step 3: Commit**

```bash
git add src/thinker/layers/provider_guidance.rs
git commit -m "thinker: stop Google directive from suppressing per-step narration"
```

---

## Task 4: Promote `StepCard` narration to a markdown lead (Block ③)

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs` — imports (~`:18`) and `StepCard` narration block (`:158-162`)

- [ ] **Step 1: Import the markdown renderer**

In `interfaces/webchat/src/components/workspace_panel.rs`, add the import next to the existing `run_id_from_message_id` use (after line 19):

```rust
use crate::components::markdown::MarkdownRenderer;
```

- [ ] **Step 2: Render narration as a markdown lead**

In the `StepCard` component, replace the narration `<Show>` block (lines 158-162) — currently:

```rust
            <Show when=move || has_narration>
                <p class="text-xs text-text-secondary whitespace-pre-wrap leading-relaxed">
                    {narration.clone()}
                </p>
            </Show>
```

with:

```rust
            <Show when=move || has_narration>
                <div class="text-sm text-text-primary leading-relaxed aleph-step-narration">
                    <MarkdownRenderer content=narration.clone() />
                </div>
            </Show>
```

(The `#iteration` label stays above and the tool list stays below, so the visual order is already label → narration → tools — only narration's prominence and markdown rendering change.)

- [ ] **Step 3: Build the WASM panel to verify it compiles**

Run: `just wasm`
Expected: SUCCESS — rebuilds `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css}` with no compile errors. (If `just wasm` is unavailable, use the project's documented WASM build; a clean build is the pass criterion.)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm interfaces/webchat/dist/tailwind.css
git commit -m "webchat: render workspace step narration as a markdown lead"
```

---

## Task 5: Whole-system verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full core lib test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: PASS — all tests green, including the touched `tool_loop`, `test_guidelines_content`, and `provider_guidance` tests.

- [ ] **Step 2: Compile-check the server binary**

Run: `cargo check -p alephcore --bin aleph-server 2>&1 | tail -10`
Expected: `Finished` with no errors.

- [ ] **Step 3: Live e2e (manual, optional deploy)**

Per `CLAUDE.md` Panel↔Daemon embed chain, to see the change live: `just wasm` → `cargo build --release -p alephcore --bin aleph-server` → hot-swap the binary → supervisor relaunch. Then in the chat window run a multi-step task (e.g. a "search X + build an HTML report" task like the one in `/Volumes/TBU4/goal.md`) and confirm:
- the workspace panel shows a **natural-language narration line for each step** (markdown-rendered), not just tool rows;
- a deliberately induced identical-call loop (same tool + same args ≥5×) **still gets vetoed/halted** even though the model narrates.

Note: deployment is the user's call — do not hot-swap a running daemon without confirmation.

- [ ] **Step 4: Final commit (if any uncommitted verification artifacts remain)**

```bash
git status --short
# Only commit intended files; the repo convention is explicit-path staging on main.
```

---

## Self-Review

**Spec coverage:**
- Block ① (rule 17 flip + test) → Task 2. ✓
- Block ② (remove has_text + comments + tests) → Task 1. ✓
- Block ③ (StepCard markdown lead) → Task 4. ✓
- Block ④ (Google directive softening) → Task 3. ✓
- Verification (lib tests, cargo check, wasm, live e2e) → Tasks 1–5. ✓
- Redlines R10/R7/R9 — no `src/harness/` edits (verifier is in `src/verification/`); narration is prompt-driven model behavior; UI/data-link untouched. ✓

**Placeholder scan:** No TBD/TODO; every code step shows exact before/after text. ✓

**Type consistency:** `MarkdownRenderer content=...` matches the prop used at `messages.rs:538`; `TurnVerifyContext`/`ToolCallSummary`/`make()` match the existing test fixtures; verdict helpers `is_veto()`/`is_continue()` match existing tests. ✓
