# A3 — Cluster node runtime / file commands / reverse RPC / approval: severed-wire audit

- **Repo:** `/home/zou/data/workspace/Aleph` (branch `main`)
- **Date:** 2026-11-20
- **Mode:** READ-ONLY. No source file was edited; no `git add/commit/checkout/stash` was run.
- **Slice (read in full, incl. `#[cfg(test)] mod tests`):**
  `src/cluster/node_runtime.rs` (384 lines), `src/cluster/node_file_cmd.rs` (~635 lines),
  `src/cluster/reverse_rpc.rs` (~770 lines), `src/cluster/node_approval.rs` (~250 lines).
- **Consumer side read:** `src/bin/aleph-server/commands/node.rs`, `src/gateway/server/connection/{mod.rs,cleanup.rs}`,
  `src/gateway/server/handler.rs`, `src/gateway/handlers/cluster.rs`, `src/approval/node_requester.rs`,
  `src/builtin_tools/{node_invoke.rs,node_invoke_many.rs,node_file.rs,node_manage.rs,node_list.rs}`,
  `src/cluster/registry.rs`, `src/sandbox/{workspace/mod.rs,workspace/path.rs,exec_approval/gate.rs,factory.rs}`,
  `src/tools/budget.rs`, `src/session/steer_signal.rs`, `src/exec/manager.rs`,
  `tests/cluster_node_runtime.rs`, `tests/cluster_reverse_rpc.rs`, `tests/cluster_node_approval.rs`.

## Summary

| | count |
|---|---|
| Prior findings re-verified | 8 → **6 FIXED**, **2 PARTIALLY FIXED**, 0 OPEN, 0 REFUTED |
| New candidates | **4** (A3-01 … A3-04) |
| — hypothesis CONNECT / CUT / DECIDE | 0 / 1 / 3 |

The single highest-value new finding (**A3-01**) is the same hazard that prior finding #12 named — *"the peer keeps
executing the command while the center has told its caller 'timed out'"* — surviving through a **second arm** that the
#12 fix did not touch: the **operator-approval park is nested inside the `tool.call` response window**, and the nested
wait's ceiling (130 s on the node, 120 s of decision time on the center) is *longer* than the outer window the center
gives the tool call (120 s by default).

---

# PART A — prior-finding re-verification

| ID | prior title | status | evidence |
|---|---|---|---|
| #4 | jail root never created on the file path | **FIXED** | `src/bin/aleph-server/commands/node.rs:475-492` (`setup_file_commands` → `create_dir_all(jail_root)`), called at `:452` |
| #10 | `CommandTable` allowlist key vs self-reported descriptor name | **FIXED** | `src/cluster/node_runtime.rs:57-80` (key = `cmd.descriptor().name`, `:59`) |
| #11 | file-command jail root vs bash session dir | **FIXED** | `src/cluster/node_runtime.rs:193-201` (one derivation) → `src/sandbox/workspace/path.rs:65` |
| #12 | reverse-RPC response deadline taken at call start | **FIXED** | `src/cluster/reverse_rpc.rs:365` (deadline taken after the enqueue) |
| #16 | containment canonicalizes, kernel re-resolves (intermediate dir swap) | **PARTIALLY FIXED** | `src/cluster/node_file_cmd.rs:76-82` (the code's own NOTE admits the race is *not* closed) |
| #19 | non-regular file (FIFO) inside the workspace blocks dispatch forever | **FIXED** | `src/cluster/node_file_cmd.rs:158-164` (write), `:261` (read) |
| #21 | `OutboundWedged` reports the caller's `timeout_ms` | **FIXED** | `src/cluster/reverse_rpc.rs:334`, `:355` |
| #22 | `descriptors()` sorted with `sort_unstable_by` on a self-reported name | **FIXED** | `src/cluster/node_runtime.rs:100` (total order with a tiebreaker) |

### A3-P04 — the node's file-command jail root is now created on the file path
- Form: 2 (prior: stub far-end)
- Prior status: **FIXED**
- Producer: `src/bin/aleph-server/commands/node.rs:475-492` — `setup_file_commands()` now does
  `tokio::fs::create_dir_all(&jail_root)` where
  `jail_root = alephcore::sandbox::workspace::session_workspace_dir(workspace_root, session)`,
  and only then calls `table.register_file_commands_for_session(workspace_root, session)`.
