# Panel 社区资产发布与受控浏览器挂载计划 / Panel Community Asset Publication and Controlled Browser Mount Plan

日期 / Date: 2026-10-30

## Goal / 目标

在已完成的 canonical snapshot transport 之后，建立从已认证 Panel declaration 到可验证 asset publication，再到显式、受控的 webchat community Panel mount 的下一阶段契约。

This phase advances the existing discovery transport to asset publication and a controlled browser mount. It must preserve Core as the declaration authority and reuse the completed integrity-gated `PanelLoader`.

当前基线 / Current baseline: `main` at `c3c9e25d6` (`fix(panel): fence concurrent discovery refreshes`). The previous `panel-community-transport` branch has been fast-forwarded into `main`.

## Phase boundary / 阶段边界

本阶段只处理：

- 从 admitted extension state 到可审计、受路径约束的 Panel asset publication；
- 将已接受的 canonical snapshot declaration 映射到现有 webchat discovery state；
- 在明确的 route/selection seam 中调用现有 `PanelShell`、`PanelRegistry` 与 `PanelLoader`；
- 对 publication bytes 执行现有 integrity gate，并验证 mount/unmount 与 late-result cleanup。

本阶段仍不处理：

- install/update/uninstall、marketplace、signature service 或后台下载；
- 远程 URL、multi-file asset graph、external WASM 或 arbitrary filesystem exposure；
- default release 切换、自动选择 community Panel 或隐式 route takeover；
- live snapshot notification、第二个 registry、第二个 trust/revision authority；
- ZahirFacade runtime mount、`src/harness/`、PTY/Gateway unrelated business logic 或真实 Gateway parity。

如果现有 asset/publication 或 browser route seam 不能在不新增 authority 的前提下复用，必须停在 audit/design 结果，并记录 deferred boundary，不通过新增 endpoint 绕过约束。

## Architecture decisions / 架构裁定

- `ExtensionManager::panel_ui_capability_snapshot()` 仍是唯一 declaration/snapshot authority。
- server publication adapter 只把已被 Core 接受的 declaration 映射到受约束的 asset address/bytes；它不重新解析 manifest、不重新做 trust decision、不生成 revision。
- webchat 只消费 shared `PanelUiCapabilitySnapshot` 和现有 publication response；WASM 不依赖 Core、Server、`PluginManifest` 或 `ExtensionManager`。
- `asset_integrity` 是 publication bytes 的 exact-byte contract。任何 bytes、地址或 descriptor 不匹配都 fail closed，并保留错误状态。
- community mount 必须是显式的用户/调用方选择结果。snapshot 到达本身不得触发 route selection、module import 或 mount。
- 所有异步 publication/load/mount 结果都必须同时受 connection/request epoch、descriptor identity 和 loader lifecycle fence 保护。

## Task 1: Audit publication and browser selection owners

### Read first / 先读

- `AGENTS.md`, `docs/reference/REDLINES.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/SECURITY.md`;
- `docs/reference/FEATURE_LOCATOR.md` §6.11 and `docs/reference/TERMINAL_RUNTIME.md`;
- `docs/reports/panel-community-snapshot-transport-validation.md`;
- `shared/protocol/src/panel.rs`;
- `src/extension/{manifest,panel,projection,lifecycle,mod}.rs`;
- existing Gateway/static artifact/browser-viewer handlers and auth/path guards;
- `interfaces/webchat/src/panel/{source,discovery,projection,registry,loader,shell}.rs` and `interfaces/webchat/src/app.rs`;
- existing Panel Node/Playwright/component fixtures.

### Required audit output / 必须记录

在 SDD ledger 中记录：

- current owner for asset bytes and public address;
- whether an existing authenticated/static publication seam can be extended;
- current owner for explicit route selection and PanelShell lifecycle;
- whether the existing loader contract is sufficient;
- exact reason for adding or not adding any route, handler, or response shape.

禁止在 audit 未完成前写 production code。若 publication seam 或 route-selection owner 不清晰，先提交 design-only finding。

### RED contract / RED 契约

先写最小失败测试，描述：

- canonical declaration can identify one admitted asset without exposing raw plugin paths;
- exact publication bytes pass the existing integrity gate;
- wrong bytes, traversal, arbitrary URL, missing asset, and unauthorized publication fail closed;
- snapshot arrival alone does not call route selection, `PanelShell`, or `PanelLoader`;
- an explicit selection is required before loading a community descriptor.

## Task 2: Add the smallest asset publication bridge

只有在 Task 1 证明已有 owner 后实现。

