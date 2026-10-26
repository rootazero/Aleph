# Panel Tab UI Capability Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the Panel UI Capability contract and prove that one wide Tab-shaped WASM module can be built separately, loaded on demand, mounted into the Shell's two-panel frame, and disposed without loading the pilot into the initial Shell bundle.

**Architecture:** Keep the current Panel as the production fallback while adding a feature-gated spike path. The host owns a typed Tab descriptor, registry, loader state machine, fixed sidebar/content roots, and a minimal Panel SDK. A separate `terminal-pilot` WASM crate is built with an explicit browser module ABI and loaded through a small JavaScript dynamic-import shim; it does not import the host's Leptos contexts or `DashboardState` directly.

**Tech Stack:** Rust 1.99 workspace, Leptos 0.8 CSR, `wasm-bindgen` 0.2.129, `web-sys`, `serde`, `serde_json`, Trunk for the existing host, direct `wasm-bindgen --target web` output for the pilot experiment, Bun/Playwright for browser checks.

**Spec:** `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`

## Global Constraints

- All implementation edits happen in a dedicated Git worktree based on `origin/dev`; use a branch of at most three hyphen-separated words.
- The first implementation slice is Stage 0–1 only: the descriptor/lifecycle contract, baseline measurement, and one disposable dynamic-loading spike.
- The existing ExtensionManager, plugin state, Capability registry, and lifecycle remain authoritative; the Panel does not create a second plugin registry.
- The first spike uses the wide Panel path. Phone and tablet continue using the current application path.
- The module API major is `1`; unsupported major versions are rejected before mount.
- The Shell owns the outer frame and supplied sidebar/content roots. A Tab owns only descendants of those roots and must release every subscription and global listener it creates.
- UI unmounting does not stop Core providers or sessions. Provider lifecycle requires an explicit Core operation.
- Same-origin browser WASM is treated as trusted or verified code, never as a security sandbox.
- Do not add public Protocol or Server HTTP APIs in the spike unless the current host bridge is proven insufficient; if such an API is required, add the shared contract first and run the repository's client generation step.
- Tests run from affected package directories; the repository-wide verification is `bun run check` from the repository root.

## Review Focus

1. **Descriptor identity and ordering:** duplicate `(plugin_id, tab_id)` entries, built-in namespace shadowing, and equal order values must produce deterministic registry behavior. Test in Task 2.
2. **Load failure and incompatible API:** a missing module, rejected dynamic import, malformed descriptor, or unsupported API major must leave the Shell usable and show a retryable error. Test in Task 4.
3. **Unmount during async work:** an in-flight load or SDK callback must not append DOM, update disposed state, or leave a subscription after the Tab is removed. Test in Task 4.
4. **Initial bundle exclusion:** the pilot implementation must not be statically imported by the Shell, and the pilot asset must be requested only after activation. Verify in Task 5.
5. **Core/UI lifecycle separation:** leaving or disabling the UI must dispose browser resources without issuing an implicit provider stop. Verify with SDK call recording in Task 4 and browser observation in Task 5.

## File Map

- **Create:** `shared/protocol/src/panel.rs` — single-source wire/domain types for UI Tab descriptors, identity, trust, lifecycle status, and API version.
- **Modify:** `shared/protocol/src/lib.rs` — register and re-export the new protocol module.
- **Create:** `interfaces/webchat/src/panel/mod.rs` — Panel Shell spike module and public wiring surface.
- **Create:** `interfaces/webchat/src/panel/registry.rs` — pure descriptor registry and deterministic ordering.
- **Create:** `interfaces/webchat/src/panel/loader.rs` — load state machine, API validation, mount/unmount orchestration, and JavaScript bridge.
- **Create:** `interfaces/webchat/src/panel/sdk.rs` — minimal host SDK passed to a Tab module.
- **Modify:** `interfaces/webchat/src/lib.rs` — expose the feature-gated Panel spike module.
- **Modify:** `interfaces/webchat/src/app.rs` — add the feature-gated Shell frame without changing the default production route behavior.
- **Modify:** `interfaces/webchat/Cargo.toml` — add the `panel-tab-spike` feature and only the dependencies needed by the loader contract.
- **Create:** `interfaces/webchat/panel_loader.js` — dynamic import and module ABI validation shim copied into the host output.
- **Modify:** `interfaces/webchat/index.html` — copy the loader shim and expose the pilot asset root for the spike build.
- **Create:** `interfaces/webchat/tabs/terminal-pilot/Cargo.toml` — independent WASM crate for the disposable pilot.
- **Create:** `interfaces/webchat/tabs/terminal-pilot/src/lib.rs` — Leptos pilot module exporting `describe`, `mount`, and `MountedTab::unmount` through `wasm-bindgen`.
- **Create:** `interfaces/webchat/tabs/terminal-pilot/index.html` — standalone Trunk entry used for the alternative build comparison.
- **Modify:** `Cargo.toml` — add the pilot crate to workspace members.
- **Create:** `interfaces/webchat/scripts/build-panel-spike.ts` — reproducible build of host and pilot artifacts, manifest placement, and size report.
- **Create:** `interfaces/webchat/scripts/panel-spike-report.ts` — derive raw/compressed artifact sizes and duplicate dependency measurements from build outputs.
- **Create:** `interfaces/webchat/tests/panel-spike.spec.ts` — Playwright acceptance checks for deferred load, mount, error handling, and cleanup where the existing browser harness can serve the spike output.
- **Modify:** `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md` — append measured spike results and the selected build arrangement after Task 5; do not rewrite the approved goals.

