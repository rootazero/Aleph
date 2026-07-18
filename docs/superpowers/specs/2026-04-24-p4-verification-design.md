# P4 — Verification Module Minimization Design

**Date**: 2026-04-24
**Phase**: P4 (third phase of harness dissolution roadmap)
**Parent Roadmap**: [`2026-04-24-harness-dissolution-roadmap.md`](./2026-04-24-harness-dissolution-roadmap.md)
**Risk**: 🟢 Low (downgraded from 🟡 Medium via YAGNI)
**Estimate**: 1–2 hours (shortened from 1.5 weeks via YAGNI)
**Status**: Approved (brainstorm phase)

---

## 1. Goal

Delete the orphaned Rust struct `VerifyStopHook` (194 lines in `src/verification/verify_stop_hook.rs`, zero production instantiations) and document Aleph's prompt-driven verification model in `src/verification/mod.rs`. **No new traits, no `Verifier` / `LlmJudge` / `VisualDiffer` contracts introduced.**

The roadmap §4.2 originally specified "rule / visual / LLM-judge contracts" as P4's exit artifact. This commitment is **explicitly retracted** here: the brainstorm discovered that Aleph's verification logic already lives entirely in prompt templates (R10) and relies on the LLM itself (R8). Rust-level verifier traits would be speculative abstraction with no present consumer.

## 2. Brainstorm Findings

### 2.1 Aleph's actual verification model

The live verification mechanism is two-part, with only one half wired:

- **Prompt half (wired)**: `src/thinker/layers/agent_role.rs` includes a template that instructs agents to emit a structured VERDICT block before stopping:
  ```
  VERDICT: PASS | FAIL | PARTIAL
  REASON: <one-line summary>
  CHECKS:
  - [x] build: <result>
  - [x] tests: <N passed, M failed>
  - [x] lint: <result>
  ISSUES: ...
  ```
  The LLM performs the actual verification (running `cargo check`, `cargo test`, lint via its tool-calling loop) and reports results in the VERDICT block. This is R8 + R10 in action: intelligence in the prompt, the LLM is the judge.

- **Rust half (orphaned)**: `src/verification/verify_stop_hook.rs` defines a `VerifyStopHook` struct implementing `StopHookHandler`. Its job is to block the agent's stop attempt when the VERDICT block is absent from the final output. A grep across the codebase shows **zero production instantiations** — `VerifyStopHook::new(...)` is called only inside its own unit tests. No code path pushes a `VerifyStopHook` into `HarnessDeps.stop_hooks`.

### 2.2 Git archaeology on VerifyStopHook

Original introduction: commit `b54877d7f` (2026-04-01) — *"feat(agents): agent prompt pipeline with section registry and verification"* — which added both the prompt-side VERDICT template and the Rust-side enforcement struct in one patch. Only the prompt side was wired; the Rust side was never plumbed through. It survived phase 6b relocation (`dd7ff0d71`) and P0 relocation (`045dd317a`) as dead scaffolding.

### 2.3 Why delete rather than keep

Per R3 (Core Minimalism), R8 (LLM Sovereignty), and the P1 precedent on `src/compressor/`: dead code with zero consumers is deleted, not renamed or preserved. Git history retains the implementation if future work requires a Rust-layer verdict enforcer.

## 3. Scope Decisions

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 1 | P4 foundational stance | **A — Trait contracts only (skeleton)** | Aleph's verification semantics live in prompts (R8/R10). A Rust `Verifier` trait with no concrete consumer would be speculative abstraction. YAGNI. |
| 2 | `VerifyStopHook` disposition | **D1 — Delete entirely** | Zero production consumers (only its own tests reference it). Matches the compressor-deletion precedent from P1. Git retains it for future revival. |
| 3 | Commit granularity | **2 commits** (delete + docs) | The deletion is a code change; the roadmap + YAGNI record is a documentation change. Independent actions. |
| 4 | `src/verification/stop_hooks.rs` handling | **Unchanged** | It's live infrastructure (ShellStopHook + trait + execute_stop_hooks); P4 does not touch its internals. |

## 4. Out of Scope (P4)