- Reuse the existing authenticated control/static asset seam when possible; do not create a second HTTP/WS endpoint merely for Panel assets.
- Derive the served address from admitted host state and the canonical declaration. Never expose a plugin root, arbitrary filesystem path, credential, or arbitrary remote URL.
- Preserve descriptor identity, source revision, plugin version, public base and integrity semantics.
- Serve only the already admitted single-file module shape supported by `PanelLoader`.
- Ensure traversal, symlink escape, path mismatch, missing bytes, unauthorized owner, and stale/retracted declaration fail closed.
- Keep publication read-only and deterministic. Do not add download, install, update, or background refresh behavior.

Tests must cover exact bytes, integrity mismatch, replacement/retraction, stale descriptor, auth failure, path traversal, and generic plugin fields not creating an asset.

## Task 3: Connect explicit selection to existing browser mount

- Keep snapshot discovery passive. A successful snapshot must update discovery state only.
- Add or reuse the narrowest existing explicit selection/route seam. Selection must validate that the descriptor is present in the latest accepted snapshot and that its source/revision is still current.
- Pass the selected descriptor through existing `PanelProjection`/registry validation and the completed integrity-gated `PanelLoader`; do not reimplement loading or create a community-only loader.
- Use existing `PanelShell` mount/unmount ownership. Preserve Terminal/built-in behavior.
- On disconnect, retraction, revision replacement, descriptor change, loader failure, or unmount failure, fail closed and prevent stale late results from reattaching a module.
- Do not automatically mount the first community declaration or change default release behavior.

Tests must prove:

- no mount on snapshot arrival;
- explicit selection mounts the intended descriptor only;
- stale selection is rejected after revision/retraction;
- exact integrity and descriptor identity reach the existing loader;
- mount/unmount cleanup happens once;
- late async results cannot attach after deselection, disconnect, or replacement;
- built-in Terminal source/projection/registry/loader tests remain green.

## Task 4: End-to-end deterministic fixture and browser verification

Add a deterministic fixture that exercises:

`canonical snapshot → explicit selection → existing publication seam → exact-byte loader → PanelShell mount → unmount cleanup`.

The fixture must not be described as live Gateway evidence. It must not use a fake JSON-RPC endpoint as proof of production routing. Run existing Node loader tests and component/browser tests when the required generated artifact is present; otherwise record the exact missing artifact/build boundary.

## Task 5: Documentation and review

Update bilingual documentation only after behavior is verified:

- `docs/reference/FEATURE_LOCATOR.md` §6.11;
- `docs/reference/TERMINAL_RUNTIME.md` only for changed publication/mount ownership;
- add `docs/reports/panel-community-assets-mount-validation.md`.

The report must distinguish:

- discovery transport already proven in `c3c9e25d6`;
- publication bytes and exact integrity evidence proven in this phase;
- explicit browser selection and mount evidence proven in this phase;
- install/update/default-release/live-notification/live-Gateway boundaries still deferred.

Run an independent read-only review focused on path traversal, arbitrary URL/filesystem exposure, integrity-to-bytes mismatch, selection bypass, auto-mount, stale async mount, duplicate authority, and Core-to-WASM dependency direction.

## Verification discipline / 验证纪律

- Before every Cargo subprocess, run `python qa/browser_live/check_memory.py` from the repository/worktree root and require exit 0.
- Run Cargo serially from the owning package directory.
- Use targeted RED→GREEN tests first, then affected package check/clippy/test, Node loader tests, and browser/component tests when artifacts exist.
- Run targeted rustfmt, `git diff --check`, and full `cargo fmt --check`; report unrelated baseline failures honestly.
- Use a fresh `panel-community-assets` worktree from current `main`; do not edit shared `main` during implementation and do not merge/push until a later explicit authorization.

## Acceptance criteria / 验收标准

- One existing, authenticated publication/control seam serves only admitted Panel assets.
- No raw plugin path, arbitrary URL, credential, or generic plugin field crosses the boundary.
- Exact publication bytes are checked by the existing integrity-gated loader.
- Discovery remains passive; mount requires explicit selection.
- Existing `PanelShell`/`PanelLoader` ownership is reused and built-in Terminal behavior stays green.
- Stale snapshot, revision, descriptor, request, connection, and loader lifecycle results cannot mount or reattach a module.
- No install lifecycle, default-release change, live notification, or live Gateway claim is introduced.
- Validation report records all proven, skipped, and blocked checks.

Recommended implementation commits:

```text
feat(panel): publish admitted community assets
feat(panel): mount explicitly selected community panels
 test(panel): verify publication and mount fencing
 docs(panel): validate community asset publication and mount
```