### Task 1: Create the shared UI Tab contract

**Files:**
- Create: `shared/protocol/src/panel.rs`
- Modify: `shared/protocol/src/lib.rs`

**Interfaces:**
- Produces `PanelTabKey { plugin_id: String, tab_id: String }` with a stable `qualified()` representation.
- Produces `PanelTabDescriptor { key: PanelTabKey, label: String, icon: String, order: i32, entry: String, api_major: u16, trust: PanelUiTrust, permissions: Vec<String>, status: PanelTabStatus, status_detail: Option<String> }`.
- Produces `PanelUiTrust::{Verified, Trusted}` with lowercase serde names.
- Produces `PanelTabStatus::{Enabled, Disabled, Incompatible, Failed}` with lowercase serde names.
- Produces `PANEL_UI_API_MAJOR: u16 = 1`.

- [ ] **Step 1: Write the contract tests in `shared/protocol/src/panel.rs`**

  Add module tests that assert:

  - a descriptor serializes with the exact stable field names and lowercase enum values;
  - serialize/deserialize round-trips preserve `PanelTabKey`, permissions, trust, status, and optional diagnostic text;
  - `PanelTabKey::qualified()` uses `plugin_id + ":" + tab_id` and does not collapse distinct plugin namespaces;
  - a missing optional `status_detail` deserializes as `None`.

- [ ] **Step 2: Run the focused protocol test before implementation**

  Run: `cargo test -p aleph-protocol panel --lib`

  Expected: FAIL because `panel` is not registered and the new types do not exist.

- [ ] **Step 3: Implement the single-source contract**

  Define the types in `shared/protocol/src/panel.rs` with `Serialize`, `Deserialize`, `Clone`, `Debug`, `Eq`, and `PartialEq` as appropriate. Use serde field names directly rather than a second hand-written JSON schema. Register `pub mod panel;` and the explicit public re-exports in `shared/protocol/src/lib.rs`.

- [ ] **Step 4: Run the focused protocol test after implementation**

  Run: `cargo test -p aleph-protocol panel --lib`

  Expected: PASS with no unknown-field or enum spelling warnings.

- [ ] **Step 5: Commit the contract**

  ```bash
  git add shared/protocol/src/panel.rs shared/protocol/src/lib.rs
  git commit -m "feat(protocol): add panel tab capability contract"
  ```

### Task 2: Implement the pure Tab registry

**Files:**
- Create: `interfaces/webchat/src/panel/mod.rs`
- Create: `interfaces/webchat/src/panel/registry.rs`
- Modify: `interfaces/webchat/src/lib.rs`
- Modify: `interfaces/webchat/Cargo.toml`

**Interfaces:**
- Consumes `aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey, PanelTabStatus}` from Task 1.
- Produces `pub struct TabRegistry`.
- Produces `TabRegistry::insert_or_replace(&mut self, descriptor: PanelTabDescriptor) -> Result<(), RegistryError>`.
- Produces `TabRegistry::remove(&mut self, key: &PanelTabKey) -> bool`.
- Produces `TabRegistry::get(&self, key: &PanelTabKey) -> Option<&PanelTabDescriptor>`.
- Produces `TabRegistry::ordered(&self) -> Vec<PanelTabDescriptor>`.
- Produces `TabRegistry::active_after_removal(&self, removed: &PanelTabKey) -> Option<PanelTabKey>`.
- Produces `RegistryError::{BuiltInShadowed, DuplicateKey, InvalidEntry}`.

