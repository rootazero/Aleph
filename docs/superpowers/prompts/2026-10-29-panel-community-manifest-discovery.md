# Panel community manifest declaration — next-session restart prompt

实施 Panel 社区 UI 的下一阶段：**把社区 Panel tab 从通用插件 manifest 的隐式世界提升为显式、可审计的 manifest declaration，并把它转换成现有 Panel capability snapshot；本阶段停在 discovery contract，不进入浏览器生产挂载、安装流程或默认 release。**

上一阶段已在 `main` 合入 `fc7d8a81b`：现有 projection → registry → loader 能够对带已验证 SRI 的受控单文件 community ES module 做精确字节校验、fail-closed import、mount/unmount cleanup 和 late-result fencing。当前仍没有生产 ExtensionManager → Panel declaration 的显式入口；不能从通用插件 `entry`、插件类型或 WASM capability 推断 Panel UI。

## Starting point / 起点

- 当前共享 `main` HEAD 为 `fc7d8a81b`，已 fast-forward 合入 `panel-community-loader` 的 9 个提交；不要重新实现 integrity-gated loader。
- `panel-community-loader` worktree 可能仍存在。只读检查其状态即可；新工作请从当前 `main` 创建独立 worktree/branch `panel-community-discovery`，不要复用或删除旧 worktree。
- 不直接在共享 `main` 实现；本阶段默认不 merge、不 push，除非会话收到新的明确授权。
- 使用 `superpowers:executing-plans` 顺序执行：
  `docs/superpowers/plans/2026-10-29-panel-community-manifest-discovery-plan.md`。
- 测试先写最小 RED，确认失败原因后实现最小局部修复；每项完成后保留实际验证证据和 conventional commit。

## Read first / 先读

1. 当前 git root 与上级适用的 `AGENTS.md`，以及：
   - `docs/reference/REDLINES.md`
   - `docs/reference/ARCHITECTURE.md`
   - `docs/reference/FEATURE_LOCATOR.md` §6.11
   - `docs/reference/TERMINAL_RUNTIME.md`
   - `docs/reference/CODE_ORGANIZATION.md`
2. 本 prompt 关联 plan，以及已完成的：
   - `docs/superpowers/plans/2026-10-28-panel-community-mode-contract-plan.md`
   - `docs/superpowers/plans/2026-10-28-panel-community-module-loading-plan.md`
   - `docs/reports/panel-community-module-loading-validation.md`
3. Extension manifest / lifecycle / projection：
   - `src/extension/manifest/types.rs`
   - `src/extension/manifest/mod.rs`
   - `src/extension/manifest/adapter.rs`
   - `src/extension/lifecycle.rs`
   - `src/extension/projection.rs`
   - `src/extension/registry.rs`
   - `src/extension/types.rs`
4. Panel protocol and current browser projection：
   - `shared/protocol/src/panel.rs`
   - `interfaces/webchat/src/panel/{source,projection,registry,loader,shell,sdk}.rs`
   - `interfaces/webchat/src/app.rs`
   - existing Panel Node/Playwright fixtures and generated artifact fixtures.
5. Verify the actual crate dependency graph before choosing a wire seam. Core already depends on `aleph-protocol`; client runtime must not depend on Core or Server. Do not solve the boundary by importing `PluginManifest` into WASM.
6. Read the matching test skill(s) before changing tests. Do not reopen the already accepted source → snapshot → projection → registry → loader architecture unless source inspection proves a contract error.

## Current facts / 当前事实

- `PluginManifest` already has generic `entry`, `permissions`, `kind`, optional `version`, and plugin-root path validation. These fields describe plugin runtime behavior; they do not constitute a Panel UI declaration.
- `ExtensionManager` and `src/extension/projection.rs` already have a single lifecycle transition/reprojection chokepoint. Panel discovery must use that authority if it is wired into production projections.
- `PanelUiCapabilityDeclaration`, `PanelUiCapabilitySnapshot`, and `PanelUiCapabilityOrigin` currently live with the webchat source contract. The preferred dependency-safe direction is to promote the serializable declaration/snapshot/origin to the shared protocol crate and remove the local duplicate, while keeping `PanelUiCapabilitySource`, built-in source construction, and browser validation in webchat. If the dependency audit proves a different seam is required, update the plan before implementation; do not create two shapes.
- Community `asset_integrity` is required for a usable community declaration and is validated by the existing Panel projection. Trust is host policy, not a manifest-controlled field. A manifest must never select `Verified` or bypass owner admission.
- The completed loader supports only a self-contained single-file ES module. Relative imports, external WASM, multi-asset graphs, remote URLs, installation, updates, uninstall, marketplace, and same-origin sandboxing remain out of scope.

## Required outcome / 必须产出

