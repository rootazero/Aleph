# Severed-wire audit — `src/cli`, `src/cluster`, `src/command`

Date: 2026-11-20 · Branch `main` @ `83ff852ee` · Worktree `.worktrees/sev-wire-ccc`
Scope: 13 files, ~7.6k lines — `src/cli/{mod,endpoint,ipc_client,policy}.rs`,
`src/cluster/{mod,enrollment,registry,node_runtime,node_file_cmd,reverse_rpc,node_approval}.rs`,
`src/command/{mod,parser}.rs`.

## Method

- Four read-only slices (one per seam group) were audited by parallel
  `sev-auditor` subagents; the reasoning model was `minimax-cn/MiniMax-M3`
  (A1/A4 fell back to `88api/gpt-6.2-sol` after the MiniMax token plan was
  exhausted mid-session).
- Each slice re-verified the 31 findings of
  `review-results/2026-10-30-rust-doctor-cluster-cli-command-findings.md`
  (copied here as `PRIOR-FINDINGS.md`) and then hunted NEW severed wires
  against the 6 seam forms and the 7 parity lenses of the skill's
  `references/seam-catalog.md`.
- The code map (`graphify-out/`, rebuilt this session: 154 224 nodes /
  315 528 edges) was used for consumer lookups (`graphify explain <Symbol>`).
- A mechanical form-1 scan over the three modules found **0 exported symbols
  without a consumer outside their own file**.

## Jev usage (disclosure)

Classification, filtering and routing judgments were delegated to the Jev
model (`jev_evaluate`); multi-constraint reasoning was kept on the reasoning
models. Jev was used for:

| Step | Input | Jev verdict |
|---|---|---|
| Triage disposition (CONNECT / CUT / DECIDE) | A2-01 … A2-06, A3-01, A3-03, A3-04 | CONNECT 1.00 · CUT 0.83/0.74/0.78/0.77 · DECIDE 0.69/0.58/0.49 |
| Prior-finding re-verification labelling | the 31 prior findings | used only as a hint; every label was re-checked by hand |
| Seam routing (which slice owns which form) | the 13-file inventory | form→slice routing table |

Jev was **not** used for: the code reading, the root-cause analysis, the fix
design, the diff review, or the final disposition — those are reasoning-model
and human-judgment steps.

Where I overrode Jev:

- **A3-04** — Jev said CUT (0.77); I kept both sides of the
  `approved_session` arm, because it is deliberate protocol vocabulary
  (see below), and cutting it would turn an operator approval into a refusal.
- **A2-06** — Jev said DECIDE with confidence 0.24; I treated it as CONNECT
  (the two ends of the same event had to agree on the name).

## Prior findings — re-verification

Every prior finding that falls inside the audited slices was re-verified
against the current tree. The code is far more hardened than the 2026-10-30
report assumed.

| Slice | Prior findings | Verdicts |
|---|---|---|
| A2 `src/cluster/{mod,enrollment,registry}.rs` | P01 P02 P03 P06 P07 P08 P09 P12 P14 P18 | 6 FIXED · 3 PARTIALLY FIXED (P01 P08 P09) · 1 REFUTED-as-design (P03: LAN trust has no proof of possession) |
| A3 `src/cluster/{node_runtime,node_file_cmd,reverse_rpc,node_approval}.rs` | P04 P10 P11 P12 P16 P19 P21 P22 | 7 FIXED · 1 PARTIALLY FIXED (P16: the leaf symlink swap is closed with `O_NOFOLLOW`, the intermediate-directory swap needs an `openat`-on-root-fd rewrite) |
| A1 `src/cli/**` | P17 P23 P24 P25 P26 P29 | 5 FIXED · 1 OPEN by design (P29: the `LockOrIpc` route is `&'static str`, so per-key admin routes stay unreachable from the CLI) |
| A4 `src/command/**` | P05 P27 P28 P30 | 4 FIXED |


The four `src/command` prior findings are all FIXED: the `system_prompt` copy
is gone (the prompt rides the tool description), `tool_registry_arc()` is gone
(the `Arc` is not cloned out), an unknown `ToolSource` logs and degrades to a
plain chat turn, and the `pattern` fallback documents the name fallback.

## New findings

