# Review Report — Batch 1 (Cluster registry: NodeRegistry + resolve + tagging)

**Scope:** `src/cluster/registry.rs` (964 LOC)
**Date:** 2026-08-11
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-cluster` (branch `review/cluster`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 3 |
| Medium   | 2 |
| Low      | 3 |

The three High findings cluster around a single theme: **the reconnect and
re-registration paths leak session / connection resources**. The fix set in this
batch closes two of them by reusing the existing `close_connection()` signal
infrastructure; the third is a defense-in-depth hardening against malformed
connect frames.

---

## Findings

### [HIGH] src/cluster/registry.rs:155 — Re-registering the same `node_id` silently overwrites the previous session's `conn_id` without closing that connection

**Category:** Logic / Lifecycle
**Confidence:** High

**Description:**
`NodeRegistry::register` overwrites any previous `NodeSession` under the same
`node_id` and removes the old `nodes_by_conn[prev_conn_id]` mapping — but it
**never signals the previous connection to close**. The dropped session's
`ReverseRpcChannel` clone is still inside the connection task, which is still
listening on its inbound side and still running whatever the center dispatched
before. Worse: if the new connection is the same socket re-dialling (a future
optimization), the writer task in the connection is still alive and `channel`
clones elsewhere in the program can still `call()` a session the registry no
longer knows about.

Reconnect is rare enough that no existing test catches it, but the symmetric
guard is already in place (`forget` calls `close_connection()` on eviction)
— the same signal just needs to fire on overwrite.

**Fix:**
```rust
if let Some(prev) = inner.nodes_by_id.get(&node_id).cloned() {
    inner.nodes_by_conn.remove(&prev.conn_id);
    // Eviction semantics: drop the write lock before notifying, since the
    // notified connection task re-enters the registry via `deregister`.
    drop(inner);
    prev.channel.close_connection();
    inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
}
```
Re-acquire the write lock around the rest of the body. This guarantees the old
session is signalled to close before its state is overwritten.

---

### [HIGH] src/cluster/registry.rs:434 — Malformed `commands` / `tags` / `version` fields silently downgrade to empty / None, and a node with no declared commands still occupies a fleet slot

**Category:** Security / Logic
**Confidence:** High

**Description:**
`maybe_register_node` parses each field with `serde_json::from_value(...).ok().unwrap_or_default()`.
A node presenting a malformed-but-otherwise-shape-conforming connect frame
(`role == "node"` + a `device_name` + a non-array `commands`) **still gets
registered**: `declared_commands` becomes `vec![]`, the slot occupies a registry
entry, and any later `node_invoke` from the model will hit `node_invoke`'s
fail-fast declared-commands check and return "command not permitted" for every
tool. The node is registered, addressable, online in `environments.list`, but
cannot run anything. From the operator's point of view this is a confusing
"online but broken" state. From a DoS angle, a peer can hold arbitrary slots by
repeatedly connecting with broken frames.

This is also a **log-signal gap**: no `warn!` line surfaces the malformed frame,
so an operator chasing "this node is online but doesn't respond" has no breadcrumb.

**Fix:**
- `tracing::warn!` on each parse failure (commands / tags / version), including
  the `device_name` and the offending JSON fragment.
- A `role == "node"` connect with no parseable `commands` is genuinely
  anomalous (every node-side runtime ships `bash`); downgrade to
  `maybe_register_node == false` in that case and log.

---

### [HIGH] src/cluster/registry.rs:150 — Two distinct `node_id`s registering on the same `conn_id` leave the first session orphaned (data inconsistency)

**Category:** Logic
**Confidence:** Medium-High

**Description:**
Re-register path:

```rust
if let Some(prev) = inner.nodes_by_id.get(&node_id) {
    let prev_conn = prev.conn_id.clone();
    inner.nodes_by_conn.remove(&prev_conn);
}
inner.nodes_by_conn.insert(conn_id, node_id.clone());
inner.nodes_by_id.insert(node_id, session);
```

This only cleans up a previous session whose `node_id` matches the new one.
If two different `node_id`s collide on the same `conn_id` (today this only
happens in tests where the conn id is fabricated, but future refactors can
create the same shape with a real WS reuse path), the older session sits in
`nodes_by_id` forever: `deregister(conn_id)` removes only the **second**
session's mapping (the later `nodes_by_conn[conn_id] = Y` overwrote the first).

The currently-in-scope call site (handler.rs `maybe_register_node` with a
fresh `conn_id`) cannot hit this, but the registry's invariant
"every `conn_id` in `nodes_by_conn` points to a present session in
`nodes_by_id`" must be self-enforced.

**Fix:** Add a `nodes_by_conn.remove(conn_id)` lookup at the top of `register`
and, if it returns a different `node_id`, drop that session too (treating the
collision the same way `forget` does — signal close + drop).

---

### [MEDIUM] src/cluster/registry.rs:155 — Reconnect path does not log, making "node came back online" invisible to operators

**Category:** Quality / Observability
**Confidence:** High

**Description:** `register` and `deregister` and `forget` are all silent
operators. Operators chasing fleet topology issues ("why did `node-X` reappear
with a fresh socket?") have no breadcrumb. The version-skew warning already in
place proves the codebase's preference for surfacing these events.

**Fix:** `tracing::debug!` on register/deregister/forget with the relevant ids.

---

### [MEDIUM] src/cluster/registry.rs:297 — `resolve_all_by_tags` returns HashMap iteration order, so a fan-out's `JoinSet::spawn` order differs between calls

**Category:** Logic / Determinism
**Confidence:** Medium

**Description:**
`node_invoke_many` already sorts the **results** by `(node, node_id)` (line 137
of `node_invoke_many.rs`), so the JSON the model sees is stable. But the
**spawn order** still varies, which affects: (a) test reproducibility, (b) any
upstream consumer that observes JoinSet completion order for its own ordering,
(c) a future "best-effort early abort" optimization that wants to know the
spawn order to short-circuit. Sorting at the registry boundary is cheap and
keeps the property local.

**Fix:** sort `Vec<NodeMatch>` by `node_id` before returning.

---

### [LOW] src/cluster/registry.rs:106 — `NodeMatch` has no `Debug` impl, so test failures cannot pretty-print it

**Category:** Quality
**Confidence:** High

**Fix:** `#[derive(Debug)]` on `NodeMatch`. Cheap.

