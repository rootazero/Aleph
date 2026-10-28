# Panel Community Integrity-Gated Module Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将已验证的社区 SRI 资产声明贯通到现有 PanelLoader，在任意社区模块代码执行前校验精确字节，并保留现有 Terminal 与 loader 生命周期兼容性。

**Architecture:** 继续沿用唯一的 source → snapshot → `PanelProjection` → `TabRegistry` → `PanelLoader` 路径。projection 已验证的 optional integrity 存在现有 registry 条目的附加元数据中；loader 将其作为 load attempt identity 的一部分传给现有 JS loader。无 integrity 的 built-in 保持直接 import，有 integrity 的社区单文件 ES module 走 fetch → WebCrypto SHA-256 → Blob import。

**Tech Stack:** Rust、Leptos、wasm-bindgen、JavaScript ES modules、Fetch、WebCrypto、Blob URL、Node test runner、Playwright。

**Spec:**
- `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`
- `docs/superpowers/plans/2026-10-28-panel-community-mode-contract-plan.md`
- `docs/superpowers/prompts/2026-10-28-panel-community-module-loading-restart.md`

## Global Constraints

- 以 `main` 最新提交创建 `panel-community-loader` worktree；`aab26f257` 必须是祖先。不直接在共享 `main` 实现，不 merge/push。
- `PanelTabDescriptor` 仍是唯一浏览器 projection shape；不得新增第二份 descriptor、registry、loader、权限或 trust 模型。
- 只允许已经通过 `PanelProjection::from_snapshot` / `replace_snapshot` 验证的 integrity 进入 loader；不得放宽 entry、public base、trust、permission 或 revision 校验。
- 本阶段社区资产仅为自包含单文件 ES module fixture。相对 import、外部 WASM、多资产 manifest、动态 asset build、远程 URL 均不支持。
- 不实现 install/update/uninstall、marketplace、签名服务、production ExtensionManager/discovery、后台 refresh、ZahirFacade mount 或默认 release flip。
- 不在 production `app.rs` 挂 fixture source，不添加社区 production route；fixture 成功不等于真实安装或 production 社区 UI 已完成。
- same-origin `Trusted` module 不是沙箱；不得接收超出既有 `PanelSdk` 权限门的 Core/transport 能力。
- 每个 cargo 子进程前从 worktree root 运行 `python qa/browser_live/check_memory.py`，只接受 exit 0；cargo 从 `interfaces/webchat` 串行运行。
- 不运行真实 Gateway，不记录 credential、token、session id 或 PTY id。loopback/artifact evidence 不得写成 live Gateway evidence。
- 中文沟通，代码注释英文，文档中英双语；每个行为变更先 RED 后 GREEN，最终独立只读 review。

## Review Focus

- **Hash mismatch 的模块已有 top-level side effect**：Task 2 必须以 import spy/全局计数证明 mismatch 在 import 与 initializer 之前拒绝。
- **HTTP redirect 将相对 entry 带到其他内容或 origin**：Task 2 必须以 redirect fixture 证明 `AssetFetchFailed`，且不执行响应模块。
- **同 descriptor、不同 integrity 的新 snapshot**：Task 4 必须证明旧 attempt 即使同 key 也不能 mount。
- **descriptor 移除后残留 integrity**：Task 1/4 必须证明 replacement 移除 key 后旧 metadata 和旧 attempt 均不可复用。
- **built-in 无 integrity 路径被意外改成 fetch/Blob**：Task 2/5 必须保留直接 import、lazy request/cache 和现有 Terminal fixtures。
- **Blob URL 或单文件边界被误解为通用多资产支持**：Task 2/4 必须证明 URL cleanup，并明确拒绝/不声明相对 import 与外部 WASM 支持。

---

### Task 1: Preserve Validated Integrity in the Existing Registry

**Files:**
- Modify: `interfaces/webchat/src/panel/registry.rs`
- Modify: `interfaces/webchat/src/panel/projection.rs`

**Interfaces:**
- Consumes: `PanelUiCapabilityDeclaration.asset_integrity: Option<String>` already validated by `PanelProjection`.
- Produces: `pub(crate) fn TabRegistry::asset_integrity(&self, key: &PanelTabKey) -> Option<&str>` and a crate-private setter used only while constructing a validated projection.

- [ ] **Step 1: Write failing projection/registry tests**

Add tests proving:
- built-in Terminal has no asset integrity;
- a valid community declaration exposes the exact validated SRI through `projection.registry().asset_integrity(key)`;
- newer replacement removes metadata for removed keys and replaces it for changed keys;
- stale or invalid replacement leaves both descriptors and integrity metadata unchanged.

- [ ] **Step 2: Run the tests and confirm RED**

Run from `interfaces/webchat` after the memory gate:

```bash
cargo test --lib --no-default-features --features panel-modular panel::projection
```

Expected: FAIL because `TabRegistry::asset_integrity` does not exist.

- [ ] **Step 3: Add minimal metadata storage to `TabRegistry`**

Store optional integrity beside the existing descriptor entry, not in a new registry. Populate it only in `PanelProjection::registry_from_snapshot` after declaration validation. Any direct descriptor removal/replacement must also remove old metadata so a later load cannot inherit stale integrity.

