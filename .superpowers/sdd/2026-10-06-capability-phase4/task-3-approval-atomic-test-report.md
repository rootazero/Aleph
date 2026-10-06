# Approval memo atomicity guard

Added `session::in_process::tests::approval_memo_batch_is_one_contiguous_commit`.

The test uses the real `InProcessActorSessionService::emit_batch` path with a `ToolCallApproved` event followed by `ApprovalMemo`. It verifies that the returned event sequences are contiguous, the durable log preserves decision-before-memo order, and the relevant event fields round-trip. This pins the atomic batch boundary used by `emit_approval_decision_batch` without introducing a second store or changing production behavior.

Verification:

- `cargo test -p alephcore --lib session::in_process::tests::approval_memo_batch_is_one_contiguous_commit --no-fail-fast`: 1 passed.
- `cargo check -p alephcore`: passed.
- `git diff --check`: passed.
