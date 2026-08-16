# Severed-Wire Audit — `src/verification`

- **Batch:** agents-batch-6
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)
- **Scope:** all 13 `.rs` files under `src/verification/` (including `tests/`), 2890 LOC

## Headline result

**The module is genuinely well-wired.** All five verifiers — `StopHookVerifier`,
`ExtensionStopHookVerifier`, `ToolLoopVerifier`, `ScratchpadGoalVerifier`,
`MutationEvidenceVerifier` — are registered into the `VerifierChain`
(`orchestrator_init.rs:162-188`) and the chain is actually invoked every turn
(`harness/agent/think.rs:1397 chain.verify(...)`). Every producer symbol in the
module has a live consumer on the other side of the seam:

- `hash_tool_args` → `harness/agent/think.rs:933`
- `ModelRobustnessProfile::for_behavior(...).clamped()` → `runner_impl.rs:634-635`
- `steer_max` → `harness/agent.rs:684`
- `session_plan` (scratchpad resolver) → `builtin_tools/scratchpad.rs:219`
- `MUTATION_EVIDENCE_NUDGE` → defined in `thinker/nudges.rs:74`
- `effective_gate` / `build_from_config` → `goal_continuation.rs:356` / `orchestrator_init.rs:164`
- `is_shell_safe` → `builtin_tools/goal.rs:677`
- mutation/evidence tool names (`file_write`, `file_edit`, `apply_patch`, `bash`, `code_exec`, `code_check`) → all are real registered tool names (no classifier-vs-handler name drift)
- Stub sweep: no `unimplemented!` / `todo!` / `// TODO` anywhere in the module.

No severed wire found at high/medium severity. The four findings below are all
low-severity maintainability/drift hazards.

**Counts:** critical 0 · high 0 · medium 0 · low 4 · total 4
**Decisions:** CONNECT 3 · CUT 1 · DECIDE 0

---

## Findings

### [LOW] src/verification/turn_verifier.rs:133 — `VerifierChain::len()` / `is_empty()` are dead public methods
- **Category:** quality
- **Decision:** CUT
- **Description:** `len()` (line 133) and `is_empty()` (line 137) have zero callers
  repo-wide — not even in tests. `empty()` (line 122) is exercised only by a test.
  The production "no verifiers" path is expressed as `Option<Arc<VerifierChain>> = None`
  (`harness/deps.rs:40`), so none of these observability constructors carries load.
- **Suggested fix:** Delete `len()`, `is_empty()`, and (if the test helper is not needed)
  `empty()`; keep the `builder()` + `verify()` surface.

### [LOW] src/verification/stop_hook_verifier.rs:45 — byte-cap `4096` duplicated instead of sharing `LAST_MESSAGE_ENV_CAP`
- **Category:** quality
- **Decision:** CONNECT
- **Description:** `LAST_MESSAGE_ENV_CAP = 4096` is the canonical constant
  (`extension_stop_gate.rs:51`) but is a private `const`; this file re-hardcodes
  `let cap = 4096; // mirror extension_stop_gate::LAST_MESSAGE_ENV_CAP`. Two
  unsynchronized copies of one fact — the exact single-source-of-truth smell the
  skill's guard phase targets. Change one and the other silently diverges, so TOML
  stop hooks and extension Stop hooks would truncate `final_text` differently.
- **Suggested fix:** Promote `LAST_MESSAGE_ENV_CAP` to `pub(crate)` and reference it
  from both truncation sites (drop the local `let cap`).

### [LOW] src/verification/mutation_evidence_verifier.rs:50 — `"end_turn"` literal duplicated across producer/consumer
- **Category:** architecture
- **Decision:** CONNECT
- **Description:** The only stop-reason the harness emits is the literal `"end_turn"`
  (`harness/agent/think.rs:936`), and this verifier gates on
  `ctx.stop_reason != Some("end_turn")`. The three sibling stop-gating verifiers use
  `stop_reason.is_some()/is_none()` and are immune; this one is string-literal-coupled.
  If the producer string drifts (or a second stop reason is added), the verify-on-stop
  nudge silently never fires, with no compile error.
- **Suggested fix:** Hoist a canonical stop-reason symbol (const/enum) shared by the
  harness producer and this consumer, instead of a bare `"end_turn"` on each side.

### [LOW] src/verification/robustness_profile.rs:39 — `for_behavior` doc omits `behavior_hint`/vendor-identity in the resolution path
- **Category:** quality
- **Decision:** CONNECT
- **Description:** The doc states the behavior name resolves "via
  `protocol_to_behavior` / `model_behavior_override`", but the real single resolver is
  `behavior_resolve::resolve_behavior`, which additionally consults
  `provider.behavior_hint()` (`vendor_identity` → `"strict"` for Kimi/Minimax/DeepSeek/
  Qwen/GLM). The omission understates the actual wiring and could mislead a maintainer
  into thinking weak-vendor self-identification does not affect the watchdog thresholds.
- **Suggested fix:** Re-point the doc comment at `resolve_behavior` as the single source
  of truth.

---

## Negative / not done

- No edits were made to any source file (read-only audit).
- Cross-references (e.g. the harness `Halt → TerminateReason::StopHookHalt` naming for
  tool-loop halts, and `subagent_spawner` hardcoding `conservative()` rather than
  resolving the subagent's model) live **outside** `src/verification` and were noted but
  intentionally not filed as findings here.
- Not verified with a compile/test pass (audit is static); the CUT recommendation for
  `len()`/`is_empty()` should be confirmed with `cargo test --lib --no-run` before removal.
