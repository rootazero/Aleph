# Spec — Gateway Robustness Kit (Spec 2)

**Date**: 2026-05-21
**Branch (planned)**: `worktree-gateway-robustness-kit`
**Companion**: Spec 1 = `2026-05-20-gateway-tools-trace-wiring-design.md` (merged `4139ff14c`)
**Inspired by**: OpenClaw gateway (`/Volumes/TBU4/Github/openclaw/src/gateway/`)

---

## 1. Problem Statement

OpenClaw's gateway exposes a small "robustness kit" missing from Aleph's
gateway today: idempotency on side-effecting methods, `/health` + `/ready`
HTTP probes, and a session-generation counter so clients can detect server
restarts. Re-reading Aleph's code shows that — like Spec 1 — most of the
infrastructure already exists; the gaps are surface-level.

### 1.1 Reconnaissance — what's actually in the code

| Component                  | OpenClaw                                   | Aleph today                                                                                                                                       |
| -------------------------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Idempotency machinery**  | Per-method-bucket + RFC-9457 idempotency_key | `src/gateway/idempotency.rs` (357 LoC, full RAII slot + DashMap cache + TTL prune). Wired in WS request flow at `handler.rs:367-438`              |
| **Lane → idempotency gate** | Method-table scope                          | `src/gateway/lane.rs:37` — **hardcoded match on 17 specific method names**. All other methods (>100) fall through to `Lane::Query`, skipping idempotency |
| **Connection budget**       | preauthConnectionBudget                    | `handler.rs:65-72` — `max_connections=1000` checked **before** `ws.on_upgrade`. **Already exists**; not a gap                                      |
| **`/health` HTTP probe**    | `/health` + `/healthz` mounted on root      | `handle_health` exists as a JSON-RPC method (`channel.rs:779`) but the axum router mounts only `/ws`, `/v1/admin/*`, OpenAI, A2A, static fallback |
| **`/ready` HTTP probe**     | `/ready` + `/readyz` (distinct from health) | None                                                                                                                                              |
| **Session-generation counter** | Server-wide generation_id in handshake     | `StateVersionTracker` (3 domains: presence / health / config) exists. Bumped server-side at `handler.rs:250,484,684`. **Never sent to clients**   |
| **`gateway.identity.get`** | Returns version + connection id + supported protocols | None. JSON-RPC `version` handler returns version string only; no instance identity                                                       |

### 1.2 Gaps to close

- **G1**: Lane map (`Lane::for_method`) is too narrow. New RPCs default to `Lane::Query` → side-effecting methods bypass idempotency even though all the plumbing is wired.
- **G2**: No HTTP probe endpoints. Reverse proxies (nginx, traefik), container orchestrators (k8s), and uptime monitors cannot detect the gateway via HTTP.
- **G3**: `StateVersionTracker` is "write-only" — server bumps but no client sees the version. Clients can't detect server restart / config reload / presence churn cheaply.
- **G4**: No `gateway.identity.get` RPC. Clients have no programmatic way to inspect the running gateway (version, instance id, connection metadata).

Each gap is small in isolation; together they form a deployable robustness baseline.

---

## 2. Goals & Non-Goals

### Goals

1. **G1 — Smart lane mapping**: replace hardcoded match with a suffix-heuristic dispatcher + explicit override map, so new RPCs route correctly without manual maintenance.
2. **G2 — HTTP probes**: add `/health` and `/ready` axum routes returning JSON status; `/ready` flips green only after boot phase 2 completes.
3. **G3 — Surface state versions**: include `StateVersion` snapshot in (a) `auth.connect`'s response and (b) every event payload, so clients can detect server-side restart and skip stale event processing.
4. **G4 — `gateway.identity.get` RPC**: return version, instance id, supported protocols, available methods count — enough for clients to validate compatibility.

### Non-Goals (deferred)

- **OpenClaw's 7 auth modes** (trusted-proxy, tailscale, etc.). Aleph's Bearer + device-pairing is sufficient for the self-hosted use case; multi-mode is its own spec.
- **Per-IP auth rate limiting** beyond the existing `RateLimiter` semaphore. Auth-failure throttling is a separate spec.
- **OpenClaw's `gateway.restart.{request,preflight}`** RPCs. They require process-level orchestration (graceful drain, atomic config swap) that's out of scope for a wiring spec.
- **HTTP CORS / TLS / proxy headers**. Networking concerns live in deployment configuration; no code change here.
- **Schema/JSON Schema for `idempotency_key` field**. Already informal; protocol spec lives in `aleph-protocol` crate and is out of scope.

