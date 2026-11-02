# Panel Trusted Project Context Implementation Plan

Execution status / 执行状态: **Completed on the isolated branch. / 已在隔离分支完成。** This approved plan is retained as written; the [run ledger](../runs/2026-11-01-panel-trusted-project-context.md) records the final contracts, verification and explicit implementation rulings where the spec takes precedence. / 本文保留获批计划，最终契约、验证及以 spec 为准的执行裁决见台账。

- **Goal / 目标:** Replace the deliberate Global-only Panel ingress with one authenticated, server-owned project selection shared by `panel.capabilities.snapshot` and `/tabs/...` asset delivery, while preserving Core as the only declaration, immutable-byte, visibility, trust, and revision authority.
- **Architecture / 架构:** A process-local selector keyed by stable authenticated device identity. The selector stores only `project_id` and a monotonically increasing selector generation. A typed caller context is derived at the authenticated dispatch boundary and passed explicitly through the RPC middleware terminal and the HTTP route. Every use revalidates the selected project through `ProjectStore` and constructs the Core `VisibilityCtx` from the internal `Project.workspace_path`.
- **Tech Stack / 技术栈:** Rust `alephcore` gateway/Core, Tokio/Axum, existing `ProjectStore`/`SecurityStore`/device-token identity, Leptos/WASM webchat, existing Panel publication APIs, existing production-library `tests/panel_http_publication.rs` target.
- **Spec / 规范:** `docs/superpowers/specs/2026-11-01-panel-trusted-project-context-design.md` (approved by the user).
- **Global Constraints / 全局约束:**
  - No project claim means Global-only. An explicit malformed, missing, inaccessible, cross-user, archived, revoked, or no-workspace claim refuses generically and never falls back to Global.
  - A claimed project requires strict `verified_user` plus a stable paired `device_id`. Loopback compatibility identity, legacy/shared credentials, unbound devices, and stale task-local identity cannot create or resolve a project selection.
  - Never infer a project from daemon CWD, filesystem root, URL path, query, arbitrary header, client boolean, chat room binding, or ambient tool actor. Preserve refusal of `x-aleph-project` and `x-project-id`.
  - `ProjectStore` is the project authority. Do not call `authz::project_for(..., actor=None)` for a claimed request. Do not add a second registry, bytes map, trust model, revision counter, or project authority.
  - Keep `workspace_path` internal. It may be used to build Core `VisibilityCtx`, but never enters selector state, protocol, browser state, logs, or error text.
  - Preserve existing bearer validation, `resolve_client`, insecure-remote refusal, `OriginPolicy`, wall checks, private rate limits, exact generic refusal, built-in `ControlPlaneAssets`, `private, no-store`, JavaScript content type, `nosniff`, `Vary`, and passive discovery.
  - Do not change `PanelUiCapabilitySnapshot { revision, declarations }` or introduce generated client files. The project selection request is an internal typed server/browser request with a private method name shared by the two implementations only if needed; no public snapshot metadata is required.
  - Keep browser behavior bounded by existing connection, request, descriptor, revision, loader-disposal and projection fences. Cross-tab selection changes are observed at the next discovery/asset boundary because live notifications are out of scope. Already-loaded idle Panels are not promised immediate destruction.
  - Before every Cargo command, run `python qa/browser_live/check_memory.py` from the worktree root. Run Cargo serially with `CARGO_BUILD_JOBS=1`; prefer the independent production-library integration target over the known monolithic LLVM-OOM harness. No merge or push.
- **Review Focus / 审查重点:**
  1. Is the actor/device proof strict at both RPC and HTTP boundaries, including loopback, shared-token, unbound-device and revoked-user cases?
  2. Do snapshot and bytes call the same selector resolver and the same Core `VisibilityCtx`, with no Global fallback after an explicit claim fails?
  3. Is caller context passed explicitly across dispatch/spawn boundaries without repurposing task-locals or `request_state`?
  4. Can project switching, retraction, revision changes, revocation or late browser results expose stale declarations or bytes?
  5. Are Terminal/public assets, exact refusal/cache behavior, passive discovery and existing project/session semantics preserved?

## Implementation order

### Task 1 — Add the typed server-owned selector and fail-closed resolver

**Files and boundaries**

- Add `src/gateway/panel_project_context.rs`.
- Register the module in `src/gateway/mod.rs`.
- Extend the existing identity code in `src/gateway/handlers/connect.rs` only as needed to expose one strict principal result while retaining the compatibility `resolve_connection_identity(...)` tuple API for existing callers.
- Keep all new selector tests beside the selector implementation; do not add a second project store or publication state.

**Contract**

Define a crate-visible typed contract equivalent to:

```rust
pub struct PanelCallerContext {
    pub verified_user: Option<String>,
    pub device_id: Option<String>,
}

pub enum PanelProjectSelection {
    Global,
    Project { project_id: String, generation: u64 },
}

pub struct PanelResolvedContext {
    pub selection: PanelProjectSelection,
    pub visibility: VisibilityCtx,
}

pub struct PanelProjectSelector { /* ProjectStore + selector map */ }

impl PanelProjectSelector {
    pub fn new(projects: Arc<ProjectStore>) -> Arc<Self>;
    pub fn select(
        &self,
        caller: &PanelCallerContext,
        project_id: Option<String>,
    ) -> Result<PanelProjectSelection, PanelProjectContextError>;
    pub fn resolve(
        &self,
        caller: &PanelCallerContext,
    ) -> Result<PanelResolvedContext, PanelProjectContextError>;
}
```

The exact private field names may follow repository style, but the observable contract is fixed: selector state contains only device key, optional project ID and generation; `resolve` re-reads the current `ProjectStore` record, requires `ProjectStatus::Active`, strict actor authorization and `Some(workspace_path)`, then constructs `VisibilityCtx` without the CWD fallback. `select(Some(id))` validates before replacing the prior valid selection. A failed selection leaves the previous selection unchanged. `select(None)` is an explicit clear and requires the same strict device-bound caller proof.

The strict principal result must distinguish a real active user bound to a paired device from implicit owner compatibility identity. Preserve the existing behavior for callers that only need `(Option<String>, &'static str)`; Panel code consumes the strict `verified_user` field.

**RED**

Add tests that fail before the selector exists:

- no selection resolves Global and never exposes a root;
- valid Project A is accepted for its authenticated actor/device and returns Global plus Project A through Core visibility;
- Project B, cross-user, unknown, archived, deleted, malformed and `workspace_path: None` claims refuse generically;
- failed selection preserves the previous valid selection;
- loopback compatibility, legacy/shared credential, unbound device and absent verified user cannot select or resolve a project;
- `actor=None` is not used in the claimed path.

**GREEN and verification**

Implement the selector and strict identity result with the existing `ProjectStore`, `SecurityStore` and `VisibilityCtx`. Run:

```text
cd <worktree-root>
python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo test -p alephcore panel_project_context
```

Commit the green result as:

```text
feat(gateway): add authenticated panel project selector
```

### Task 2 — Thread authenticated Panel context through RPC dispatch

**Files and boundaries**

- Modify `src/gateway/server/connection/dispatch.rs`.
- Modify `src/gateway/server/mod.rs` to propagate `ConnectionState.device_id` and strict `verified_user` atomically through `CallerSnapshot`; do not infer from task-locals after a spawn.
- Modify `src/gateway/middleware/chain.rs` and `src/gateway/middleware/handler_service.rs` only through the correct worktree paths; keep the existing request-only path unchanged.
- Modify `src/gateway/handlers/mod.rs` with a narrow `handle_with_panel_context(...)` path. Existing `HandlerFn`, `register`, and unrelated handlers remain request-only.
- Modify `src/gateway/handlers/panel_capabilities.rs`.
- Add a focused context-aware Panel handler module under `src/gateway/handlers/` for the internal selection operation, or keep it in `panel_capabilities.rs` if that is smaller and does not mix unrelated handlers.

**Contract and dispatch shape**

Add a parallel path equivalent to:

```rust
pub async fn process_request_with_panel_context(
    text: &str,
    middleware_chain: &MiddlewareChain,
    panel_context: Option<PanelRequestContext>,
) -> String;

pub async fn serve_with_panel_context(
    &self,
    request: JsonRpcRequest,
    panel_context: PanelRequestContext,
) -> JsonRpcResponse;

pub async fn handle_with_panel_context(
    &self,
    request: &JsonRpcRequest,
    panel_context: &PanelRequestContext,
) -> JsonRpcResponse;
```

`PanelRequestContext` contains the typed `PanelCallerContext` and the shared `Arc<PanelProjectSelector>`. The normal `process_request`/`serve`/`handle` path remains available for existing methods and tests. The dispatch boundary constructs the typed context from the atomic `CallerSnapshot`; a missing strict identity is represented explicitly and cannot become a claimed project principal.

The selection method accepts only a typed object containing `project_id: Option<String>` (null means explicit clear). It calls `PanelProjectSelector::select`, returns a generic JSON-RPC refusal on failure, and never calls snapshot discovery or imports/mounts anything. The snapshot handler resolves the selector and calls the existing Core accessor with the resulting `VisibilityCtx`; no snapshot wire shape changes. A resolution refusal is returned as the existing generic error projection.