- ❌ **No new traits**. Reject `Verifier`, `LlmJudge`, `VisualDiffer`, `RuleVerifier`, `VerificationStrategy` enum — any of them would be abstraction without consumer.
- ❌ **No wire-up of `VerifyStopHook` into `HarnessDeps`**. That would be feature work, not refactor.
- ❌ **No changes to `src/verification/stop_hooks.rs`**. The `StopHookHandler` trait, `StopHookContext`, `StopHookVerdict`, `ShellStopHook`, and `execute_stop_hooks` are live infrastructure consumed by `harness/agent.rs`, `harness/deps.rs`, `orchestrator/harness_bridge.rs`, and `harness/tests/task10_wiring.rs`.
- ❌ **No changes to `src/thinker/layers/agent_role.rs`**. The VERDICT prompt template is the production verification mechanism and stays as-is.
- ❌ **No renames of existing types**. No type renames anywhere.
- ❌ **No resolution of Open Question #4** (HarnessError location). Cross-cutting error-type questions defer to P6 or later.

## 5. Action Manifest

### 5.1 Commit 1: `verification: delete orphan VerifyStopHook (0 consumers)`

**Files**:
- Delete: `src/verification/verify_stop_hook.rs` (194 lines + ~11 embedded unit tests)
- Modify: `src/verification/mod.rs` — remove `pub mod verify_stop_hook;` line; rewrite the module doc comment as shown below.

**New `src/verification/mod.rs` content**:
```rust
//! Verification — stop-hook infrastructure for Aleph's prompt-driven
//! verification model.
//!
//! Aleph's verification logic lives entirely in prompts (see
//! `src/thinker/layers/agent_role.rs`): agents are instructed to emit a
//! `VERDICT: PASS|FAIL|PARTIAL` block summarizing their self-checks
//! before stopping. Per redlines R8 (LLM Sovereignty) and R10
//! (Intelligence Lives in the Prompt), no Rust-level verifier, judge,
//! or critic is introduced. The `StopHookHandler` trait below hosts
//! the generic stop-interception mechanism plus `ShellStopHook` for
//! shell-command hooks.
//!
//! A separate `VerifyStopHook` Rust struct existed from April 2026
//! through P0 but was never wired into production (zero instantiation
//! sites outside its own tests). It was deleted in P4 (2026-04-24)
//! because the prompt-level mechanism fully covers the use case.
//! Retrievable from git history at commit b54877d7f if future work
//! requires a Rust-layer verdict enforcer.

pub mod stop_hooks;
```

**Verification before commit**:
- `grep -rn "VerifyStopHook\|verify_stop_hook" src/` — 0 matches after edits
- `cargo check -p alephcore` — green
- `cargo clippy -p alephcore -- -D warnings` — no new issues beyond P0-inherited pre-existing errors
- Test count drops by ~11 (the embedded unit tests in the deleted file) — no regression

### 5.2 Commit 2: `docs(spec): mark P4 complete; record YAGNI + orphan-deletion findings`