---

## 3. Architecture & Approach

### 3.1 G1 — Lane mapping

Replace `Lane::for_method` body with a three-step resolution:

```
1. Explicit override map (HashMap<&'static str, Lane>)
   - Covers known special cases (e.g., a `.get` that mutates, a `.set` that's idempotent)
2. Suffix heuristic:
   - .get / .list / .search / .status / .describe / .history / .effective / .catalog → Query
   - .install / .uninstall / .delete                                                   → System
   - .run / .send / .invoke / .execute                                                 → Execute
   - .create / .update / .set / .patch / .apply / .store / .add / .remove
     / .approve / .reject / .compact / .reset / .rotate / .revoke / .wake
     / .pair / .unpair / .start / .stop / .toggle                                       → Mutate
3. Default: Mutate (fail safe — new RPC gets idempotency by default)
```

The current behavior — default to Query — is unsafe: any new side-effecting method silently bypasses idempotency. Inverting to "default Mutate" means new RPCs are protected by default; the explicit override map handles the (rarer) "this looks mutating but is actually read-only" cases.

### 3.2 G2 — HTTP probes

Two new axum routes registered before `fallback_service`:

```
GET /health  → 200 OK + {"status": "ok", "version": "2026.05.21"}
GET /ready   → 200 OK + {"ready": true,  "phase": "complete", "version": ...}
                503     + {"ready": false, "phase": "booting" | "shutting_down"}
```

Readiness is tracked by a new `Arc<AtomicBool>` in `GatewaySharedState`. The boot path sets it to `true` after `agent_init.rs` returns; shutdown flips it back to `false`. The handler reads atomically — no lock.

`/health` is "process alive + handler chain compiled". `/ready` is "boot complete, ready to accept traffic". This matches the k8s `livenessProbe` vs `readinessProbe` convention.

### 3.3 G3 — State version exposure

`StateVersion` is already serializable (`#[derive(Serialize)]` at `state_version.rs:22`). Two surfaces:

1. **`auth.connect` response**: include `"state_version": {presence, health, config}` in the success payload. Clients capture the values to detect server-side bumps.
2. **Event payloads**: extend `GatewayEvent` with an optional `state_version` field at the envelope level. Bumped only on events that *cause* a version change (presence / health / config); other events leave it None. Wire-compatible (clients ignoring the field still parse).

Clients with `state_version` knowledge can:
- Compare connect-time version against post-reconnect version → detect server restart.
- Compare event version against last-known → skip redundant re-render.

### 3.4 G4 — `gateway.identity.get`

New JSON-RPC method returning:

```json
{
  "version": "2026.05.21",
  "instance_id": "<uuid-v4 generated at startup>",
  "started_at_unix": 1716248400,
  "supported_protocols": ["jsonrpc/2.0", "openai-compat/v1", "a2a/v1"],
  "registered_method_count": 217,
  "state_version": { ... }
}
```

`instance_id` is a per-process UUID-v4 generated once at startup and stored in `GatewaySharedState`. Clients use it to detect "same gateway came back" (instance unchanged + state_version reset) vs "different gateway" (new instance_id).

No new dependency — `uuid` is already in the workspace.

### 3.5 Components touched

```
src/gateway/lane.rs                     Modify  ~80 LoC rewrite Lane::for_method
src/gateway/server/mod.rs               Modify  ~50 LoC: ready_flag + instance_id + identity route
src/gateway/server/handler.rs           Modify  ~10 LoC: include state_version in event envelope
src/gateway/handlers/auth/connect.rs    Modify  ~15 LoC: append state_version to response
src/gateway/server/probe.rs             Create  ~50 LoC: /health + /ready axum handlers
src/gateway/handlers/identity.rs        Create  ~60 LoC: gateway.identity.get handler
src/gateway/handlers/mod.rs             Modify  ~5 LoC: register identity handler (phase-1 placeholder; phase-2 binds GatewaySharedState)
src/bin/aleph-server/commands/start/builder/agent_init.rs   Modify  ~30 LoC: register gateway.identity.get with state, flip ready_flag after wiring complete
tests/gateway_lane_routing.rs           Create  unit-style coverage for the new heuristic
tests/gateway_http_probes.rs            Create  integration coverage for /health + /ready
tests/gateway_state_version_handshake.rs Create  integration coverage for G3+G4
```

Total: ~250 LoC change. Zero new crates. Zero protocol-breaking changes.

---

## 4. Component-by-component Design

### 4.1 G1 — `Lane::for_method` rewrite

Current (lane.rs:37-53) is a closed match on 17 names. Replace with:

