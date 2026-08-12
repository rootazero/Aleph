# Review Report — Batch 2 (Node lifecycle: enrollment / admit_node / deregister)

**Scope:** `src/cluster/enrollment.rs` (612 LOC)
**Date:** 2026-08-11
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-cluster` (branch `review/cluster`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 0 |
| Medium   | 3 |
| Low      | 3 |

The originally-suspected High finding (B2-01: `Uuid::new_v4()` panic) was
withdrawn after cross-checking the dependency tree — `uuid 1.x` documents
`pub fn new_v4() -> Uuid` (infallible), confirmed against
`/home/zou/.cargo/registry/.../uuid-1.22.0/src/v4.rs:pub fn new_v4() -> Uuid`.
No panic risk at the call site.

All other findings are defense-in-depth.

---

## Findings

*(B2-01 originally proposed under "HIGH": `mint_node_device` panics if
`Uuid::new_v4()` fails. Withdrawn: `uuid 1.x` exposes
`pub fn new_v4() -> Uuid`, an infallible constructor. Confirmed against the
crate source in the local registry.)*

---

### [MEDIUM] src/cluster/enrollment.rs:204 — `mint_node_device` fingerprint is `&device_id[..16]`, which assumes ASCII-UUID

**Category:** Logic / Correctness
**Confidence:** High

**Description:**
`mint_node_device` uses `&device_id[..16]` for the UNIQUE fingerprint. The
device_id at this site is always `Uuid::new_v4().to_string()` (36 ASCII chars),
so this is safe **at this site**. But the function is a private helper and a
future caller might pass a non-ASCII id. `admit_node`'s path 4 (unknown id)
already uses `truncate_on_char_boundary(id, 16)` to defend against exactly
this; mint should too, so the two writers of `role=node` device rows use the
same fingerprint contract.

**Fix:** swap `&device_id[..16]` for `truncate_on_char_boundary(&device_id, 16)`.

---

### [MEDIUM] src/cluster/enrollment.rs:189 — `mint_node_device` mint failure produces a confusing error string that includes a `format!` of the result

**Category:** Quality / Error reporting
**Confidence:** High

**Description:**
`map_err(|e| format!("failed to register node device: {e}"))` — fine, but the
error then bubbles up through `enroll_node_device` and `admit_node`'s path 5
unchanged. Operator sees the same string for every failure mode; the log
context (`node_name`, the `tracing::warn!` call) is the only way to tell
which path failed. Either: (a) attach path context to the error, or (b) move
the `tracing::warn!` into `mint_node_device` so the log has the same line
number as the error.

**Fix:** add `tracing::warn!` in `mint_node_device` itself, including the
`node_name`.

---

### [MEDIUM] src/cluster/enrollment.rs:283 — `deregister_node` only `warn!`s on `revoke_device` failure, never retries

**Category:** Logic / Reliability
**Confidence:** Medium

**Description:**
`store.revoke_device(&node_id).unwrap_or_else(|e| { warn!(...); false })` —
the deregister has already evicted the live session via `forget`, but the
device record is what **makes the deregistration sticky** (the next
`admit_node` sees `revoked_at`). If the revoke fails, the node is gone from
the fleet view but can self-revive on its next reconnect. The function
already returns `DeregisterOutcome { device_removed: false, .. }` so the
caller can detect this; but no caller in scope (`cluster.deregister` RPC) acts
on it.

**Fix:** add a one-line operator-visible log in the RPC handler when
`device_removed == false`. (Touches `src/gateway/handlers/cluster.rs` —
out-of-scope for this batch but cross-reference noted.)

---

### [LOW] src/cluster/enrollment.rs:174 — `admit_node` path 4's `truncate_on_char_boundary(id, 16)` produces a fingerprint that may collide for short non-ASCII ids

**Category:** Logic / Edge case
**Confidence:** Medium

**Description:** if two nodes present two distinct ids that share their first
16 bytes (`truncate_on_char_boundary` rounds back to a char boundary, so the
result may be < 16 bytes), the UNIQUE constraint on `fingerprint` will reject
the second upsert with an error that is then `warn!`'d — but the node is
still admitted (the admit path does not abort on upsert failure). Net effect:
the second node has no device row, but the operator-visible fleet view
silently drops one of the two.

**Fix:** fingerprint the **full** id (sha256(id) or hex(uuid-v5 of id)), so
collisions cannot happen even for tiny inputs.

---

### [LOW] src/cluster/enrollment.rs:316 — `deregister_node`'s ambiguity branch loses the structured `ResolveError::Ambiguous` payload

**Category:** Logic / UX
**Confidence:** High

**Description:** `Err(e @ (ResolveError::Ambiguous(_) | ResolveError::NodeNotFound { .. })) => return Err(DeregisterError::Ambiguous(e.to_string()))` — the stringified
payload goes to the operator, which is fine, but the structured candidate
list (`Vec<String>`) is lost. A future LLM-tool wrapper that wants to
suggest the operator a disambiguating choice needs the list, not a string.

**Fix:** change `DeregisterError::Ambiguous` to carry the structured
information: `Ambiguous { candidates: Vec<String>, name_or_id: String }` (or
two separate variants).

---

### [LOW] src/cluster/enrollment.rs:1 — Module doc says "R10: does not enter src/harness/" but this file IS called from `src/gateway/server/handler.rs::admit_node`

**Category:** Documentation
**Confidence:** High

**Description:** `cluster/mod.rs`'s header says the cluster module does not
enter `src/harness/`, which is the R10 contract — but this file's doc
re-iterates the same claim, which is correct. No fix needed; the lint is that
the same one-line R10 caveat is duplicated in two module headers. Cosmetic.

---

## Files reviewed (cross-referenced, not in findings scope)

- `src/cluster/registry.rs` — `NodeRegistry::resolve_id` and `NodeRegistry::forget`
  are the two primitives `deregister_node` builds on. Reviewed in Batch 1.
- `src/gateway/security/store.rs` — `SecurityStore::upsert_device`,
  `revoke_device`, `list_devices`, `get_device`. Read-only cross-reference.

## Clean areas

- `enroll_node_device`'s idempotence contract is well-tested
  (`enroll_is_idempotent_across_name_spellings`).
- `admit_node`'s Decision Order is documented and matches the comments.
- `deregister_is_sticky_and_reaches_offline_nodes` covers the offline-fallback
  path completely.
- The `IdentityConflict` variant is the right design choice — folding it into
  `Deregistered` would have hidden the security-critical reason for refusal.
- `mint_node_device`'s fingerprint is ASCII-safe at its current call site.