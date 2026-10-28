# Stage E Bounded Executable-Code Loading Plan

> **For next session:** use `docs/superpowers/prompts/2026-10-28-gateway-stage-e-code-loading-prompt.md`.
>
> **Status:** handoff draft for the next Stage E slice. The mechanism is limited to the existing Extism runtime and one real test guest. Physical JIT/RAM reclamation and production channel dynamism are not pre-approved claims.
>
> **Handoff base:** `main` at `ee6e65885` when written. Re-read actual HEAD before branching.

## Goal / 目标

为一个真实 Extism test guest 闭合 executable-code lifecycle 证据：cold discovery/read/projection 不读取 guest bytes、不构造 runtime/instance；显式 activation 读取并执行真实 guest；stop/replacement 使旧 binding 失效并释放 exact old instance；失败不伪造 clean/unloaded。

这补足 Stage D 的 runtime-resource 证据，但不证明内置静态 channel code 已被卸载，也不以 binary size 或 RSS 代替 code-lifecycle 证据。

## Non-goals / 非目标

- No native dylib hot unload, `unsafe`, `libloading`, new Wasmtime runtime or new dependency.
- No second enabled/mount/registry/owner/cleanup authority and no new process/supervisor model.
- No production Gateway/Extension mount, Feishu/R2, Terminal/Memory/Sandbox migration or all-channel packaging.
- No claim that dropping an Extism `Plugin` necessarily unloads engine-wide JIT code, anonymous mappings or physical RAM.

## Task 1 — Establish the actual loader and memory baseline

**Files:** read only; write evidence under `.superpowers/sdd/2026-10-28-gateway-stage-e-code-loading-plan/`.

- [ ] Confirm the docs are already part of the current clean `main`; create `stage-e-code-loading` from actual `main`. Record HEAD and any new worktree path.
- [ ] Census all `WasmRuntime` construction, `PluginBuilder::build`, `Wasm::file`, `LoadedWasmPlugin` `Arc` clones, `WasmBinding`, `WasmMountIdentity`, `unload_plugin`, `remove_if_current`, `PluginLoader::shutdown` and disposer drop paths. Include exact file:line references. Decide which bulk shutdown evidence is in scope and keep it separate from identity-fenced replacement cleanup.
- [ ] Verify which current production paths call `declare_cold`, `activate_owner`, `load_plugin_with` and `ExtensionManager::unmount`/stop/dispose. Do not copy the stale 2026-10-22 “no production transitions” statement without checking.
- [ ] Run existing focused WASM/Extension tests as baseline. Record counts and any pre-existing failures; do not label a new regression without A/B evidence.
- [ ] Define the evidence vocabulary before writing assertions:
  - manifest read: discovery metadata only;
  - guest-byte read: actual `.wasm` content enters the process;
  - instance create/drop: actual Extism owner boundary;
  - OS mapping: platform-specific named/anonymous/executable pages;
  - RSS: process-level noisy observation, never a per-provider unload proof.
- [ ] Inspect only tools available on the current host. `vmmap` may show whole-process mappings; `otool`/`nm` do not prove runtime guest mappings. If per-guest JIT mappings cannot be attributed, record that limitation and use loader/instance receipts for the bounded claim.

### Task 1 exit

A source-backed ownership census and baseline command ledger distinguish code loading from runtime resource allocation. No production edits yet.

## Task 2 — Add behavioral RED contracts

**Files:**
- Modify: `src/extension/loader.rs` for a `#[cfg(test)]` loader-owned probe that exists before lazy `WasmRuntime` construction, plus a `pub(crate)` test accessor if needed by lifecycle tests
- Modify: `src/extension/runtime/wasm/mod.rs` for exact loaded-instance create/drop observation
- Modify: `src/extension/runtime/wasm/tests/guest_fixture.rs` only if a new guest shape is essential
- Modify: existing `src/extension/lifecycle.rs` `b3_*` test module that already covers activation/stop/replacement; do not create a parallel lifecycle harness
- Test evidence: ignored SDD directory

- [ ] Add a loader-owned test-only probe created in `PluginLoader::new`, cloned into `WasmRuntime` when it is lazily constructed, with `guest_bytes_read` and byte count. Add exact-instance `guest_instance_created`/`guest_instance_dropped` receipts to the loaded incarnation. No process-global mutable registry and no production receipt field.
- [ ] Extend the existing `src/extension/lifecycle.rs` `b3_*` tests with `cold_discovery_and_projections_do_not_load_guest_code`: before a runtime exists, real manifest discovery and cold list/descriptor/projection paths produce zero guest reads, zero instances and `PluginLoader::is_wasm_runtime_active() == false`. Manifest reads are allowed and separately counted.
- [ ] Extend the existing activation positive control: explicit activation reads nonzero guest bytes, creates one instance and a real guest call returns fixture payload A. This positive control must pass before any later zero claim is trusted.
- [ ] Extend the existing stop/replacement tests: after real `ExtensionManager::unmount().await` returns, release every test-retained binding/identity/receipt/Arc clone, then assert the exact old instance drop. Never assert drop while intentionally retaining an Arc, and never assert from inside a drop hook.
- [ ] Extend the existing replacement test through unmount→mount, not two direct `load_plugin_with` calls: payload B requires a real new read/instance; old generation/binding fails; old and new drop receipts are identity-specific, with no double count.
- [ ] Add failed-build and failed-activation assertions: invalid WASM may have a real byte-read attempt but no created instance; partial activation follows the existing disposer and exposes any residual. A no-op error is not cleanup success.
- [ ] Run each new receipt assertion before the production loading/drop change. Capture valid behavioral RED (assertion failure or watchdog), not parser/import/setup failure. Existing activation, stale-binding and replacement behavior is already covered and must not be weakened.

