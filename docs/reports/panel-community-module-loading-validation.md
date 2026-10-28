# Panel Community Module Loading Validation

> Date / 日期: 2026-10-28
> Scope / 范围: `panel-community-loader` worktree, community asset integrity loading stage only
> Artifact / 产物: clean `.panel-tabs-final`, Terminal build id `6dc1cdba239f4234`

## Intent / 目的

This report records the validated boundary for loading a controlled community ES module through the existing Panel projection, registry, and loader path. It does not describe an installation system or production community discovery.

本报告记录受控 community ES module 经过现有 Panel projection、registry、loader 路径的验证边界，不把本阶段扩展为安装系统或 production discovery。

## Validated behavior / 已验证行为

- `PanelUiCapabilityDeclaration.asset_integrity` is validated by `PanelProjection` and retained only through the existing `TabRegistry` metadata path. Built-in Terminal descriptors retain no integrity metadata.
- A same-key, same-descriptor snapshot replacement with changed integrity participates in the existing loader attempt identity. The old attempt cannot mount or overwrite the replacement.
- Built-in Terminal keeps its direct `import()` path.
- A community entry with validated integrity uses `fetch` → HTTP status and redirect checks → exact response bytes → SHA-256/SRI verification → Blob URL `import()`. The module is not imported before verification.
- Malformed integrity, HTTP failure, redirect, and hash mismatch fail closed with stable JS error codes. Rust maps fetch, malformed-integrity, and mismatch failures through `PanelLoadError::{AssetFetchFailed,InvalidAssetIntegrity,IntegrityMismatch}`.
- The community fixture is a self-contained single-file ES module. It does not permit relative imports, external WASM, remote URLs, or dynamic multi-asset graphs.
- Blob URLs are revoked after successful import/initialization and after import or initialization failure. Existing mount, unmount, SDK permission, generation, and exact descriptor fencing remain in the loader.

- `PanelUiCapabilityDeclaration.asset_integrity` 由 `PanelProjection` 验证，并仅通过现有 `TabRegistry` metadata 路径保留；内置 Terminal descriptor 不携带 integrity。
- 同 key、同 descriptor 但 integrity 改变的 snapshot replacement 纳入既有 loader attempt identity；旧 attempt 不能 mount 或覆盖 replacement。
- 内置 Terminal 保持 direct `import()` 路径。
- 已验证 integrity 的 community entry 经过 `fetch` → HTTP status/redirect 检查 → 精确 response bytes → SHA-256/SRI 校验 → Blob URL `import()`；校验完成前不会执行 module。
- malformed integrity、HTTP failure、redirect、hash mismatch 均 fail closed，并通过稳定 JS error code；Rust 通过 `PanelLoadError::{AssetFetchFailed,InvalidAssetIntegrity,IntegrityMismatch}` 映射 fetch、malformed-integrity 与 mismatch 失败。
- community fixture 是自包含单文件 ES module，不支持相对 import、外部 WASM、远程 URL 或动态多文件 asset graph。
- 成功、import failure 或 initializer failure 后都会 revoke Blob URL；既有 mount、unmount、SDK permission、generation 与精确 descriptor fencing 仍由现有 loader 负责。

## Artifact identity / 产物身份

The staged artifact was built from the worktree HEAD after implementation commits with:

```text
PANEL_OUTPUT_ROOT=.panel-tabs-final
Terminal entry: ./tabs/terminal/6dc1cdba239f4234/terminal.js
```

The fixture build used clean output and the tests consumed that same output. No stale `.panel-tabs` candidate was used.

## Verification / 验证

All cargo commands were preceded by `python qa/browser_live/check_memory.py` from the worktree root and run from the owning package directory.

| Check | Result |
|---|---:|
| `cargo fmt -- --check` | pass |
| `cargo test --lib --no-default-features --features panel-modular --quiet` | 1585 passed, 0 failed |
| `cargo test --lib --quiet` | 1535 passed, 0 failed |
| `cargo check --lib --no-default-features --features panel-modular --quiet` | pass |
| `cargo check --target wasm32-unknown-unknown --lib --no-default-features --features panel-modular` | pass |
| `cargo clippy --lib --no-default-features --features panel-modular -- -D warnings` | pass |
| `node --test tests/panel_loader.test.mjs` | 12 passed, 1 skipped, 0 failed |
| `ALEPH_PANEL_BRIDGE_GLUE=.panel-spike/shell/aleph_panel.js node --test tests/panel_loader.test.mjs` | 13 passed, 0 skipped, 0 failed |
| `bun run panel:spike:test` | 4 passed, 6 skipped, 0 failed |
| build/artifact/memory/protocol/host fixtures with `PANEL_OUTPUT_ROOT=.panel-tabs-final` | 15 passed, 0 failed |
| `git diff --check` | pass |

The default Node run skips the opt-in generated WASM glue bridge test because `ALEPH_PANEL_BRIDGE_GLUE` is unset. The generated glue run above enables that test and passes it with the staged ABI adapter. The six Playwright skips are the explicit full Shell/WS tests that require a live Gateway.

默认 Node 运行因未设置 `ALEPH_PANEL_BRIDGE_GLUE` 跳过 opt-in generated WASM glue bridge 测试；上面的 generated glue 运行启用该测试，并通过 staged ABI adapter。Playwright 的 6 个 skip 是明确要求 live Gateway 的 full Shell/WS 测试。

## Deferred / 未实现

This stage does not implement install/update/uninstall, marketplace or signature service, production ExtensionManager/discovery adapter, production ZahirFacade mount, background refresh, default release switching, arbitrary multi-file assets, external WASM, a same-origin security sandbox, or real Gateway integration. The artifact and host/protocol fixtures use local loopback infrastructure; they are not live Gateway evidence.

本阶段未实现 install/update/uninstall、marketplace/signature service、production ExtensionManager/discovery adapter、production ZahirFacade mount、后台 refresh、默认 release 切换、任意多文件资产、外部 WASM、same-origin 安全沙箱或真实 Gateway 接入。artifact 与 host/protocol fixture 使用本地 loopback 基础设施，不属于 live Gateway evidence。
