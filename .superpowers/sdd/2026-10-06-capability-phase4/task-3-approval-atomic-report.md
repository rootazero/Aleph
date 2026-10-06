# Task 3 — Approval atomic persistence

## Change

- Added `emit_approval_decision_batch` to `src/session/call_log.rs`.
- The helper resolves the global session service and ambient call identity once, emits `ToolCallApproved`/`ToolCallDenied` plus `ApprovalMemo` via one `emit_batch`, and logs failures without fallback.
- Updated `record_approval_decision` in `src/tools/scoped/dispatch.rs` to preserve ledger and approval semantics while using the atomic helper.

## Verification

- `cargo check -p alephcore`: passed.
- `git diff --cached --check`: run before commit.
- Commit: `fix(capability): atomically persist approval memos`
