# Panel 社区生产资产发布与浏览器认证投递计划 / Panel Community Production Asset Publication and Authenticated Browser Delivery Plan

日期 / Date: 2026-10-30

## Goal / 目标

在 `panel-community-assets` 已完成的 canonical publication contract、explicit selection 和 deterministic mount fixture 之上，补齐 production HTTP asset publication 与 browser credential propagation，使显式选择的 community Panel 能通过真实受认证的 same-origin asset request 到达现有 `PanelLoader` / `PanelShell` 生命周期。

Starting from the merged `main` commit `fd47b8805`, this phase wires the already-admitted canonical publication record into a narrow authenticated HTTP route and validates real browser delivery. Core remains the only declaration, trust, snapshot and source-revision authority; the browser remains a consumer of the shared snapshot and existing loader contract.

## Current state / 当前状态

已完成：

- Core canonical `PanelUiCapabilitySnapshot` and authenticated `/ws` discovery transport;
- `PanelAssetPublicationStore` with canonical identity, exact SHA-256 bytes, replacement/retraction and monotonic stale-revision fencing;
- explicit `/panel/<plugin_id>/<tab_id>` selection through `PanelProjection`;
- existing `PanelLoader` / `PanelShell` integrity, descriptor, async-result and one-shot cleanup fencing;
- deterministic `panel-tab-spike` fixture and bilingual validation report;
- review finding for stale overwrite fixed.

明确 deferred：

- production HTTP publication route;
- route-owned authentication and browser credential propagation;
- live Gateway asset delivery and its integration evidence;
- install/update/uninstall, marketplace/signature service, background refresh, live notifications, default-release changes, remote URL, multi-file graph, external WASM and Zahir runtime mount.

The current `PanelAssetRequest.authorized: bool` is an unmounted in-process seam only. It is not an acceptable production authorization boundary and must be removed or replaced before route registration.

## Phase boundary / 阶段边界

本阶段只做：

- audit and select an existing authenticated HTTP seam;
- bind publication requests to route-owned authenticated client/device/shared-token context;
- inject canonical admitted publication state into a narrow route;
- deliver exactly one admitted single-file module by its canonical versioned entry;
- pass the response through the existing exact-byte loader into an explicitly selected `PanelShell`;
- validate origin/TLS/auth, identity, revision, entry, integrity and cleanup fencing;
- add deterministic and, where the environment permits, real Gateway/browser integration evidence.

本阶段不做：

- install/update/uninstall/download lifecycle;
- marketplace, signature service, background refresh or live notification;
- automatic selection, default-release changes or route takeover;
- arbitrary filesystem/plugin-root publication, remote URL fetch, external WASM or multi-file graph;
- second registry, trust authority, revision source, loader or Zahir runtime mount.

If a safe producer/auth injection cannot be identified, stop after the audit with a design-only finding. Do not create an unowned endpoint merely to claim live parity.

## Architecture decisions / 架构裁定

1. **Core authority remains singular.** `ExtensionManager::panel_ui_capability_snapshot()` and its existing projection/lifecycle path remain the only source of declaration, trust/status and source revision. The HTTP route may consume admitted state but must not parse manifests, redo trust, or create revisions.
2. **Publication is canonical and single-file.** The route resolves `(plugin_id, plugin_version, tab_id, source_revision)` and the declaration's versioned `./tabs/` entry against a server-owned publication record. Client-provided paths, URLs, revisions, generic fields and plugin roots are never authority.
3. **Authorization is route-owned.** A request must carry a validated authenticated client/device/shared-token context produced by an existing auth seam. A boolean supplied by a caller or browser parameter is insufficient. The route state must remain narrow and must not turn `GatewaySharedState` into a second Panel registry.
4. **HTTP guards are explicit.** `/ws` authentication does not transfer to HTTP GET. The route repeats the existing `OriginPolicy`, insecure-remote/TLS, token/client-scope and rate-limit checks used by other protected HTTP routes, with generic refusal responses.
5. **Browser selection remains explicit.** Snapshot arrival only updates discovery. `/panel/<plugin_id>/<tab_id>` selection validates the latest accepted snapshot and source revision before requesting the declared asset and using the existing loader/shell lifecycle.
6. **No new browser loader.** Exact bytes, SRI, module ABI, descriptor identity, async generation and cleanup continue to be owned by the existing `PanelLoader`/JS loader and `PanelShell`.

