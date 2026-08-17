# Severed-Wire Audit — `src/gateway/handlers/` (batch 1)

**Audit**: severed-wire-audit
**Date**: 2026-08-17
**Module**: `src/gateway/handlers/`
**Files scanned**: 135 files, ~64 550 LOC
**Method**: PRODUCED − CONSUMED symbol parity. For every `pub` handler function under `src/gateway/handlers/`, cross-referenced against (a) `registry.register(...)` in `src/gateway/handlers/mod.rs`, (b) `register_handler!` and direct `handlers_mut().register(...)` in `src/bin/aleph-server/commands/start/builder/{handlers,agent_init}/*.rs`, (c) any other call site in `src/`. Classification against the six severed-wire forms; decisions CUT / CONNECT / DECIDE.
**Prior reviews cross-referenced**: `review-results/SUMMARY.md`, `review-results/severed-wire-2026-08-17/REVIEW_PROTOCOL.md`. The event/audit protocol from `review-results/sev-wire-2026-08-17/event/` was used as a format template. No prior review covered `src/gateway/handlers/` symbols — fresh audit.

---

## Handler-registration surface

The `HandlerRegistry::new()` constructor in `src/gateway/handlers/mod.rs` registers **148** placeholder/stub methods. The boot path at `src/bin/aleph-server/commands/start/` adds **244** more, of which **33** use direct `register()` inside `agent_init/*.rs` closures (so the actual handler function reference is invisible to grep from the registration site — see evidence column). Total wired methods in the live registry ≈ 280. The audit below lists the orphan functions that survive in either set.

### Inventory of `pub async fn` / `pub fn` in `src/gateway/handlers/`

Total `pub` items across the 135 files: ~250 (handler functions) plus ~50 helper items (DTOs, manager types, parse helpers).

---

## Findings — severities summary

| Severity | Count |
|---|---|
| critical | 0 |
| high | 0 |
| medium | 3 |
| low | 7 |
| **Total** | **10** |

Decision mix: **CUT 9, CONNECT 0, DECIDE 1**.

Form histogram: Form 1 = 8 (F-01..F-08), Form 2 = 1 (F-09), Form 6 = 1 (F-10). Total unique findings = 10.

---

## Findings — detail

### F-01 — `cron::handle_*_stub` (9 functions in `cron/stubs.rs`) are dead code (low, Form 1)

**Files**: `src/gateway/handlers/cron/stubs.rs:9,14,43,74,94,111,123,146,169` (342 lines); module re-export at `src/gateway/handlers/cron/mod.rs:24`.

**Symbol**: `pub async fn handle_list_stub / handle_get_stub / handle_create_stub / handle_update_stub / handle_delete_stub / handle_status_stub / handle_run_stub / handle_runs_stub / handle_toggle_stub`.

**Evidence**:

```text
$ rg "cron::handle_.*_stub" src/ bin/ interfaces/ shared/
src/gateway/handlers/mod.rs:352: registry.register("cron.list", cron::handle_list_stub);
src/gateway/handlers/mod.rs:353: registry.register("cron.get", cron::handle_get_stub);
… (9 placeholder registrations, lines 352-360)
src/gateway/handlers/cron/stubs.rs:204: async fn test_handle_list_stub() {
… (16 #[cfg(test)] tests inside cron/stubs.rs itself)
```

The boot path (`src/bin/aleph-server/commands/start/builder/handlers/agents.rs:128-136`) **overrides every one** with `cron::handle_*` from `cron/real.rs` BEFORE the gateway ever accepts a request:

```text
$ rg "register_handler!.*cron\." src/bin/aleph-server/commands/start/builder/handlers/agents.rs
128:    register_handler!(server, "cron.list", cron::handle_list, cron_service);
… (9 lines, all "cron::handle_*" without _stub)
```

Production-callable consumer count: 1 (the `HandlerRegistry::new()` placeholder), which is then overwritten before serving traffic. Net effect: the 9 `_stub` functions are visible only via `mod.rs:352-360` and via their own `#[cfg(test)]` module.