- [ ] **Step 4: Run focused and full modular tests**

```bash
cargo test --lib --no-default-features --features panel-modular panel::projection
cargo test --lib --no-default-features --features panel-modular
```

Expected: all PASS; prior projection/registry behavior remains green.

- [ ] **Step 5: Commit the isolated metadata seam**

```bash
git add interfaces/webchat/src/panel/registry.rs interfaces/webchat/src/panel/projection.rs
git commit -m "feat(panel): retain validated community asset integrity"
```

### Task 2: Verify Community Bytes Before JavaScript Import

**Files:**
- Modify: `interfaces/webchat/panel_loader.js`
- Modify: `interfaces/webchat/index.html`
- Test: `interfaces/webchat/tests/panel_loader.test.mjs`

**Interfaces:**
- Consumes: optional SRI string from Task 1, currently `sha256-<base64>` after projection validation.
- Produces: `loadPanelModule(entry, integrity = null)` with the existing return ABI and stable errors `AssetFetchFailed`, `InvalidAssetIntegrity`, and `IntegrityMismatch`.

- [ ] **Step 1: Write failing loader tests**

Cover:
- valid integrity reaches the existing describe/mount ABI in the browser fixture owned by Task 4;
- malformed integrity returns `InvalidAssetIntegrity`;
- HTTP failure and redirect return `AssetFetchFailed`;
- wrong hash returns `IntegrityMismatch`;
- a mismatch module whose top level increments a test counter leaves the counter at zero, proving no import/default initializer occurred;
- no-integrity built-in calls retain the current direct-import behavior.

- [ ] **Step 2: Run Node loader tests and confirm RED**

```bash
node --test tests/panel_loader.test.mjs
```

Expected: new integrity cases FAIL because `loadPanelModule` neither fetches nor verifies integrity.

- [ ] **Step 3: Implement the integrity-gated path**

Keep `loadPanelModule(entry)` unchanged for null integrity. For a present integrity value:
1. resolve against the existing loader URL;
2. fetch with same-origin credentials and redirect rejection;
3. require a successful response and compute SHA-256 over the exact response bytes with WebCrypto;
4. compare against the declared SRI before any module import;
5. import a single-file Blob module, then run the existing ABI/default-initializer validation;
6. revoke the Blob URL in `finally` after import/initialization settles.

Do not add multi-file resolution or pretend Blob import provides a sandbox. Forward the optional second argument through `globalThis.__alephLoadPanelModule` in `index.html`.

- [ ] **Step 4: Run loader regression tests**

```bash
node --test tests/panel_loader.test.mjs
```

Expected: all existing tests and new failure-path tests PASS; generated-WASM opt-in may remain skipped only when its required glue env is absent.

- [ ] **Step 5: Commit the JS verification boundary**

```bash
git add interfaces/webchat/panel_loader.js interfaces/webchat/index.html interfaces/webchat/tests/panel_loader.test.mjs
git commit -m "feat(panel): verify community module bytes before import"
```

### Task 3: Thread Integrity and Stable Errors Through the Rust Loader

**Files:**
- Modify: `interfaces/webchat/src/panel/loader.rs`
- Modify: `interfaces/webchat/src/panel/loader/browser.rs`
- Test: `interfaces/webchat/tests/panel_loader.test.mjs`

**Interfaces:**
- Consumes: `TabRegistry::asset_integrity(&PanelTabKey) -> Option<&str>` and JS errors from Tasks 1–2.
- Produces: browser load call `load_module(entry, integrity)` and `PanelLoadError::{AssetFetchFailed, InvalidAssetIntegrity, IntegrityMismatch}`.

- [ ] **Step 1: Write failing Rust bridge/loader tests**

Prove the import callback receives the exact optional integrity for built-in and community entries, and JS error names map to the new stable `PanelLoadError` variants. Update the generated-glue test adapter to forward two arguments without weakening the lazy bridge assertion.

- [ ] **Step 2: Run focused loader tests and confirm RED**

```bash
cargo test --lib --no-default-features --features panel-modular panel::loader
```

Expected: FAIL because the loader import callback/bridge has no integrity input and the error variants are absent.

- [ ] **Step 3: Thread metadata through the existing attempt**

Read descriptor and integrity under the same registry lock at activation. Pass `Option<String>` alongside `PanelTabDescriptor` through the existing import closure to `browser::load`; call the two-argument JS bridge and map the three new codes. Do not create another loader or module registry.

- [ ] **Step 4: Run Rust and Node loader tests**

```bash
cargo test --lib --no-default-features --features panel-modular panel::loader
node --test tests/panel_loader.test.mjs
```

Expected: all PASS apart from the documented opt-in generated-glue skip when unavailable.

- [ ] **Step 5: Commit the bridge**

```bash
git add interfaces/webchat/src/panel/loader.rs interfaces/webchat/src/panel/loader/browser.rs interfaces/webchat/tests/panel_loader.test.mjs
git commit -m "feat(panel): pass community integrity through module loader"
```