- [ ] **Step 1: Write pure registry tests**

  Add `#[cfg(test)]` tests in `registry.rs`:

  - `ordered_uses_order_then_qualified_key` inserts equal-order entries and asserts stable ordering;
  - `plugin_cannot_shadow_reserved_builtin` rejects a plugin descriptor whose key collides with an Aleph-reserved key;
  - `replacement_is_explicit_and_preserves_one_entry` replaces an existing key only through the replacement operation and never returns two rows;
  - `disabled_entries_are_not_ordered_for_mount` keeps disabled descriptors queryable for re-enable but excludes them from the active list;
  - `active_after_removal_falls_back_to_first_enabled_tab` returns the default enabled entry after the active key is removed.

- [ ] **Step 2: Run the focused registry tests before implementation**

  Run: `cargo test -p aleph-panel --lib --features panel-tab-spike panel::registry`

  Expected: FAIL because the registry module and feature-gated panel module do not exist.

- [ ] **Step 3: Implement deterministic registry logic**

  Store descriptors keyed by `PanelTabKey`. Treat the reserved built-in namespace as an explicit constant in the registry. Validate non-empty plugin id, Tab id, label, entry, and API major before insertion. Keep disabled rows available for status display but exclude them from `ordered()` and mount selection. Sort only at the projection boundary by `(order, qualified_key)`.

- [ ] **Step 4: Register the module without changing the default app**

  Add `pub mod panel;` behind the `panel-tab-spike` feature in `interfaces/webchat/src/lib.rs`. Add the empty feature to `interfaces/webchat/Cargo.toml`; do not import the existing `TerminalView` into the new module.

- [ ] **Step 5: Run the focused registry tests after implementation**

  Run: `cargo test -p aleph-panel --lib --features panel-tab-spike panel::registry`

  Expected: PASS, including deterministic ordering and namespace collision behavior.

- [ ] **Step 6: Commit the registry**

  ```bash
  git add interfaces/webchat/src/panel interfaces/webchat/src/lib.rs interfaces/webchat/Cargo.toml
  git commit -m "feat(panel): add deterministic tab registry"
  ```

### Task 3: Define the minimal Panel SDK and Shell lifecycle state

**Files:**
- Create: `interfaces/webchat/src/panel/sdk.rs`
- Modify: `interfaces/webchat/src/panel/mod.rs`

**Interfaces:**
- Produces `pub struct PanelSdk` containing only validated host operations.
- Produces `PanelSdk::new(state: DashboardState, tab: PanelTabKey) -> Self`.
- Produces `PanelSdk::invoke(&self, method: &str, params: serde_json::Value) -> LocalBoxFuture<'static, Result<serde_json::Value, String>>`.
- Produces `PanelSdk::subscribe(&self, topic: &str) -> LocalBoxFuture<'static, Result<PanelSubscription, String>>`.
- Produces `PanelSdk::navigate(&self, key: PanelTabKey) -> Result<(), PanelSdkError>`.
- Produces `PanelSdk::theme(&self) -> PanelThemeSnapshot` and `PanelSdk::locale(&self) -> String` as read-only snapshots.
- Produces `PanelSubscription::dispose(self)` and makes disposal idempotent.
- Produces `PanelSdkError::{Denied, InvalidMethod, InvalidNavigation, Disposed}`.

- [ ] **Step 1: Write SDK lifecycle tests**

  Test that:

  - `invoke` records or forwards an allowed host operation without exposing `DashboardState` to the module;
  - an operation outside the declared permission set returns `Denied` without calling RPC;
  - disposing a subscription twice is harmless;
  - SDK disposal prevents later calls and does not issue a provider-stop operation;
  - navigation rejects a key absent from the current registry.

  Use the existing `DashboardState` test construction and RPC recording patterns in `interfaces/webchat/src/context.rs`; do not introduce `globalThis` mocks.

