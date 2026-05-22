# Logic Review Report
**Module**: group_chat
**Scope**: Full module review — 8 files under src/group_chat/ + 2 gateway handler files
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Critical] persist_turn sequence out of sync with message.sequence
- **Location**: `src/group_chat/executor.rs:249-256`
- **Trigger condition**: Any call to `execute_round` that persists turns to the database
- **Expected behavior**: The `sequence` field stored in the database should match the `sequence` field in the returned `GroupChatMessage`
- **Actual behavior**: `persist_turn` is called with `sequence + 1`, but the `GroupChatMessage` is built with the original `sequence` value. When `coordinator_visible=true`, the coordinator message gets `message.sequence=0` but `persist_turn(..., 1, ...)`. The first persona gets `message.sequence=1` but `persist_turn(..., 2, ...)`. This creates a permanent offset between in-memory message ordering and database ordering.
- **Suggested fix**: Change `self.persist_turn(&session.id, round, sequence + 1, &speaker, &persona_response)` to `self.persist_turn(&session.id, round, sequence, &speaker, &persona_response)`. Alternatively, adjust the sequence calculation so the persisted value matches the message value.

### [Warning] Partial round failure leaves session in inconsistent state
- **Location**: `src/group_chat/executor.rs:140-274`
- **Risk**: If `execute_round` fails after adding the System turn (step 1) or after some persona responses have been added, the session retains those turns but the caller receives `Err`. A retry by the caller will re-add the System turn and re-invoke personas, producing duplicate history entries.
- **Current impact**: medium — affects retry behavior and can pollute session history
- **Suggestion**: Consider making `execute_round` atomic with respect to session mutations, or document the non-atomic behavior clearly so callers know they must not retry blindly. One minimal fix: move `session.add_turn(round, Speaker::System, ...)` after the coordinator LLM call succeeds, so coordinator failures do not mutate session state.

### [Warning] Round limit exceeded does not end session in RPC handler
- **Location**: `src/gateway/handlers/group_chat.rs:223-232`
- **Risk**: The `handle_continue_with_targets` RPC handler returns an error when `session.current_round >= max_rounds`, but it does NOT end the session. In contrast, `gateway/inbound_router/group_chat_handler.rs:334-357` ends the session, removes it from tracking, and sends a friendly end message when the limit is reached. This behavioral inconsistency means RPC clients get a hard error while channel clients get a graceful termination.
- **Current impact**: medium — inconsistent UX between RPC and channel interfaces
- **Suggestion**: In `handle_continue_with_targets`, end the session (call `session.end()`) before returning the error response, matching the channel handler behavior. Alternatively, extract a shared helper that enforces the round limit consistently.

### [Warning] Test code imports std::sync::atomic directly
- **Location**: `src/group_chat/executor.rs:290`
- **Risk**: The `SequentialMockProvider` and other test structs import `std::sync::atomic::{AtomicUsize, Ordering}` directly. Aleph's sync primitives rule requires using `crate::sync_primitives` for atomics so that `--features loom` can instrument them. While test code may not run under loom today, violating this rule means future loom coverage for group_chat will silently test the wrong primitive.
- **Current impact**: low — affects future loom test coverage only
- **Suggestion**: Replace `use std::sync::atomic::{AtomicUsize, Ordering};` with `use crate::sync_primitives::{AtomicUsize, Ordering};` in test modules.

### [Warning] Silent provider fallback may confuse users
- **Location**: `src/group_chat/executor.rs:72-87`
- **Risk**: When a persona specifies a `provider` override that is not in the registry, the executor silently falls back to the default provider after logging a warning. The end user has no way of knowing their chosen provider was ignored.
- **Current impact**: low — operational surprise, not a correctness bug
- **Suggestion**: Consider surfacing the fallback in the response metadata or returning an error when a requested provider is missing, so the caller can inform the user.

### [Suggested Test] Sequence consistency between message and database
```rust
#[tokio::test]
async fn test_persist_sequence_matches_message_sequence() {
    let coordinator_response = r#"{"respondents":[{"persona_id":"arch","order":0,"guidance":""}],"need_summary":false}"#;
    let provider = Arc::new(SequentialMockProvider::new(vec![
        coordinator_response.to_string(),
        "Response.".to_string(),
    ]));
    let executor = GroupChatExecutor::new(Arc::new(
        crate::providers::StaticDefault::new(provider as Arc<dyn AiProvider>),
    ))
    .with_coordinator_visible(true);

    let mut session = make_session();
    let messages = executor
        .execute_round(&mut session, "Hello", &[])
        .await
        .unwrap();

    // When coordinator_visible=true:
    // messages[0] = Coordinator, sequence should be 0
    // messages[1] = Persona, sequence should be 1
    assert_eq!(messages[0].sequence, 0);
    assert_eq!(messages[1].sequence, 1);
    // (If a mock database is added, verify db.sequence == message.sequence)
}
```

### [Suggested Test] Retry after partial failure does not duplicate history
```rust
#[tokio::test]
async fn test_retry_after_coordinator_failure_preserves_history() {
    // First call: coordinator fails
    // Second call: succeeds
    // Assert that history contains exactly one System turn, not two
}
```

### [Suggested Test] Round limit behavior consistency
```rust
#[tokio::test]
async fn test_round_limit_ends_session() {
    // Create session with max_rounds=1
    // Execute one round
    // Attempt second round — should end the session, not just return error
}
```

## Summary
| Level | Count |
|-------|-------|
| Critical | 1 |
| Warning | 4 |
| Suggested Test | 3 |

## Cross-Module Findings

### [Warning] Behavioral drift between RPC and channel group_chat handlers
- **Modules**: `gateway/handlers/group_chat.rs` → `gateway/inbound_router/group_chat_handler.rs`
- **Risk**: The two handler layers implement round-limit enforcement differently. The channel handler ends the session gracefully; the RPC handler returns a raw error. Over time this drift will confuse API consumers and make debugging harder.
- **Suggested fix**: Extract a shared `enforce_round_limit` helper into `group_chat` core that both handlers call, ensuring identical behavior.
