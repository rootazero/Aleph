# Tool Pipeline Hardening Design

Date: 2026-04-07
Status: Approved
Scope: `src/agent_loop/safety.rs`, `src/agent_loop/tool_pipeline.rs`, `src/agent_loop/loop_core.rs`, `src/agent_loop/tool_orchestrator.rs`, `src/config/types/policies/tool_safety.rs`

## Background

Comparative analysis of Claude Code's 14-stage tool execution chain (from `claude-code-deep-dive-README.md` section 8) against Aleph's 7-stage ToolPipeline revealed four areas where Aleph's implementation has gaps or internal inconsistencies. This design addresses all four while preserving Aleph's architectural advantages (Hook Kind taxonomy, priority system, concurrent batching, PermissionDecision 4-level model).

## Problem Statement

1. **SafetyGuard ↔ ToolSafetyPolicy disconnection**: `ToolSafetyPolicy` (tool_safety.rs) has 40+ keywords across 4 risk categories and per-category fallback defaults, but `SafetyGuard` (safety.rs) ignores it entirely and hardcodes 4 tool names in `default_high_risk_permissions()`.

2. **NeedsConfirmation silently downgraded**: When SafetyGuard returns `NeedsConfirmation`, `loop_core.rs` downgrades it to a denial. The `needs_user_confirmation` field on `PipelineOutcome` is pre-wired but never consumed. Users never see a confirmation prompt.

3. **Unstructured Pipeline tracing**: Pipeline stages use ad-hoc `tracing::debug!` and manual `Instant::now()` timing. No structured span hierarchy exists for trace visualization or future OpenTelemetry integration.

4. **Hook Ask / Safety Ask collision**: When a Hook emits `Ask` and SafetyGuard independently also returns `NeedsConfirmation`, the Safety result overwrites the Hook's decision, short-circuiting the confirmation flow.

## Non-Goals

- Pipeline stage objectification (trait-based middleware chain) — violates P6/YAGNI
- New dependency introduction — all changes use existing `tracing` and `serde_json`
- Channel-specific confirmation UI implementation — each Channel implements `on_confirmation_needed()` independently, outside this design's scope

## Design

### Part A: Unified Safety Layer (SafetyGuard ← ToolSafetyPolicy)

**Current state:**

```
ToolSafetyPolicy                    SafetyGuard
┌───────────────────────┐          ┌───────────────────────────┐
│ high_risk_keywords    │          │ default_high_risk_perms() │
│ low_risk_keywords     │   ✗     │   hardcoded: bash_exec,   │
│ readonly_keywords     │ ─断裂─→ │   file_delete, shell,     │
│ reversible_keywords   │          │   code_exec               │
│ fallbacks per category│          └───────────────────────────┘
└───────────────────────┘
```

**Target state:**

`SafetyGuard` holds an optional `ToolSafetyPolicy` and uses keyword inference for tools not explicitly listed in `tool_permissions`.

#### Changes to `safety.rs`

1. Add `safety_policy: Option<ToolSafetyPolicy>` field to `SafetyGuard`.

2. New method `infer_permission(&self, tool_name: &str) -> PermissionAction`:
   - If `tool_name` matches `high_risk_keywords` → `Ask`
   - If `tool_name` matches `low_risk_keywords` → `Ask` (lower risk but still irreversible)
   - If `tool_name` matches `readonly_keywords` → `Allow`
   - If `tool_name` matches `reversible_keywords` → `Allow`
   - Else → use `default_permission`

3. Modify `check()`: when `tool_permissions.get(tool_name)` returns `None`, call `infer_permission()` instead of falling through to `default_permission` directly.

4. Delete `default_high_risk_permissions()` function.

5. Modify `SafetyGuard::default_guard()`: construct with `ToolSafetyPolicy::default()` instead of hardcoded tool list.

6. Modify `SafetyGuard::from_permissions()`: accept optional `ToolSafetyPolicy` parameter.

7. Update `is_high_risk()`: delegate to `infer_permission()` returning `Ask`.

#### Changes to `tool_safety.rs`

None — `ToolSafetyPolicy` struct is unchanged. It remains a pure configuration type.

#### Code to delete

- `default_high_risk_permissions()` in `safety.rs` (~10 lines)
- Hardcoded `high_risk_tools` array

### Part B: Confirmation Flow (Channel-Autonomous Model)

**Architecture alignment:** R4 (Interface is pure I/O), P1 (low coupling)

#### LoopCallback extension

Add to the `LoopCallback` trait:

```rust
/// Request user confirmation for a high-risk tool call.
///
/// Called when either a Hook emits `PermissionDecision::Ask` or
/// SafetyGuard classifies a tool as `NeedsConfirmation`.
///
/// Default implementation returns `false` (reject), preserving
/// backward compatibility with all existing Channel implementations.
async fn on_confirmation_needed(
    &mut self,
    tool_name: &str,
    tool_input: &Value,
    reason: &str,
) -> bool {
    false
}
```

#### loop_core.rs changes

When `PipelineOutcome.needs_user_confirmation == true`:

1. Call `callback.on_confirmation_needed(tool_name, tool_input, reason)`.
2. If `true` → re-execute the tool through Pipeline Stage 5 only (skip validation/hooks/safety — they already passed).
3. If `false` → return `[DENIED] Tool requires user confirmation. Confirmation was rejected.`

This replaces the current hard-coded denial in `map_safety_error()` for `NeedsConfirmation`.

#### SafetyGuard NeedsConfirmation path

`map_safety_error()` for `NeedsConfirmation` currently produces a hard denial message. This changes:

- In `tool_pipeline.rs` Stage 4: `NeedsConfirmation` sets `needs_user_confirmation = true` instead of returning an error outcome immediately.
- The denial only happens if the callback rejects (or defaults to reject).

### Part C: Structured Pipeline Tracing

#### Span naming convention

All spans follow `pipeline.<stage>` pattern, nested under the existing `#[tracing::instrument(name = "tool_pipeline")]` parent span.

| Stage | Span name | Key fields |
|-------|-----------|------------|
| 2 | `pipeline.validate` | `tool`, `tool_id`, `result` (pass/fail) |
| 3 | `pipeline.pre_hooks` | `tool`, `hooks_count`, `decision` |
| 4 | `pipeline.safety` | `tool`, `hook_decision`, `safety_decision`, `final_action` |
| 5 | `pipeline.execute` | `tool`, `tool_id` (duration auto-recorded by span) |
| 6-7 | `pipeline.post_hooks` | `tool`, `is_error`, `hooks_count` |

#### Code to delete

- Manual `Instant::now()` + `elapsed_ms` calculations in `tool_pipeline.rs` (~10 occurrences)
- Ad-hoc `tracing::debug!("pipeline_execute: start")` messages replaced by span entry

#### Code to keep

- `tool_orchestrator.rs` retains its own `Instant::now()` for `ToolOutcome.duration_ms` field (this is a data field, not a log)

### Part D: Hook-Permission Collision Fix

#### Problem

```
Hook: Ask { reason: "needs review" }
  → needs_user_confirmation = true

SafetyGuard: NeedsConfirmation { tool: "shell" }
  → returns error outcome immediately  ← OVERWRITES Hook's Ask
```

#### Fix

In Stage 4 of `tool_pipeline.rs`, when `needs_user_confirmation` is already `true` (set by Hook Ask in Stage 3):

```rust
SafetyError::NeedsConfirmation { .. } => {
    // Hook already requested confirmation — Safety agrees.
    // Don't return error; let the confirmation flow handle it.
    tracing::debug!("safety confirms hook ask — deferring to confirmation flow");
}
```

This means both Hook Ask and Safety Ask converge to the same confirmation flow (Part B), rather than Safety overriding Hook.

**Phase dependency note:** In Phase 1 (before Part B is implemented), this fix ensures the `needs_user_confirmation` flag survives Stage 4 rather than being overwritten by an error return. The default `LoopCallback::on_confirmation_needed()` still returns `false`, so the net behavior remains a denial — but it flows through the correct path, ready for Phase 2.

#### Final decision trace

After Stage 4 completes, emit:

```rust
tracing::info!(
    tool = name,
    hook_decision = ?decision,       // None | Allow | Ask | Block | Deny
    safety_passed = safety_ok,       // bool
    final_action = final_action_str, // "execute" | "confirm" | "deny" | "block"
    "permission resolved"
);
```

## Phase Plan

**Phase 1 (internal cleanup, zero risk):**
- Part A: SafetyGuard ← ToolSafetyPolicy unification
- Part D: Hook-Permission collision fix
- Part C: Structured tracing (can be done in parallel)

**Phase 2 (capability addition):**
- Part B: Confirmation flow via LoopCallback

Phase 1 changes are purely internal refactoring with full test coverage. Phase 2 adds a new trait method with a backward-compatible default, so no existing code breaks.

## Testing Strategy

- **Part A**: Update existing `SafetyGuard` tests to verify keyword inference. Add tests for edge cases: tool matching multiple keyword categories (high_risk wins), unknown tools using fallback.
- **Part B**: Add `loop_core` test with a mock LoopCallback that returns `true` from `on_confirmation_needed()`, verify tool executes. Test with `false`, verify denial.
- **Part C**: No functional tests needed — verify spans appear in test trace output.
- **Part D**: Add test: Hook Ask + Safety NeedsConfirmation → `needs_user_confirmation == true` (not error).

## Files Modified

| File | Change type | Estimated lines |
|------|-------------|-----------------|
| `src/agent_loop/safety.rs` | Modify + delete | -10, +40 |
| `src/agent_loop/tool_pipeline.rs` | Modify | +30, -20 |
| `src/agent_loop/loop_core.rs` | Modify | +20 |
| `src/agent_loop/mod.rs` | Modify (LoopCallback trait) | +10 |
| `src/config/types/policies/tool_safety.rs` | No change | 0 |
| `src/config/types/policies/tool_permissions.rs` | No change | 0 |

Total: ~100 lines net change (excluding tests)