## Task 1: Audit production owner and authentication seam / 审计 owner 与认证 seam

Read:

- `docs/reference/REDLINES.md`, `docs/reference/SECURITY.md`, `docs/reference/ARCHITECTURE.md`;
- `docs/reference/FEATURE_LOCATOR.md` §6.11 and `docs/reference/TERMINAL_RUNTIME.md`;
- `docs/reports/panel-community-assets-mount-validation.md`;
- `src/gateway/server/panel_asset_route.rs`, `src/gateway/server/mod.rs`;
- artifact/canvas/browser-viewer HTTP routes and their `OriginPolicy`, TLS, token, client and rate-limit guards;
- `SharedTokenManager`, `DeviceTokenManager`, `resolve_client`, MCP authorization and browser bootstrap credential code;
- `src/extension/{panel,projection,lifecycle,manifest,types}.rs`;
- `interfaces/webchat/src/{context,app}.rs` and `interfaces/webchat/src/panel/{discovery,projection,loader,shell,source}.rs`.

Record in the plan ledger:

- the sole owner of the admitted publication record and bytes;
- the route-owned authentication proof and its scope;
- exact HTTP/origin/TLS/rate-limit guards and why each applies;
- browser credential transport and same-origin behavior;
- whether an existing route can be extended or a narrow route is justified;
- what remains impossible without a real producer or service fixture.

Write RED tests before production code:

- missing/invalid auth and invalid origin/insecure remote fail closed;
- a valid authenticated request can only resolve the current canonical identity and exact entry;
- caller-provided path, URL, raw root, generic plugin field or revision cannot alter the result;
- stale, equal, retracted and replaced publication records fail closed;
- discovery snapshot arrival still has no fetch/import/mount side effect.

## Task 2: Replace the unbound authorization verdict / 移除未绑定授权 verdict

Implement the smallest safe internal contract:

- remove or make private the public construction path for `PanelAssetRequest.authorized: bool`;
- introduce a route-owned authenticated context or equivalent non-forgeable internal proof that is created only after existing token/client validation;
- keep the publication store independent of HTTP and filesystem concerns where possible;
- preserve generic refusal semantics and avoid credential/path leakage;
- add tests proving a caller cannot manufacture authorization by setting a field.

If the existing authentication APIs cannot provide a narrow proof without broad state or a second authority, stop and record a design finding instead of weakening the boundary.

## Task 3: Wire a narrow authenticated HTTP publication route / 接入窄 authenticated HTTP route

After Task 1 resolves the seam, implement or extend the smallest route that can be mounted before SPA fallback:

- use server-owned route state containing the admitted publication resolver and the minimum authenticated context;
- apply HTTP transport/origin/TLS/auth/scope/rate-limit guards explicitly;
- accept only canonical identity plus the declaration-derived entry shape;
- resolve only the current admitted publication and exact bytes;
- fail closed generically for unauthorized, stale, retracted, missing, mismatch, traversal, arbitrary URL and invalid revision requests;
- do not expose `PluginRecord::root_dir`, arbitrary filesystem paths, manifest contents, credentials or arbitrary URLs;
- verify content type/cache behavior and exact bytes expected by the existing JS loader;
- ensure the route does not change built-in ControlPlane SPA fallback or Terminal behavior.

Tests must exercise the actual route handler/state boundary, not only the pure store.

## Task 4: Connect browser credential delivery and verify explicit mount / 接通 browser credential 与显式挂载

