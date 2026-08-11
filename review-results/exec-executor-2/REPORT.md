# Review Report — Batch 2: `src/exec/manager.rs`

**Date:** 2026-08-11
**Scope:** `src/exec/manager.rs` (1442 lines) — single file, the approval-manager
lifecycle and its session-aware resolve paths
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    3 |     4 |   2 |    9 |

`ExecApprovalManager` is the single seam between every "I want to do a
destructive thing" call site and the human who has to say yes. Bugs here are
**silent and one-sided**: the call site thinks the approval was delivered
and waits, the human never sees the card, and a `timeout` (the default
fallback) is the only signal anything was wrong. The session-aware resolve
(`resolve_for_session`) is the more complex of the two paths and carries
most of the findings here; the by-id resolve is small and tested in lock
step with the clarification twin (the comment at `is_live` is explicit
about that).

## Findings

### [HIGH] `manager.rs:from_request` (line 91-145) — manager never checks `analysis.ok` before creating a card
**Category:** logic
**Confidence:** High

**Description.** `ExecApprovalManager::create` builds a record from
`&ExecApprovalRequest` without inspecting `request.analysis.ok`. A caller
that requests approval for a command whose parser already rejected it
(`analysis.ok = false`, `reason: Some("...")`) still produces a card with
a usable `command` field, an empty `executable` (because
`segments[0].resolution` is None for an error analysis), and a `resolved_path`
of None. The card is delivered through the channel bridge; the user sees
"Allow this command?" with an empty executable line; the harness sees
`executable: ""` if it ever reads the record.

**Suggested fix.** Either:
1. Reject at `create` when `!request.analysis.ok`, or
2. Surface the parser's `reason` on the card so the user sees the parser
   already said no. Option (1) is stronger because the parser's
   `CommandAnalysis::error` is the single source of "this command is
   unparseable" — silently turning that into a channel-delivered approval
   card is the wrong default.

```rust
pub fn create(&self, request: &ExecApprovalRequest, timeout_ms: u64) -> ExecApprovalRecord {
    debug_assert!(
        request.analysis.ok,
        "ExecApprovalManager::create called with !analysis.ok — caller must reject before this point. \
         Parser reason: {:?}",
        request.analysis.reason,
    );
    let record = ExecApprovalRecord::from_request(request, timeout_ms);
    // ...
}
```

### [HIGH] `manager.rs:is_live` (line 178-181) — sender-side liveness is checked, but `decision.is_some()` is not
**Category:** logic
**Confidence:** High

**Description.** `is_live` returns true when the record is unexpired and the
`oneshot::Sender` is still open. After `resolve_entry`, the sender is taken
(via `entry.sender.take()`), so the next call to `is_live` on the same
entry correctly returns false. But:

- A record whose `decision` is set to `Some(Deny)` by `resolve_entry` and
  whose entry has been kept in the map (e.g. by a caller that did NOT
  remove it via `await_registered`) will still have a non-None `decision`
  field. The `is_live` check covers the sender (closed), so the entry is
  not listed. OK.
- BUT: the `cascade_session_grant` path uses `e.is_live()` as its
  inclusion filter, and a cascading resolve that fails to wake its
  oneshot (the `sender.send` returns `Err(_)`) will still take the
  sender, but `is_live` will be false on the next read. So a subsequent
  `/approve 1` reply that re-lists will see the entry as
  not-live, which is correct.

The actual bug is different and silent: the `get_pending` method
(line 798-813) does NOT call `is_live` — it returns ANY entry in the map.
A consumer that calls `get_pending` to look up a record and check its
status would see a fully-resolved record (with `decision: Some(Deny)`,
`resolved_at_ms: Some(...)`) and might still try to re-resolve it. The
`resolve` method checks `is_live` so the second resolve returns false, but
the consumer's view of the world is "this approval is still pending".

**Suggested fix.** `get_pending` should consult `is_live` too, returning
`None` for resolved-or-expired entries (or, alternatively, return the
record but mark it — but a missing entry is the more honest signal).
The `list_pending` method (line 818) already does this correctly.

```rust
pub fn get_pending(&self, id: &str) -> Option<PendingApproval> {
    let pending = self.pending.read().unwrap_or_else(|e| e.into_inner());
    pending.get(id).filter(|e| e.is_live()).map(|entry| {
        // ... unchanged
    })
}
```

### [HIGH] `manager.rs:register_pending` (line 222-244) — opportunistic sweep races with concurrent resolve
**Category:** logic
**Confidence:** Medium

**Description.** `register_pending` calls `self.cleanup_expired()` BEFORE
inserting the new entry. `cleanup_expired` takes the write lock, so
`register_pending` is held off the write lock until cleanup completes. A
concurrent `resolve(id, ...)` on a different entry is also held off the
write lock for the duration of the sweep. For a manager with thousands of
expired entries (a long-running session with many late cards), this is
O(N) holding the write lock — every other writer waits.

