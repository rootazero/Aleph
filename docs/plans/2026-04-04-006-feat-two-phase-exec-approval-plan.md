---
title: "feat: Two-Phase Exec Approval with LLM Self-Assessment"
type: feat
status: active
date: 2026-04-04
origin: docs/brainstorms/2026-04-04-two-phase-exec-approval-requirements.md
---

# feat: Two-Phase Exec Approval with LLM Self-Assessment

## Overview

Replace the confidence-threshold-based tool confirmation system with LLM-driven approval decisions. The LLM outputs `approval_action` (auto_execute / ask_user / block) alongside each tool_call in the same generation, eliminating extra LLM calls while providing semantically richer safety judgments. A deterministic `always_confirm` safety floor protects against prompt injection and circular trust risks.

## Problem Frame

The current system uses a numeric confidence threshold (default 0.7) to decide whether a tool call needs user confirmation. This violates Arch-R8 (LLM Sovereignty) by substituting deterministic rules for LLM reasoning. Low-risk operations trigger unnecessary confirmations while high-risk operations may pass if confidence scores happen to be high. (see origin: docs/brainstorms/2026-04-04-two-phase-exec-approval-requirements.md)

## Requirements Trace

- R1. LLM outputs tool_call + approval_action in same generation, zero extra LLM calls
- R2. approval_action enum: auto_execute | ask_user | block
- R3. approval_reason in text for tracing logs (no independent audit storage)
- R4. always_confirm tool list — deterministic safety floor, LLM cannot override
- R5. Parse failure fallback → ask_user, never auto_execute
- R6. block + block_action: notify | retry
- R7. retry max 2 per invocation, then escalate to notify
- R8. retry: remove blocked turn from history, inject system message with block reason
- R9. notify: reuse existing confirmation UI (Approve/Cancel)
- R10. Replace both dispatcher confirmation subsystem and agent loop safety path
- R11. Preserve TrustStage as LLM context input
- R12. Preserve approval_bridge forwarding for ask_user
- R13. Approval instructions via system prompt template
- R14. TrustStage aggregate list + always_confirm list in prompt
- R15. Prompt forbids parameter reproduction in approval_reason
- R16. approval_action carried via ProviderResponse.text JSON block

## Scope Boundaries

- No independent Gate LLM call (Arch-R10)
- No TrustStage upgrade logic changes
- No approval_bridge forwarding mechanism changes (only trigger condition changes)
- No new UI components — block/notify reuses existing Approve/Cancel
- No independent audit storage — approval_reason via tracing only

## Context & Research

### Relevant Code and Patterns

**Dispatcher Confirmation (to be replaced):**
- `src/dispatcher/confirmation.rs` — `ToolConfirmation`, `ConfirmationConfig`, confidence threshold check
- `src/dispatcher/async_confirmation.rs` — `PendingConfirmation`, `PendingConfirmationStore`, `AsyncConfirmationHandler`
- These operate at the routing/dispatcher layer, NOT within the agent loop

**Agent Loop Tool Execution (primary integration target):**
- `src/agent_loop/loop_core.rs` — `AgentLoop<P>`, `execute_turn_tools()` (~line 1443), `resolve_turn_response()`
- `src/agent_loop/tool_pipeline.rs` — `ToolPipeline` 7-stage pipeline, Stage 4 safety via `SafetyGuard`
- `src/agent_loop/safety.rs` — `SafetyGuard`, `SafetyError::NeedsConfirmation` (currently downgraded to denied)
- `StreamingToolBridge` starts tool execution concurrently during LLM streaming (~line 1113)

**Provider Response:**
- `src/providers/adapter.rs` — `ProviderResponse { text, tool_calls, thinking, stop_reason, usage }`
- `NativeToolCall { id, name, arguments }` — no extension fields, not modified
- `text` field carries reasoning alongside tool_calls — natural carrier for approval JSON block

**Prompt Builder:**
- `src/agent_loop/prompt_builder.rs` — `PromptBuilder`, section registry with priority ordering
- `src/agent_loop/prompt_sections/` — individual section renderers (actions.rs, tools.rs, system_rules.rs)
- Pattern: `register(PromptSection { name, stability: Dynamic, priority, content })`

**TrustStage:**
- `src/exec/approval/types.rs` — `TrustStage { Draft, Trial, Verified }`, currently only in exec sandbox path
- Needs threading into AgentLoop via builder for prompt injection