**RED**

Add tests for:

- `CallerSnapshot` carrying `device_id` and `verified_user` from one `ConnectionState` read;
- missing connection, loopback, shared-token and unbound-device snapshots having no strict project principal;
- the context-aware middleware terminal reaching only Panel methods while request-only handlers still use the old path;
- selection RPC success, explicit clear, malformed/foreign/revoked refusal and no discovery side effect;
- snapshot RPC returning Global with no selection and Global + Project A for a valid selection;
- failed explicit selection never changing the prior valid selection and never falling back to Global.

**GREEN and verification**

Implement the parallel dispatch path, shared selector wiring in the live Gateway handler/middleware construction, context-aware selection handler and context-aware snapshot handler. Run:

```text
cd <worktree-root>
python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo test -p alephcore panel_capabilities
python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo test -p alephcore gateway::server::connection::dispatch
```

Commit:

```text
feat(gateway): thread authenticated panel context through rpc
```

### Task 3 — Use the same resolver in authenticated HTTP delivery

**Files and boundaries**

- Modify `src/gateway/server/panel_asset_route.rs`.
- Modify `src/gateway/server/mod.rs` route construction to inject the one shared selector and the same `ProjectStore` authority.
- Extend `src/gateway/server/panel_asset_http_tests.rs`.

**Contract**

Extend private `PanelHttpAuthContext` with the strict verified-user result derived from the existing device authentication identity path. A validated shared token, legacy/unbound device and credential-less compatibility identity may continue to use the existing Global behavior but cannot resolve a claimed project. Keep fields private so tests and request headers cannot manufacture an authenticated caller.

Extend `PanelAssetRouteState::new(...)`/`production_state(...)` with `Arc<PanelProjectSelector>`. After all existing transport, origin, rate-limit and bearer gates pass, `asset(...)` calls the selector resolver exactly once and uses its `VisibilityCtx` for both `panel_ui_capability_snapshot_for` and `panel_ui_asset`. A claimed resolution failure returns the existing generic refusal. Built-in `ControlPlaneAssets` handling remains before community authentication and unchanged.

**RED**

Extend the existing HTTP fixture with authenticated Project A and Project B records/devices and add failing assertions for:

- no selection preserves Global-only behavior and exact existing response headers;
- selecting A exposes Global records plus A bytes, while B bytes refuse;
- switching A→B changes both snapshot matching and bytes;
- cross-user, archived, deleted, no-workspace and revoked selections refuse without Global fallback;
- selection failure leaves the prior valid selection intact;
- query/header project claims remain refused;
- built-in Terminal, generic refusal, `private, no-store`, `nosniff`, origin/TLS/proxy and rate-limit regressions remain green.

**GREEN and verification**

Implement route-state injection and shared resolution. Run:

```text
cd <worktree-root>
python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo test -p alephcore panel_asset_http
```

Commit:

```text
fix(gateway): authorize panel assets by trusted project context
```

### Task 4 — Connect webchat project selection to Panel context and fence browser results

**Files and boundaries**

- Modify `interfaces/webchat/src/panel/discovery.rs`.
- Modify `interfaces/webchat/src/context.rs`.
- Modify `interfaces/webchat/src/components/sidebar/projects.rs`.
- Modify `interfaces/webchat/src/components/project_page/settings.rs`.
- Touch `interfaces/webchat/src/panel/projection.rs`, `interfaces/webchat/src/panel/shell.rs`, or loader tests only where the existing acceptance path cannot carry the context generation fence; do not add a second selection state or use `ChatState::room_project_id`.
- Extend the existing browser tests under `interfaces/webchat/tests/`.

**Contract and behavior**

Add the smallest local project-context generation/epoch field to `PanelDiscoveryState`. Extend the request acceptance path with that generation so a snapshot begun under A cannot replace projections or activate a loader after B is current. Preserve `SnapshotUpdate::{Accepted, StaleEpoch, StaleRequest, StaleRevision}` semantics, adding a project-context stale outcome only if the existing enum cannot represent it cleanly.

Add one DashboardState helper for the internal selection method, with a typed `project_id: Option<String>` parameter. The helper must use the existing `rpc_call` readiness/error path, advance the local context fence before a new selection, and leave the previous valid Panel state intact on refusal. The sidebar’s `ProjectsTabState::selected_project_id` remains the user-facing source of selection; it is committed after the authenticated selection succeeds. `ProjectsApi::touch` remains best-effort and unrelated to authorization. Chat room project state is untouched.

