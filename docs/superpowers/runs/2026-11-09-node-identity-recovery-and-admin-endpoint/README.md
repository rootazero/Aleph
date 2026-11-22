# Decision round — node identity recovery, refusal cases, admin endpoint contract

Round date: 2026-11-09 · Base commit: `0ae48a487` · Follows:
[`2026-11-07-occams-cli-cluster-command`](../2026-11-07-occams-cli-cluster-command/README.md) ·
Trigger: the user closed that round's four open decision items with
*"同意 `scripts/spec_c_regression.sh` 的红灯 单开一轮。剩下的你按最佳决策实施。"*

This round implements **three of the four** items (the script-gate red light is explicitly
deferred to its own round) and changes **no** architecture: one new CLI flag, one additive wire
field, two corrected messages, two documentation corrections.

---

## Item 1 — `scripts/spec_c_regression.sh` is RED on main → **DEFERRED**

The script fails on `main` today (checks 1/4 and 2/4 report ~10 production `Connection::open`
call sites and a raw vault write; check 3/4 greps for `with_policy\|run_no_lock\|with_policy_owned`
as TEXT, and `with_policy_owned` exists nowhere but in three `//!` comments). Repairing checks 1/4
and 2/4 means touching files far outside the reviewed modules, and the script is not wired into CI.
**Per the user's instruction this is a separate round** — nothing in this commit touches it.

---

## Item 2 — `admit_node`: presented id vs. `device_name` → **NO CODE CHANGE**

**Decision: the presented `device_id` is the identity, and that is already the documented policy.**
No code changed; the policy is now stated where it is enforced.

Evidence:

- `src/cluster/enrollment.rs:20-26` (module doc) already states it: *"The presented id is
  authoritative … the id is the bearer secret … the name is a display affordance a peer may
  re-spell, and renaming a machine must never change its identity. Under LAN trust (nodes hold
  no tokens) the id IS the credential — this is the policy, not an oversight."*
- The earlier worry — *"a node presenting a different id evicts the victim's session"* — is
  **wrong**: the registry is keyed by `node_id` (`src/cluster/registry.rs:192-198`) and
  `register_session` closes **only the session with the same `node_id`** (a stale socket of the
  same node). A node presenting another id never evicts another node's session.
- The node id is **not peer-visible**: `environments.list` is `Class::Admin`
  (`src/gateway/method_census.rs:188`) and `cluster.*` is admin-gated
  (`src/gateway/method_admin.rs:120`, `cluster.enroll` / `cluster.deregister` at `:684` / `:688`).
  A node cannot learn another node's id through the gateway, so the id cannot be *guessed* from
  the panel's point of view.
- **Rejected — persist the presented name.** It would let a node rename itself onto another
  node's name ⇒ duplicate-name ambiguity, the exact hazard `src/cluster/enrollment.rs` exists to
  prevent. The online/offline name divergence is cosmetic and, per the module doc, by design.

---

## Item 3 — a revoked device cannot be re-registered → **the recovery becomes executable**

**Decision: keep the protocol (the revocation protects the CREDENTIAL, not the name) and make the
one documented recovery path executable and discoverable.** Two changes, both additive:

### 3a. `aleph-server node --reset-identity --name <name>`

The recovery was already documented (`docs/reference/CLUSTER.md`: delete the identity file, then
the node reconnects as a FIRST BOOT and the center re-uses the row an operator re-enrolled for
that name, or mints a new one) — but it required **manual file deletion on the node machine**
(`rm` does not exist on Windows), and the node's refusal log named neither the file nor the step.

- `src/bin/aleph-server/cli.rs` — new `Node { reset_identity: bool }` flag; `--center` becomes
  `required_unless_present = "reset_identity"`.
- `src/bin/aleph-server/main.rs` — the reset is dispatched **before** any session: it is a local,
  one-shot action on the node machine and needs no center.
- `src/bin/aleph-server/commands/node.rs` — `reset_identity(name)` resolves the identity path
  (`identity_path`, which already rejects path traversal) and removes it. A missing file is
  **not** an error (`Ok(false)`): the operator asked for a state the node is already in.

**Rejected — automatic credential rotation on the revoked path.** A rogue can squat the name
while the node is down, so a revoked node would regain access through its own recovery. The
rotation has to be an operator action, which is exactly what the new flag is.

**Rejected — a name-based credential.** It contradicts the module's own policy (item 2): under
LAN trust the id is the credential because the name is not secret.

### 3b. The refusal carries a machine-readable CASE (`reason_code`)

The three terminal refusals need **three different operator actions**, but the center sent only
prose (`reason`) — and the node could not tell them apart:

| case | what happened | operator step |
|---|---|---|
| `revoked` | the device record was revoked by `cluster.deregister` | re-enrol the name, then reset the dead credential |
| `ambiguous_name` | several enrolled nodes share this name | deregister the duplicates **by id** |
| `identity_conflict` | the presented id belongs to a paired Panel / operator device | free that id |

- `src/gateway/server/connection/mod.rs` — the three refusal payloads now carry
  `"reason_code": "revoked" | "ambiguous_name" | "identity_conflict"` next to the existing
  `reason` prose. `reason` stays the center-side detail; the code is the CASE. **Both are
  additive**: an older node client ignores the extra field, and an older center sends no code.