**Approval Bridge (preserved):**
- `src/gateway/handlers/approval_bridge.rs` — `ForwardMode`, `get_forward_targets()`
- `src/gateway/handlers/exec_approvals.rs` — `ExecApprovalManager`, `wait_for_decision()`

**Tool Metadata:**
- `src/dispatcher/types/definition.rs` — `ToolDefinition { requires_confirmation: bool, ... }`
- `src/agent_loop/tool.rs` — `LoopToolRegistry`, `LoopTool` trait

**Conversation History:**
- `Vec<UnifiedMessage>` in `LoopRuntime.messages`
- `remove_oldest_complete_round()` shows safe turn removal pattern (drain + reinsert)
- System message injection: `messages.push(UnifiedMessage::user("[SYSTEM] ..."))`

## Key Technical Decisions

- **Text-based carrier with delimiter tags**: approval_action and approval_reason are emitted as a tagged JSON block in `ProviderResponse.text` (e.g., `<exec-approval>{"action":"auto_execute","reason":"..."}</exec-approval>`). Parsed before tool execution. Rationale: avoids modifying `NativeToolCall` or any of the 4 protocol adapters. Falls back to ask_user on parse failure (R5). (see origin)
- **Gate in execute_turn_tools, post-execution outcome control**: `StreamingToolBridge` starts tool execution concurrently during LLM streaming — tools may have already executed by the time the gate runs. The gate therefore operates on post-execution `PipelineOutcome` results: it controls whether results are committed to history and surfaced to the user, not whether execution occurs. For ask_user/block paths, the tool has already run but its results are held pending approval. This is a meaningful UX distinction documented in the gate's API. Rationale: minimal disruption to the 7-stage pipeline architecture; preventing execution entirely would require modifying StreamingToolBridge internals
- **Dependency inversion for approval requests**: `ExecApprovalManager` lives in the gateway layer and cannot be imported by `agent_loop` without violating Arch-R4. Define an `ApprovalRequester` trait in `agent_loop` that the gateway layer implements, injected into `AgentLoop` at construction via the builder pattern. This follows the existing pattern of injecting capabilities into the loop. Rationale: R12 preservation, clean layer boundary
- **Retry via history mutation**: On block+retry, drain the last assistant turn + tool_results, push a system message with block reason. This follows the established `remove_oldest_complete_round()` pattern. Rationale: R8, prevents LLM from repeating blocked call
- **always_confirm as config, not code**: The always_confirm list is stored as a `HashSet<String>` in `ApprovalConfig`, loaded from the agent/tool config. Note: the agent loop's `ToolDefinition` (in `src/agent_loop/tool.rs`) is intentionally simpler than `dispatcher::ToolDefinition` and does NOT have a `requires_confirmation` field. The gate checks always_confirm by matching tool names from `PipelineOutcome`, not via ToolDefinition metadata. Rationale: R4, minimal new infrastructure

## Open Questions

### Resolved During Planning

- **Where to intercept tool execution?** — In `execute_turn_tools()` after `StreamingToolBridge` completes, before tool results are pushed to history. The bridge already collects all `PipelineOutcome` results; the approval gate processes each outcome before committing.
- **How to parse approval from text?** — Delimiter-tagged JSON block in `ProviderResponse.text`: `<exec-approval>{"action":"...","reason":"..."}</exec-approval>`. Regex extraction with serde_json fallback. Missing/malformed → ask_user.
- **How to inject TrustStage?** — Aggregate all registered tools' TrustStage into a single prompt section. Tools without explicit TrustStage default to Draft. Injected as a Dynamic prompt section at priority 450.
- **Where does always_confirm config live?** — `ApprovalConfig.always_confirm: HashSet<String>` loaded from agent/tool config. The agent loop's `ToolDefinition` does not have a `requires_confirmation` field — the gate checks by tool name match against the HashSet, not via ToolDefinition metadata.
- **How does retry interact with StreamingToolBridge?** — Retry triggers a full new turn: drain history, inject system message, call `think_turn()` again. The bridge runs fresh for the new turn. Retry counter is local to the approval gate function.

### Deferred to Implementation

