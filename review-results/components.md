# Module: src/components

- Path: `src/components/`
- Files scanned: 20
- Total LOC: 1905
- Confidence threshold: 80 (all reported findings considered actionable)

## Summary
| Severity | Count |
|----------|------:|
| critical | 0     |
| high     | 5     |
| medium   | 9     |
| low      | 5     |
| **Total**| **19**|

## High-Confidence Issues

### Perspective 1 — Security & Robustness

```
ISSUE|src/components/types/parts/streaming.rs:41-44|medium|StreamingTextPart::append has no size cap on `content`; a misbehaving tool or runaway stream can grow the buffer unboundedly (DoS / OOM).
ISSUE|src/components/types/context.rs:349-414|medium|ExecutionContext::to_full_prompt serializes the full acquired_knowledge and decision_trail into the LLM prompt with no truncation; long-running sessions accumulate unbounded text (prompt-explosion DoS / cost amplification).
ISSUE|src/components/types/part_id.rs:114-117|medium|PartUpdateData::added silently swallows JSON serialization failure by emitting an empty `part_json` string; UI consumers receive a corrupt payload with no error path, leading to silent misrender.
ISSUE|src/components/types/part_id.rs:131-134|medium|PartUpdateData::updated silently swallows JSON serialization failure by emitting an empty `part_json` string; same hidden corruption path as `added`.
ISSUE|src/components/types/part_id.rs:25-58|low|part_id() uses DefaultHasher (SipHash, non-cryptographic) combined with coarse-grained `i64` timestamps; rapid same-second events with identical content can collide, and the trait exposes the hash format as part of the public API surface.
```

### Perspective 2 — Logic & Correctness

```
ISSUE|src/components/types/session.rs:14-49|high|ExecutionSession declares fields (last_compaction_index, needs_compaction, original_request, context, started_at, recent_calls, iteration_count, total_tokens) but the impl block only exposes `new`, `with_model`, `with_original_request`, `with_context`; the other fields are written-by-default and never read or mutated anywhere — half-implemented session model with hidden dead state.
ISSUE|src/components/types/session.rs:111-123|high|Decision (CallTool/Stop/AskUser) and Complexity (Simple/NeedsPlan) enums are `pub` and exported from `types/mod.rs` but have zero consumers outside the module — dead types that imply an orchestration contract the rest of the system does not honor.
ISSUE|src/components/types/context.rs:158-166|medium|GoalStatus is a 5-variant state machine but `ExecutionContext::set_goal` accepts arbitrary Goal values with no transition validation; any state can transition to any other state (broken invariant).
ISSUE|src/components/types/context.rs:223-237|medium|ExecutionPhase is a 5-state machine but `ExecutionContext::set_phase` accepts arbitrary transitions (e.g., Summarizing→Understanding); the documented phase order is enforced nowhere.
ISSUE|src/components/types/part_id.rs:22-60|medium|PartId trait + impl for SessionPart has zero external consumers; the trait is a dead seam that adds public API surface for no caller.
ISSUE|src/components/types/session.rs:135-149|medium|ComponentContext::new discards any caller notion of a stable session_id and silently generates a new UUID each call; callers cannot reuse an existing identifier and any future caller passing one would get a silent mismatch.
```

### Perspective 3 — Architecture Compliance

```
ISSUE|src/components/types/session.rs:126-149|high|ComponentContext is a runtime context carrier (Arc<RwLock<ExecutionSession>>, AtomicBool, EventBus, ToolCatalog) sitting in a `types` module that the mod.rs comment describes as "shared domain types"; this is business orchestration infrastructure, not a passive data type (R4: interface-layer adjacent code carrying business state).
ISSUE|src/components/types/context.rs:340-463|high|ExecutionContext::to_prompt / to_full_prompt / to_incremental_prompt / to_minimal_prompt hardcode LLM-prompt formatting (`**Bold**`, `**Goal**:`, `Decision History` section) inside a data-type module; presentation/prompt logic belongs in a harness/prompt layer, not a types module (R10: intelligence should live in the prompt via thin harness).
ISSUE|src/components/mod.rs:1-7|medium|The mod-level comment claims "shared domain types remain; they are consumed by agents/rig (Knowledge) and the event system (part types)"; in fact only `Knowledge` and `PartUpdateData` have external consumers — the comment is misleading and hides a large dead-code problem.
ISSUE|src/components/types/session.rs:14-100|medium|ExecutionSession carries `agent_id` ("main"), `model` ("default"), and `status: SessionStatus` as runtime configuration embedded in a `types` module rather than exposed as a tool (R9: all configurability exposed as tools).
```

### Perspective 4 — Code Quality

```
ISSUE|src/components/types/session.rs:127|low|`session` field uses fully-qualified `Arc<crate::sync_primitives::AsyncRwLock<ExecutionSession>>` even though `Arc` and `AsyncRwLock as RwLock` are imported on line 4; inconsistent alias usage vs. the constructor signature on line 136.
ISSUE|src/components/types/context.rs:349,417|low|`to_full_prompt` and `to_incremental_prompt` are private while sibling `to_minimal_prompt` (line 445) and `to_prompt` (line 340) are `pub`; inconsistent visibility for parallel methods.
ISSUE|src/components/types/mod.rs:14-53|low|Re-exports 23+ `pub` items that have zero external consumers (Complexity, Decision, ComponentContext, ToolCallRecord, ExecutionSession, ReminderType, SessionStatus, SystemReminderPart, ContextVerbosity, DecisionRecord, Entity, ExecutionContext, ExecutionPhase, Goal, GoalStatus, UserIntent, AiResponsePart, ReasoningPart, UserInputPart, SubAgentPart, PlanPart, PlanStep, StepStatus, SummaryPart, CompactionMarker, FileSnapshot, FileChange, FileChangeType, PatchPart, SnapshotPart, StepStartPart, StepFinishPart, StepFinishReason, StepTokenUsage, StreamingTextPart, ToolCallPart, ToolCallStatus, PartId); the module-level comment misrepresents their liveness.
ISSUE|src/components/types/tests/mod.rs:3|low|`use crate::components::types::*;` from a child module should be `use super::*;`.
ISSUE|src/components/types/context.rs:158-166|low|`GoalStatus::Failed(String)` payload variant has no constructor or accessor; the only way to construct it is via raw enum literal, which the tests never exercise.
```

## Notes

- Only two `pub` items from this module have external consumers in the production tree: `Knowledge` (consumed by `src/agents/rig/types.rs:183,241,412`) and `PartUpdateData` (consumed by `src/event/types.rs:125-127`).
- The `PartEventType` referenced in `src/event/types.rs:582-620` is reached only from a test inside `event/types.rs`; no production code path consumes it.
- The module is otherwise a graveyard of half-implemented legacy event-handler state machines; the only types with real downstream contracts are `Knowledge` and `PartUpdateData`.
- No platform-API calls (R1 ✓), no regex for intent (R8 ✓), no heavy core deps (R3 ✓).