| ID | Form | Finding | Disposition |
|---|---|---|---|
| A2-01 | 2 | `node.disconnected` is never published on the operator path — the production `AuthContext` carries `event_bus: None` | **CONNECT** (fixed) |
| A2-02 | 3 | `CommandDescriptor.schema` is bounded, stored, cloned into every `environments.list` and shipped to the Panel, and read by nobody | **CUT** (fixed) |
| A2-03 | 3 | `Environment.connected_at` has a producer and no consumer (and `0` for offline is indistinguishable from the epoch) | **CUT** (fixed) |
| A2-04 | 3/5 | `devices.fingerprint` for node rows stores a truncated device id while the column means "16 hex chars of SHA256(public_key)"; its only reader has no production caller | **CUT** (fixed) |
| A2-05 | 5 | tags are peer-declared but documented as "operator-assigned"; they are also never persisted | **DOCS** (fixed; persistence not added) |
| A2-06 | 5 | `node.connected` publishes the raw peer `device_name` while every other event publishes the bounded one | **CONNECT** (fixed) |
| A3-01 | 5 | the operator-approval park is nested inside the tool.call window and loses the race — the model is told "timed out" while the node is still parked, and the verdict is discarded | **CONNECT** (fixed) |
| A3-02 | 3 | `CommandDescriptor.schema` again from the runtime slice (same root as A2-02) | **CUT** (fixed) |
| A3-03 | 2 | the node's reverse-RPC channel carries no close signal, so a wedged outbound stream never tears the node session down | **CONNECT** (fixed) |
| A3-04 | 1 | `"approved_session"` has no live producer (the center's `once_only` clamp) | **KEEP + document** (Jev said CUT; overridden) |
| A4-01 | 3 | the retired routing knobs (`provider`, `strip_prefix`, `intent_type`, `preferred_model`, `icon`) parse but have no effect — and `provider` is still REQUIRED by `Config::validate` | **DECIDE** — left open for the operator (see below) |
| A1 | — | `src/cli` produced **0** new candidates | — |

## Fixes applied

All fixes landed on the worktree branch `audit/sev-wire-cli-cluster-command`
and were merged into `main` (fast-forward). 19 files, +217/−118.

1. **A2-01** — `src/bin/aleph-server/commands/start/builder/subsystems.rs`:
   `initialize_vault(port, node_registry, event_bus)` gained the gateway bus and
   the production `AuthContext` now carries `event_bus: Some(event_bus)`;
   `src/bin/aleph-server/commands/start/mod.rs:627` passes
   `server.event_bus().clone()`. The operator-initiated `node.disconnected`
   event is published again.
2. **A2-06** — `src/gateway/server/connection/mod.rs`: `node.connected` now
   publishes the registry's authoritative bounded name
   (`node_registry.node_identity_by_conn(&conn_id)`), falling back to the raw
   claim name. The main gateway WebSocket route sets no `max_message_size`,
   so the raw peer name was fanned out verbatim to every admin subscriber.
3. **A2-03** — removed `Environment.connected_at` (registry), its offline
   initializer (`gateway/handlers/cluster.rs`) and the TS mirror field.
   `NodeSession.connected_at` / `NodeSummary.connected_at` stay (the latter is
   read by `node_list`).
4. **A2-02 / A3-02** — `src/cluster/registry.rs` now carries TWO shapes:
   `CommandDeclaration { name, schema }` (the WIRE shape — its `schema` key is
   required, so a catalog without it stays malformed and fail-closed, and old
   centers still parse it) and `CommandDescriptor { name }` (the STORED/SHIPPED
   shape). `bound_commands` still measures the schema (the size bound is real)
   but no longer stores it, so `environments.list` stops cloning ≤512 KiB of
   unread schema per node per refresh. Updated: `node_runtime.rs`,
   `node_file_cmd.rs`, `cluster/mod.rs`, `bin/aleph-server/commands/node.rs`,
   the test fixtures, and the TS mirror.
5. **A2-04** — `src/cluster/enrollment.rs` stores the FULL device id in
   `devices.fingerprint` (the store's lookup-key convention, matching the
   panel path) instead of a 16-char truncation; the stale doc line was
   corrected. `src/gateway/security/store/devices.rs` no longer advertises a
   "read on every remote connect" path that has no production caller.
6. **A2-05** — docs only: `interfaces/webchat/src/api/cluster.rs` and
   `src/cluster/registry.rs` now say the tags are declared BY THE NODE, not
   operator-assigned, and that they are a selection hint, never an
   authorization gate.
7. **A3-01** — the three node tools' `DEFAULT_TIMEOUT_MS` 120_000 → 150_000
   (`src/builtin_tools/{node_invoke,node_invoke_many,node_file}.rs`). The
   window must ENCLOSE the center's approval window
   (`DEFAULT_APPROVAL_TIMEOUT_MS = 120_000`, `src/exec/manager.rs`); with both
   at 120 s the node's explicit "approval timeout" verdict always lost the
   race and was discarded as an unknown id (`src/cluster/reverse_rpc.rs`). The
   harness budget (270 s) and the reverse-RPC ceiling (240 s) still fit.
8. **A3-03** — the node session channel is now built with
   `ReverseRpcChannel::with_close(out_tx, close)` and the node's read loop
   selects on the close signal, so a wedged outbound stream tears the session
   down and the node reconnects instead of staying "online" while every
   command times out. `src/cluster/reverse_rpc.rs` documents that `new` is
   for one-shot/test channels only.
9. **A3-04** — kept both arms of the approval vocabulary and documented the
   center's `once_only` clamp on the node side, so a center that does send
   `"approved_session"` is never refused as protocol drift.

## Guard (phase 5)

`scripts/cluster_wire_audit.py` — a seam-invariant guard for the facts the
type system cannot express. It uses the repo's brace-matching test stripper
(`scripts/wiring_strip.py`) so an inline `#[cfg(test)]` helper cannot mask
production code.

| Check | Invariant |
|---|---|
| `shape` | `CommandDeclaration` keeps the wire `schema` field; `CommandDescriptor` must NOT carry `schema` (no consumer reads it) |
| `timing` | every node tool's `DEFAULT_TIMEOUT_MS` and `NODE_APPROVAL_TIMEOUT_MS` must strictly exceed `DEFAULT_APPROVAL_TIMEOUT_MS` |
| `liveness` | the node session channel must bind a close signal (`with_close`, never bare `new`) |
| `bus` | the production `AuthContext` must carry the gateway event bus, and `start` must hand it over |

Negative test (the guard is a detector, not a decoration): on the pre-fix tree
it reports **8 findings** and exits 1; on the fixed tree it prints
`cluster_wire_audit: PASS` and exits 0.

Run from the repo root:

    python3 scripts/cluster_wire_audit.py


## Open decisions (left for the operator — NOT changed here)

1. **A4-01 — `provider` is required but inert.** `Config::validate` refuses a
   command rule without a `provider` naming a configured provider, yet nothing
   consumes it (there is no routing pass that turns a rule's `provider` into a
   model choice). The module doc already says so honestly. Relaxing the
   requirement, or wiring the field, is a config-compatibility decision: it
   changes which operator TOML files load.
2. **A2-05 — tags are peer-declared and not persisted.** The docs now say so,
   but the backend still drops them when a node goes offline, and a peer can
   still choose the tag a fan-out selects on. Persisting them (or refusing
   peer-declared tags) is a product decision.
3. **A3-P16 — the jail symlink race.** The leaf half is closed with
   `O_NOFOLLOW`; closing the intermediate-directory swap needs an
   `openat`-on-a-root-fd rewrite of the jail resolver.
4. **Prior #29 — the `LockOrIpc` route is `&'static str`.** Per-key admin
   routes stay unreachable from the CLI; the route type has to become
   dynamic first.

## Negatives — what was NOT done

- **No test was added** for any fix. The existing suites cover the touched
  paths, but the new invariants (the 150 s window, the node close signal, the
  event bus) have no unit test of their own — only the grep guard.
- **No runtime verification.** Nothing was exercised against a real center and
  a real node: no end-to-end approval round trip, no reconnect after a wedged
  outbound stream, no `node.disconnected` observed in the Panel.
- **Tags are still not persisted** (A2-05) and the `provider` gate is
  untouched (A4-01) — both are decisions, not oversights.
- **The `environments.list` schema was not removed from the wire.** Older
  centers still require the `schema` key, so the field had to stay in
  `CommandDeclaration`; only the stored/shipped copy was cut.
- **Prior #16 was not finished** (the jail `openat` rewrite) and **prior #29
  was not fixed** (the `&'static str` route type); both are design changes.
- **The audit did not cover** the modules around these three: the full
  gateway, the tool registry's dispatch universe, the config surface outside
  `routing.rs`, or the event-bus contract as a whole. The four slices also
  did not run cargo, build or test anything — the verification below was run
  by the parent session, after the review, as one pass.
- **The guard is not wired into CI** — like every other `scripts/*_wiring_audit.py`
  in this repo, it has to be run by hand from the repo root.
- **The `graphify` map was rebuilt but not committed** (`graphify-out/` is
  gitignored), and the graph is a snapshot of the pre-fix tree.

## Verification

One unified pass, after the review (the brief asked for a single check at the
end and none per fix):

    cargo check -p alephcore -p aleph-panel --all-targets -j 4         # HOST_EXIT=0, 13m01s
    cargo check -p aleph-panel --target wasm32-unknown-unknown -j 4     # WASM_EXIT=0

`--all-targets` compiles the TEST code too, which is where the verification
belongs (a fast `cargo check --lib` would have missed the `connected_at`
field left in a registry test — it was caught here).

Memory control: `-j 4` / `CARGO_BUILD_JOBS=4` on a 19 GB / 12-core box, with
the repo's `.cargo/config.toml` already forcing `-C link-arg=-fuse-ld=lld`
(GNU ld exceeded 15 GB RAM on the alephcore dev/test binaries). The WASM pass
reported the 3 pre-existing `dead_code` warnings in
`interfaces/webchat/src/context.rs` (`epoch`, `PtyCoverageWork::Acquire`) —
not from this change.

Guard: `python3 scripts/cluster_wire_audit.py` → `cluster_wire_audit: PASS`
(on the pre-fix tree: 8 findings, exit 1).

**No test suite was executed** — only the compile pass. The guard and the
compile are the only automated evidence in this report.