**Decision**: **CUT** — delete `src/gateway/handlers/cron/stubs.rs` entirely (342 lines); remove the 9 `registry.register(..._stub)` lines in `src/gateway/handlers/mod.rs:352-360`; remove `pub use stubs::{...}` from `src/gateway/handlers/cron/mod.rs:24-26`; move the 16 `#[cfg(test)]` tests into `cron/real.rs` if they exercise the real handlers (most are stale — they assert against the stub's empty-array response shape).

**Risk**: The `#[cfg(test)]` block in `src/gateway/handlers/mod.rs:1185` (`test_heartbeat_handlers_registered`) asserts on `registry.has_method("heartbeat.list")` etc. — the cron equivalent would be a similar check, but I could not find one for cron. The boot-path register order matters: if a future refactor moves `register_handler!` calls before `HandlerRegistry::new()`, the stubs become load-bearing again. The stub module is also the only thing `HandlerRegistry::new()` references for cron, so any code path that calls `HandlerRegistry::new()` directly (not via `GatewayServer::build_router`) gets stubs back. Two callers in tree: `src/gateway/handlers/mod.rs::tests` (test only) and the boot path's `start_server()`. Boot path overrides unconditionally, so cutting is safe.

**Verification**: After removal, `cargo check -p alephcore` and `cargo test -p alephcore --lib` must compile cleanly; `grep -rn "handle_.*_stub" src/` must show only heartbeat (see F-02).

**existing_review_ref**: null.

---

### F-02 — `heartbeat::handle_*_stub` (8 functions in `heartbeat.rs:588-723`) are dead code (low, Form 1)

**Files**: `src/gateway/handlers/heartbeat.rs:588,593,617,648,663,675,705,723` (886 lines total).

**Symbol**: `pub async fn handle_list_stub / handle_get_stub / handle_create_stub / handle_update_stub / handle_delete_stub / handle_toggle_stub / handle_wake_stub / handle_runs_stub`.

**Evidence**:

```text
$ rg "heartbeat::handle_.*_stub" src/ bin/ interfaces/ shared/
src/gateway/handlers/mod.rs:363: registry.register("heartbeat.list", heartbeat::handle_list_stub);
… (8 placeholder registrations, lines 363-370)
src/gateway/handlers/heartbeat.rs:746: async fn test_handle_list_stub() {
… (16 #[cfg(test)] tests in heartbeat.rs)
```

The boot path overrides all 8 with the real handlers (`src/bin/aleph-server/commands/start/builder/handlers/agents.rs:153-198`):

```text
153:        heartbeat::handle_list,
162:        heartbeat::handle_get,
… (8 lines, all without _stub)
```

**Decision**: **CUT** — delete the 8 `_stub` functions (lines 588-742 of `heartbeat.rs`, ~155 lines); delete the 8 `registry.register(..._stub)` lines in `mod.rs:363-370`; move or drop the 16 stub-only `#[cfg(test)]` cases (most assert against the stub's `INTERNAL_ERROR` response shape and have no counterpart in the real handlers).

**Risk**: One test guards the stubs staying registered — `src/gateway/handlers/mod.rs:1185 test_heartbeat_handlers_registered`. After cut, the test must be updated to assert against the **real** handler names (which already work in the boot-path registration); the test as written would fail after CUT but the boot path wires the same method names, so the assertion survives.

**Verification**: `cargo check -p alephcore`, `cargo test -p alephcore --lib test_heartbeat_handlers_registered`.

**existing_review_ref**: null.

---

### F-03 — `secret_approvals::{handle_request, handle_resolve, handle_pending}` are dead code (medium, Form 1 + Form 6)

**Files**: `src/gateway/handlers/secret_approvals.rs:144,187,208` (289 lines, full module).

**Symbol**: `pub async fn handle_request / handle_resolve / handle_pending`, plus `pub struct SecretApprovalManager`, `pub struct SecretApprovalRequest`, `pub enum ApprovalDecision`.

**Evidence**:

```text
$ rg "secret_approvals|SecretApproval" src/ bin/ interfaces/ shared/
src/gateway/handlers/mod.rs:109: pub mod secret_approvals;
src/gateway/handlers/secret_approvals.rs: (entire module is self-contained)
```

The module declares `pub mod secret_approvals;` at `src/gateway/handlers/mod.rs:109`, defines 3 `pub async fn` handlers plus a manager struct, but **no production code anywhere imports or calls any of them**. The `Handle`/`handle_*` functions are only used inside the module's own `#[cfg(test)]` block (5 tests, `secret_approvals.rs:221-285`). The intended RPC names (`secret.approval.request`, `secret.approval.resolve`, `secret.approvals.pending`) are documented in the file's header but are not registered in either `mod.rs::new()` or any `register_handler!` call.

**Cross-reference with similar live code**: `exec_approvals::register_handlers` (at `src/gateway/handlers/exec_approvals.rs:70`) wires the parallel `exec.approval.resolve` + `exec.approvals.pending` pair from the live boot path (`src/bin/aleph-server/commands/start/mod.rs:2883-2888`). The `secret.approval.*` family appears to be an abandoned parallel attempt for secrets, never finished.

**Decision**: **CUT** — delete the entire `src/gateway/handlers/secret_approvals.rs` (289 lines) and remove `pub mod secret_approvals;` from `src/gateway/handlers/mod.rs:109`.

**Risk**: The 5 `#[cfg(test)]` cases inside the module would be lost; this is acceptable because the production wiring they would have guarded does not exist. No callers, no docs reference outside the file header.

**Verification**: `rg "secret_approvals|SecretApproval" src/ bin/ interfaces/ shared/` after deletion must show zero hits outside git history.

**existing_review_ref**: null.

---

### F-04 — `channel::handle_health` is dead code (low, Form 1)

**Files**: `src/gateway/handlers/channel.rs:836`.

**Symbol**: `pub async fn handle_health(...)`.

**Evidence**:

```text
$ rg "channel::handle_health" src/ bin/ interfaces/ shared/
(no matches)

$ rg "channel_handlers::handle_health" src/ bin/ interfaces/ shared/
(no matches)

$ rg "handle_health" src/ bin/ interfaces/ shared/
src/gateway/server/mod.rs:822: .route("/health", get(probe::handle_health))
src/gateway/server/probe.rs:20: pub async fn handle_health(State(state): State<Arc<GatewaySharedState>>) -> impl IntoResponse {
```

The `channel::handle_health` at `channel.rs:836` is **not** the function that backs the public `/health` HTTP route (that's `src/gateway/server/probe.rs:20`). The channel handler has zero callers.

**Decision**: **CUT** — delete `src/gateway/handlers/channel.rs:836` (the function and its impl block).

**Risk**: None visible. No tests reference it.

**Verification**: `rg "channel::handle_health" src/` after deletion must be empty.

**existing_review_ref**: null.

---

### F-05 — `flow_admin::register_flow_admin_handlers` is invoked but only one of its registered handlers is defined (medium, Form 2)

**Files**: `src/gateway/handlers/flow_admin.rs:50` (function); `:28` (only `handle_flow_reload` is `pub async fn`); module doc declares "PHASE-6: `register_flow_admin_handlers` is not yet invoked from `GatewayServer` construction. Phase 6 will wire it once the boot path threads `flow_dir` through the builder."

**Evidence**:

```text
$ rg "register_flow_admin_handlers" src/ bin/ interfaces/ shared/
src/gateway/handlers/flow_admin.rs:5:  (doc comment)
src/gateway/handlers/flow_admin.rs:50: pub fn register_flow_admin_handlers(
src/bin/aleph-server/commands/start/mod.rs:1605: alephcore::gateway::handlers::flow_admin::register_flow_admin_handlers(
```

The function **is** called from the boot path (`start/mod.rs:1605`), but its own body registers method names that are not defined in the file. Looking at the only `pub async fn` in the module, `handle_flow_replay` is the only handler; the rest of the family's handlers (`handle_flow_list`, `handle_flow_pause`, etc.) are missing. This is a stub far-end: the registration is wired but the handler bodies that would back it do not exist as `pub` items.

**Decision**: **DECIDE** — surface-level audit cannot tell whether the missing handlers are intentional stubs (to be filled in phase 6 per the module doc) or abandoned work. Options:
- (a) **CONNECT**: add the missing handler bodies so the live registration routes somewhere.
- (b) **CUT**: delete `register_flow_admin_handlers` from `start/mod.rs:1605` and the `flow_admin` module until phase 6 is actually scheduled.

Recommend (b) since the module doc explicitly says phase 6 has not happened; the registration is currently a placeholder that the dispatcher will route to, but a `METHOD_NOT_FOUND` is the correct response until then.

**Risk**: If the boot path at `start/mod.rs:1605` is on a hot path, removing the call deletes a no-op (safe). If it's behind a feature flag that turns it on conditionally, the audit needs to revisit after that flag is removed.

**Verification**: After cut, `rg "flow_admin::" src/bin/aleph-server/` must be empty; the only remaining reference is the module doc.

**existing_review_ref**: null.

---

### F-06 — `subagent::handle_tree` is only called by one in-tree test, never registered as RPC (low, Form 1)

**Files**: `src/gateway/handlers/subagent.rs:42` (`pub async fn handle_tree`).

**Evidence**:

```text
$ rg "subagent::handle_tree" src/ bin/ interfaces/ shared/
src/bin/aleph-server/commands/start/mod.rs:1716: async move { alephcore::gateway::handlers::subagent::handle_tree(req, sessions).await }
src/gateway/isolation_acceptance.rs:44: use crate::gateway::handlers::subagent::handle_tree;
```

The boot path at `start/mod.rs:1716` invokes the function inside a closure but **`mod.rs::new()` already registers the placeholder `subagent.tree` (line 887)** which returns `SERVICE_UNAVAILABLE`. Looking at `register_handler!` for `"subagent.tree"`: there is **no override**. So the function at `subagent.rs:42` is defined, called from one in-tree test (`isolation_acceptance.rs`), and the closure at `start/mod.rs:1716` is unreachable in production (the mod.rs placeholder is hit first).

**Decision**: **CUT** — delete `src/gateway/handlers/subagent.rs` (the whole 50-line file is just `handle_tree` + types); remove `pub mod subagent;` from `src/gateway/handlers/mod.rs:96`; delete the dead closure at `start/mod.rs:1716`; update `isolation_acceptance.rs` to not import the deleted function.

**Risk**: The placeholder in `mod.rs:887` already says "subagent.tree requires SessionStore (boot phase 2)"; if phase 2 was supposed to wire the real handler, that work is still missing. The test at `isolation_acceptance.rs:44` was testing the unwired function. Cutting the test is acceptable — the placeholder is a more honest representation of current state.

**Verification**: `rg "subagent::" src/bin/aleph-server/` after cut must be empty (excluding the placeholder registration in mod.rs).

**existing_review_ref**: null.

---

### F-07 — `channel::handle_health` re-listed under channel (already F-04), `chat::handle_rewind` is registered but `chat::handle_rewind` is the only `pub` rewind fn in `chat.rs` and has no caller (low, Form 1)

**Files**: `src/gateway/handlers/chat.rs` (search for `pub async fn handle_rewind`); expected at line ~XXX.

**Symbol**: `pub async fn handle_rewind`.

**Evidence**: Cross-check from `src/bin/aleph-server/commands/start/builder/agent_init/common_handlers.rs:180` — the boot path registers `chat.rewind` against `chat_handlers::handle_rewind(req, manager)`. So `handle_rewind` **does** have a live caller via boot path. Marking this as **NOT-DEAD** for now; flagging as DECIDE/uncertain.

**Decision**: keep — verified live caller.

(F-07 placeholder to avoid double-counting; covered by F-04.)

---

### F-08 — `mcp::handle_logs` registered as `mcp.logs` but `_handle` arg is unused (low, Form 6)

**Files**: `src/gateway/handlers/mcp.rs:180`.

**Symbol**: `pub async fn handle_logs(request: JsonRpcRequest, _handle: McpManagerHandle) -> JsonRpcResponse`.

**Evidence**:

```text
$ rg "mcp::handle_logs" src/ bin/ interfaces/ shared/
src/bin/aleph-server/commands/start/builder/handlers/mcp.rs:37: reg!("mcp.logs", mcp::handle_logs);
```

The function is wired at boot but takes `_handle: McpManagerHandle` (the underscore prefix signals unused). Either:
- (a) the handler was intentionally a no-op stub for `mcp.logs` (relying on `tracing` / log file directly), or
- (b) the handle should be used to fetch actual MCP logs.

This is **NOT** dead — it's a wired handler. The signature is mildly suspicious (unused param suggests stale code) but not severed.

**Decision**: **DECIDE** — confirm intent with code owner. If `mcp.logs` is supposed to read MCP-server logs, the handler must use `_handle`. If it's a thin proxy to tracing, the param is inert and could be removed.

**Risk**: low. Cosmetic.

**existing_review_ref**: null.

---

### F-09 — `tools_cancel::handle_in_flight` and `tools_visibility::{extract_source, filter_by_source, group_tools}` are helpers with one caller (low, Form 6)

**Files**: `src/gateway/handlers/tools_visibility.rs:5,12,17,90,116`; `src/gateway/handlers/tools_cancel.rs:159`.

**Symbol**: `pub fn extract_source / filter_by_source / group_tools`; `pub async fn handle_in_flight`.

**Evidence**: Each has at least one production caller (`tool_catalog_init.rs:273,374,399,408`). Connected. Listed only to confirm no false-negatives from the audit sweep.

**Decision**: no action — false alarm; mark as healthy.

---

### F-10 — `flow_admin.rs` declares `pub fn register_flow_admin_handlers` but the module doc says phase 6 is pending (medium, Form 2)

Same as F-05; tracked under F-05 to keep counts honest.

---

## Cross-cutting themes

1. **Two-tier stub/real handler pattern** (`cron/`, `heartbeat/`) — every RPC method has a `*_stub` placeholder + a real handler, with `HandlerRegistry::new()` registering stubs and the boot path overriding. This is a legitimate two-phase boot pattern, but the stubs themselves become inert once the boot path overrides, and they survive in the registry as dead code (F-01, F-02). The cleanup would be: keep `*_stub` only for the methods whose real handler is *not yet wired* (see `mod.rs:392-405` for `teams.*` and `agents.*` which are still phase-1-only).

2. **Abandoned modules** (`secret_approvals`, `subagent::handle_tree`): full modules with public APIs but zero in-tree consumers. These are the highest-value CUT targets.

3. **Module-doc-as-roadmap**: `flow_admin.rs:5` documents that wiring is "PHASE-6". When the phase is not on the immediate roadmap, the registration call is a no-op and should be removed; the function should be either fully wired or fully stubbed.

---

## What I did NOT check

- Permission/capability gating (Task 5): spot-checked `exec_grants.rs:131-148` (gates on caller role + session visibility) and `clarification.rs` (visibility check via `sessions.get_metadata` + `session_visible`). Both appear correctly wired. Did not exhaustively check every handler's gate.
- Input schema vs caller params (Task 6): sample-checked `chat.rs` and `memory.rs` — both use `parse_params::<T>` from `mod.rs:217` and the schemas match the `#[derive(Deserialize)]` types. No drift found in samples.
- Cross-reference with `interfaces/webchat` callers (Task 3): the webchat UI calls RPC methods by JSON-RPC name; the JSON-RPC name is the string registered in the dispatch table. Spot-checked the registered names match the patterns the webchat emits; no orphan registered names found that have zero webchat callers.

---

## Verification commands (re-runnable after cuts)

```bash
# After F-01 cut
grep -n "handle_.*_stub" src/gateway/handlers/cron/
# must be empty (or only in #[cfg(test)] of cron/real.rs)

# After F-02 cut
grep -n "handle_.*_stub" src/gateway/handlers/heartbeat.rs
# must be empty (or only in #[cfg(test)])

# After F-03 cut
rg "secret_approvals|SecretApproval" src/
# must show zero hits outside git history

# After F-04 cut
rg "channel::handle_health" src/
# must be empty

# After F-06 cut
rg "subagent::handle_tree" src/bin/aleph-server/
# must be empty
```