- Exact prompt wording for approval instructions (requires iterative testing with real LLM responses)
- Whether `<exec-approval>` tags should use XML-style or markdown code fence delimiters (depends on which LLM providers handle better in practice)
- Performance impact of TrustStage aggregate list in prompt (measure after implementation)

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
┌─────────────────────────────────────────────────────┐
│                   AgentLoop Turn                     │
│                                                      │
│  1. think_turn()                                     │
│     ├─ System prompt includes:                       │
│     │   [exec_approval section @ priority 450]       │
│     │   - Approval instructions                      │
│     │   - TrustStage aggregate list                  │
│     │   - always_confirm list                        │
│     │                                                │
│     └─ LLM generates:                                │
│         text: "<exec-approval>{action,reason}</exec..│
│         tool_calls: [NativeToolCall, ...]             │
│                                                      │
│  2. parse_approval(response.text)                    │
│     ├─ Found + valid enum → ApprovalDecision         │
│     └─ Missing / malformed → ask_user (R5)           │
│                                                      │
│  3. apply_safety_floor(decision, tool_calls)         │
│     └─ Any tool in always_confirm? → force ask_user  │
│                                                      │
│  4. route_by_decision:                               │
│     ├─ auto_execute → execute_turn_tools() normally  │
│     ├─ ask_user → ExecApprovalManager.create()       │
│     │             → wait_for_decision()              │
│     │             → approval_bridge forwards         │
│     │             → Approve: execute / Cancel: skip  │
│     └─ block:                                        │
│         ├─ notify → same as ask_user path            │
│         └─ retry (count < 2):                        │
│             ├─ drain last assistant turn              │
│             ├─ push system msg with block reason      │
│             └─ goto 1 (new think_turn)               │
│                                                      │
│  5. execute_turn_tools() → PipelineOutcome[]         │
│  6. Push tool results to history                     │
│  7. Continue loop                                    │
└─────────────────────────────────────────────────────┘
```

## Implementation Units

- [ ] **Unit 1: Approval Types and Config**

**Goal:** Define the core types for LLM approval decisions and the always_confirm configuration.

**Requirements:** R2, R4, R5, R6

**Dependencies:** None

**Files:**
- Create: `src/agent_loop/exec_approval/types.rs`
- Create: `src/agent_loop/exec_approval/mod.rs`
- Modify: `src/agent_loop/mod.rs` (add `exec_approval` module)
- Test: `src/agent_loop/exec_approval/tests.rs`

**Approach:**
- Define `ApprovalAction` enum: `AutoExecute`, `AskUser`, `Block { action: BlockAction }` — use `#[serde(deny_unknown_fields)]` for strict deserialization
- Define `BlockAction` enum: `Notify`, `Retry`
- Define `ApprovalDecision` struct: `action: ApprovalAction`, `reason: String` — strict serde: unknown fields or invalid enum variants trigger parse failure → ask_user
- Define `ApprovalConfig` struct: `always_confirm: HashSet<String>`, loaded from tool config
- Define `ApprovalRequester` trait: `async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome` — gateway layer implements, injected via AgentLoop builder
- Implement `Default` for `ApprovalDecision` → `AskUser` with reason "parse failure fallback" (R5)

**Patterns to follow:**
- `src/exec/approval/types.rs` — enum + struct pattern for approval domain types
- `src/dispatcher/types/definition.rs` — ToolDefinition field conventions

**Test scenarios:**
- Happy path: Deserialize valid ApprovalDecision with each ApprovalAction variant
- Happy path: ApprovalConfig correctly identifies tools in always_confirm set
- Edge case: Default ApprovalDecision is AskUser (R5 invariant)
- Edge case: Empty always_confirm set — no tools forced

**Verification:**
- All types compile, serialize/deserialize correctly, Default produces ask_user

---

- [ ] **Unit 2: Text-Based Approval Parser**

**Goal:** Parse `<exec-approval>` JSON block from ProviderResponse.text, with fallback to ask_user.

**Requirements:** R5, R16

**Dependencies:** Unit 1

**Files:**
- Create: `src/agent_loop/exec_approval/parser.rs`
- Modify: `src/agent_loop/exec_approval/mod.rs`
- Test: `src/agent_loop/exec_approval/tests.rs`

**Approach:**
- Extract content between `<exec-approval>` and `</exec-approval>` tags from text field
- Parse extracted JSON into `ApprovalDecision` via serde_json
- On any failure (no tags, malformed JSON, invalid enum value, missing fields) → return `ApprovalDecision::default()` (ask_user)
- Log parse failures at `warn!` level with the raw text snippet for debugging
- Function signature: `fn parse_approval(text: &Option<String>) -> ApprovalDecision`