- [ ] **Step 2: Run the focused SDK tests before implementation**

  Run: `cargo test -p aleph-panel panel::sdk`

  Expected: FAIL because the SDK types and methods do not exist.

- [ ] **Step 3: Implement the minimal permission-gated SDK**

  Keep the SDK as an adapter over the existing `DashboardState` and host-owned registry. The Tab receives an opaque SDK value through the browser ABI; it never receives `DashboardState`, a WebSocket handle, or a Core provider handle. Keep storage/theme/locale as narrow methods or snapshots, not a general JavaScript object with arbitrary host access.

- [ ] **Step 4: Add the lifecycle state model in `interfaces/webchat/src/panel/mod.rs`**

  Define `PanelMountState::{Idle, Loading, Mounted, Failed, Disabled}` and the transitions needed by the loader. Make `Mounted` carry only the module's disposable handle and Tab key, not a Core provider owner.

- [ ] **Step 5: Run the focused SDK and module tests**

  Run: `cargo test -p aleph-panel panel::sdk panel::registry`

  Expected: PASS, with no provider lifecycle calls caused by UI disposal.

- [ ] **Step 6: Commit the SDK state boundary**

  ```bash
  git add interfaces/webchat/src/panel/sdk.rs interfaces/webchat/src/panel/mod.rs
  git commit -m "feat(panel): define tab sdk lifecycle boundary"
  ```

### Task 4: Implement the dynamic module ABI and feature-gated Shell

**Files:**
- Create: `interfaces/webchat/src/panel/loader.rs`
- Create: `interfaces/webchat/panel_loader.js`
- Modify: `interfaces/webchat/src/panel/mod.rs`
- Modify: `interfaces/webchat/src/app.rs`
- Modify: `interfaces/webchat/index.html`

**Interfaces:**
- JavaScript module exports `describe()`, `mount(sidebarRoot, contentRoot, sdk)`, and the returned `MountedTab.unmount()` method.
- Produces `PanelLoader::new(registry: TabRegistry) -> Self`.
- Produces `PanelLoader::activate(&mut self, key: PanelTabKey, sidebar_root: Element, content_root: Element, sdk: PanelSdk) -> LocalBoxFuture<'static, Result<(), PanelLoadError>>`.
- Produces `PanelLoader::deactivate(&mut self) -> Result<(), PanelLoadError>`.
- Produces `PanelLoadError::{ImportFailed, InvalidDescriptor, UnsupportedApiMajor, MountFailed, UnmountFailed, Cancelled}`.
- Produces a feature-gated `PanelShell` view that renders exactly one sidebar root, one content root, loading/error state, retry action, and the registry-driven active Tab.

- [ ] **Step 1: Write loader state tests**

  Add tests for:

  - an unsupported `api_major` is rejected before `mount` is called;
  - a malformed module result becomes `InvalidDescriptor`;
  - `ImportFailed` leaves the registry and Shell usable;
  - activating another key first disposes the current mounted handle;
  - deactivation during a pending load marks the request cancelled and ignores its eventual completion;
  - `unmount` is called exactly once even when the Tab is disabled and then removed.

- [ ] **Step 2: Run the focused loader tests before implementation**

  Run: `cargo test -p aleph-panel panel::loader`

  Expected: FAIL because the loader and JavaScript ABI are not present.

- [ ] **Step 3: Implement the JavaScript loader shim**

  In `interfaces/webchat/panel_loader.js`, dynamically import the descriptor's entry URL, call its default initializer when the generated module requires initialization, validate that `describe` and `mount` are functions, and return a normalized module object. Do not expose arbitrary host objects; pass a narrow SDK facade and the two root elements only.

- [ ] **Step 4: Implement the Rust loader state machine**

  Use `wasm_bindgen` to call the shim. Validate the descriptor against `PANEL_UI_API_MAJOR` and the current registry before invoking the module. Track a monotonically increasing activation generation so an old async load cannot mount after navigation or disablement. On every terminal path, dispose the previous mounted handle before publishing the next state.

- [ ] **Step 5: Wire the feature-gated Shell into the existing wide app**

  Add the Shell under `cfg(feature = "panel-tab-spike")` at the wide content boundary in `interfaces/webchat/src/app.rs`. Keep default builds on the current `PanelMode` rendering and retain the existing `TerminalView` import for the default path. The feature-gated Shell must use a static pilot descriptor and must not add a second authoritative plugin registry.

