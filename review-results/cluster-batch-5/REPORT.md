# Review Report — Batch 5 (Node approval requester + module entry)

**Scope:** `src/cluster/node_approval.rs` (211 LOC), `src/cluster/mod.rs` (41 LOC)
**Date:** 2026-08-11
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-cluster` (branch `review/cluster`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 0 |
| Medium   | 2 |
| Low      | 3 |

The node approval requester is small, focused, and tested. No findings rise
to High. Both Medium findings are documentation / signal improvements.

---

## Findings

### [MEDIUM] src/cluster/node_approval.rs:79 — `outcome_from_str` does not distinguish between center-known and unknown outcome strings; an attacker-controlled center response can downgrade an approved outcome to Denied

**Category:** Security / Fail-closed
**Confidence:** Medium-High

**Description:**
```rust
pub(crate) fn outcome_from_str(s: &str) -> ApprovalOutcome {
    match s {
        "approved" => ApprovalOutcome::Approved,
        "approved_session" => ApprovalOutcome::ApprovedForSession,
        "timeout" => ApprovalOutcome::Timeout,
        _ => ApprovalOutcome::Denied,
    }
}
```

The cluster module assumes LAN-trust (no token, the node trusts the center).
This fail-closed behavior is **intentional**: an unknown string from the
center cannot accidentally elevate a privilege. But the existing comment
says "Any unknown value (including `"denied"`) is fail-closed `Denied`." The
"including `denied`" half is misleading — `"denied"` IS the canonical denial
string and is mapped explicitly to `Denied`, the same as the unknown
strings. The comment makes it sound like both branches are unknowns.

This is **a documentation bug, not a behavior bug**. Today, the only producer
of this string is the center, and the center's code path that produces the
outcome string is `src/approval/...::outcome_for_response`. As long as that
producer stays in lock-step with this consumer, the behavior is correct.
But the next time someone adds an `ApprovalOutcome::ApprovedWithConstraints`
or similar, they will update the center's `outcome_for_response` and miss
this `match`.

**Fix:** introduce `pub(crate) enum KnownOutcome { Approved, ApprovedForSession, Timeout }`
and a single source-of-truth mapping (a `match (KnownOutcome::parse(s), fallback)`);
or simply add a `tracing::warn!` when the unknown arm fires so an operator
can spot drift. The latter is one line.

---

### [MEDIUM] src/cluster/node_approval.rs:50 — `tracing::warn!` "node approval requested with no live center channel; denying" fires per escalation, not once per disconnect

**Category:** Quality / Log noise
**Confidence:** High

**Description:** in a headless node that loses its center connection, every
escalation logs the same warning. With the typical "approval per
sensitive bash invocation" cadence, this can produce thousands of identical
lines per minute. Use a `OnceCell<bool>` or similar to log once per
disconnect; or include the disconnect timestamp and dedupe downstream.

Cheaper fix: emit a counter metric (`approval_no_channel_total`) so an
operator can correlate, and down-grade to `debug!` after the first
occurrence.

---

### [LOW] src/cluster/node_approval.rs:91 — `request_approval` sends `action.summary` verbatim without size cap

**Category:** Quality / Defense in depth
**Confidence:** Medium

**Description:** `action.summary` is the redacted, human-readable rendering of
the escalation that the operator sees. The upstream sandbox code produces
it from the tool's args after redaction, so its size is bounded by the
`redaction::MAX_SUMMARY_BYTES` (a constant the producer enforces). Today the
consumer trusts the producer; this is fine. Flagged so the next reviewer
sees the trust boundary.

---

### [LOW] src/cluster/mod.rs:14 — `pub(crate) use node_file_cmd::sha256_hex` is the only `pub(crate)` re-export; tests inside `src/cluster` and `src/cluster/node_file_cmd` are the only consumers

**Category:** Documentation
**Confidence:** High

**Description:** cosmetic; one-line comment about which file is allowed to
import this would help the next person. Low priority.

---

### [LOW] src/cluster/mod.rs:38 — `pub use registry::normalize_node_key` is `pub(crate)`, used by `enrollment.rs::match_by_name`; correct visibility, but the `pub(crate)` keyword is buried inside a long `pub use` list

**Category:** Documentation
**Confidence:** High

**Description:** cosmetic. The visibility is right; the readability could be
improved by splitting `pub use ... { ... }` into two blocks (crate vs
external). One-line refactor.

---

## Files reviewed (cross-referenced, not in findings scope)

- `src/sandbox/exec_approval/gate.rs` — `ApprovalRequester` trait and
  `ApprovalResponse` / `ApprovalOutcome` types. Read-only cross-reference.
- `src/sandbox/exec_approval/mod.rs` — `ApprovalAction::summary` producer.
  Read-only cross-reference.

## Clean areas

- Fail-closed semantics are correctly implemented across all transport
  failure modes (no channel, transport closed, error response, RPC error).
- `NODE_APPROVAL_TIMEOUT_MS = 130_000` is correctly above the center's
  default approval timeout (120s).
- `outcome_from_str` is exhaustive over the center's known outcomes (covered
  by `outcome_mapping_is_fail_closed` test).
- All four transport-failure paths (`none_channel_denies`,
  `json_rpc_error_response_denies`, `transport_closed_denies`,
  `round_trip_maps_center_outcome`) are tested.
- `slot_with_channel` test helper correctly drops the outbound receiver to
  simulate disconnect (`transport_closed_denies`).
- Module-level docstring on `mod.rs` correctly states the cluster's redline
  compliance (no LLM reasoning, no harness).