# Jev Classification Report: Batch GUARD1

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Target Module**: `src/guardrails`  
**Files Audited**:
- `src/guardrails/mod.rs`
- `src/guardrails/traits.rs`
- `src/guardrails/decision.rs`
- `src/guardrails/pii_secrets.rs`
- `src/guardrails/registry.rs`
- `src/guardrails/tests/bench.rs`
- `src/guardrails/tests/input.rs`
- `src/guardrails/tests/output.rs`
- `src/guardrails/tests/registry.rs`

---

## 1. Executive Summary

Batch GUARD1 covers the guardrail core types, traits, registry, PII/secret scanning adapter, and unit test suites.
The codebase is generally well-structured with clear security boundaries (fail-closed semantics, DoS depth/node caps, and deduplication of warnings).
However, static review reveals several redundant allocation patterns on the hot execution paths (turn-entry input evaluation, turn-exit output evaluation, tool dispatch argument evaluation, and session history replay screening), as well as unnecessary indirection in resolver reference coercion.

---

## 2. Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **G1-1** | `src/guardrails/registry.rs:124, 273` | `REDUNDANT-ALLOCATION` | `let mut current = text.to_string();` in `evaluate_input` and `evaluate_output` unconditionally heap-allocates an owned `String` before scanning, even though >99% of inputs/outputs are clean and never modified. | **FIX-NOW** | Use `Cow::Borrowed(text)`; only allocate if sanitized or warned. Zero-allocation on clean path. |
| **G1-2** | `src/guardrails/registry.rs:303` | `REDUNDANT-ALLOCATION` | `let mut current = args.clone();` in `evaluate_tool_call` deep-clones the entire JSON `Value` tree on every tool call before inspecting, even when no guardrail modifies the arguments. | **FIX-NOW** | Use lazy rewrite tracking `Option<Value>`; avoid deep clone when no guardrail sanitizes. |
| **G1-3** | `src/guardrails/registry.rs:227` | `REDUNDANT-ALLOCATION` | `screenable_text(&record.event).map(str::to_string)` clones every replayed user/system message into a heap `String` just to pass `&text` to `evaluate_input`. | **FIX-NOW** | Borrow `screenable_text(&events[idx].event)` directly by index without `.map(str::to_string)`. |
| **G1-4** | `src/guardrails/registry.rs:386-399` | `REDUNDANT-ALLOCATION` | `settle_chain` takes `text: String` by value, forcing caller to pass owned string even when returning `Allow`. | **FIX-NOW** | Take `Cow<'_, str>` and short-circuit to `Allow` before `into_owned()`. |
| **G1-5** | `src/guardrails/pii_secrets.rs:158-163` | `REDUNDANT-ALLOCATION` | Matching `&r` by reference in `scan_tool_arg_leaf` forces `text.clone()` on placeholder substitution, even though `r` is owned and never used again in that branch. | **FIX-NOW** | Match `r` by value directly to move `text` into `Ok(text)`. |
| **G1-6** | `src/guardrails/pii_secrets.rs:391-394` | `CONTROL-FLOW` | Over-complicated closure and manual cast: `self.resolver.as_ref().map(\|a\| a.as_ref() as &dyn AsyncSecretResolver)`. | **FIX-NOW** | Simplify to `self.resolver.as_deref()`. |
| **G1-7** | `src/guardrails/pii_secrets.rs:159, 167, 171` | `ACCIDENTAL-COMPLEXITY` | Duplicate sources and warnings accumulated across leaves in `scan_tool_arg_leaf`, creating noisy audit strings. | **FIX-NOW** | Deduplicate `sources` and `warnings` when pushing. |
| **G1-8** | `src/guardrails/decision.rs:46` | `ACCIDENTAL-COMPLEXITY` | Speculative accessor methods (`Replacement::text()`, `Replacement::source()`). | **REJECT** | Public fields already exist and are used; adding unused accessor methods adds complexity without benefit. |
| **G1-9** | `src/guardrails/traits.rs:34` | `INVARIANT-TIGHTENING` | Mutating `&Value` to `Value` or `&mut Value` in `ToolCallGuardrail::evaluate_tool_call`. | **REJECT** | Violates public trait stability invariant. |
| **G1-10** | `src/guardrails/decision.rs:75-93` | `DEAD-CODE` | `is_block`, `is_allow`, `is_sanitize`, `is_warn` methods. | **REJECT** | Already scoped to `#[cfg(test)]` and used throughout unit tests; not dead in test builds. |

---

## 3. Invariant Safety Pre-Flight

1. **Public API Contracts**:
   - `GuardrailRegistry::evaluate_input`, `evaluate_output`, `evaluate_tool_call`, `screen_session_input`, `redact_session_input` signatures remain identical.
   - `Replacement` and `GuardrailDecision` types remain unchanged.
   - All trait definitions (`InputGuardrail`, `OutputGuardrail`, `ToolCallGuardrail`) remain untouched.
2. **Behavioral Equivalence**:
   - Clean / benign inputs continue to return `GuardrailDecision::Allow`.
   - Sanitized inputs continue to return `GuardrailDecision::Sanitize(Replacement { text, source })`.
   - Blocked inputs continue to return `GuardrailDecision::Block { reason, class }`.
   - Warning collection and deduplication are preserved.
   - Replay screening degradation for older turns vs current turn is preserved.
3. **Performance Impact**:
   - Eliminates 1 heap allocation per input evaluation on the benign path.
   - Eliminates 1 heap allocation per output evaluation on the benign path.
   - Eliminates deep AST cloning and redundant serialization on the tool call dispatch benign path.
   - Eliminates N heap allocations during `screen_session_input` history re-screening.