- [ ] **Step 6: Copy the loader asset in the host build**

  Add the Trunk asset declaration in `interfaces/webchat/index.html` for `panel_loader.js`. Keep the pilot asset root configurable by the generated descriptor so the same Shell can later consume installed plugin assets.

- [ ] **Step 7: Run focused compile and test checks**

  Run:

  ```bash
  cargo test -p aleph-panel panel::loader panel::registry panel::sdk
  cargo check -p aleph-panel --features panel-tab-spike --target wasm32-unknown-unknown
  ```

  Expected: PASS; the default feature set must continue to compile without the pilot.

- [ ] **Step 8: Commit the host loader**

  ```bash
  git add interfaces/webchat/src/panel interfaces/webchat/src/app.rs interfaces/webchat/index.html
  git commit -m "feat(panel): add feature-gated dynamic tab loader"
  ```

### Task 5: Build the independent terminal-shaped pilot

**Files:**
- Modify: `Cargo.toml`
- Create: `interfaces/webchat/tabs/terminal-pilot/Cargo.toml`
- Create: `interfaces/webchat/tabs/terminal-pilot/src/lib.rs`
- Create: `interfaces/webchat/tabs/terminal-pilot/index.html`

**Interfaces:**
- Produces a separate `panel-tab-terminal-pilot` WASM crate with no dependency on `aleph-panel`.
- Exports `describe() -> PanelTabDescriptor` through the browser module ABI.
- Exports `mount(sidebar_root: web_sys::Element, content_root: web_sys::Element, sdk: JsValue) -> Result<MountedTab, JsValue>`.
- Exports `MountedTab::unmount(&mut self)` and makes it idempotent.
- Renders a terminal-shaped two-surface UI: a session list in the sidebar, a canvas/log area in the content root, and one host SDK action that demonstrates capability invocation without holding a host transport handle.

- [ ] **Step 1: Add the pilot crate and verify it is not part of the host dependency graph**

  Add the crate to the workspace and keep its dependencies limited to Leptos, `wasm-bindgen`, `web-sys`, `serde`, `serde_json`, and `aleph-protocol` as needed. Do not depend on `aleph-panel`; that dependency would statically pull the entire host UI into the pilot.

- [ ] **Step 2: Write the pilot module ABI tests**

  Add host-compilable pure tests for descriptor identity, API major `1`, the `terminal-pilot` entry name, and the idempotent mounted-handle state. Browser-only DOM mount behavior is covered in Task 6.

- [ ] **Step 3: Implement the pilot component**

  Mount only into the supplied roots. Use a scoped cleanup owner for event listeners and SDK subscriptions. The component may call the SDK's permitted read operation, but it must not import or construct `DashboardState`, `WasmConnector`, or a Core provider.

- [ ] **Step 4: Provide the standalone Trunk entry**

  Add a minimal `index.html` for the alternative multi-Trunk build. It exists to compare artifact shape; it is not the host's production entry.

- [ ] **Step 5: Run the pilot build check**

  Run:

  ```bash
  cargo check -p panel-tab-terminal-pilot --target wasm32-unknown-unknown
  cargo test -p panel-tab-terminal-pilot
  ```

  Expected: PASS; the pilot compiles independently and does not cause `aleph-panel` to be rebuilt as a dependency.

- [ ] **Step 6: Commit the pilot module**

  ```bash
  git add Cargo.toml interfaces/webchat/tabs/terminal-pilot
  git commit -m "test(panel): add independent terminal tab pilot"
  ```

### Task 6: Make the spike reproducible and measure the result

**Files:**
- Create: `interfaces/webchat/scripts/build-panel-spike.ts`
- Create: `interfaces/webchat/scripts/panel-spike-report.ts`
- Create: `interfaces/webchat/tests/panel-spike.spec.ts`
- Modify: `interfaces/webchat/package.json`
- Modify: `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`

**Interfaces:**
- `build-panel-spike.ts` accepts `--clean` and `--arrangement=wasm-bindgen|trunk` and writes all artifacts under a disposable `interfaces/webchat/.panel-spike/` directory.
- `panel-spike-report.ts` prints a stable JSON report with `shell_raw_bytes`, `pilot_raw_bytes`, `shell_brotli_bytes`, `pilot_brotli_bytes`, `pilot_request_count`, and `duplicate_dependency_bytes`.
- The Playwright test suite targets the built spike URL and observes network requests and DOM cleanup; it does not mock `globalThis` or the host transport.