---

### [LOW] src/cluster/registry.rs:62 — `ResolveError` has `Display` but no test coverage

**Category:** Quality
**Confidence:** High

**Fix:** one-line test covering each variant's `Display` output.

---

### [LOW] src/cluster/registry.rs:455 — `maybe_register_node` falls back to `"unknown"` when `device_name` is absent without logging

**Category:** Quality
**Confidence:** Medium

**Description:** every shipped node runtime sends `device_name`; a missing
field is suspicious enough to log at `warn` once per connect.

**Fix:** `tracing::warn!` on missing `device_name`, including the presented
`device_id`.

---

## Files reviewed (cross-referenced, not in findings scope)

- `src/cluster/reverse_rpc.rs` — `ReverseRpcChannel::close_connection` is the
  signal primitive that Batch 1 fixes depend on. Reviewed in Batch 3.
- `src/builtin_tools/node_invoke_many.rs` — `resolve_all_by_tags` consumer.
  Reviewed in cross-reference. Already sorts results.
- `src/gateway/handlers/cluster.rs` — `environments.list` aggregator.
  Reviewed in cross-reference. Merges online registry + offline `security_store`
  rows; offline rows already come with `status: "offline"`.

## Clean areas

- `truncate_on_char_boundary` correctness (delegates to `utils::text_format::truncate_bytes`,
  which has property tests for UTF-8 safety).
- Lock-poisoning recovery (`unwrap_or_else(|e| e.into_inner())`) is consistent.
- `forget` already drops the write lock before signalling — no deadlock on
  re-entry via `deregister`.
- `normalize_node_key` is Unicode-aware and tested against CJK names.
- `register_then_list_projects_environment`, `deregister_removes_from_both_maps`,
  `reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new` cover the
  happy path and the basic reconnect invariant.