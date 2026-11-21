# A2 — Severed-wire audit: `src/cluster/` (enrollment + registry)

- **Slice**: `src/cluster/mod.rs` (42 L), `src/cluster/enrollment.rs` (1586 L), `src/cluster/registry.rs` (1857 L) — read in full, including `#[cfg(test)] mod tests`.
- **Consumer side swept**: `src/gateway/handlers/cluster.rs`, `src/gateway/server/handler.rs`, `src/gateway/server/connection/{mod.rs,auth.rs,cleanup.rs}`, `src/gateway/handlers/auth/mod.rs`, `src/gateway/security/store/{devices.rs,users.rs}`, `src/gateway/security/device_token_manager.rs`, `src/gateway/security/crypto.rs`, `src/gateway/handlers/users.rs`, `src/builtin_tools/{node_manage.rs,node_invoke.rs,node_invoke_many.rs,node_list.rs,node_file.rs}`, `src/bin/aleph-server/commands/{node.rs,start/mod.rs,start/builder/subsystems.rs,start/builder/agent_init/mod.rs,start/builder/handlers/core.rs}`, `src/executor/builtin_registry/**`, `src/tools/adapters/registry_adapter.rs`, `interfaces/webchat/src/api/cluster.rs`, `interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs`, `tests/cluster_node_enrollment.rs`.
- **Method**: producer→consumer grep on both ends for every `pub` item, every struct field, every event topic, every tool/method name; then a read of each candidate's far end. No symbol is reported as a candidate unless the consumer side was grepped and read.
- **Read-only**: no source file was modified; no git write operation was performed.
- **Date**: 2026-11-20. **Prior review**: `review-results/sev-wire-2026-11-20-cli-cluster-command/PRIOR-FINDINGS.md`.

## Verdict summary