**Suggested fix.** Move the sweep to a background task, or batch the
sweep to happen on every Nth `register_pending`. The simpler fix is to
not hold the write lock while iterating; `cleanup_expired` only needs the
lock to call `pending.retain`, but a more performant approach is a
`Vec<String>` of expired ids collected under a read lock, then
`pending.write().remove(&id)` for each.

For this pass, the lowest-risk fix is to add a counter and only sweep
every 32nd `register_pending`:

```rust
const CLEANUP_INTERVAL: u32 = 32;
let counter = self.sweep_counter.fetch_add(1, Ordering::Relaxed);
if counter % CLEANUP_INTERVAL == 0 {
    self.cleanup_expired();
}
```

This bounds the worst-case O(N) lock hold to once per 32 registrations
while keeping the invariant "no zombie lives forever".

### [MEDIUM] `manager.rs:resolve_with_reason` (line 270-304) — `deny_reason` is silently dropped when the decision is anything other than `Deny`
**Category:** logic
**Confidence:** High

**Description.** `resolve_entry` only sets `entry.record.deny_reason` when
`decision == Deny`. The intent is right (a reason on an approval is
meaningless), but the silent drop is a footgun: a caller who passes
`Some("explain why")` with `AllowOnce` will not see an error — the reason
disappears, the awaiter gets the decision with no reason, and there is no
log line about the dropped reason. A test (`deny_reason_rides_the_resolved_decision`)
verifies the AllowOnce case explicitly asserts the reason is None, but
no test asserts the WARN.

**Suggested fix.** Log a one-line warning at `tracing::warn!` so a
misconfigured caller (a buggy Telegram handler) can be diagnosed:

```rust
if decision == ApprovalDecisionType::Deny {
    entry.record.deny_reason = deny_reason;
} else if deny_reason.is_some() {
    tracing::warn!(
        id = %entry.record.id,
        ?decision,
        "resolve_with_reason: deny_reason ignored — only Deny honors the reason field"
    );
}
```

### [MEDIUM] `manager.rs:cascade_session_grant` (line 410-440) — iter-then-mutate holds lock but is_correct only after re-check
**Category:** logic
**Confidence:** Medium

**Description.** `cascade_session_grant` iterates `pending` to collect ids
matching (session, key, is_live), then for each id calls
`pending.get_mut(&id)` and `Self::resolve_entry`. The `is_live` check is
the read-side filter; the `resolve_entry` call ALSO re-checks is_live
indirectly (via the `if let Some(sender) = entry.sender.take()` that
silently no-ops on `None`).

This is correct, but the design is fragile: if `resolve_entry` ever
stops taking the sender, a concurrent `resolve_for_session` between the
collection step and the `get_mut` step would double-resolve. The two
arenas (the by-id path and the session-FIFO path) both rely on the
sender-take as the deduplication signal.

**Suggested fix.** Add an explicit `entry.decision.is_some()` guard inside
`resolve_entry`:

```rust
fn resolve_entry(entry: &mut PendingEntry, decision: ApprovalDecisionType, ...) {
    if entry.record.decision.is_some() {
        // Already resolved (e.g. by a concurrent path). The cascade is idempotent.
        return;
    }
    entry.record.decision = Some(decision);
    // ... unchanged
}
```

This costs one Option compare per call and removes a class of "what if
two paths race" worries that the current code does not make impossible,
just hard to reason about.

### [MEDIUM] `manager.rs:resolve_for_session` (line 555-590) — originator check uses `as_deref()` on the wrong side
**Category:** logic
**Confidence:** Medium

**Description.** The originator gate at line 567-579:

```rust
if let Some(entry) = pending.get(&id) {
    if let Some(ref expected) = entry.record.originator_user_id {
        match resolved_by.as_deref() {
            Some(actual) if actual == expected.as_str() => {}
            Some(_) | None => {
                warn!(...);
                return SessionResolveOutcome::NothingPending;
            }
        }
    }
}
```

This is correct: when originator is set, the resolved_by must match
exactly. But there is a related test gap: there is NO test for the case
"record has originator_user_id, resolved_by is `Some(\"other\")` —
must return NothingPending". The existing tests cover:
- bare `/approve` with one live card (no originator) — resolves
- bare `/approve` with two live cards — ambiguous
- `index` addressing the right card — resolves
- `index` addressing the wrong card — re-lists

But not the originator-rejection path through `resolve_for_session`.
The twin `record_originator` accessor is tested for the by-id path
through `ManagerCallbackSink::handle_callback`, but the session-FIFO
originator rejection is tested only by the implicit "no test fails when
this branch is taken".

