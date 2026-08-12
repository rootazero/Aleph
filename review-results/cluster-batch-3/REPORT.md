# Review Report — Batch 3 (Reverse RPC: PendingInvokes + ReverseRpcChannel)

**Scope:** `src/cluster/reverse_rpc.rs` (493 LOC)
**Date:** 2026-08-11
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-cluster` (branch `review/cluster`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 2 |
| Medium   | 2 |
| Low      | 2 |

Both High findings are well-bounded, fail-fast correctness issues. Both have
excellent existing test coverage that catches the intended scenarios; the
fixes here harden edge cases the existing tests do not exercise.

---

## Findings

### [HIGH] src/cluster/reverse_rpc.rs:283 — `call()` may deliver a frame to the peer and then return `Timeout`; the peer gets an orphan request with no id-routing path

**Category:** Logic / Race
**Confidence:** High

**Description:**
The current implementation:

```rust
match tokio::time::timeout_at(deadline, self.outbound.send(frame)).await {
    Ok(Err(_)) => { ... TransportClosed }
    Err(_) => { ... OutboundWedged(timeout_ms); close_connection() }
    Ok(Ok(())) => {}
}
match tokio::time::timeout_at(deadline, rx).await {
    Ok(Ok(resp)) => Ok(resp),
    Ok(Err(_)) => Err(ReverseRpcError::Cancelled),
    Err(_) => {
        self.pending.cancel(&id);
        Err(ReverseRpcError::Timeout(timeout_ms))
    }
}
```

The second `timeout_at(deadline, rx)` reuses the SAME `deadline` as the first
send. If the outbound push consumed, say, 80% of the budget and the response
arrives just past the deadline, the function returns `Timeout`. The peer
**has already received and parsed the request frame** and will eventually send
back a response carrying the same `id`. The response is then routed by the
inbound loop via `PendingInvokes::resolve(&id, response)` → no waiter exists
(cancel removed it) → `resolve` returns `false` and the inbound loop logs a
stray response (or silently drops it).

The peer then ran the command but the center told its caller "timed out" —
a half-executed command. Today this is logged as a "stray reverse-RPC response"
in the inbound loop's debug log, but the caller has no way to know the
command actually ran on the peer.

**Fix:** give the response-wait its own sub-budget rather than sharing with
send. Concretely: split the budget into `enqueue_budget = min(timeout_ms, SOCKET_PUSH_BUDGET_MS)`
(default 500ms) and `response_budget = timeout_ms - enqueue_budget`. If the
push was fast, give the response the full remaining budget; if the push
consumed most of the budget, the response window is small but the OUTCOME is
honest (the peer received the frame and either responded in time or did not).

Alternative cheaper fix: when the second `timeout_at` fires, ALSO fire
`close_connection()` if the channel has a close signal (same as wedge) —
the half-executed command is a stronger signal of trouble than a slow node.

---

### [HIGH] src/cluster/reverse_rpc.rs:96 — `PendingInvokes::resolve()` returns `true` for an already-resolved id (dropped receiver), masking double-resolve

**Category:** Logic / Correctness
**Confidence:** High

**Description:**
```rust
pub fn resolve(&self, id: &Value, response: JsonRpcResponse) -> bool {
    let Some(key) = id.as_str() else { return false };
    let sender = self.waiters.lock()...remove(key);
    match sender {
        Some(tx) => { let _ = tx.send(response); true }
        None => false,
    }
}
```

The docstring says "returns `true` if an entry existed for this id (even if
its receiver was already dropped, e.g. the caller timed out — still counts
as a handled reverse RPC response)". This is intentional: a known id with a
dropped receiver is "handled" so the inbound loop does not warn. But this
also masks a real bug class: if the SAME id is resolved twice (a buggy peer
that re-sends a response, or a future refactor that loses the resolve-id
uniqueness), the second `resolve` returns `false` → the inbound loop warns
"unknown reverse-RPC id" → operator sees a stream of warnings.

A safer contract: `resolve` returns `Ok(())` for "handled" (as today), but a
new `PendingInvokes::known_id(id)` lets the inbound loop distinguish "we
remembered this id and the receiver was dropped" from "we have never heard
of this id" → log differently.

**Fix:** introduce `PendingInvokes::remembered(&self, id: &str) -> bool`
that returns `true` iff the id has been registered at any point. The
inbound loop can then log at `debug!` for `remembered=true, resolve=false`
("response for an already-handled id — peer or upstream bug") and `warn!`
only for `remembered=false` ("unknown id"). Cheap, does not change
`resolve`'s semantics.

---

### [MEDIUM] src/cluster/reverse_rpc.rs:285 — `serde_json::to_string(&req)?` happens BEFORE `pending.register()`, so a serialization failure never leaks a registered waiter

**Category:** Quality / Defensive
**Confidence:** High

**Description:**
Currently `register()` is called first, then `serde_json::to_string(&req)?`
happens. A `serde_json::Error` is mapped to `ReverseRpcError::Serialize`,
but the registered waiter is never cancelled. It sits in `waiters` until
the per-id timeout fires (or until the next call to `cancel_all`). Tiny
window, but trivial to close:

```rust
let frame = match serde_json::to_string(&req) {
    Ok(f) => f,
    Err(e) => {
        self.pending.cancel(&id);
        return Err(ReverseRpcError::Serialize(e));
    }
};
```

**Fix:** add the cancel on the error path (one line).

---

### [MEDIUM] src/cluster/reverse_rpc.rs:25 — `PendingInvokes::counter` starts at 0 forever; once it wraps past `u64::MAX` it can collide with prior ids

**Category:** Logic / Theoretical
**Confidence:** Low (realistically never)

**Description:** `AtomicU64` wraps after `u64::MAX - 1` increments. At one
register per millisecond, that is ~584 million years. Document the assumption
and move on; no fix.

---

### [LOW] src/cluster/reverse_rpc.rs:248 — `call()`'s `deadline = now + budget` is computed BEFORE `register()`, so the timeout already counts register cost

**Category:** Quality
**Confidence:** High

**Description:** cosmetic; the `register` call is microseconds. No fix needed;
flagged so the budget accounting is auditable.

---

### [LOW] src/cluster/reverse_rpc.rs:185 — `with_close` does not document that `Arc::Notify` permits are NOT stacked — `notify_one()` after a `notified().await` resolves the next call instantly, which is the desired "no lost wakeup" behavior

**Category:** Documentation
**Confidence:** High

**Description:** already covered by the test `wedged_outbound_notifies_close_signal_on_with_close_channel`
("notify_one() before the waiter still leaves a stored permit"). Expand the
docstring to reference this guarantee.

---

## Files reviewed (cross-referenced, not in findings scope)

- `src/cluster/registry.rs` — `forget` calls `channel.close_connection()`.
  Reviewed in Batch 1; this file's wedge-handling is what makes it work.
- `src/cluster/node_approval.rs` — calls `channel.call("node.approval.request", ...)`,
  depends on `Cancelled` / `Timeout` / `OutboundWedged` semantics.
- `src/gateway/server/handler.rs` — the inbound loop that calls
  `pending.resolve(...)` on every inbound frame.

## Clean areas

- `OutboundWedged` vs `Timeout` distinction is correctly typed.
- `cancel_all` is correctly implemented (drains + drops senders so all
  waiters resolve at once).
- The `close_connection` permit storage semantics (`notify_one` stores a
  permit when nobody is waiting) is correctly relied upon.
- `with_close` vs `new` separation between node-side and center-side
  channels is exactly the right boundary.
- Existing tests cover the happy path, timeout, cancel-all, wedge, slow
  response, transport-closed, and the close-signal-fires-on-wedge cases.