**Files**:
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`

**Changes**:
1. §7 Status Tracking: P4 row `📋 Planned | — | —` → `✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p4-verification-design.md](./2026-04-24-p4-verification-design.md) | [2026-04-24-p4-verification.md](../plans/2026-04-24-p4-verification.md)`
2. §4.2 P4 row: Risk `🟡 Medium` → `🟢 Low²`, Estimate `1.5 weeks` → `1–2 hours²`, Exit Artifact revised to `src/verification/ houses StopHookHandler trait + ShellStopHook only; VerifyStopHook deleted (see note ²)`
3. New footnote ² (after the existing ¹ from P1):
   ```
   ² **P4 YAGNI downscoping + orphan-code deletion (2026-04-24)**: During P4
   brainstorm, the roadmap's "rule / visual / LLM-judge contracts"
   commitment was retracted. Aleph's verification logic lives entirely
   in prompt templates (see `src/thinker/layers/agent_role.rs` VERDICT
   block) per R8/R10; no Rust-level verifier trait has a present
   consumer. A separate finding: `VerifyStopHook` (194 lines in
   `src/verification/verify_stop_hook.rs`) was orphaned code — zero
   production instantiations since its April 2026 introduction in
   commit b54877d7f — and was deleted per the P1 compressor precedent
   (dead code with zero consumers gets removed, not renamed). Risk
   downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2
   hours. See P4 design §2–§4 for details.
   ```

## 6. Verification Plan

**After Commit 1**:
1. `cargo check -p alephcore` — must pass
2. `cargo clippy -p alephcore -- -D warnings` — must pass (same level as baseline; P0-inherited pre-existing errors remain, no new errors introduced)
3. Static greps:
   - `grep -rn "VerifyStopHook" src/` = 0 matches
   - `grep -rn "verify_stop_hook" src/` = 0 matches (including imports and file paths)
4. `ls src/verification/` — expect exactly `mod.rs` + `stop_hooks.rs`
5. `just test-all` — full suite green; test count drops by ~11 (the embedded unit tests from the deleted file)

**After Commit 2**:
1. Roadmap markdown still renders correctly
2. `grep -n "P4" docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` shows the updated status row and the new ² footnote

No HTTP smoke test needed — P4 touches zero runtime paths (deleted code was never executed in production).

## 7. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| A hidden consumer of `VerifyStopHook` that grep missed (e.g., trait-object construction via string-keyed factory) | Very Low | Low | Brainstorm did an exhaustive `grep -rn "VerifyStopHook"` — only test-code matches found. Any miss would surface as `unresolved name` at `cargo check` and be fixed in the same commit. |
| Future engineer wants to re-enable Rust-level verification and doesn't know the code used to exist | Low | Low | `src/verification/mod.rs` doc comment explicitly documents the deletion + the reviving git SHA. The decision is traceable via `git log -S VerifyStopHook`. |
| The VERDICT prompt mechanism breaks in a way the (deleted) safety net would have caught | Low | Medium | P0 precedent: 3+ weeks of production run without the Rust enforcement haven't surfaced such a failure. If this ever happens, reviving `VerifyStopHook` from commit b54877d7f is a known remedy path. |
| Pre-existing P0-documented clippy/phase5 warnings surface during verification | Medium | Low | Same as P0/P1: inherit exemption; do not treat as P4 regressions. |

## 8. Rollback

Each commit is independently revertable via `git revert`.
- Commit 1 (delete) reverted → VerifyStopHook restored as orphaned code once again
- Commit 2 (docs) reverted → roadmap reverts to pre-P4 state

No database migrations, no config changes, no runtime state touched.

## 9. Not Doing in P4 (explicit deferrals)

The following are explicitly deferred, with rationale:

- **`Verifier` / `LlmJudge` / `VisualDiffer` / `RuleVerifier` traits** — Deferred indefinitely. Aleph's verification semantics live in prompts; a Rust facade would duplicate that responsibility without adding value. Adding them now violates YAGNI and R8.
- **Wiring up a replacement stop-hook enforcer** — Deferred. If the prompt-level VERDICT mechanism ever proves insufficient, the fix is likely a prompt adjustment, not a Rust structure. If Rust enforcement becomes genuinely necessary, it can be rebuilt cheaply from commit b54877d7f.
- **`src/context/window/` equivalent for verification** — N/A; verification has no natural "window" concept.
- **Open Question #4 (HarnessError location)** — Deferred to P6 or later, per roadmap §6.

## 10. References

- Parent roadmap: [`2026-04-24-harness-dissolution-roadmap.md`](./2026-04-24-harness-dissolution-roadmap.md)
- Prior phase specs:
  - [`2026-04-24-p0-slim-harness-design.md`](./2026-04-24-p0-slim-harness-design.md) (created `src/verification/` skeleton)
  - [`2026-04-24-p1-context-management-design.md`](./2026-04-24-p1-context-management-design.md) (precedent: delete dead code, compressor case)
- Origin of deleted code: commit `b54877d7f` (2026-04-01) — *"feat(agents): agent prompt pipeline with section registry and verification"*
- Architectural redlines: `CLAUDE.md` R3 (Core Minimalism), R8 (LLM Sovereignty), R10 (Intelligence Lives in the Prompt)
- Related prompt mechanism: `src/thinker/layers/agent_role.rs` (VERDICT template — the actual verification engine)