**Patterns to follow:**
- Existing tag-parsing patterns in the codebase (if any)
- `serde_json::from_str` with graceful error handling

**Test scenarios:**
- Happy path: Valid JSON with action=auto_execute, reason="safe read operation"
- Happy path: Valid JSON with action=block, block_action=retry
- Happy path: Valid JSON with action=ask_user
- Edge case: No `<exec-approval>` tags in text → ask_user fallback
- Edge case: Text is None → ask_user fallback
- Edge case: Tags present but JSON is malformed → ask_user fallback
- Edge case: Tags present but action value is not in enum → ask_user fallback
- Edge case: Tags present but reason field missing → ask_user with empty reason
- Edge case: Multiple `<exec-approval>` blocks → use first one
- Error path: Valid tags wrapping non-JSON content → ask_user fallback

**Verification:**
- All parse paths return valid ApprovalDecision, never panics, fallback is always ask_user

---

- [ ] **Unit 3: Prompt Section for Approval Instructions**

**Goal:** Create a prompt section that injects approval instructions, TrustStage aggregate, and always_confirm list into the system prompt.

**Requirements:** R13, R14, R15

**Dependencies:** Unit 1

**Files:**
- Create: `src/agent_loop/prompt_sections/exec_approval.rs`
- Modify: `src/agent_loop/prompt_sections/mod.rs`
- Modify: `src/agent_loop/prompt_builder.rs` (add builder method)
- Test: `src/agent_loop/prompt_sections/exec_approval_tests.rs`

**Approach:**
- Define `ExecApprovalContext` struct: TrustStage map (`HashMap<String, TrustStage>`), always_confirm list
- Render function produces a `PromptSection` with stability=Dynamic, priority=450
- Content template instructs LLM to:
  - Output `<exec-approval>{"action":"...","reason":"..."}</exec-approval>` before/alongside tool calls
  - Choose auto_execute for clearly safe operations on Verified tools
  - Choose ask_user when uncertain or tool is sensitive
  - Choose block with notify for dangerous operations, retry for "better approach available"
  - Never reproduce tool parameter values in approval_reason (R15)
- Include TrustStage aggregate: `"Tools trust levels: {read_file: Verified, bash_exec: Draft, ...}"`
- Include always_confirm notice: `"Tools requiring mandatory confirmation: [bash_exec, file_delete, ...]"`
- Add convenience method `with_exec_approval(context)` on PromptBuilder

**Patterns to follow:**
- `src/agent_loop/prompt_sections/actions.rs` — section renderer pattern
- `src/agent_loop/prompt_sections/tools.rs` — tool metadata injection pattern
- `with_default_behavior_sections()` — builder chaining pattern

**Test scenarios:**
- Happy path: Render section with 3 tools at different TrustStages — output contains all tool names and stages
- Happy path: Render with non-empty always_confirm list — output contains tool names
- Edge case: Empty tool map — section still renders with instructions but no tool list
- Edge case: Empty always_confirm — section renders without mandatory confirmation notice
- Integration: Section registered at priority 450, stability Dynamic

**Verification:**
- Prompt section renders correctly with all context, registered in PromptBuilder

---

- [ ] **Unit 4: Approval Gate in Agent Loop**

**Goal:** Intercept tool execution in `execute_turn_tools()` to apply LLM approval decisions, always_confirm override, and route to auto_execute / ask_user / block paths.

**Requirements:** R1, R2, R4, R5, R9, R12

**Dependencies:** Unit 1, Unit 2, Unit 3

**Files:**
- Create: `src/agent_loop/exec_approval/gate.rs`
- Modify: `src/agent_loop/loop_core.rs` (integrate gate into execute_turn_tools)
- Modify: `src/agent_loop/exec_approval/mod.rs`
- Test: `src/agent_loop/exec_approval/gate_tests.rs`

**Approach:**
- New `ApprovalGate` struct holds `ApprovalConfig`, `Option<Box<dyn ApprovalRequester>>`, and retry state
- Note: `resolve_turn_response()` already pushes the assistant message to history before `execute_turn_tools()` runs. The gate operates on post-execution outcomes — tools have already executed via StreamingToolBridge by this point
- After `execute_turn_tools()` collects `PipelineOutcome` results:
  1. Call `parse_approval(response.text)` → `ApprovalDecision`
  2. For each tool outcome, check `always_confirm` by tool name → override to ask_user if matched
  3. Route by final decision:
     - `auto_execute` → commit tool results to history normally
     - `ask_user` → hold results, call `ApprovalRequester::request_approval()`, wait for user decision via approval_bridge. Approve → commit results. Cancel → discard results, push "[CANCELLED]"
     - `block + notify` → same as ask_user (tool name + reason shown, Approve/Cancel)
     - `block + retry` → delegate to retry handler (Unit 5)