### Task 4: Fence Integrity Replacement and Prove Browser Mount Cleanup

**Files:**
- Modify: `interfaces/webchat/src/panel/loader.rs`
- Modify: `interfaces/webchat/src/panel/registry.rs` only if Task 1 replacement semantics need a minimal fix
- Test: `interfaces/webchat/tests/panel-spike.spec.ts`

**Interfaces:**
- Consumes: descriptor + integrity attempt identity from Tasks 1–3.
- Produces: late-result rejection when the current registry no longer contains the exact descriptor and integrity captured by that attempt.

- [ ] **Step 1: Write failing late-result tests**

Use deferred imports to prove:
- same key and same descriptor with changed integrity invalidates the old attempt;
- key removal invalidates the old attempt and disposes any late handle;
- stale snapshot replacement cannot reactivate the old integrity;
- an unrelated valid built-in load retains existing behavior.

- [ ] **Step 2: Run the focused test and confirm RED**

```bash
cargo test --lib --no-default-features --features panel-modular panel::loader
```

Expected: the changed-integrity case FAILS because current fencing compares only generation/active key/descriptor.

- [ ] **Step 3: Include integrity in current-attempt validation**

Capture integrity with the descriptor in `LoadState`; `current(...)` must compare both against one current registry snapshot. Reuse existing cancellation/disposal paths rather than adding a second state machine.

- [ ] **Step 4: Add the real browser fixture**

In the artifact-only Playwright suite, serve a self-contained community module through `page.route` with exact known bytes. Prove:
- valid SRI loads, `describe` matches the fixture identity, both roots mount, and idempotent unmount clears them;
- wrong SRI returns `IntegrityMismatch`, executes no top-level/default initializer, and leaves both roots empty;
- redirected content returns `AssetFetchFailed` without execution;
- Blob URLs are revoked after both success and failure paths;
- no SDK, transport, Gateway or production installation behavior is implied.

Run the existing artifact spike build/test with the required memory gates; do not set `PANEL_SPIKE_FULL_SHELL`.

- [ ] **Step 5: Run focused and full regression tests**

```bash
cargo test --lib --no-default-features --features panel-modular
bun run panel:spike:build
bun run panel:spike:test
node --test tests/panel_loader.test.mjs
```

Expected: all applicable tests PASS; live-Gateway-dependent spike cases retain their existing explicit skip.

- [ ] **Step 6: Commit fencing and fixture evidence**

```bash
git add interfaces/webchat/src/panel/loader.rs interfaces/webchat/src/panel/registry.rs interfaces/webchat/tests/panel-spike.spec.ts
git commit -m "test(panel): cover community integrity load fencing"
```

### Task 5: Documentation, Full Verification, Review, and Handoff

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify: `docs/reference/TERMINAL_RUNTIME.md`
- Create: `docs/reports/panel-community-module-loading-validation.md`
- Modify only if verification exposes an owned defect: affected Panel source/test files

**Interfaces:**
- Consumes: all behavior and evidence from Tasks 1–4.
- Produces: an auditable statement of supported single-module loading and the remaining installation/discovery/multi-asset scope.

- [ ] **Step 1: Update only verified documentation facts**

Record the integrity metadata path, exact-byte verification, stable load errors, attempt fencing, Blob cleanup and fixture scope. Explicitly state that install/update/uninstall, production discovery, multi-file modules, external WASM, real Gateway and default-release enablement remain unimplemented.

- [ ] **Step 2: Run Rust verification**

From `interfaces/webchat`, with a memory gate before every cargo invocation:

```bash
cargo fmt -- --check
cargo test --lib --no-default-features --features panel-modular
cargo test --lib
cargo check --lib --no-default-features --features panel-modular
cargo clippy --lib --no-default-features --features panel-modular -- -D warnings
```

Expected: all PASS. If spike-specific source changed, also run the established spike-target projection tests with `ALEPH_PANEL_PILOT_ENTRY=panel-tabs/terminal-pilot/terminal-pilot.js`.

- [ ] **Step 3: Rebuild and run existing Panel fixtures**

Use a clean `PANEL_OUTPUT_ROOT=.panel-modular-dist` and the build CLI `--output=.panel-modular-dist`. Run build/report, build tests, artifact tests, protocol, production-host loopback, loader and memory-gate fixtures. Do not connect a real Gateway.

- [ ] **Step 4: Run `git diff --check` and independent read-only review**

Ask a fresh reviewer to inspect the complete diff for integrity bypass, redirect/module execution order, stale metadata, late-result races, Blob cleanup, built-in regression and overclaimed docs. Fix concrete findings with RED → GREEN and resume the same reviewer for closure.

- [ ] **Step 5: Write the validation report and final commit**

Include actual commands, counts, skips, fixture build ID, review findings/fixes and deferred scope. Finish with a clean worktree and a conventional final commit if documentation or review fixes are uncommitted. Do not merge or push.

English completion note: this stage proves integrity-gated loading for explicitly trusted, self-contained community ES module fixtures only. It does not prove a production plugin installation lifecycle, arbitrary JS/WASM asset graphs, live Gateway behavior, or a safe untrusted same-origin sandbox.