```rust
/// Explicit overrides for methods whose name doesn't match the heuristic.
/// First lookup; falls back to suffix heuristic; falls back to Mutate.
fn lane_overrides() -> &'static HashMap<&'static str, Lane> {
    static OVERRIDES: OnceLock<HashMap<&'static str, Lane>> = OnceLock::new();
    OVERRIDES.get_or_init(|| {
        let mut m = HashMap::new();
        // Read-only operations that don't end in a Query-suffix
        m.insert("echo", Lane::Query);
        m.insert("version", Lane::Query);
        m.insert("health", Lane::Query);
        m.insert("request.state", Lane::Query);
        m.insert("system.info", Lane::Query);
        // Identity/restart admin
        m.insert("gateway.identity.get", Lane::Query);  // looks-like Query already, defensive
        // Future: explicit Mutate overrides for misnamed methods here
        m
    })
}

pub fn for_method(method: &str) -> Self {
    if let Some(lane) = lane_overrides().get(method) {
        return lane.clone();
    }
    // Suffix heuristic
    if let Some(dot) = method.rfind('.') {
        let suffix = &method[dot + 1..];
        match suffix {
            "get" | "list" | "search" | "status" | "describe"
            | "history" | "effective" | "catalog" | "neighbors" | "subscribe"
            | "unsubscribe" | "stats" => return Lane::Query,
            "install" | "uninstall" | "delete" => return Lane::System,
            "run" | "send" | "invoke" | "execute" => return Lane::Execute,
            // Everything else: fall through to default
            _ => {}
        }
    }
    // Default = Mutate. New side-effecting RPCs are protected by default.
    Lane::Mutate
}
```

The previous `for_method`'s default-to-Query semantics is exactly what's broken; flipping the default closes G1.

**Migration risk**: changing default from Query to Mutate means every never-seen method now passes through the idempotency layer. Idempotency is conservative — it just looks at the `idempotency_key` param; if the client doesn't send one, it falls through to direct lane dispatch (see `handler.rs:435-438`). So flipping default is **wire-compatible**: no client behavior changes; only the *option* to send `idempotency_key` becomes available for previously-uncovered methods.

### 4.2 G2 — HTTP probes

Add to `GatewaySharedState`:

```rust
pub struct GatewaySharedState {
    // ... existing fields ...
    pub ready: Arc<AtomicBool>,
    pub instance_id: String,                 // UUID v4 from gateway startup
    pub started_at_unix: i64,
}
```

Set `ready = false` at construction, flipped to `true` at the end of `agent_init.rs`'s phase-2 wiring block. On graceful shutdown, flipped back to `false`.

Add routes in `build_router`:

```rust
let mut router = Router::new()
    .route("/ws", get(handler::ws_upgrade_handler))
    .route("/health", get(probe::handle_health))
    .route("/ready", get(probe::handle_ready))
    .fallback_service(control_plane)
    .with_state(shared)
    .merge(openai);
```

`probe.rs` (new module under `src/gateway/server/`):

```rust
pub async fn handle_health(
    State(state): State<Arc<GatewaySharedState>>,
) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "version": env!("ALEPH_VERSION"),
            "instance_id": state.instance_id,
            "uptime_secs": (chrono::Utc::now().timestamp() - state.started_at_unix).max(0),
        })),
    )
}

pub async fn handle_ready(
    State(state): State<Arc<GatewaySharedState>>,
) -> impl IntoResponse {
    let ready = state.ready.load(Ordering::Acquire);
    let status = if ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    let phase = if ready { "complete" } else { "booting" };
    (
        status,
        Json(json!({
            "ready": ready,
            "phase": phase,
            "version": env!("ALEPH_VERSION"),
        })),
    )
}
```

### 4.3 G3 — State version exposure

**Connect response**: extend `auth.connect` payload (handlers/auth/connect.rs:19) to include the snapshot. Clients store it for later comparison.

**Event envelopes**: extend the JSON object that `GatewayEventBus::publish` produces with an optional `state_version` field (top-level, sibling to `event`/`payload`). Populated only at the three bump sites in `handler.rs` (`:250` presence on subscribe, `:484` presence on agent.run, `:684` presence on disconnect — confirmed via grep). Other event publishers leave it unset.

Concretely, the publish path takes the snapshot from `ctx.state_versions.snapshot()` immediately after the `bump_*()` call and threads it into the event envelope. Event types whose origin doesn't touch the version (chat deltas, tool events, agent traces) are not modified.

