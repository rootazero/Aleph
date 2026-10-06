# Task 3 — Approval decision memo

## Implemented

`record_approval_decision` now preserves the existing `ToolCallApproved` / `ToolCallDenied` emission and appends an `ApprovalMemo` through `emit_for_ambient_call`.

## Call identity mapping

- Ambient `call_id` is resolved by `emit_for_ambient_call` from the current `CallIdentity`.
- The memo stores that same value as `SessionEvent::ApprovalMemo.request_id`.
- The memo text includes the existing decision detail, optional gate rule, and approval fingerprint.
- The memo timestamp is generated at append time with `now_ms()`.
- If no ambient identity exists, `emit_for_ambient_call` skips the memo and logs the existing warning path.

## Delivery semantics

The approval event and memo are two sequential normal appends, not one transactional batch. This is intentionally at-least-once/ best-effort and does not claim exactly-once delivery. Cross-restart deduplication is not implemented.

## Scope

Only `src/tools/scoped/dispatch.rs` and this report were changed. No event enum, store API, hooks, or `effect_claim` changes were made.