### Task 2 exit

Each missing code-lifecycle observation has a real failing contract and an active positive control. Commit RED tests only if useful for review; do not claim Stage E closed.

## Task 3 — Implement the smallest existing-runtime loading boundary

**Files:** expected `src/extension/loader.rs`, `src/extension/runtime/wasm/mod.rs` and existing lifecycle tests only. Any additional production file requires a stated reason.

- [ ] Replace the unobservable `Wasm::file` boundary with explicit `std::fs::read` followed by `Wasm::data`; record the real read result/byte count before `PluginBuilder::build`. Accept the temporary bytes-plus-compiled-artifact peak memory trade-off.
- [ ] Record instance creation only after `PluginBuilder::build` succeeds. A read attempt or successful byte read is not an instance.
- [ ] Add exact-instance drop observation to `LoadedWasmPlugin` under `#[cfg(test)]`; only claim it after `unmount().await` and release of every test-held Arc/binding/identity/receipt clone. Keep production behavior unchanged; no drop assertion from inside a drop hook.
- [ ] Reuse current admission close, in-flight drain, `WasmMountIdentity`/`WasmBinding` identity checks, provider generation and disposer. Do not add a loader map, deferred cleanup task or second lifecycle state machine.
- [ ] Preserve fail-closed replacement: same-ID new code cannot make old bindings valid; late disposer cannot remove the replacement; failed cleanup retains observable residual. Test replacement through the real unmount→mount path.
- [ ] If evidence requires changing global Extism engine/cache behavior, public lifecycle semantics, native unloading or process isolation, stop and request an architecture decision instead of expanding scope.

### Task 3 exit

The five bounded contracts are behavioral GREEN without a new authority. Any engine/JIT/RSS limitation remains explicit.

## Task 4 — Verify the actual claim and regression surface

Every Cargo command below must be immediately preceded by a separate `python qa/browser_live/check_memory.py` returning `ALLOW`.

- [ ] Run every new focused test three times serially with `--exact --nocapture`; save logs and exact exit/passed/failed/ignored counts.
- [ ] Run existing Extension/WASM suites selected in Task 1, including runtime-owner stop/replacement/dispose tests and `tests/wasm_capability_test.rs` where applicable. `#[cfg(test)]` receipt assertions belong only to `src/` unit tests; the integration test can verify behavior but cannot access those receipts.
- [ ] Re-run Stage D and Stage C regressions:

```bash
cargo test -p alephcore --lib extension::runtime::wasm::
cargo test -p alephcore --lib extension::lifecycle::tests::b3_
cargo test -p alephcore --test wasm_capability_test
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::
cargo test -p alephcore --test sms_delivery_policy_test
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo test -p alephcore --lib gateway::interfaces::webhook::
cargo test -p alephcore --lib capability::
cargo check -p alephcore
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo fmt --all --check
git diff --check
```

- [ ] If the implementation changes feature gating, public slot APIs, bins or shared interfaces, add the matching no-default/default-feature, slot-api, example and workspace checks. Do not substitute targeted suites for newly affected surfaces.
- [ ] Record platform and architecture. Use OS/memory observations only as supporting evidence; if per-guest JIT pages cannot be isolated, say so rather than inventing a threshold.
- [ ] Self-review that all temporary guest files/tasks/handles are removed after each test, including failed cases.

## Task 5 — Independent review and truthful closure

- [ ] Dispatch a fresh read-only reviewer for the complete diff and original source. Require Critical/Important/Minor and a Ready verdict; do not rerun the entire suite in the reviewer.
- [ ] Review specifically: cold read side effects, real guest positive controls, all `Arc` owners, stale binding/generation behavior, partial activation/dispose, exact drop receipts, no second authority, and no OS-unload overclaim.
- [ ] Resolve Critical/Important and re-review. If reviewer infrastructure fails, preserve the full error and perform the same checklist manually without claiming independent approval.
- [ ] Only after GREEN and review, write bilingual `docs/superpowers/runs/2026-10-28-gateway-stage-e-code-loading-run.md`; update source plan Stage E, `docs/reference/GATEWAY.md`, `docs/reference/PLUGIN_SYSTEM.md` and `FEATURE_LOCATOR.md` §3.5i with the exact bounded claim and unproven engine/JIT/RSS/native-channel boundaries. Check `PLUGIN_SYSTEM.md` for stale `Wasm::file`/unload wording.
- [ ] Commit docs separately, confirm clean branch and `git diff --check`. Do not merge, push, clean worktrees or begin another stage without user instruction.

## Final handoff must answer

1. What was loaded/read at cold, activation and replacement?
2. Which exact object was dropped at stop, and which handles had to be released first?
3. What remains in the Extism engine/process that this slice cannot measure?
4. Did any built-in static channel become dynamically loaded? (Expected: no.)
5. Which commands actually ran, which reviewer ran, and what remains deferred?