Wire-compat: the new field is optional; clients ignoring it parse cleanly. We do NOT change the existing `event` / `payload` shapes.

### 4.4 G4 — `gateway.identity.get`

New file `src/gateway/handlers/identity.rs`:

```rust
pub async fn handle_identity_get(
    request: JsonRpcRequest,
    state: Arc<GatewaySharedState>,
    method_count: usize,
) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "version": env!("ALEPH_VERSION"),
            "instance_id": state.instance_id,
            "started_at_unix": state.started_at_unix,
            "supported_protocols": ["jsonrpc/2.0", "openai-compat/v1", "a2a/v1"],
            "registered_method_count": method_count,
            "state_version": state.state_versions.snapshot(),
        }),
    )
}
```

Registered at boot (phase-2) since it needs `GatewaySharedState`. Phase-1 in `handlers/mod.rs` reuses the `service_unavailable()` helper from Spec 1 — keeps the two-phase pattern consistent.

`method_count` is read from `HandlerRegistry::len()` (a new accessor; the existing `handlers` field is a private `HashMap<String, HandlerFn>` so we add a single `pub fn method_count(&self) -> usize` line). Pass it captured at boot time — the count is stable once phase-2 completes.

---

## 5. Testing Strategy

### 5.1 Unit tests

- **`gateway::lane::tests`** (extend existing): test the suffix heuristic — `foo.get` → Query, `foo.create` → Mutate, `foo.run` → Execute, `foo.install` → System; test the override path; test that unknown methods default to Mutate (the key fix).
- **`handlers::identity::tests`**: assert the response JSON shape.

### 5.2 Integration tests

- **`tests/gateway_lane_routing.rs`**: spin a minimal lane-dispatch fixture; verify side-effecting methods now hit the idempotency path. Regression catcher.
- **`tests/gateway_http_probes.rs`**: start a gateway server in a tokio task; `reqwest::get("/health")` → 200; `reqwest::get("/ready")` → 503 before boot signal, 200 after. Probe-format JSON shape pinned.
- **`tests/gateway_state_version_handshake.rs`**: connect, capture `state_version`; bump server-side (`bump_health()`); verify the next event surfaces the new version; verify `gateway.identity.get` returns matching identity.

### 5.3 Acceptance criteria

1. `lane_overrides().get("tools.invoke")` returns None → falls through suffix → returns `Lane::Execute` (suffix `invoke`).
2. `Lane::for_method("agents.create")` → `Lane::Mutate` (suffix `create`); idempotency keyed on the request now applies.
3. `GET /health` returns 200 with the expected JSON shape.
4. `GET /ready` returns 503 during a window before phase-2 completes, then 200 after.
5. `auth.connect` response includes `state_version`. Two connects across a `bump_config()` show different `config` values.
6. `gateway.identity.get` returns a UUID-v4 instance_id; same id across two calls within the same process.
7. All new integration tests green; `cargo check -p alephcore` + `--bin aleph-server` clean.

---

## 6. Risk & Migration

- **G1's "default → Mutate" flip is conservative on wire**: idempotency lookups depend on the client sending `idempotency_key`. Servers that don't are unaffected. Servers that do get the protection they were missing.
- **`/health` + `/ready` are additive routes**: nothing else moves. Reverse proxies will start seeing the endpoints; the absence-of behavior continues to work.
- **`state_version` in event envelope is optional**: clients that ignore it parse cleanly. New clients can opt-in.
- **`instance_id` per-process**: regenerated on each restart. Clients can detect this — that's exactly the value proposition. No persistence.
- **No protocol-breaking changes**. No CalVer bump required.
- **Pre-existing baseline test failures stay in scope**: per `project_baseline_test_failures` memory, main has 19 lib-test failures unrelated to gateway. The Spec 1 drive-by fix to `sandbox/workspace.rs` is already merged.

---

## 7. Out of scope, deferred

- The other 22 unwired RPC stubs in `handlers/mod.rs` (cron / heartbeat / group_chat / workspace.* / teams.* / agents.* / arena.* / runtimes.install). Each is a separate subsystem-level wiring decision.
- OpenClaw's 7-mode auth model (trusted-proxy / tailscale / device-token / bootstrap-token / password / token / none). Spec 1+2 keep the Bearer + device-pairing model.
- OpenClaw's `gateway.restart.request` / `gateway.restart.preflight`. Graceful drain is process-level orchestration, not a gateway-layer concern.
- A `state_version` query parameter for "events since version N" replay. Aleph's events are best-effort fire-and-forget; replay needs a different storage model.
- TLS, CORS, proxy-header normalization — deployment concerns.