Successful archive must explicitly clear the server selector before or as part of clearing `ProjectsTabState::selected_project_id`; a failed clear preserves the existing refusal projection and must not silently expose Global records. Revocation or refusal observed at the next discovery/asset boundary drops active project Panel state and requires explicit user action to choose another project or clear Global. Asset fetch credentials remain host-injected; no project ID/root is added to module or SDK credentials.

**RED**

Add/extend tests for:

- A→B stale snapshot rejection at the project-context fence;
- stale projection, descriptor activation and late loader result rejection after a switch or disposal;
- selection RPC refusal preserving the prior valid selection;
- sidebar selection using the Panel selector and retaining best-effort touch;
- archive performing explicit Panel clear;
- passive discovery: snapshot decode/store never imports or mounts;
- existing readiness, RPC error projection, disconnect and revision monotonicity behavior.

**GREEN and verification**

Implement the DashboardState selection helper, sidebar/settings coordination and discovery fence. Run from the webchat package directory:

```text
cd interfaces/webchat
bun run panel:tabs:test
bun run panel:tabs:protocol:test
```

If a Rust webchat test target is required by the touched module, run the repository’s existing owning-package command only after the memory guard, with `CARGO_BUILD_JOBS=1`; do not invent a new test harness. Commit:

```text
feat(webchat): fence panel project switching
```

### Task 5 — Prove the production-library path and update bilingual evidence

**Files and boundaries**

- Extend `tests/panel_http_publication.rs` and, if needed, `tests/panel_http/common.rs`.
- Update `docs/reports/panel-community-live-publication-validation.md`.
- Update `docs/superpowers/runs/2026-10-31-panel-http-publication.md` or add the next dated bilingual run ledger without rewriting prior evidence.
- Update the relevant `docs/reference/FEATURE_LOCATOR.md` and `docs/reference/TERMINAL_RUNTIME.md` sections to describe the trusted selector, no-fallback refusal and cross-tab/revocation boundary.
- Keep all documentation bilingual, with exact commands and actual pass/skip/failure results.

**RED**

Extend the independent `tests/panel_http_publication.rs` target before implementation to require:

- two active project records with different workspaces and two bound authenticated devices;
- authenticated RPC selection followed by HTTP asset delivery through the real `GatewayServer` router;
- Project A and Project B isolation, Global + selected-project snapshot behavior, explicit switch behavior and cross-user refusal;
- revoked/archived/no-workspace refusal without Global fallback;
- existing Terminal/public/auth/origin/TLS/proxy/rate-limit and exact response behavior.

**GREEN and production verification**

Run the real target serially:

```text
cd <worktree-root>
python qa/browser_live/check_memory.py
CARGO_BUILD_JOBS=1 cargo test --test panel_http_publication
```

Then run the existing fresh production-feature/browser evidence flow from the worktree using the repository’s documented `qa/browser_live` commands. Record whether it proves A/B isolation, switching, revocation and late-result behavior. A monolithic Core LLVM OOM, unavailable native WebView, skipped fixture or timeout must be recorded exactly and cannot be called a pass.

Run documentation checks and workspace checks without Cargo where applicable, including `python qa/browser_live/check_docs.py` and `git diff --check`. Commit the evidence as:

```text
test(panel): prove trusted project publication
```

If documentation is intentionally separated from tests by repository convention, use a second conventional commit `docs(panel): record trusted project context validation`; do not combine unrelated cleanup.

## Final self-review before handoff

Before asking for plan review, verify this document against the approved design:

- [ ] Every approved goal, non-goal and anti-goal appears above.
- [ ] File paths are scoped to the isolated worktree and written relative to its repository root; the existing high-level plan is preserved.
- [ ] Core remains the only declaration/bytes/trust/scope/revision authority; selector state contains no root or bytes.
- [ ] Actor/device proof is explicit at RPC and HTTP boundaries; no `actor=None` claimed path exists.
- [ ] Global behavior, Terminal delegation, exact refusal/cache headers and passive discovery are tested.
- [ ] Browser switch/revocation/revision/late-result fencing is covered without using room binding.
- [ ] Every implementation task has a meaningful RED, a bounded GREEN change, a verification command, and a conventional commit.
- [ ] Cargo commands are serial and each is preceded by `python qa/browser_live/check_memory.py`.
- [ ] The independent production-library target is the final HTTP proof; monolithic OOM is not silently treated as success.
- [ ] No public snapshot shape, generated client, install/update path, marketplace/signature path, default release switching, notification channel, multi-file asset, external WASM, native WebView QA or Zahir mount is introduced.
- [ ] The plan does not repurpose `request_state`, task-locals across spawn, URL/query/header claims, or CWD fallback.

This plan is ready for review. Implementation must wait until the user confirms that it captures the goal; after that handoff, invoke the executing-plans workflow and follow the tasks in order.