| Bucket | Count |
|---|---|
| New candidates (A2-01 … A2-06) | 6 |
| — of which Hypothesis **CUT** (wire severed, no live end) | 4 (A2-01, A2-02, A2-03, A2-04) |
| — of which Hypothesis **DECIDE** (wire present but semantics/ownership wrong) | 2 (A2-05, A2-06) |
| — of which Hypothesis **CONNECT** | 0 |
| Prior findings re-verified | 10 (#1, #2, #3, #6, #7, #8, #9, #12, #14, #18) |
| — FIXED | 6 (#2, #6, #7, #12, #14, #18) |
| — PARTIALLY FIXED | 3 (#1, #8, #9) |
| — REFUTED-as-bug (accepted design) | 1 (#3) |

The `src/cluster` module is in good shape: **every** registry state transition, every `NodeAdmission` / `EnrollError` / `DeregisterError` variant, every `ResolveError` / `EnvironmentStatus` variant, and all five cluster tool names have live, wired consumers. The six findings below are the residue.

---

# PART A — Prior-finding re-verification

### A2-P01 — Unbounded peer-declared metadata re-cloned on every `list_environments()` feeding `node_list`
- Form: 1 → now 3 (inert clone path)
- Producer: `src/cluster/registry.rs:354` — `NodeRegistry::list_environments` (clones the **full** catalog incl. schemas at `src/cluster/registry.rs:363`)
- Consumer: `src/gateway/handlers/cluster.rs:226` — `handle_environments_list` (`ctx.node_registry.list_environments()`); the model path now reads `src/cluster/registry.rs:382` `list_node_summaries` from `src/builtin_tools/node_list.rs:63`
- Live caller search: `rg -n 'list_environments' --glob '*.rs' src` → `src/gateway/handlers/cluster.rs:226` (production) + `src/gateway/server/handler.rs:687`, `src/builtin_tools/node_manage.rs:236`, `src/gateway/handlers/cluster.rs:360,378`, `src/gateway/server/mod.rs:1971` (tests) + `tests/cluster_node_enrollment.rs:176,239`. `rg -n 'list_node_summaries' --glob '*.rs' src` → `src/builtin_tools/node_list.rs:63` only.
- Symptom: the "model-callable `node_list` re-clones the whole catalog" half of the finding is gone: `node_list` now projects **names only** (`registry.rs:382-400`, doc at `:375-381` explains exactly this). The remaining deep clone is on the **operator** path (`environments.list`), which is called once per Panel refresh — bounded by `MAX_COMMAND_COUNT=64` / `MAX_COMMAND_SCHEMA_BYTES=8 KiB` (`registry.rs:843,845`, enforced in `bound_commands` `registry.rs:946-1008`), so a single node can contribute ≤ 512 KiB per call.
- Hypothesis: **DECIDE** — PARTIALLY FIXED. The unbounded-metadata half is fixed (every peer-declared field is now bounded: `registry.rs:840-845`, `bound_device_name:855`, `bound_tags:869`, `bound_commands:946`). The re-clone half survives only on the operator path, where it is now bounded rather than unbounded. Evidence that would settle it: whether the Panel needs the schemas at all (see **A2-02** — it does not read them).
- Confidence: high

### A2-P02 — `deregister_node` resolves through the registry's FUZZY name-substring tier before the precise store lookup
- Form: 5
- Producer: `src/cluster/enrollment.rs:825` — `resolve_deregister_target`
- Consumer: `src/cluster/enrollment.rs:750` — `deregister_node` (the only caller), reached from `src/gateway/handlers/cluster.rs:146` and `src/builtin_tools/node_manage.rs` (deregister action)
- Live caller search: `rg -n 'resolve_deregister_target|deregister_node' --glob '*.rs' src` → `enrollment.rs:750,825`, `handlers/cluster.rs:146`, `builtin_tools/node_manage.rs` (spawn_blocking), `mod.rs:32` (re-export).
- Symptom: none observed. The target resolution is now **precise first, fuzzy last**: `src/cluster/enrollment.rs:833-851` first calls `store.get_device(query)` (exact `device_id`, including revoked rows) and only falls through to `resolve_enrolled_node` (`enrollment.rs:677`, exact-id → normalized-name equality) and *then* to the registry's fuzzy tier (`registry.rs:426 match_id` → `registry.rs:516 resolve_id`). The fuzzy tier can therefore only fire for a query that matches **no** enrolled row at all.
- Hypothesis: **CONNECT** — FIXED. Regression test `enrollment.rs` `deregister_does_not_fuzzy_hit_a_live_neighbour` asserts exactly the scenario in the prior finding.
- Confidence: high

### A2-P03 — Node identity admitted with no proof of possession
- Form: 3 (accepted risk, documented)
- Producer: `src/cluster/enrollment.rs:367` — `admit_node` (accepts a peer-presented `device_id` as the credential) + `src/cluster/registry.rs:258` — `register_session`
- Consumer: `src/gateway/server/connection/mod.rs:1143` — the connect handler calls `admit_node` and, on `Admitted`, `maybe_register_node` at `:1194`
- Live caller search: `rg -n 'admit_node' --glob '*.rs' src` → `src/cluster/enrollment.rs:367`, `src/cluster/mod.rs:31` (re-export), `src/gateway/server/connection/mod.rs:1146` (production), `src/cluster/enrollment.rs` tests.
- Symptom: an unauthenticated LAN peer that knows an enrolled node's `device_id` is adopted into that node's row and receives its commands. This is the **documented** LAN-trust policy, not an oversight: `src/cluster/enrollment.rs:1-29` ("Under LAN trust (nodes hold no tokens) the id IS the credential — this is the policy, not an oversight") and `src/gateway/server/connection/auth.rs:344-350` ("LAN-trust cluster-node detection at `connect` time … a cluster node announces itself by request shape").
- Hypothesis: **CONNECT** — REFUTED as a *bug*; it is an accepted, documented trust model. The structural part of the finding ("register hands identity over → later dispatches deliver command bodies/file payloads to impostor") remains true by construction; the mitigation is that a node's `device_id` is a random v4 UUID minted by the center (`enrollment.rs:142 mint_node_device`) and never printed to the network (it travels only in the center→node connect reply, `src/gateway/server/connection/mod.rs:1231-1236`). Evidence that would settle it: a decision on whether LAN trust is still the intended product posture.
- Confidence: high (that it is intentional); medium (that the residual risk is acceptable)

### A2-P06 — `admit_node` unknown-id path backfills a row with the peer-supplied name without consulting the by-name match
- Form: 5
- Producer: `src/cluster/enrollment.rs:429-523` — the unknown-`device_id` branch of `admit_node`
- Consumer: `src/cluster/enrollment.rs:443` — `match_by_name` is consulted **before** any write: `Unique(existing) != presented_id` ⇒ `NodeAdmission::Admitted { node_id: existing, minted: true }` (converge onto the existing row) at `enrollment.rs:444-452`
- Live caller search: `rg -n 'match_by_name' --glob '*.rs' src` → `enrollment.rs:205` (def), `enrollment.rs:293` (`enroll_node_device`), `enrollment.rs:443` (`admit_node` unknown-id path), `enrollment.rs:563` (first-boot path).
- Symptom: none observed. The unknown-id path now **converges** onto the by-name row instead of creating a self-sealing duplicate. Two residual edges remain, both deliberate and documented: an *ambiguous* by-name match refuses to backfill (`enrollment.rs:465-477` — warns, admits the presented id with no row) and an *unaddressable* name refuses to write a row (`enrollment.rs:500-505`).
- Hypothesis: **CONNECT** — FIXED. Regression test `unknown_id_under_an_enrolled_name_converges_instead_of_duplicating` asserts the scenario.
- Confidence: high

### A2-P07 — `admit_node` adopts any active device row regardless of role; only `device_type="panel"` is refused
- Form: 3
- Producer: `src/cluster/enrollment.rs:367` — `admit_node`
- Consumer: `src/cluster/enrollment.rs:405-422` — two explicit refusals: `device_type == PANEL_DEVICE_TYPE` ⇒ `IdentityConflict` (`:405-412`), `row.role != "node"` ⇒ `IdentityConflict` (`:413-422`)
- Live caller search: `rg -n 'IdentityConflict' --glob '*.rs' src` → `enrollment.rs:78` (variant), `enrollment.rs:411,421` (raises), `src/gateway/server/connection/mod.rs:1272` (handled → `reason_code: "identity_conflict"`), `src/bin/aleph-server/commands/node.rs:190` (node-side `RefusalCode::IdentityConflict`).
- Symptom: none observed. A peer presenting a paired-Panel id or an operator device id is refused with a distinct, machine-readable code that the node client turns into an operator recovery hint.
- Hypothesis: **CONNECT** — FIXED. Regression test `node_claiming_a_non_node_role_is_refused`.
- Confidence: high

### A2-P08 — An empty/punctuation-only node name creates a row that can never be addressed by name
- Form: 3
- Producer: `src/cluster/enrollment.rs:237` — `unaddressable_name_reason` (rejects names with no letters/digits) + `src/cluster/enrollment.rs:293` — `enroll_node_device`
- Consumer: `src/cluster/enrollment.rs:205` — `match_by_name` (a name whose normalized key is empty can never match any row: `enrollment.rs:214-217` returns `NameMatch::None` for an empty key)
- Live caller search: `rg -n 'unaddressable_name_reason' --glob '*.rs' src` → `enrollment.rs:237` (def), `enrollment.rs:142` (`mint_node_device` refuses), `enrollment.rs:293` (`enroll_node_device` refuses), `enrollment.rs:500` (`admit_node` unknown-id path).
- Symptom: partially gone. The **ghost row** is fixed: `enroll_node_device` now refuses the name (`EnrollError::Unaddressable`) and `mint_node_device` refuses it too, so no unaddressable row is ever written by the enroll path. **Residual**: `admit_node`'s first-boot path handles `MintError::Unaddressable` by handing the node an **ephemeral UUID with no row at all** (`enrollment.rs:601-613`: `node_id: uuid::Uuid::new_v4().to_string(), minted: true`). Such a node is online and addressable **by id only**; it is not in the device store, so it does not appear in the offline half of `environments.list` (`src/gateway/handlers/cluster.rs:232-247` reads `store.list_devices()`), and `cluster.deregister` cannot revoke it by name (there is no row to revoke). The node is also told `persist: true` (`src/gateway/server/connection/mod.rs:1231-1236`), so it persists an id the center has no row for.
- Hypothesis: **DECIDE** — PARTIALLY FIXED. The unaddressable *row* is gone; an unaddressable *identity* is still minted, and it lives outside the store's reach. Evidence that would settle it: whether the center wants a row for such a node (it would need a name that is addressable by construction, e.g. derived from the id).
- Confidence: high

### A2-P09 — Name→row enrollment is SELECT-then-INSERT with no lock and no unique index on `device_name`
- Form: 3
- Producer: `src/cluster/enrollment.rs:293` — `enroll_node_device` (SELECT at `:305` via `match_by_name`, INSERT at `:318` via `mint_node_device`)
- Consumer: `src/cluster/enrollment.rs:61` — `static NODE_NAME_LOCK: Mutex<()>` (process-wide mutex taken at `enrollment.rs:302` before the SELECT and held across the INSERT)
- Live caller search: `rg -n 'NODE_NAME_LOCK' --glob '*.rs' src` → `enrollment.rs:61` (def), `enrollment.rs:302` (`enroll_node_device`), `enrollment.rs:442` (`admit_node` unknown-id path), `enrollment.rs:555` (`admit_node` first-boot path).
- Symptom: concurrent enrolls of the same name can no longer mint two rows **within one process** — the name→row resolution is serialized by `NODE_NAME_LOCK`, which the module doc (`enrollment.rs:61-70`) states is the OUTERMOST lock (name lock → store connection lock, leaf). Two edges remain: (a) there is still **no unique index** on `device_name` (`src/gateway/security/store/devices.rs:14 DEVICE_COLUMNS` shows `device_id` is the only key), so two *processes* sharing one store can still create duplicates; (b) `NODE_NAME_LOCK` does not guard the presented-id lookup in `admit_node` (`enrollment.rs:399 store.get_device(id)`), which is by design (that path is by id, not by name).
- Hypothesis: **DECIDE** — PARTIALLY FIXED. The in-process race is closed; the cross-process race and the missing unique index remain. Evidence that would settle it: whether more than one server process can share a `SecurityStore` file (the store is `Arc<SecurityStore>` over a SQLite file, so yes in principle).
- Confidence: high (in-process closed); medium (cross-process race real but unexercised)

### A2-P12 — Reverse-RPC response deadline taken at call start, so outbound-enqueue time is subtracted from the response window
- Form: 5
- Producer: `src/cluster/reverse_rpc.rs:315` (prior location) — the response deadline derivation in `ReverseRpcChannel`
- Consumer: `src/cluster/reverse_rpc.rs:333-364` — `outbound_deadline` is capped by `OUTBOUND_PUSH_BUDGET_MS`, and `response_deadline` is computed **after** the outbound enqueue returns
- Live caller search: `rg -n 'response_deadline|outbound_deadline|OUTBOUND_PUSH_BUDGET_MS' --glob '*.rs' src/cluster` → `reverse_rpc.rs:315` (call-start capture), `reverse_rpc.rs:333-364` (independent budgets).
- Symptom: none observed. The response window is now independent of the enqueue budget: the enqueue gets its own capped deadline, and the response deadline is anchored **after** the enqueue completes.
- Hypothesis: **CONNECT** — FIXED.
- Confidence: high

### A2-P14 — A malformed peer `commands` array collapses to an EMPTY catalog, which the invoke path reads as "the node is authoritative"
- Form: 2
- Producer: `src/cluster/registry.rs:667` — `maybe_register_node`; the `FrameField::Malformed` arm at `src/cluster/registry.rs:711` yields `(Vec::new(), catalog_malformed = true)`
- Consumer: `src/cluster/registry.rs:489` — `resolve_with_catalog` returns the `catalog_malformed` flag; the three call sites **fail closed** on it: `src/builtin_tools/node_invoke.rs:67-68` → check at `:81`, `src/builtin_tools/node_file.rs:76-77` → check at `:91`, `src/builtin_tools/node_invoke_many.rs:62`
- Live caller search: `rg -n 'catalog_malformed' --glob '*.rs' src` → `registry.rs:180,258,489-509,531-560,706-708` + the three builtin_tools call sites above.
- Symptom: none observed. A peer that sends a malformed `commands` array is registered with an empty catalog **and** the malformed flag; every invoke path then refuses with an explicit error instead of silently deferring to the node. `Absent` (no `commands` key at all) is deliberately kept distinct from `Malformed` (`registry.rs:792-802 FrameField`, `registry.rs:803-833 params_typed`).
- Hypothesis: **CONNECT** — FIXED. Regression test `maybe_register_node_distinguishes_a_malformed_catalog_from_an_empty_one`.
- Confidence: high

### A2-P18 — `DeregisterOutcome.device_removed` conflates "already revoked" with "the revoke write failed"
- Form: 3
- Producer: `src/cluster/enrollment.rs:887` — `revoke_node_device` → `Result<bool, DeregisterError>`
- Consumer: `src/cluster/enrollment.rs:763` — `let device_removed = revoke_node_device(store, &node_id)?;` → `src/cluster/enrollment.rs:793-797 DeregisterOutcome`
- Live caller search: `rg -n 'device_removed' --glob '*.rs' src` → `enrollment.rs:649` (field), `enrollment.rs:763,795` (producer), `src/gateway/handlers/cluster.rs:176` (RPC payload), `src/builtin_tools/node_manage.rs` (tool payload).
- Symptom: none observed. `revoke_node_device` now returns `Ok(false)` for "already revoked" (`enrollment.rs:895-905`) and `Ok(true)` for a fresh revoke (`enrollment.rs:893-894`), and turns a failed write into `Err(DeregisterError::StoreUnavailable(...))` (`enrollment.rs:906-917`, `:922-925`). The handler's own comment (`src/gateway/handlers/cluster.rs:153-158`) now states the corrected semantics, and an already-revoked repeat is logged as an idempotent repeat rather than warned about as "partial state".
- Hypothesis: **CONNECT** — FIXED. Regression test `deregister_reports_no_device_removal_for_an_already_revoked_row`.
- Confidence: high

---

# PART B — New severed wires

### A2-01 — The operator-initiated `node.disconnected` event is never published on the Panel/RPC leg
- Form: 2 (stub far-end: the publish arm exists and is documented as the operator-visible signal, but the bus it needs is never supplied in production)
- Producer: `src/cluster/enrollment.rs:773-791` — `deregister_node`'s publish arm (`if let (Some(session), Some(bus)) = (evicted_session, event_bus)` at `:773`, `bus.publish_json(&TopicEvent::new("node.disconnected", …))` at `:775-780`)
- Consumer: `src/gateway/handlers/cluster.rs:139` — `let event_bus = ctx.event_bus.clone();` → passed as `event_bus.as_deref()` at `src/gateway/handlers/cluster.rs:146`. `ctx.event_bus` is `src/gateway/handlers/auth/mod.rs:55` — `pub event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>`
- Live caller search:
  - `rg -n 'AuthContext \{' --glob '*.rs' src` → exactly **two** hits: `src/bin/aleph-server/commands/start/builder/subsystems.rs:252` (the only production construction) and `src/gateway/handlers/auth/mod.rs:76` (the `#[cfg(test)]` helper `create_test_context`).
  - `read src/bin/aleph-server/commands/start/builder/subsystems.rs:252-257` → `auth_ctx: Arc::new(auth_handlers::AuthContext { shared_token_mgr, security_store: auth_security_store, node_registry, event_bus: None })` — **`event_bus: None` at line 256 in the fully-booted production server.**
  - `rg -n 'event_bus' --glob '*.rs' src/bin/aleph-server/commands/start` → only that one hit.
  - The chain is unbroken otherwise: `src/bin/aleph-server/commands/start/mod.rs:685` calls `register_core_handlers(&mut server, &auth_bundle.auth_ctx, …)` unconditionally in the production start path; `src/bin/aleph-server/commands/start/builder/handlers/core.rs:32-42` registers `cluster.enroll` / `cluster.deregister` / `environments.list` with exactly that `auth_ctx`.
  - `rg -n 'node.disconnected' --glob '*.rs' src` → `enrollment.rs:777` (this producer), `src/gateway/server/connection/cleanup.rs:97` (the socket-drop producer — **not** affected: `cleanup.rs` uses `ctx.event_bus` directly, which is non-`Option`), `src/gateway/event_scope.rs:504,505,564,586` (scope rules), `src/gateway/source_census.rs:440,442` (source-text census).
  - The **model** leg is wired correctly: `src/executor/builtin_registry/registry/tool_registry_impl.rs:523-526` passes `self.gateway_event_bus.get().map(Arc::clone)`, and the bus is injected at `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:761` (`tool_registry.set_gateway_event_bus(event_bus.clone())`).
- Symptom: an operator who deregisters a node from the Panel (`cluster.deregister` RPC) causes **no** `node.disconnected` event at all. The socket-drop arm cannot cover for it either: `registry.forget()` (`src/cluster/enrollment.rs:764` → `src/cluster/registry.rs:570`) removes the `conn_id → node_id` mapping, so when the node's socket later closes, `src/gateway/server/connection/cleanup.rs:88` (`ctx.node_registry.deregister(conn_id)`) returns `None` and the drop arm's publish at `cleanup.rs:93-98` is skipped too. **Mitigating**: the Panel reloads the fleet list itself right after the RPC returns (`interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:350` — `load();` inside the `spawn_local` after `ClusterApi::deregister_node`), so the user-visible symptom the producer's own doc predicts ("the operator's Panel keeps showing the node online until a hard refresh", `src/cluster/enrollment.rs:768-772`) does **not** actually materialize — the event's only subscriber (`state.subscribe_topic("node.**")`, `interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:179`) already had its refresh forced by the RPC result. The wire is severed but currently harmless.
- Three stale justifications compound it: (a) `src/gateway/handlers/auth/mod.rs:51-53` says the bus is "Absent in probe-mode / partially-booted processes" — false, it is absent in the **fully-booted production server too**; (b) the same doc block's preamble (`src/gateway/handlers/auth/mod.rs:41-47`) claims "Every field is read by a live handler" — `event_bus` is *read* but never *set* to a live value; (c) the **model** leg's own comment (`src/executor/builtin_registry/registry/tool_registry_impl.rs:519-522`) asserts "The bus is the SAME one the RPC leg publishes on (`node.disconnected`), so a deregistration driven by the model announces itself exactly like one driven by the operator" — that equivalence is precisely what does not hold. The census at `src/gateway/source_census.rs:442` lists `("src/cluster/enrollment.rs", "node.disconnected")` as a producer; because the census is a **source-text** scan, that entry stays non-vacuous even though this producer never fires in production.
- Hypothesis: **CUT** — the RPC leg has no bus, so the publish arm is unreachable there; the model leg is connected. The right fix is to hand the production `AuthContext` the same `Arc<GatewayEventBus>` the tool registry already gets (`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:761`), not to delete the arm.
- Confidence: high

### A2-02 — `CommandDescriptor.schema` is produced, stored, cloned and serialized — and read by nobody
- Form: 3 (inert payload / stale contract)
- Producer: `src/cluster/registry.rs:30` — `pub struct CommandDescriptor { pub name: String, pub schema: Value }`; the value is peer-declared in the connect frame (`src/cluster/registry.rs:698-712`), bounded at `src/cluster/registry.rs:946-1008 bound_commands` (≤ 64 commands × ≤ 8 KiB, oversized/unserializable ⇒ `serde_json::json!({})` at `:1002`), stored in `NodeSession.declared_commands` (`src/cluster/registry.rs:49`), deep-cloned into `Environment.commands` (`src/cluster/registry.rs:363`), and serialized into the `environments.list` payload (`src/gateway/handlers/cluster.rs:270`)
- Consumer: **none for the value.** The only Rust consumer of the *field* is `src/cluster/registry.rs:973` (`serde_json::to_writer(&mut sink, &command.schema)` — the bound measurement itself) and `src/cluster/registry.rs:991,1002` (the fallback). The Panel's mirror struct declares `pub schema: Value` (`interfaces/webchat/src/api/cluster.rs:18`) and **never reads it**; the Panel view reads only `node.commands.len()` (`interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:260`).
- Live caller search: `rg -n '\.schema' --glob '*.rs' src` → only `src/cluster/registry.rs:973,991,1002` plus two test assertions (`registry.rs:1794,1801`); every other `.schema` hit in `src/` is an unrelated type (`src/tools/**`, `src/capability/**`, `src/config/**`, `src/hub/**`). `rg -n 'schema' interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs` → no hits.
- Symptom: the center pays for up to 512 KiB of peer-declared JSON per node per `environments.list` call and ships it to every operator client, for a payload nobody reads. The node's own declaration is a placeholder anyway — every shipped node command declares `{"type":"object"}` with no properties (`src/cluster/node_runtime.rs:148-151`, `:175-178`).
- Stale docs on both ends: `src/cluster/registry.rs:22` says the schema is "surfaced in `environments.list` / **`node_list`**" — the `node_list` half is false (that path projects names only, `src/cluster/registry.rs:382-400`); `src/cluster/registry.rs:350-352` says the operator path's "consumer may read the schemas" — no consumer does; `src/cluster/registry.rs:48` says "the center only stores and **displays** it" — the Panel counts, it does not display.
- Hypothesis: **CUT** — a value-level wire with no consumer end. Either the schemas should be consumed (Panel renders them) or the field should stop being cloned into `environments.list`.
- Confidence: high

### A2-03 — `Environment.connected_at` is produced by both halves of `environments.list` and read by nobody
- Form: 3 (inert field)
- Producer: `src/cluster/registry.rs:365` — `connected_at: n.session.connected_at` (online nodes) and `src/gateway/handlers/cluster.rs:241` — `connected_at: 0` (offline nodes)
- Consumer: **none.** The Panel's mirror struct declares `pub connected_at: i64` (`interfaces/webchat/src/api/cluster.rs:36`) and never reads it; the Panel view reads `id`, `name`, `status`, `tags`, `last_seen_at`, `version` and `commands.len()` only (`interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:253-271`). The model path has its own, *different*, field: `NodeSummary.connected_at` (`src/cluster/registry.rs:147`) is read at `src/builtin_tools/node_list.rs:73` — but that is `node_list`, not `environments.list`.
- Live caller search: `rg -n 'connected_at' --glob '*.rs' src` → `src/cluster/registry.rs:100` (field), `:365` (online producer), `:397` (`NodeSummary` producer, **read** at `src/builtin_tools/node_list.rs:73`), `src/gateway/handlers/cluster.rs:241` (offline producer); `rg -n 'connected_at' --glob '*.rs' interfaces/webchat` → `interfaces/webchat/src/api/cluster.rs:36` (declaration only).
- Symptom: an operator reading `environments.list` sees `connected_at: 0` for every offline node and a real epoch for every online one, and nothing ever displays it. `0` is also indistinguishable from the Unix epoch, so if a consumer is added later it cannot tell "never connected" from "connected in 1970".
- Hypothesis: **CUT** — a field with a producer and no consumer.
- Confidence: high

### A2-04 — The `devices.fingerprint` value written for node rows is in a different namespace than the type that owns the column, and its only reader has zero production callers
- Form: 3 + 5 (inert column, and a derivation drift)
- Producer: `src/cluster/enrollment.rs:146-147` — `mint_node_device` writes `fingerprint: truncate_on_char_boundary(&device_id, 16)`; `src/cluster/enrollment.rs:508-509` — the `admit_node` backfill writes the same derivation
- Consumer: `src/gateway/security/store/devices.rs:54-63` — `SecurityStore::get_device_by_fingerprint`, whose **only** caller in the whole tree is the unit test `src/gateway/security/store/tests.rs:394`
- Live caller search: `rg -n 'get_device_by_fingerprint' --glob '*.rs' src` → `src/gateway/security/store/devices.rs:11` (a doc comment), `:54` (the definition), `src/gateway/security/store/tests.rs:394` (a test). No production caller.
- Symptom: two silent drifts. (a) **Inert column**: the fingerprint written for every node row is never read by production code — node admission resolves by `device_id` (`src/cluster/enrollment.rs:399 store.get_device(id)`), not by fingerprint. (b) **Derivation drift**: the column's documented meaning is "first 16 hex characters of `SHA256(public_key)`" (`src/gateway/security/crypto.rs:35`, `DeviceFingerprint::from_public_key` at `src/gateway/security/crypto.rs:41-48`), but the node path stores a **truncated device id** instead — and it has to, because the node's `public_key` is the constant `[0u8; 32]` (`src/cluster/enrollment.rs:150`), whose real fingerprint would be identical for every node in the fleet. A future reader of the column that trusts the `DeviceFingerprint` type would silently mis-key every node row. The stale comment at `src/gateway/security/store/devices.rs:11` ("`get_device_by_fingerprint` (every remote connect)") advertises a production read path that no longer exists.
- Hypothesis: **DECIDE** — the column is inert **and** its value violates the namespace of the type that claims it. Either the node path should store a real fingerprint (needs a per-node key) or the column should be documented as node-id-derived for `role="node"` rows.
- Confidence: high (inert); high (derivation differs from `crypto.rs:35`)

### A2-05 — Tags are peer-declared at connect time, but the Panel's contract documents them as operator-assigned labels
- Form: 5 (name/semantics drift across the boundary)
- Producer: `src/cluster/registry.rs:713-720` — `maybe_register_node` reads `params["tags"]` **from the peer's connect frame** (`FrameField::Present(tags) => bound_tags(tags, device_id)`), stores them in `NodeSession.tags` (`src/cluster/registry.rs:50`), and exposes them via `NodeSummary.tags` (`src/cluster/registry.rs:147`) and `Environment.tags` (`src/cluster/registry.rs:99`)
- Consumer: `src/builtin_tools/node_invoke_many.rs:106-110` — `resolve_all_by_tags` (`src/cluster/registry.rs:531`, AND-match at `:536`) selects the fan-out targets by tag; `src/builtin_tools/node_list.rs` surfaces the tags to the model; the Panel renders them
- Live caller search: `rg -n 'tags' --glob '*.rs' src/cluster/registry.rs` → `:50,99,116,124,147,364,397,521-542,713-720,765,803,838,868` (producer + projections); `rg -n 'Operator-assigned' --glob '*.rs' interfaces/webchat` → `interfaces/webchat/src/api/cluster.rs:30` (the consumer-side doc).
- Symptom: the Panel's API doc states the tags are "**Operator-assigned labels.** These are what `node_invoke_many` selects on" (`interfaces/webchat/src/api/cluster.rs:30-33`), which implies a trust property the implementation does not have: any LAN peer that can complete a connect frame chooses its own tags (`src/cluster/registry.rs:713-720`), so it can claim an operator's fan-out tag and be picked up by a `node_invoke_many` broadcast that was meant for a different machine. A second, related gap: tags are **never persisted**, so the offline half of `environments.list` always reports `tags: Vec::new()` (`src/gateway/handlers/cluster.rs:240`) — an offline node's Panel row loses its tags, which the Panel doc promises it will show.
- Hypothesis: **DECIDE** — the wire is connected, but the *ownership* of the key that drives fan-out selection is misdocumented across the boundary. Either the doc must say "peer-declared" (and the fan-out must be treated as untrusted), or the center must own the tag set.
- Confidence: medium-high (the code path is unambiguous; the severity depends on whether operators treat tags as a trust boundary)

### A2-06 — The `node.connected` event carries the RAW peer `device_name`, while the registry and the `node.disconnected` event carry the BOUNDED one
- Form: 5 (value drift between two ends of the same event pair)
- Producer: `src/gateway/server/connection/mod.rs:1203-1206` — `ctx.event_bus.publish_json(&TopicEvent::new("node.connected", serde_json::json!({"node_id": node_id, "name": &claim.device_name, "conn_id": &conn_id})))`, where `claim.device_name` is the **raw, unbounded** peer string from `src/gateway/server/connection/auth.rs:358-364` (`p.get("device_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()`)
- Consumer: the registry's copy is bounded — `src/cluster/registry.rs:681` `bound_device_name(params_str(params, "device_name") …, device_id)` (≤ `MAX_DEVICE_NAME_BYTES = 128`, `src/cluster/registry.rs:840`); the *other* end of the same event pair is bounded too — `src/cluster/enrollment.rs:778` publishes `session.device_name` and `src/gateway/server/connection/cleanup.rs:95` publishes `session.device_name`
- Live caller search: `rg -n 'device_name' --glob '*.rs' src/gateway/server/connection` → `auth.rs:358-364` (raw parse), `connection/mod.rs:1206` (raw publish), `cleanup.rs:89` (bounded publish); `rg -n 'bound_device_name' --glob '*.rs' src` → `src/cluster/registry.rs:681,855`, `src/cluster/enrollment.rs:293,396` (the bound is applied to the store/registry copies only).
- Symptom: the two ends of the `node.connected` / `node.disconnected` event pair disagree on the literal `name` value for the same node — the connect event carries the unbounded peer string, the disconnect event carries the ≤ 128-byte bounded one. The magnitude is bounded only by the transport: the **main** gateway WebSocket route sets no `max_message_size` (`rg -n 'max_message_size|max_frame_size' --glob '*.rs' src` → the only hit in the tree is `src/gateway/server/browser_viewer_route.rs:360`, an 8 KiB cap on the *browser viewer* route), so a peer can push a multi-megabyte `device_name` and have it fanned out verbatim to every admin subscriber of `node.**`.
- Hypothesis: **DECIDE** — the wire exists but the two ends disagree on the value; the fix is to publish the bounded name (`ctx.node_registry` already holds it) rather than the raw claim.
- Confidence: high on the drift; medium on the impact (depends on the effective WS frame limit in the deployed proxy)

---

# Swept and found CONNECTED (negative results)

These are recorded so a later reader does not re-open them.

- **Registry state machine** — every transition has a live production caller:
  - `register_session` (`src/cluster/registry.rs:258`) ← `maybe_register_node` (`src/cluster/registry.rs:667`) ← `src/gateway/server/connection/mod.rs:1194`.
  - `register` (`src/cluster/registry.rs:225`) is `#[cfg(test)]`-only (a thin wrapper) — production flows through `maybe_register_node`, as the doc at `src/cluster/registry.rs:215-225` states.
  - `resolve_with_catalog` (`src/cluster/registry.rs:489`) ← `src/builtin_tools/node_invoke.rs:67-68`, `src/builtin_tools/node_file.rs:76-77`.
  - `resolve_id` (`src/cluster/registry.rs:516`, `pub(crate)`) ← `src/cluster/enrollment.rs:825-887` (the only caller — the `pub(crate)` is correctly scoped).
  - `resolve_all_by_tags` (`src/cluster/registry.rs:531`) ← `src/builtin_tools/node_invoke_many.rs:106,110`.
  - `list_environments` (`src/cluster/registry.rs:354`) ← `src/gateway/handlers/cluster.rs:226`; `list_node_summaries` (`src/cluster/registry.rs:382`) ← `src/builtin_tools/node_list.rs:63`.
  - `deregister` (`src/cluster/registry.rs:329`) ← `src/gateway/server/connection/cleanup.rs:88`.
  - `forget` (`src/cluster/registry.rs:570`) ← `src/cluster/enrollment.rs:764` (single caller, inside `deregister_node`).
  - `node_identity_by_conn` (`src/cluster/registry.rs:410`) ← `src/gateway/server/connection/mod.rs:374` (the node-approval path, which deliberately takes identity from the connection, not from params).
  - There is **no** `update` method on `NodeRegistry` — the brief's mention of one does not correspond to any symbol in `src/cluster/registry.rs`.
- **Variants** — every produced variant is handled by a live consumer:
  - `NodeAdmission` (`src/cluster/enrollment.rs:74`): all five arms handled at `src/gateway/server/connection/mod.rs:1195` (Admitted), `:1247` (Deregistered → `reason_code: "revoked"`), `:1273` (IdentityConflict → `"identity_conflict"`), `:1292` (Ambiguous → `"ambiguous_name"`, payload deliberately omits `node_id`), `:1316` (StoreUnavailable → **no** payload, so the node retries). The node client parses all four codes at `src/bin/aleph-server/commands/node.rs:186-193` and turns them into operator recovery hints (`node.rs:199-217`).
  - `EnrollError` (`src/cluster/enrollment.rs:253`): all three arms mapped at `src/gateway/handlers/cluster.rs:51-90`.
  - `DeregisterError` (`src/cluster/enrollment.rs:622`): all three arms mapped at `src/gateway/handlers/cluster.rs:180-210` and in `src/builtin_tools/node_manage.rs`.
  - `ResolveError` (`src/cluster/registry.rs:64`): `NotFound` / `Ambiguous` both produced by `match_id` (`src/cluster/registry.rs:426-487`) and both surfaced to the model by `src/builtin_tools/node_invoke.rs` / `node_invoke_many.rs`.
  - `EnvironmentStatus` (`src/cluster/registry.rs:79`): `Online` produced at `src/cluster/registry.rs:362`, `Offline` at `src/gateway/handlers/cluster.rs:238`; both matched by the sort rank at `src/gateway/handlers/cluster.rs:262-264`; the wire spelling is `"online"`/`"offline"` (`serde(rename_all = "lowercase")`) and the Panel compares plain strings.
- **CommandDescriptor — the four ends agree on the literal names**:
  1. node declares — `CommandTable::descriptors` (`src/cluster/node_runtime.rs:99-107`, sorted by `(name, key)`);
  2. node allowlist — `CommandTable::register` (`src/cluster/node_runtime.rs:59-90`) now **forces the table key to equal `cmd.descriptor().name`** and logs an error on mismatch, so the two namespaces cannot diverge (this also closes prior findings #10 and #22);
  3. center admits — `maybe_register_node` → `params_typed::<Vec<CommandDescriptor>>(params, "commands", …)` (`src/cluster/registry.rs:698-712`);
  4. center invokes — `src/builtin_tools/node_invoke.rs` / `node_invoke_many.rs` compare `c.name == args.command` against the admitted catalog.
  Literal names agree across ends: `"bash"` (`src/cluster/node_runtime.rs:148`, registered at `node_runtime.rs:163`), `"file.read"` / `"file.write"` (`src/cluster/node_runtime.rs:176-180`). The node's connect frame sends exactly these keys (`src/bin/aleph-server/commands/node.rs:527-534`: `device_name`, `commands`, `tags`, `version`, plus `device_id` when persisted), and the center reads the same keys (`src/cluster/registry.rs:681,700,715,723`).
  The **empty-catalog** case cannot silently disable the center's fail-fast layer: a *malformed* catalog sets `catalog_malformed = true` and every invoke path refuses (`node_invoke.rs:81`, `node_file.rs:91`, `node_invoke_many.rs:62`); an *empty* catalog means the node declared no commands, and the node-side `CommandTable` remains the authoritative allowlist (`src/cluster/node_runtime.rs:117-133 dispatch` → `format!("command '{tool}' not permitted on this node")`).
- **Tool-name parity**: `node_list`, `node_invoke`, `node_file`, `node_invoke_many`, `node_manage` — catalog entries `src/executor/builtin_registry/definitions.rs:293,304`, dispatch arms `src/executor/builtin_registry/registry/tool_registry_impl.rs:471,481,491,501,511`, group `src/executor/builtin_registry/groups.rs:323`. All five names agree on both ends.
- **RPC-method parity**: `cluster.enroll`, `cluster.deregister`, `environments.list` — registered `src/bin/aleph-server/commands/start/builder/handlers/core.rs:24-44` (`register_handler!`), wired `src/bin/aleph-server/commands/start/mod.rs:685`; classified as `Class::Admin` at `src/gateway/method_census.rs:151-152`; prefix-gated at `src/gateway/method_admin.rs:121,135`. The Panel's payload keys (`node_name` for enroll, `node` for deregister) match the handler's `DeregisterParams` (`src/gateway/handlers/cluster.rs:120`).
- **Event emit-vs-subscribe parity**: `node.connected` (`src/gateway/server/connection/mod.rs:1205`) and `node.disconnected` (`src/cluster/enrollment.rs:777`, `src/gateway/server/connection/cleanup.rs:97`) are subscribed by the Panel via `state.subscribe_topic("node.**")` (`interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:179`) and are refused to non-operators (`src/gateway/event_scope.rs:504-505,564,586`). The one gap is the RPC-leg publish (see **A2-01**).
- **DB-column-reader parity**: `src/gateway/security/store/devices.rs:14 DEVICE_COLUMNS` is shared by three SELECTs; `upsert_device` (`:33-58`) refreshes `device_name`/`last_seen_at`/`revoked_at`/`user_id` on conflict and does **not** touch `role`/`scopes`/`fingerprint`/`device_type`/`public_key` — consistent with the enrollment path's intent. `get_device` (`:86-94`) includes revoked rows (needed by `admit_node`); `list_devices` (`:96-107`) filters `revoked_at IS NULL`, so revoked nodes correctly drop out of the offline half of `environments.list`. `revoke_device` (`:120-127`) guards on `revoked_at IS NULL`, which is what makes the "already revoked ⇒ `Ok(false)`" branch in `revoke_node_device` reachable.
- **Unit parity**: `last_seen_at` is stored in **milliseconds** (`src/gateway/security/store/devices.rs:110-118 touch_device` → `current_timestamp_ms`) and converted to Unix seconds at the boundary (`src/gateway/handlers/cluster.rs:243` `d.last_seen_at.map(|ms| ms / 1000)`), matching the `Environment` contract.
- **Not findings** (deliberately not reported): `pub use reverse_rpc::ReverseRpcError` (`src/cluster/mod.rs:42`) has no external *named* consumer, but it is the error type of a live public API — not a severed wire. `MAX_DEVICE_NAME_BYTES` (`src/cluster/registry.rs:840`) is `pub(crate)` yet used only inside `registry.rs` — a trivial over-export, not a severed wire. The `role` gate in `maybe_register_node` (`src/cluster/registry.rs:675-677`) is dead in production by design and says so (`src/cluster/registry.rs:660-666`).

---

# Seams NOT covered (explicit)

1. **`src/cluster/reverse_rpc.rs`** — not in this slice's file list. Prior finding **#12** was re-verified by reading the deadline derivation at `src/cluster/reverse_rpc.rs:315,333-364`, but the transport itself (enqueue, wedging, `#21`) was **not** audited.
2. **`src/cluster/node_runtime.rs`, `node_file_cmd.rs`, `node_approval.rs`** — read only far enough to verify the CommandDescriptor four ends and prior findings #10/#11/#22/#4/#16/#19. The file-command jail implementation (`node_file_cmd.rs`) was not audited.
3. **`src/gateway/security/store/{devices.rs,users.rs}`** and **`device_token_manager.rs`** — read the device path only; the **user** store, the token manager and the Panel/operator pairing flow were not audited. Whether a *panel* device row can collide with a node row (the `device_type == PANEL_DEVICE_TYPE` refusal at `src/cluster/enrollment.rs:405-412`) was not tested against the pairing code.
4. **The WebSocket transport limits** — the main gateway route's effective frame-size limit was not traced through the axum/tungstenite stack; `A2-06`'s magnitude estimate therefore stays at "medium" confidence.
5. **The `node.**` event fan-out to non-Panel clients** — the delivery queue's own payload cap (`src/gateway/server/delivery_queue.rs:123 max_payload_bytes = 1 MiB`) was not traced for the `node.connected` path; a >1 MiB peer name may be dead-lettered rather than delivered, which would change **A2-06**'s symptom from "fanned out" to "dead-lettered with an error the operator never sees". Not resolved.
6. **`src/gateway/event_scope.rs` / `source_census.rs` rules for `node.*`** — read as consumers; their internal derivation was not audited.
7. **`interfaces/webchat`** — only the Rust (Leptos) side was read. The TypeScript/webchat client and the Panel's `environments.list` rendering were not audited.
8. **Cross-process concurrency** — the SQLite store's locking under two `aleph-server` processes sharing one file (relevant to **A2-P09**) was not exercised.
9. **`tests/cluster_node_enrollment.rs`, `tests/cluster_node_runtime.rs`, `tests/cluster_reverse_rpc.rs`, `tests/cluster_node_approval.rs`** — grepped for consumer evidence only; their assertions were not re-derived.
10. **Runtime behaviour** — no server was booted; every claim above is from source reading and greps. The one behavioural claim I could not source statically is the WS frame-size limit (see 4).
