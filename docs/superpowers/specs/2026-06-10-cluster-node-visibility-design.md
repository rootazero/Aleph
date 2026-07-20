# Cluster Node Visibility — Design (2026-06-10)

## Context

Gap analysis round on the gateway cluster, referencing openclaw / hermes-agent /
opensquilla. Findings:

- hermes-agent and opensquilla have **no** multi-machine federation (hermes
  "delegate" is an in-process ThreadPool; opensquilla has scope/role stubs
  only). openclaw remains the sole real reference.
- Aleph's cluster core (reverse RPC, registry, pairing, approval, tags,
  fan-out, fail-fast) is at/above openclaw parity. The true gaps were all
  **visibility/wiring**, not protocol.

## Confirmed defects & gaps

1. **BUG — `node_invoke_many` invisible to the LLM and Panel.** The previous
   round registered it in `BuiltinToolRegistry` (schema) and the dispatch match
   only. The LLM tool list is built from `BUILTIN_TOOL_DEFINITIONS`
   (`agent_init/mod.rs`) and the Panel tool-config page from `TOOL_CATEGORIES`
   — it was in neither, so the model never saw the tool.
2. **Dead column — `devices.last_seen_at` never stamped.**
   `SecurityStore::touch_device` had zero callers; node devices carried
   `last_seen_at = NULL` forever after enroll.
3. **LLM cannot discover nodes.** The three node tools' descriptions said
   "see `environments.list`" — a Panel-facing gateway RPC the model cannot
   call. The discover half of the discover→invoke loop was missing (same bug
   class as the `list_models` round: metadata reached the UI, never the LLM).
4. **Enrolled-but-offline nodes invisible.** `environments.list` projected
   only live registry sessions; an offline node vanished entirely (openclaw
   `nodes status` merges persistent pairing state with live state).

## Changes

| # | Change | Files |
|---|--------|-------|
| 1 | Advertise `node_invoke_many`: definitions entry, `create_tool_boxed` arm, `cluster` group | `executor/builtin_registry/{definitions,groups}.rs` |
| 2 | `TokenManager::touch_device` thin delegate; stamp node `last_seen_at` on connect and disconnect seams | `gateway/security/token.rs`, `gateway/server/handler.rs` |
| 3 | New `node_list` builtin tool (online projection: id/name/commands/tags/connected_at, optional AND tag filter to preview fan-outs); fixed the three node tools' descriptions to reference `node_list` | `builtin_tools/node_list.rs` (new), `builtin_tools/{mod,node_invoke,node_invoke_many,node_file}.rs`, full registration chain |
| 4 | `environments.list` merges enrolled-but-offline node devices (`status:"offline"`, `last_seen_at` in Unix seconds, `null` = never connected); `Environment` gains `last_seen_at: Option<i64>`; `cluster.deregister` falls back to the device store so visible offline nodes are deregisterable (exact id / unique exact name; `evicted:false`) | `cluster/registry.rs`, `gateway/handlers/cluster.rs` |

## Deliberately NOT done (YAGNI / R3 / R10)

- **No RTT/ping probe protocol** (openclaw `checkConnectivity`): would require a
  new node-side wire method + node binary upgrade; `node_invoke` with a trivial
  bash echo already probes liveness end-to-end.
- **No offline queue / idempotency keys / durable invoke buffer**: zero
  consumers; cluster semantics are deliberately fail-fast on disconnect.
- **No SecurityStore injection into the tool registry**: `node_list` is
  registry-only (online). Offline fleet view belongs to the Panel
  (`environments.list`); the LLM only needs actionable targets.
- **No Panel UI changes** (it tolerates the new field/status via serde
  defaults; rendering "offline + last seen" rows nicely is deferred).
- Offline fallback resolve in `cluster.deregister` is exact-match only — the
  online multi-tier fuzzy semantics are not duplicated against the store.

## Invariants preserved

- `environments.list` stays credential-free (R4); offline merge degrades
  gracefully to online-only if the store read fails (P7).
- Tags select, never authorize (R7). `node_list` is a pure read projection.
- No harness changes (R10).