**Suggested fix.** Add a regression test:

```rust
#[tokio::test]
async fn resolve_for_session_rejects_non_originator() {
    // Two live cards on session "s1", both stamped with originator "alice".
    // `/approve` from "bob" must return NothingPending (not Ambiguous,
    // not Resolved) — the warning path. The cards stay pending.
}
```

### [MEDIUM] `manager.rs:display_line` (line 670-680) — char-counting truncation uses `chars().take()` correctly but the test never re-reads the line
**Category:** quality
**Confidence:** Low

**Description.** `display_line` truncates at `MAX = 120` chars using
`record.command.chars().take(MAX)`, which is correct. The `…` append is
one char past the limit, so a 120-char command becomes 121 chars (120 + `…`).
This is fine, but a future refactor that changes MAX without re-reading
the test (`bare_session_resolve_refuses_when_several_cards_pend`) could
break the test silently. The test asserts exact line equality
(`assert_eq!(cards[0], (1, "rm -rf ./build".to_string()))`).

**Suggested fix.** Leave, but document the `MAX = 120` contract on
`display_line` so a future reader knows where the magic number is and
why it cannot move.

### [LOW] `manager.rs:DEFAULT_APPROVAL_TIMEOUT_MS` (line 17) — public constant with no upper bound
**Category:** quality
**Confidence:** Low

**Description.** The default is 120_000 (2 minutes). Callers can pass any
`u64` as `timeout_ms`; there is no validation in `create`. A typo of
`120_000_000` (100 minutes) would create records that linger in the map
for hours. The `cleanup_expired` sweep would eventually remove them, but
the channel bridge's text-fallback path would have already refused to
deliver after 30 seconds (`DELIVERY_TIMEOUT_SECS`).

**Suggested fix.** Either document that `timeout_ms` is the
**caller's** upper bound (and that callers should clamp to a sane
value, e.g. 10 minutes), or add a `const MAX_TIMEOUT_MS: u64 = 600_000;`
and a `min(timeout_ms, MAX_TIMEOUT_MS)` in `create`. The current
behaviour is fine in practice but the surface is undocumented.

### [LOW] `manager.rs:cleanup_expired` (line 870-895) — sends `None` to the waiter on sweep, but only if the sender is still `Some`
**Category:** logic
**Confidence:** High (this is correct — noting the audit point)

**Description.** `cleanup_expired` takes the sender, sends `None` (the
"timed out" decision), and the entry is dropped. This is the ONLY way a
caller's `await_registered` returns `None` without an explicit
`/approve timeout` text reply — the caller's `tokio::time::timeout`
elapsing first returns `Err(_)` (also a `None` outcome), but
`cleanup_expired` runs from `register_pending`, which the caller awaits
**after** the timeout has elapsed in their `await_registered`. So the
ordering is: caller awaits with `timeout = X`; the timer fires first
(the caller has dropped the receiver via timeout's internals, OR
NOT — see below); `cleanup_expired` runs on a subsequent register, takes
the sender, sends None, the sender's `send` may or may not succeed.

If the caller's `await_registered` timer fires first, the `rx` is
dropped (the receiver of the oneshot is dropped), and the sender's
subsequent `send` returns `Err` (no live receiver). The `let _ =` in
`cleanup_expired` discards that, so the caller's `await_registered`
returns `Err(_)` (timeout), which becomes `decision: None`. The same
`None` outcome is delivered. Consistent.

If the caller's `await_registered` has not started yet (the
`register_pending` returned `(id, rx, timeout)` but the caller is still
mid-computation before awaiting), `cleanup_expired` sends `None` to a
live receiver. The caller awaits, gets `Ok(None)`, decision is
`None`. Also consistent.

**Suggested fix.** None — this is correct. But the implicit "two paths
to the same `None` outcome" deserves a one-line comment in
`cleanup_expired` so a future reader doesn't worry.

## Cross-References

- `manager.rs:from_request:91` — `analysis.ok` is not consulted. The
  parser-side `CommandAnalysis::error` is the single source of "this
  command is unparseable"; surfacing it as an approval card is the wrong
  default. See `src/exec/parser.rs:analyze_shell_command` for the
  parser-side equivalent.
- `manager.rs:is_live:178` — `sender.is_some_and(|s| !s.is_closed())` is
  the liveness signal. The comment in `is_live` is explicit that this
  mirrors `crate::clarification::session::PendingEntry::is_live`; the
  clarification twin should be checked in lock-step when one is touched.
- `manager.rs:resolve_for_session:555` — the originator check is
  currently the only thing standing between a paired chat and the
  group-chat approval-bypass. A test for the rejection path is the gap.