- [ ] **Step 1: Add reproducible build commands**

  Add package scripts for `panel:spike:build` and `panel:spike:report`. The build script must:

  1. clean `.panel-spike/` when requested;
  2. build the current host with the `panel-tab-spike` feature;
  3. build the pilot with `cargo build --release --target wasm32-unknown-unknown`;
  4. produce browser-loadable pilot glue with `wasm-bindgen --target web` under `.panel-spike/panel-tabs/terminal-pilot/`;
  5. write a descriptor manifest whose entry points at the pilot glue module;
  6. preserve the existing host build output separately from the pilot output.

- [ ] **Step 2: Measure both build arrangements**

  Run the direct `wasm-bindgen` arrangement first. Run the standalone multi-Trunk arrangement using the pilot `index.html` if the local Trunk tool is available. Record which arrangement provides explicit module exports, reproducible asset paths, useful source maps, and the smallest initial Shell artifact. If one arrangement cannot satisfy the dynamic ABI, record the exact failure and select the other; do not silently combine incompatible output formats.

- [ ] **Step 3: Add browser acceptance tests**

  Assert that:

  - the initial page does not request the pilot module;
  - activating the pilot produces exactly one pilot module request and renders both roots;
  - the pilot's host SDK action reaches the host bridge or returns the documented permission error;
  - navigating away removes the pilot-owned DOM and records no late callback error;
  - re-entering follows the chosen cache/reload behavior;
  - an invalid entry URL or API major shows a retryable error while another registered Tab remains usable.

- [ ] **Step 4: Generate and inspect the measurement report**

  Run:

  ```bash
  bun run panel:spike:build -- --clean --arrangement=wasm-bindgen
  bun run panel:spike:report
  bun test interfaces/webchat/tests/panel-spike.spec.ts
  ```

  Expected: the report contains non-zero Shell and pilot sizes, the initial pilot request count is zero before activation, the activated request count is one, and the browser assertions pass.

- [ ] **Step 5: Update the approved design with evidence**

  Append a short dated spike-results section to `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md` containing the selected arrangement, raw and compressed sizes, instantiation timing, active memory observation, duplicate dependency estimate, cleanup result, and limitations. Do not turn the disposable pilot into a claim that the real `TerminalView` has been migrated.

- [ ] **Step 6: Run repository verification**

  Run from the repository root:

  ```bash
  bun run check
  ```

  Run focused checks again from package directories if the full check reports a package-specific failure. Expected: the default `aleph-panel` build remains green, the pilot is independently buildable, and no public Server/Protocol API was changed without generated client output.

- [ ] **Step 7: Commit the spike evidence**

  ```bash
  git add interfaces/webchat/scripts interfaces/webchat/tests interfaces/webchat/package.json docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md
  git commit -m "test(panel): measure dynamic tab wasm spike"
  ```

## Self-review

- **Spec coverage:** Goals and lifecycle are covered by Tasks 1–4; independent Tab packaging and loading by Tasks 5–6; measurement and build-arrangement selection by Task 6; worktree and validation constraints are global constraints.
- **Scope:** The plan does not implement community manifest installation, server-side descriptor projection, all built-in Tab migration, or phone/tablet migration. Those belong to later plans after the spike evidence is accepted.
- **Type consistency:** `PanelTabKey`, `PanelTabDescriptor`, `PANEL_UI_API_MAJOR`, `TabRegistry`, `PanelSdk`, `PanelLoader`, and `MountedTab` are introduced before their consumers. The browser ABI uses the same `describe`/`mount`/`unmount` names in the host shim and pilot.
- **Review focus coverage:** Registry collision/order tests are in Task 2; API/load failure and cancellation tests are in Task 4; initial bundle and request timing plus DOM cleanup are in Task 6; Core/UI separation is tested in Tasks 3, 4, and 6.
- **Proportion:** This plan deliberately stops after a disposable, measurable spike. It does not prescribe the later production migration details that the spike is meant to inform.

## Handoff

The implementation must begin by invoking the repository's worktree workflow and creating a dedicated branch from `origin/dev`; no implementation edits belong in the main worktree. After the plan is approved, select either subagent-driven execution or native execution, then implement one task at a time with its focused check and commit.