- `src/bin/aleph-server/commands/node.rs` — `RefusalCode` (+ `from_wire`, `recovery_hint`) turns
  the code into the operator step; the steps live next to the code so a renamed flag cannot leave
  the two apart. `Unknown` (an older center, or an unknown code) ⇒ the generic message — the node
  never *guesses* a recovery step.
- `docs/reference/CLUSTER.md` — the wire table, the refusal semantics and the recovery block now
  say the same thing, and name the one command that performs the reset.

**Wire compatibility:** the payload keys `reason` / `reason_code` are **new and optional**. The
node client of an older build keeps working (it reads `status` and `reason`); a node client of
this build against an older center falls back to the generic message. No status vocabulary was
added — `status:"deregistered"` stays the single terminal status.

### A clap constraint worth recording

`--center` had to become `Option<String>`:

- clap's derive marks a **non-`Option`** field `required(true)`
  (`clap_derive-4.6.7/src/derives/args.rs:333-340`), and clap's debug assert rejects
  `required` together with `required_unless_present`
  (`clap_builder-4.6.7/src/builder/debug_asserts.rs:201-206`, *"Argument {}: `required`
  conflicts with `required_unless_present`"*) — a panic in every debug build.
- The conditional requirement itself does **not** depend on that setting
  (`clap_builder-4.6.7/src/parser/validator.rs:274-311`): an `Option` field with
  `required_unless_present` still fails to parse when neither is given. Verified by the two new
  clap tests.

---

## Item 4 — the non-loopback admin endpoint contract → **the rule stays, the advice is fixed**

**Decision: keep the loopback-only rule.** It is the security property, not an accident: the
admin channel carries the **bearer token and the secret body** in cleartext HTTP
(`src/cli/ipc_client.rs`). The rule and the endpoint URL were fine; the *advice* was wrong.

Evidence:

- `src/cli/endpoint.rs:24` `build_endpoint_url` already maps the wildcards (`0.0.0.0` → `127.0.0.1`,
  `::` → `::1`); a concrete address is preserved verbatim.
- `src/cli/ipc_client.rs` `forward_to_server` refuses every non-loopback host — correct, and it is
  the only HTTP consumer of the endpoint file.
- `aleph-server stop` does **not** go through that HTTP channel (it signals the PID,
  `src/bin/aleph-server/daemon.rs`), so a LAN-bound server is still stoppable — the CLI's admin
  commands are what become unavailable, not the server's lifecycle.

What changed:

- `src/cli/ipc_client.rs` — the refusal message no longer says *"Bind the daemon to 127.0.0.1 or
  ::1 and update .ipc-endpoint.json"*: the daemon **owns and rewrites** that file on every start,
  so hand-editing it is the wrong instruction. It now says what to do (bind `127.0.0.1`, or a
  wildcard, which is published as `127.0.0.1`) and what not to do.
- `src/cli/endpoint.rs` — `build_endpoint_url`'s doc now states the consequence: a concrete
  address is preserved verbatim, but the CLI's admin channel refuses a non-loopback host, so a
  LAN-bound server has **no working local admin channel**.

---

## Files changed

| File | Change |
|---|---|
| `src/bin/aleph-server/cli.rs` | `Node { center: Option<String>, reset_identity: bool }` + 2 clap tests |
| `src/bin/aleph-server/main.rs` | dispatch the reset before any session; `Option` center |
| `src/bin/aleph-server/commands/node.rs` | `reset_identity`, `RefusalCode` (+`from_wire`, `recovery_hint`), the case-specific refusal log, 3 tests |
| `src/gateway/server/connection/mod.rs` | `reason_code` on the three terminal refusals |
| `src/cli/ipc_client.rs` | the non-loopback refusal message |
| `src/cli/endpoint.rs` | the endpoint-URL contract in the doc |
| `docs/reference/CLUSTER.md` | wire table, refusal semantics, the one-command recovery |
| `docs/guides/cluster.md` | a user-facing "Recover a revoked node" section (the guide is where an operator looks first) |

## Verification

- `cargo check -p alephcore --all-targets -j1` (memory-capped: `flock` on a lock file outside the
  repo, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`) —
  **EXIT=0, 0 errors, 25 warnings** (5m33s on a warm target dir). The 25 warnings are the same
  pre-existing dead-code warnings the previous round's baseline carried (`NoteMaintenanceOwner`,
  `spawn_note_index_rebuild`, …) — none is in a file this round touched.
- The check was re-run **after** the last source edit, so it covers the exact committed tree.
- **No `cargo test` and no clippy** were run: the new tests are compiled by `--all-targets` but
  not executed. The behavior they pin is the CLI parse (two clap tests) and the verdict mapping
  (three node tests).

## Deliberately NOT done

- **`scripts/spec_c_regression.sh` is untouched** (item 1 — its own round).
- **No credential rotation** and **no name-based credential** (item 3): both were considered and
  rejected above.
- **No store-level "the revoked name stays claimed" gate** (item 3): the current design intends a
  revoked node to be able to come back as a *new* node under the same name; the gate would break
  the documented recovery and would deny a legitimate re-enrolment.
- **No rename-persist** (item 2): the presented name is not persisted.
- **No change to the `reason` prose** the center already sent, and no new status vocabulary.