- Derive the request URL from the accepted descriptor's canonical `public_base` and entry; do not concatenate arbitrary browser path input.
- Reuse same-origin/browser credential behavior already supported by the application, and make any required token transport explicit and bounded.
- Keep `/panel/` passive and require `/panel/<plugin_id>/<tab_id>` selection.
- Verify latest snapshot/source revision/publication before import and preserve loader fencing when disconnect, retraction, replacement, route refusal, import failure or unmount failure occurs.
- Preserve `/terminal` behavior and all built-in panel tests.

Add tests for:

- no asset request on snapshot arrival or `/panel/`;
- one exact request after explicit selection;
- descriptor identity and SRI propagation;
- stale/retracted/disconnected selection refusal;
- late result disposal and one-time cleanup.

## Task 5: Deterministic and Gateway/browser integration verification / 集成验证

Keep `panel-tab-spike` as the deterministic fixture. Add the narrowest additional fixture needed to prove route-owned auth and publication injection:

`authenticated request → route guards → canonical publication record → exact bytes → existing loader → explicit PanelShell mount → cleanup`.

Use the real Gateway/service/browser path when the repository's generated artifact, service and credentials are available. If the environment cannot provide them, record exact blocked commands and retain the deterministic proof without claiming live parity. Do not add a fake JSON-RPC endpoint as a substitute for production routing.

## Task 6: Documentation, fresh review and delivery / 文档、review 与交付

Update:

- `docs/reference/FEATURE_LOCATOR.md` §6.11;
- `docs/reference/TERMINAL_RUNTIME.md` only for changed ownership/transport;
- `docs/reports/panel-community-assets-mount-validation.md` or a clearly scoped successor report;
- the plan-scoped ledger with RED/GREEN results, route/auth rulings and deferred boundaries.

Run a fresh read-only review focused on:

- route-owned auth and token scope;
- origin/TLS and transport guards;
- path traversal, URL, filesystem and plugin-root exposure;
- exact integrity-to-bytes binding;
- stale/equal/retracted/replaced revision fencing;
- selection bypass and auto-mount;
- stale async loader/shell cleanup;
- duplicate authority and Core→WASM dependency direction.

## Verification discipline / 验证纪律

- Before every Cargo subprocess: `python qa/browser_live/check_memory.py` from the worktree root, requiring exit 0.
- Run Cargo serially from the owning package directory.
- RED→GREEN for each implementation task before broader validation.
- Run targeted Rust tests/checks, modular webchat tests, Node loader tests, and browser/component tests when generated artifacts exist.
- Run targeted rustfmt and `git diff --check`; run full `cargo fmt --check` and report unrelated baseline failures without rewriting unrelated files.
- Keep one writer per worktree, preserve conventional commits, and leave the tracked worktree clean.

## Acceptance criteria / 验收标准

- A production HTTP route, or an explicitly documented design-only stop if no safe route/auth producer exists, has one clear owner.
- Production authorization is route-owned and cannot be manufactured by a caller-provided boolean.
- HTTP origin/TLS/auth/scope/rate-limit checks are explicit and fail closed.
- Only Core-admitted canonical identity/current revision/exact entry can return exact bytes.
- Existing integrity-gated loader and PanelShell lifecycle are reused.
- Snapshot discovery remains passive; community mount requires explicit selection.
- Built-in Terminal behavior remains green.
- No install lifecycle, default-release change, live notification, multi-file asset graph, external WASM or Zahir runtime mount is introduced.
- Validation report distinguishes deterministic fixture proof, real Gateway/browser proof, blocked checks and deferred work.

Recommended implementation commits:

```text
test(panel): define authenticated publication route contract
refactor(panel): bind publication requests to route auth
feat(panel): serve admitted community assets over authenticated HTTP
test(panel): verify live community asset delivery and mount fencing
docs(panel): validate authenticated community publication
```