- An explicit optional manifest section for Panel UI, with a stable schema and bilingual documentation. It must be opt-in and must not infer a Panel tab from generic plugin `entry`, `kind`, `capabilities_v2`, or `wasm_capabilities`.
- The manifest section must carry only declaration facts: tab identity/presentation, API major, relative entry, permissions, public base or host-provided asset address as defined by the plan, and required `sha256-<base64>` integrity. It must not carry trust, owner generation, runtime handles, credentials, or arbitrary URLs.
- A pure, deterministic adapter from an admitted plugin record plus the explicit manifest section to the canonical `PanelUiCapabilityDeclaration` / `PanelUiCapabilitySnapshot` shape. Plugin id, version, origin, source revision, trust, status, and removal behavior must be derived from host state and policy.
- Invalid, missing, disabled, blocked, incompatible, duplicate, traversal, missing-version, missing-integrity, or policy-rejected declarations must fail closed or be omitted with a stable diagnostic; they must never become a mountable Panel descriptor.
- The adapter must participate in the existing single projection/revision authority. Recompute must retract declarations when a plugin is disabled, unloaded, removed, or replaced; stale snapshots must not survive.
- Webchat must consume the canonical snapshot shape without importing Core types. Existing Terminal source/projection/registry/loader behavior and all completed integrity-loader fixtures must remain green.
- Add a local fixture or protocol round-trip proving one explicit manifest declaration becomes the same validated snapshot consumed by the existing Panel projection. This is discovery evidence only; it must not claim that a production browser route or community UI is visible.
- Update the relevant bilingual references and add a validation report describing the manifest contract, trust/integrity ownership, revision/retraction semantics, and the exact deferred production boundaries.

## Hard constraints / 硬约束

- Respect R1/R2/R4/R8/R9/R10. Core may derive and publish data contracts but must not call browser APIs, fetch assets, mount a Panel module, or own UI lifecycle.
- Keep exactly one canonical Panel descriptor/declaration/snapshot shape. Promoting the existing serializable source types to protocol is allowed; copying them into Core, Server, and webchat is not.
- Do not infer Panel UI from generic plugin `entry`, plugin kind, arbitrary permissions, `capabilities_v2`, or WASM declarations.
- Do not let the manifest select trust, owner generation, active status, filesystem access, or transport capabilities. Reuse existing owner/admission policy and existing Panel permission validation.
- Do not add a second plugin registry, Panel registry, permission model, loader, trust model, or snapshot revision authority.
- Do not add production `app.rs` routes, PanelShell mounting, ExtensionManager browser calls, ZahirFacade mount, default release changes, or a new HTTP/WS endpoint unless the plan proves an existing projection channel is the correct owner and the change is strictly required for this declaration contract. If no existing transport seam exists, stop at the canonical snapshot and document the next boundary.
- Do not implement install/update/uninstall, marketplace, signature service, asset publication/server, background refresh, remote URL loading, multi-file assets, external WASM, sandboxing, or real Gateway integration.
- Do not modify `src/harness/`, PTY server business logic, Gateway Bound Stage C, or unrelated plugin lifecycle behavior.
- Chinese communication, English code comments, bilingual docs; do not write machine-specific absolute paths into repository documents.
- Before every cargo subprocess, run `python qa/browser_live/check_memory.py` from the worktree root and require exit 0. Run cargo from the owning package directory, serially.
- Do not create, save, print, or expose credentials, tokens, session IDs, PTY IDs, or user Gateway state. Local fixture evidence must not be called live Gateway evidence.

## Verification and handoff / 验证与交付

Run only the checks affected by the implementation, then the required regressions:

- manifest parser/schema tests for JSON and TOML forms, unknown/missing fields, legacy manifests, path and integrity rejection;
- Core adapter/projection tests for active/inactive policy, deterministic ordering, duplicate/collision, revision increments, replacement and retraction;
- shared protocol serialization/round-trip tests;
- webchat projection/registry/loader tests and the existing Node loader tests;
- generated wasm glue test when its local artifact is available;
- existing Panel build/artifact/protocol/host fixtures, without a real Gateway;
- `cargo fmt --check`, affected `cargo check`, affected `cargo clippy`, memory gates, `git diff --check`, and an independent read-only review.

The final report must state exactly what is proven: explicit manifest parsing, canonical snapshot conversion, policy/integrity ownership, revision/retraction behavior, Terminal compatibility, and the absence of production browser discovery/mount. It must list skipped checks and explain why. Recommended final commit:

`feat(panel): derive declarations from explicit plugin manifests`

English handoff: continue from `main` at `fc7d8a81b` in a fresh `panel-community-discovery` worktree. Add an explicit opt-in Panel UI manifest declaration and derive the canonical Panel capability snapshot from admitted plugin state through the existing projection authority. Promote the existing serializable declaration/snapshot contract to the shared protocol only if that is the dependency-safe seam, and remove any duplicate shape. Preserve trust and permission ownership, do not infer UI from generic plugin fields, and stop before production asset publication, browser mount, installation lifecycle, default-release changes, merge, or push.