- Log approval decision at `info!` level: tool name, action, reason (R3)
- Integration point: in `execute_turn_tools()` after awaiting `executor_handle`, before pushing results to history

**Patterns to follow:**
- `execute_turn_tools()` flow in loop_core.rs
- `ExecApprovalManager::create()` / `wait_for_decision()` pattern
- `SafetyGuard::NeedsConfirmation` handling pattern (but improved — not downgraded to denied)

**Test scenarios:**
- Happy path: auto_execute decision → tools execute, results in history
- Happy path: ask_user decision → approval created, user approves → tools execute
- Happy path: ask_user decision → user cancels → tool result is "[CANCELLED]"
- Happy path: block+notify → same as ask_user path, approval created
- Happy path: always_confirm override → LLM says auto_execute but tool is in list → ask_user
- Edge case: Multiple tool_calls in one turn, mixed approval (some always_confirm, some not)
- Edge case: No tool_calls in response → gate is no-op
- Error path: Parse failure → ask_user fallback, approval created
- Integration: Approval reason logged via tracing at info level

**Verification:**
- Tool execution is gated by approval decision, always_confirm cannot be bypassed, parse failures default to ask_user

---

- [ ] **Unit 5: Block/Retry Handler**

**Goal:** Implement the retry mechanism for block+retry decisions: history mutation, system message injection, retry counter with escalation.

**Requirements:** R6, R7, R8

**Dependencies:** Unit 1, Unit 4

**Files:**
- Create: `src/agent_loop/exec_approval/retry.rs`
- Modify: `src/agent_loop/exec_approval/mod.rs`
- Modify: `src/agent_loop/loop_core.rs` (wire retry into main loop)
- Test: `src/agent_loop/exec_approval/retry_tests.rs`

**Approach:**
- `RetryHandler` struct with `attempts: u8` counter (per gate invocation, not persisted)
- Note: `resolve_turn_response()` has already pushed the assistant message to history before the gate runs. The drain operation is therefore unconditional — there will always be an assistant message to remove
- On block+retry:
  1. Increment counter. If > 2, escalate to notify (delegate to ask_user path)
  2. Find last assistant message index in history (always present — pushed by resolve_turn_response)
  3. Drain assistant message + all following tool_result messages (follow `remove_oldest_complete_round()` pattern)
  4. Push `UnifiedMessage::user("[SYSTEM] Your previous tool call was blocked. Reason: {reason}. Please generate an alternative approach.")`
  5. Return control to main loop to trigger new `think_turn()`
- Counter resets when `ApprovalGate` is constructed for a new tool_call sequence

**Patterns to follow:**
- `remove_oldest_complete_round()` in loop_core.rs — safe turn drainage pattern
- `find_safe_cut_point()` — history boundary detection
- System message injection: `UnifiedMessage::user("[SYSTEM] ...")`

**Test scenarios:**
- Happy path: First retry → history cleaned, system message injected, counter=1
- Happy path: Second retry → history cleaned again, counter=2
- Happy path: Third attempt → escalates to notify (ask_user path)
- Edge case: Block+retry on very first turn (assistant message is the only message) → drain succeeds, system message is sole context for retry
- Edge case: Counter resets between different tool_call sequences
- Integration: After retry, next think_turn sees clean history with system message

**Verification:**
- Retry correctly mutates history, counter escalates after 2, clean history enables LLM to generate alternatives

---

- [ ] **Unit 6: Migration — Remove Confidence-Based Confirmation**

**Goal:** Remove the old confidence-threshold system and wire the new LLM approval gate as the sole confirmation mechanism.

**Requirements:** R10

**Dependencies:** Unit 4, Unit 5