- Consumer: `src/cluster/node_file_cmd.rs:92` — `resolve_in_jail()`'s `symlink_metadata(workspace_dir)` now finds a
  real directory, so the previous `"workspace root unresolved"` failure is gone.
- Live caller search: `rg -n 'setup_file_commands' src/` → `src/bin/aleph-server/commands/node.rs:452` (production call
  inside `build_command_table`) and the definition at `:475`. **Live production caller exists.**
- Symptom (before): every `file.read`/`file.write` failed on a node that had not yet run bash, while the node still
  advertised both commands (the center's fail-fast passed, the user saw a broken tool). (After the fix: no symptom.)
- Hypothesis: CUT → CONNECT — the producer (the directory) and the consumer (the jail resolver) now agree on one path.
- Confidence: high

### A3-P10 — `CommandTable::register` key and the descriptor name are one namespace now
- Form: 5 (prior: name drift)
- Prior status: **FIXED**
- Producer: `src/cluster/node_runtime.rs:59` — `let key = cmd.descriptor().name;` — the **descriptor name is the key**;
  the `name` argument is only used for the mismatch log at `:61-67` (`tracing::error!`).
- Consumer: `src/cluster/node_runtime.rs:118` — `dispatch()` looks up `self.commands.get(tool)`, i.e. the same key; and
  the declared catalog (`descriptors()`, `:94`) carries the same descriptor name to the center, where the admission check
  compares it (`src/builtin_tools/node_invoke.rs:89-93`).
- Live caller search: `rg -n 'descriptor\(\)\.name|commands\.get\(tool\)' src/cluster/` → `:59` and `:118`. Live.
- Symptom (before): the allowlist key and the declared name could diverge, so the center's fail-fast could pass while the
  node refused the command (or vice-versa). (After the fix: one namespace.)
- Hypothesis: CUT → CONNECT
- Confidence: high

### A3-P11 — the file-command jail root and the bash session directory are one derivation now
- Form: 5 (prior: two independent inputs)
- Prior status: **FIXED**
- Producer: `src/cluster/node_runtime.rs:193-201` — `register_file_commands_for_session(workspace_root, session)`
  computes `crate::sandbox::workspace::session_workspace_dir(workspace_root, session)` (`:199`) and hands *that* to
  `register_file_commands` (`:200`).
- Consumer: `src/sandbox/workspace/mod.rs:125` — `jail_root_for(sid)` =
  `current_exec_workspace().unwrap_or_else(|| workspace_root.join(session_key_to_filename(sid)))`;
  `src/sandbox/workspace/path.rs:65` — `session_workspace_dir` = `workspace_root.join(session_key_to_filename(sid))`,
  and `session_key_to_filename` (`path.rs:74`) is the sha256-of-session-key prefix used by **both**.
  Both are derived from the same `(workspace_root, SessionKey)` pair that `src/bin/aleph-server/commands/node.rs:435-454`
  hands to `build_sandbox` (`src/sandbox/factory.rs:30` → `WorkspaceSandbox::new(cfg.workspace_root.clone(), …)`) and to
  `setup_file_commands` (`:452`).
- Live caller search: `rg -n 'register_file_commands_for_session|session_workspace_dir|jail_root_for' src/` →
  `node_runtime.rs:193/:200`, `path.rs:65`, `workspace/mod.rs:125`, `node.rs:452/:488`. Live.
- Symptom (before): a pushed script landed in a directory bash never executed in (or the reverse) — both legs reported
  success. (After the fix: impossible.)
- Hypothesis: CUT → CONNECT
- Confidence: high

### A3-P12 — the reverse-RPC response deadline is taken after the enqueue
- Form: 2 (prior: the response window was silently shortened by the enqueue)
- Prior status: **FIXED**
- Producer: `src/cluster/reverse_rpc.rs:334-338` — the enqueue half gets `outbound_budget_ms =
  timeout_ms.min(OUTBOUND_PUSH_BUDGET_MS)` (`:162`, 500 ms) and its own `timeout_at(outbound_deadline, …)`.
- Consumer: `src/cluster/reverse_rpc.rs:365` — `let response_deadline = Instant::now() + Duration::from_millis(timeout_ms);`
  is computed **after** `outbound.send` returned `Ok(Ok(()))`, so the peer gets the full `timeout_ms` to answer.
- Live caller search: `rg -n 'response_deadline|outbound_deadline' src/cluster/reverse_rpc.rs` → `:335`, `:338`, `:365`.
  Live.
- Symptom (before): a slow enqueue ate the response budget and a healthy peer's reply was rejected as a timeout while
  the peer kept executing. (After the fix: the enqueue no longer consumes the response window — **but see A3-01: the
  nested approval park still can**.)
- Hypothesis: CUT → CONNECT (for the enqueue arm only; the nested-arm variant is A3-01)
- Confidence: high

### A3-P16 — the jail containment check still canonicalizes and lets the kernel re-resolve
- Form: 2 (prior: containment check ≠ open-time resolution)
- Prior status: **PARTIALLY FIXED**
- Producer: `src/cluster/node_file_cmd.rs:67-104` — `resolve_in_jail()`: `symlink_metadata(workspace_dir)` (`:92`),
  `canonicalize(workspace_dir)` (`:97`), `check_and_resolve_path(..., Some(&root))` (`:100`),
  `if !resolved.starts_with(&root)` (`:102`).
- Consumer (the far end that re-resolves): the kernel, at `open(2)` time —
  `src/cluster/node_file_cmd.rs:203` (write) and `:285` (read) set `O_NOFOLLOW | O_NONBLOCK`.
- Live caller search: `rg -n 'canonicalize|starts_with|O_NOFOLLOW' src/cluster/node_file_cmd.rs` → `:97`, `:102`, `:203`,
  `:285`. Live.
- Symptom: the **leaf** half is closed (a leaf swapped for a symlink after the canonicalize is refused by `O_NOFOLLOW`,
  and a FIFO planted at the leaf cannot block `open(2)` because of `O_NONBLOCK`). The **intermediate-directory** half is
  still open: the path is canonicalized to a string and the kernel re-walks it at open time, so swapping an
  *intermediate* directory for a symlink still escapes the jail. The code says so itself at `:76-82`:
  *"NOTE: this narrows the race window but does not close it. Between the `symlink_metadata` and `canonicalize` calls
  below, an attacker who can win that race can still swap the root to a symlink … Closing the window entirely requires
  pinning a root directory fd and using `openat` / `O_NOFOLLOW` on every descendant operation."*
- Hypothesis: DECIDE — the residual half needs an `openat`-on-a-root-fd rewrite (a design decision), not a patch.
- Confidence: high (the residual is documented in the code itself)

### A3-P19 — a non-regular file inside the workspace no longer blocks dispatch
- Form: 2 (prior: `open(2)` on a FIFO blocks forever)
- Prior status: **FIXED**
- Producer: `src/cluster/node_file_cmd.rs:158-164` — the write side refuses a pre-existing non-regular destination
  (`symlink_metadata` → `!meta.is_file()` → `"file.write: destination is not a regular file: …"`), and `:203` adds
  `O_NONBLOCK` so a FIFO planted between the check and the open cannot park a worker thread.
  Read side: `:261` — `"file.read: not a regular file: …"`, plus `O_NOFOLLOW|O_NONBLOCK` at `:285`.
- Consumer: `src/cluster/reverse_rpc.rs:284` — `call()` now always gets an answer (error reply) instead of a hung
  dispatch, so the reverse-RPC waiter is released.
- Live caller search: `rg -n 'not a regular file|O_NONBLOCK' src/cluster/node_file_cmd.rs` → `:162`, `:203`, `:261`,
  `:285`. Live. Regression tests exist at `node_file_cmd.rs:590` (FIFO) and `:631` (directory).
- Symptom (before): the node's dispatch task blocked forever inside `open(2)` and the reverse-RPC call sat until its
  timeout with the node still holding the request. (After the fix: an immediate error reply.)
- Hypothesis: CUT → CONNECT
- Confidence: high

### A3-P21 — `OutboundWedged` no longer reports the caller's `timeout_ms`
- Form: 2 (prior: a wrong number in the error)
- Prior status: **FIXED**
- Producer: `src/cluster/reverse_rpc.rs:334` — `let outbound_budget_ms = timeout_ms.min(OUTBOUND_PUSH_BUDGET_MS);`
- Consumer: `src/cluster/reverse_rpc.rs:355` — `return Err(ReverseRpcError::OutboundWedged(outbound_budget_ms));`
  (the value is carried by the `OutboundWedged(u64)` variant at `:117` and rendered through `Display` into the tool
  error string).
- Live caller search: `rg -n 'OutboundWedged' src/` → `reverse_rpc.rs:117`, `:131`, `:133`, `:355` (+ its own tests).
  The variant is never *matched* by a production consumer — it is consumed through `Display` only, which is the intended
  shape for a tool error string.
- Symptom (before): the error claimed a 120 000 ms wait when the call had actually given up after ≤ 500 ms (240×
  overstatement). (After the fix: the reported number is the enqueue budget the call actually had, ≤ 500 ms — and since a
  wedge means the `timeout_at` fired, the budget *is* the elapsed time in practice.)
- Hypothesis: CUT → CONNECT
- Confidence: high

### A3-P22 — `descriptors()` now has a total order
- Form: 2 (prior: `sort_unstable_by` on a self-reported name, equal names → HashMap order)
- Prior status: **FIXED**
- Producer: `src/cluster/node_runtime.rs:100` —
  `out.sort_unstable_by(|a, b| a.1.name.cmp(&b.1.name).then_with(|| a.0.cmp(&b.0)));` where `a.0`/`b.0` is the table key.
- Consumer: `src/bin/aleph-server/commands/node.rs:289` — `let declared = table.descriptors();` → the catalog sent in the
  `connect` frame (`:528`) → parsed by `src/cluster/registry.rs:698` → projected to names by `NodeSummary`
  (`registry.rs:139-152`).
- Live caller search: `rg -n 'descriptors\(\)' src/` → `node_runtime.rs:94` (def), `node.rs:289` (production).
  Live.
- Symptom (before): two commands with the same declared name sorted non-deterministically between restarts, so the
  advertised catalog order flipped run to run. (After the fix: stable.)
- Hypothesis: CUT → CONNECT
- Confidence: high

---

# PART B — new candidates

### A3-01 — the operator-approval park is nested inside the `tool.call` response window, and the nested wait outlasts it
- Form: 5 (contract/budget drift between the two ends — the two ends disagree on *whose clock* governs a command that
  needs a human; see also the Form-1 flavour on the dropped late reply below)
- Producer:
  - `src/cluster/node_approval.rs:47` — `pub(crate) const NODE_APPROVAL_TIMEOUT_MS: u64 = 130_000;`
  - `src/cluster/node_approval.rs:154` — `.call("node.approval.request", params, NODE_APPROVAL_TIMEOUT_MS)`
    (the node's escalation park; reached from `src/sandbox/workspace/mod.rs:483`
    `self.approval_gate.request_approval_for_action(&action).await`, which the node's bash path hits at
    `src/cluster/node_runtime.rs:155` `self.bash.call_json(args)` → `BashExecTool` → the sandbox's elevation gate).
  - The **outer** clock: `src/builtin_tools/node_invoke.rs:19` `DEFAULT_TIMEOUT_MS = 120_000`, used at `:95-97`
    (`channel.call("tool.call", params, timeout)`); the same 120 s default in
    `src/builtin_tools/node_invoke_many.rs:23` and `src/builtin_tools/node_file.rs:24`.
  - The **inner** decision clock (center side): `src/approval/node_requester.rs:119` —
    `manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS)` with
    `DEFAULT_APPROVAL_TIMEOUT_MS = 120_000` (`src/exec/manager.rs:49`).
- Consumer: `src/gateway/server/connection/mod.rs:368-441` — the center's `node.approval.request` handler (spawns
  `crate::approval::run_node_approval(...)`, replies `JsonRpcResponse::success(req_id, {"outcome": …})`); and, for the
  outer wire, `src/cluster/reverse_rpc.rs:365-372` — the response window that ends the `tool.call`.
- Live caller search:
  - `rg -n 'node\.approval\.request' src/` → producer `src/cluster/node_approval.rs:154`; handler
    `src/gateway/server/connection/mod.rs:368`; docs in `src/cluster/registry.rs:562`, `:1686`,
    `src/cluster/reverse_rpc.rs:237`; tests `tests/cluster_node_approval.rs:36`. **Live on both ends.**
  - `rg -n 'NODE_APPROVAL_TIMEOUT_MS|DEFAULT_APPROVAL_TIMEOUT_MS' src/` → `node_approval.rs:47`, `:154`;
    `exec/manager.rs:49`; `approval/node_requester.rs:19`, `:119`. **Live.**
  - `rg -n 'request_approval_for_action' src/` → `src/sandbox/workspace/mod.rs:483` (production). **Live.**
- Symptom: the center's `tool.call` waiter dies at `T0 + 120 s` (the tool's own default `timeout_ms`), while the node is
  still parked on the operator and the center's approval driver is still counting down **its own** 120 s, started δ
  later (δ = push + WS round trip + bash start + elevation detection). Any decision that lands after
  `T0 + 120 s − δ` therefore produces this exact sequence:
  1. the model is told `node 'x' reverse-rpc failed: timed out after 120000ms`;
  2. the operator approves (or the center's driver times out) and the node **executes the command anyway**;
  3. the node's reply arrives with an id the center has already dropped → `src/cluster/reverse_rpc.rs:62-88` logs
     *"reverse-rpc resolve received response for unknown id (already resolved, cancelled, or never registered)"* and the
     result is **discarded** — a produced value with no consumer (Form-1 flavour).
  So the model's view of the world ("it timed out") and the node's view ("it ran, and here is the output") diverge, and
  the node's output is thrown away. This is precisely the hazard prior finding #12 named — *"leaving the peer to execute
  a command the center has now told its caller 'timed out'"* — surviving through the arm #12 did not touch: #12 split the
  *enqueue* half out of the response window, but the *nested approval park* is still inside it. The harness-level budget
  does not help: `src/tools/budget.rs:168-172` gives `node_invoke`/`node_invoke_many`/`node_file` a 270 000 ms budget
  (above the 240 s reverse-RPC ceiling at `src/cluster/reverse_rpc.rs:293`), so the harness never preempts — it is the
  **tool's own 120 s default** that fires, and that is *shorter* than the nested park's own ceiling of 130 s
  (`node_approval.rs:47`).
- Hypothesis: DECIDE — either the center's `tool.call` window must be sized to cover the park
  (`approval ceiling + command ceiling`) and the node must report "parked" progress, or the node must be told to abandon
  the command when the center stops waiting; which side owns the budget is a product decision, not a patch. Note the
  model *can* widen the window (`timeout_ms` is clamped to 240 s at `reverse_rpc.rs:293`), but the default — the common
  case — is the broken one.
- Confidence: high (the mechanism is code-verified end to end; the *frequency* depends on operator latency)

### A3-02 — `CommandDescriptor.schema` is declared, parsed, stored, bounded — and read by nothing
- Form: 3 (inert config)
- Producer: `src/cluster/node_runtime.rs:149-152` (`BashNodeCommand::descriptor()` →
  `schema: json!({"type": "object"})`), `src/cluster/node_file_cmd.rs:221-225` (`file.write`) and `:308-312` (`file.read`)
  — all three descriptors ship an opaque `{"type": "object"}`.
- Consumer: **no consumer found** (grepped: `rg -n 'CommandDescriptor' src/ tests/` → the only readers of the catalog
  compare `CommandDescriptor::name`; `src/cluster/registry.rs:136-139` states it outright: *"The catalog's `schema` is
  read by **no** consumer in the tree (every caller compares `CommandDescriptor::name` only)"*; `registry.rs:945-990`
  (`bound_commands`) only *bounds* the schema's size, it never reads it; `src/cluster/registry.rs:139-152`
  (`NodeSummary`) projects **names only**).
- Live caller search: `rg -n 'schema' src/cluster/registry.rs` → `:30` (field), `:945-990` (size bounding),
  `:136-139` (the "no consumer" note). `rg -n '\.schema' src/` → no reader of a declared command's schema.
  **No live consumer.**
- Symptom: the node advertises a JSON Schema for every command and the center stores it (bounded to
  `MAX_COMMAND_SCHEMA_BYTES`), but no validator ever checks a caller's `args` against it. The schema is `{"type":
  "object"}` on both ends anyway, so it carries no information; a future "the schema validates the args" assumption
  would silently validate nothing.
- Hypothesis: CUT — either drop the field from the wire or wire it to the center's arg validation; today it is a knob
  that turns nothing. (Deliberate and documented, hence low severity.)
- Confidence: high

### A3-03 — the node-side channel carries no close signal, so `close_connection()` is a no-op on the node
- Form: 2 (stub far-end: the recovery arm exists but does nothing on one side)
- Producer: `src/cluster/reverse_rpc.rs:246-250` —
  `pub fn close_connection(&self) { if let Some(close) = &self.close { close.notify_one(); } }` — a **no-op when
  `close` is `None`**; `src/cluster/reverse_rpc.rs:189-196` — `pub fn new(outbound)` sets `close: None`;
  `src/cluster/reverse_rpc.rs:353-356` — the `OutboundWedged` arm calls `self.close_connection()`.
- Consumer: the **center's** channel has the signal (`src/cluster/reverse_rpc.rs:208-215` `with_close`,
  wired at `src/gateway/server/connection/mod.rs:269`); the **node's** channel does not
  (`src/bin/aleph-server/commands/node.rs:577` — `let channel = ReverseRpcChannel::new(out_tx.clone());`).
  The node's own read loop (`node.rs:605-634`) has no wedge detector: the writer task (`node.rs:583-598`) simply blocks
  in `write.send` when the peer stops draining, the read loop keeps running, and `run_session` never returns.
- Live caller search: `rg -n 'close_connection|ReverseRpcChannel::new|with_close' src/` → `reverse_rpc.rs:189/:208/:246`,
  `node.rs:577` (node side, no signal), `src/gateway/server/connection/mod.rs:269` (center side, with signal);
  `rg -n 'close_connection' src/` → `reverse_rpc.rs:246` (def) + `reverse_rpc.rs:353` (the only caller) +
  `src/cluster/registry.rs` (the registry's own eviction path, center-side).
- Symptom: when the **node's** outbound wedges (the center stops draining the node's frames), the node cannot tear the
  connection down itself — the doc at `reverse_rpc.rs:181-188` promises *"reconnection is managed by each side's own run
  loop"*, but the node's run loop has no wedge detector, so it does not manage anything. The node stays "online" in the
  center's registry, every command sent to it times out at the center, and the node's own escalation path degrades to
  `Unavailable` (its approval RPC hits the same wedged outbound and returns `OutboundWedged` after 500 ms →
  `src/cluster/node_approval.rs:181-186`). The reaper is the **center's** inbound idle-watchdog, i.e. the node's
  recovery depends on the other side's timer.
- Hypothesis: DECIDE — the node side needs its own wedge detector (or a `with_close` equivalent) if the doc's promise is
  to hold; the asymmetry is documented but the promise is not implemented.
- Confidence: medium (the mechanism is code-verified; whether the center's idle-watchdog covers the node-side wedge in
  every configuration was not traced end to end — see "seams not covered")

### A3-04 — the `approved_session` outcome has no live producer
- Form: 1 (produced/consumed vocabulary value with no live producer)
- Producer (center): `src/approval/node_requester.rs:45-52` — `outcome_to_wire` maps
  `ApprovedForSession | ApprovedAlways => "approved_session"`.
- Consumer (node): `src/cluster/node_approval.rs:57-84` — `outcome_from_str` maps `"approved_session"` →
  `ApprovalOutcome::ApprovedForSession`, and `src/sandbox/workspace/mod.rs:496-500` grants on it
  (`Approved | ApprovedForSession | ApprovedAlways` → `ws.granted_elevations.insert(...)`).
- Live caller search: `rg -n 'ApprovedForSession|ApprovedAlways|once_only' src/` →
  `src/approval/node_requester.rs:46-47` (the mapping), `src/sandbox/workspace/mod.rs:493-500` (the grant arms, with
  the note that `ApprovedAlways` *"is unreachable"*), `src/exec/manager.rs` (`once_only()` — the allowed-decision set the
  center's resolver narrows to `AllowOnce`). The center's resolver therefore never yields `ApprovedForSession` or
  `ApprovedAlways`, so `"approved_session"` is **never emitted** in production.
- Symptom: no user-visible symptom (the node's arm is defensive). The risk is a vocabulary that looks like it has a
  producer: if the center's policy were ever relaxed to allow a session-scoped grant, the node's
  `ApprovedForSession` → "grant for this session" behaviour would engage **without the node having a way to revoke it**
  (`granted_elevations` is per-workspace/per-session and dies with the node's ephemeral session key —
  `src/routing/session_key.rs:293` `SessionKey::ephemeral` mints a fresh uuid per process start).
- Hypothesis: DECIDE — keep the arm and document that the center's `once_only` clamp is what makes it dormant, or delete
  the arm and the mapping together. Both ends currently agree on the spelling, so nothing is broken today.
- Confidence: high

---

# Checked and found clean (per lens)

| Lens | What was checked | Result |
|---|---|---|
| Registration parity | `CommandTable::register` (`:57`) vs `descriptors()` (`:94`) vs the node's `connect` frame (`node.rs:528`) vs `registry.rs:698` parse | one key, one name (`node_runtime.rs:59`) — consistent |
| Call-vs-handler parity | center → node: `"tool.call"` (`node_invoke.rs:96`, `node_invoke_many.rs`, `node_file.rs:164/:195`) vs node `dispatch` (`node_runtime.rs:109-111`). node → center: `"node.approval.request"` (`node_approval.rs:154`) vs handler (`connection/mod.rs:368`) | both directions have a handler; the node's `handle_frame` (`node.rs:642`) answers `tool.call` and ignores everything else, which matches what the center sends |
| Classifier-vs-handler parity | the center's fail-fast compares the *declared catalog* names (`node_invoke.rs:89-93`, `node_invoke_many.rs`, `node_file.rs:111-115`); the node's authoritative gate is `CommandTable::dispatch` (`node_runtime.rs:118`) | the literal names agree on all three ends: `"bash"` (`node_runtime.rs:150`), `"file.write"` (`node_file_cmd.rs:223`), `"file.read"` (`node_file_cmd.rs:310`) vs `node_file.rs:100-101` — **no drift** |
| Event emit-vs-subscribe parity | `node.connected` (`connection/mod.rs:1205`), `node.disconnected` (`cleanup.rs:97`, `enrollment.rs:777`) | the gateway bus carries them and `src/gateway/event_scope.rs:504-505` lists them as scoped topics with a role floor — subscribers exist (WS clients + the scope gate) |
| Config-reader parity | `MAX_FILE_BYTES` (`node_file_cmd.rs:20`) → read by the node (`:41`, `:49`, `:292`) **and** by the center (`node_file.rs:20`, `:150`, `:156`, `:220`, `:229`); `sha256_hex` (`node_file_cmd.rs:31`) → used by the node (`:57`) **and** by the center (`node_file.rs:20`, `:162`, `:235`) | one constant, one digest function, one base64 bound (`MAX_B64_LEN` at `:25` vs the center's inline `MAX_FILE_BYTES*4/3+4` at `node_file.rs:220` — both derive from the same constant) — **no contract drift**; the center verifies the digest it receives (`node_file.rs:236-238`) and the node verifies the digest it is sent (`node_file_cmd.rs:57-59`) |
| Path/route parity | the center's `local_path` (`node_file.rs:120`, `:238`) is resolved with `check_and_resolve_path(..., None)`; the node's `remote_path` is resolved by `resolve_in_jail` (`node_file_cmd.rs:67`) | the node's path can never escape the jail (`:102`); the center's local path is deny-list checked. Note: a **relative** `local_path` on the center resolves only if an `FsScope` base exists (`src/builtin_tools/file_ops/path_utils.rs:1134` — `output_dir_override = None`) → the tool errors instead of silently picking a base; the tool's own DESCRIPTION example uses an absolute `local_path` (`node_file.rs:70`) |
| Stub sweep | `TODO`/`todo!`/`unimplemented!` in the four files; handlers that return success with no effect | none found in the slice. `close_connection()` is the one arm that does nothing — reported as A3-03 |
| Waiter-leak sweep | `PendingInvokes::register` (`reverse_rpc.rs:40`) vs `resolve` (`:62`), `cancel` (`:90`), `cancel_all` (`:107`) | the `WaiterGuard` (`:391-409`) removes the entry on **every** non-terminal exit including a dropped future; `cancel_all` is a live production caller (`src/gateway/server/connection/cleanup.rs:72`) — **no leak** |
| Approval reachability | `ApprovalSlot` (`node_approval.rs:41`) → filled per session at `node.rs:579-581`, cleared at `node.rs:632-634`; `CenterApprovalRequester` (`node_approval.rs:122-133`) is the only production `ApprovalRequester` impl; the center's `exec_approval_manager` is `Some` in production (`src/bin/aleph-server/commands/start/mod.rs:413-416`) | **live** — the slot is not stuck at `None`; the escalation reaches the gate (`src/sandbox/workspace/mod.rs:483`). The `None` path (`node_approval.rs:139-145`) is the fail-closed arm between sessions, not the normal path |
| Outcome vocabulary | center `outcome_to_wire` (`node_requester.rs:45-52`) vs node `outcome_from_str` (`node_approval.rs:57-84`) | the five spellings agree (`approved`, `approved_session`, `denied`, `timeout`, `unavailable`) — no drift (but see A3-04: one of them has no live producer) |
| Params vocabulary | node producer `{"tool","reason","action"}` (`node_approval.rs:148-152`) vs center reader (`connection/mod.rs:390-404`) | the three keys match exactly |
| Response-frame discriminator | `parse_response_frame` (`connection/mod.rs:144-155`), shared by both ends (`node.rs:608`) | one rule, one wire — a `null`-result response is still classified as a response (the discriminator looks at the raw JSON keys, not the deserialized `Option`) |

---

# Seams not covered (and why)

1. **`src/gateway/server/connection/mod.rs`'s inbound router / lane classifier / rate limiter** — the full method→lane
   classification path for `node.approval.request` and `connect` was read only at the interception point
   (`:368`) and the connect path (`:1196`); the lane/idempotency/rate-limit arms for the *cluster* methods were not traced
   end to end. A cluster-specific classifier entry that scores `tool.call`/`node.approval.request` differently from the
   handlers above was not searched for exhaustively.
2. **`src/cluster/registry.rs`'s `forget` / `register` supersede / eviction path** — read only for the `conn_id` and
   `bound_commands` questions. The claim that a superseded node's connection is closed
   (`reverse_rpc.rs:181-188` mentions `NodeRegistry::forget` and `register`) was **not** verified against
   `registry.rs`'s implementation.
3. **The center's inbound idle-watchdog** (`idle_timeout_secs`) — the exact value and the conditions under which it
   reaps a *node-side* write wedge were not traced; this is the evidence gap behind A3-03's "medium".
4. **`src/exec/manager.rs`'s approval resolution** (`create`, `register_pending`, `await_registered`) — read only for
   `DEFAULT_APPROVAL_TIMEOUT_MS = 120_000` (`:49`) and its use at `node_requester.rs:119`. The manager's own timeout /
   expiry / revocation semantics (does an expired approval card still resolve to a decision?) were not traced.
5. **The node's WS writer/read-loop backpressure behaviour under a slow peer** — not reproduced; A3-03's symptom is
   derived from the code, not observed.
6. **Non-cluster consumers of `crate::cluster`** — `src/gateway/handlers/cluster.rs` was read (it holds
   `handle_cluster_enroll` / `handle_cluster_deregister` / `handle_environments_list`, with tests only in that file);
   the HTTP/IPC surface of `node_manage`/`node_list` was checked for name parity only, not behaviourally.
7. **`tests/cluster_*.rs`** — read for coverage-vs-production distinctions only. `tests/cluster_node_approval.rs` drives
   the node side against an **in-process fake center**, not the real WS handler at `connection/mod.rs:368`; so the
   production round trip (node → real center → operator) has **no** end-to-end test in the tree. That gap is why A3-01's
   budget skew is invisible to CI.
8. **`src/sandbox/exec_approval/gate.rs`'s `ApprovalGate` fallbacks** (unattended turn-context → `Unavailable`,
   no requester → `Unavailable`) were read at the call-site level only; the exact conditions under which the node's
   `SESSION_ID.scope` makes a turn "unattended" were not traced.