**Files:**
- Modify: `src/dispatcher/confirmation.rs` (deprecate/remove confidence logic)
- Modify: `src/dispatcher/async_confirmation.rs` (remove confidence-based triggering)
- Modify: `src/agent_loop/loop_core.rs` (remove old SafetyGuard::NeedsConfirmation downgrade)
- Modify: `src/agent_loop/safety.rs` (clean up NeedsConfirmation → denied path)
- Modify: `src/agent_loop/prompt_builder.rs` (ensure exec_approval section is wired in default builder)
- Test: `src/agent_loop/exec_approval/integration_tests.rs`

**Approach:**
- Phase approach within this unit:
  1. First, wire the new approval gate into the default AgentLoop construction (prompt section + gate)
  2. Then, remove the confidence-based trigger in `ToolConfirmation` — change `needs_confirmation()` to always return false or remove the call site
  3. Remove `SafetyGuard::NeedsConfirmation` downgrade-to-denied path in safety.rs
  4. Clean up `ConfirmationConfig.threshold` usage — the field can remain for backward compat but is no longer consulted
  5. Preserve `PendingConfirmationStore` and `AsyncConfirmationHandler` as they are reused by the new ask_user path
- Do NOT delete the dispatcher confirmation files entirely — they contain types (`PendingConfirmation`, `ConfirmationState`) reused by the approval bridge

**Patterns to follow:**
- Existing AgentLoop builder pattern for wiring new components
- Gradual migration: wire new before removing old

**Test scenarios:**
- Happy path: Full agent loop turn with LLM auto_execute → tool executes without user confirmation
- Happy path: Full agent loop turn with LLM ask_user → approval_bridge receives request
- Happy path: Full agent loop turn with LLM block+retry → history mutation, new turn generated
- Integration: Old confidence threshold no longer triggers confirmations
- Integration: always_confirm tools still trigger ask_user regardless of LLM output
- Error path: LLM response with no approval tags → defaults to ask_user, not auto_execute
- Error path: ApprovalRequester unavailable (None injected) → gate falls back to denied with explanation, matching current NeedsConfirmation behavior

**Verification:**
- Agent loop uses only LLM approval gate, confidence threshold is inactive, all existing approval forwarding paths still work

## System-Wide Impact

- **Interaction graph:** AgentLoop.execute_turn_tools() → ApprovalGate → ExecApprovalManager → approval_bridge → channel handlers (Telegram, Discord, etc.). The gate inserts between think_turn response and tool execution, affecting the core loop timing
- **Error propagation:** Parse failures in approval text → ask_user (safe default). ApprovalBridge failures → existing error handling preserved. Retry history drain failures → escalate to notify
- **State lifecycle risks:** Retry counter is ephemeral (per-gate-invocation), no persistence needed. PendingConfirmation state is already managed by AsyncConfirmationHandler. No new persistent state introduced
- **API surface parity:** All channels (Telegram, Discord, Webchat, CLI) receive approval requests through the same approval_bridge — no per-channel changes needed
- **Integration coverage:** The critical cross-layer path is: LLM text output → parser → gate → approval manager → bridge → channel → user decision → back to gate → tool execution. This end-to-end path needs integration testing
- **Unchanged invariants:** ToolPipeline 7-stage pipeline is not modified. NativeToolCall structure unchanged. Protocol adapters untouched. TrustStage upgrade logic untouched. approval_bridge forwarding logic untouched

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| LLM does not reliably output `<exec-approval>` tags | R5 fallback to ask_user ensures safety; prompt engineering iteration during implementation |
| Circular trust: LLM approves its own dangerous calls | R4 always_confirm hard floor for high-risk tools; accepted risk documented in origin |
| StreamingToolBridge starts tools before approval gate | Gate intercepts after bridge completes, before results commit — bridge collects outcomes but gate controls whether they proceed |
| Migration breaks existing approval flows | Unit 6 preserves PendingConfirmationStore and approval_bridge; wire new before removing old |
| Retry loop degenerates (LLM repeats same blocked call) | R8 history cleanup + system message; max 2 retries then escalate |
| Prompt token overhead from TrustStage list | Aggregate format is compact; measure post-implementation; defer optimization |

## Sources & References

- **Origin document:** [docs/brainstorms/2026-04-04-two-phase-exec-approval-requirements.md](docs/brainstorms/2026-04-04-two-phase-exec-approval-requirements.md)
- Related code: `src/agent_loop/loop_core.rs`, `src/dispatcher/confirmation.rs`, `src/providers/adapter.rs`
- Architecture: `docs/reference/TOOL_SYSTEM.md`, `docs/reference/AGENT_SYSTEM.md`
