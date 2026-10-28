# Stage F Provider-Boundary Code Lifecycle Plan / Stage F Provider 边界代码生命周期计划

> **Status / 状态:** queued for the next session. This plan is intentionally bounded to a test composition around the existing ProviderHandle/RuntimeOwner/ExtensionManager authority. It does not authorize production Zahir mount or provider/channel integration.
>
> **Base / 基线:** current merged `main` at `07250ba1f`; Stage E code commit `7c31a5fa6`; Stage E run record [`docs/superpowers/runs/2026-10-28-gateway-stage-e-code-loading-run.md`](../runs/2026-10-28-gateway-stage-e-code-loading-run.md).
>
> **Prompt / 接续入口:** [`docs/superpowers/prompts/2026-10-29-gateway-stage-f-provider-boundary-prompt.md`](../prompts/2026-10-29-gateway-stage-f-provider-boundary-prompt.md)

## Intent / 意图

Stage E established a bounded Extism guest-byte and exact loaded-instance boundary inside the existing Extension lifecycle. The next unclosed contract is whether that same guest instance lifecycle is constrained by the existing provider state, generation and call-admission boundary used by Capability Core.

Stage F should prove that relationship with one real test guest and an existing provider composition. The result must show that disabled/discovered/cold state has no guest code side effect, Ready activation creates and executes one current instance, stop/replacement fences old calls, and exact old instance drop occurs only after the existing owner/binding/identity `Arc` holders are released.

Stage E 已经在既有 Extension lifecycle 内建立了 guest bytes 与 exact loaded instance 的有界证据。下一条未闭合 contract 是：同一个 guest instance lifecycle 是否确实受 Capability Core 既有 provider state、generation 与 call-admission 边界约束。

Stage F 用一个真实 test guest 和现有 provider composition 证明这一关系：disabled/discovered/cold 不产生 guest code side effect；Ready activation 创建并执行当前 instance；stop/replacement 使旧调用失效；只有释放既有 owner/binding/identity `Arc` holders 后才观察到旧 instance 的精确 drop。

## Goals / 目标

1. Source-back the current relationship between `ProviderHandle`/`ProviderLifecycle`, `RuntimeOwner`, `ExtensionManager`, `PluginLoader`, `WasmBinding`, `WasmMountIdentity`, generation and disposer.
2. Add behavior RED contracts in the existing provider composition, not a parallel lifecycle harness.
3. Reuse Stage E loader-owned byte and exact instance receipts.
4. Prove provider state/generation/admission and guest instance ownership remain one lifecycle path.
5. Preserve fail-closed replacement and failed-activation residual semantics.
6. Record exact boundaries in a bilingual run record and the next `FEATURE_LOCATOR` entry only after verification and independent review.

## Non-goals / 非目标

- Native dylib loading/unloading, `unsafe`, `libloading`, Wasmtime or a new runtime/dependency.
- Production `ZahirFacade` mount, Feishu/R2/Terminal/Memory/Sandbox integration, or all-channel dynamic loading.
- A global engine/cache, process supervisor, second registry, owner map, generation map or detached cleanup authority.
- Treating ProviderState, generation, broadcast notifications or projections as exact instance-drop receipts.
- Claims about Extism engine-wide JIT code, anonymous mappings, executable OS pages, thread shutdown or RSS reclamation.
- Full production provider/kind rollout. Any public lifecycle or production mount requirement is an architecture decision gate.

## Evidence model / 证据模型

Keep these facts separate:

| Evidence | Meaning | Not equivalent to |
|---|---|---|
| Manifest read | Discovery metadata was inspected | Guest code entered the process |
| Guest-byte read | `.wasm` bytes entered the process | Extism instance was created |
| Instance create | `PluginBuilder::build` succeeded for one `LoadedWasmPlugin` | Provider is Ready or OS code was unloaded |
| Instance drop | Exact Extism `Plugin` owner was destroyed after retained Arcs released | Engine/JIT/mapping/RSS reclamation |
| Provider state/generation | Existing lifecycle/admission authority changed | Guest bytes or exact instance ownership |
| OS mapping/RSS | Optional process/platform observation | Per-provider code unload proof |

## Task 1 — Reconfirm the provider ownership census

**Read first:** `AGENTS.md`, Stage E plan/run record, `src/capability/provider.rs`, `src/extension/runtime_owner.rs`, `src/extension/plugin_state.rs`, `src/extension/lifecycle.rs`, `src/extension/loader.rs`, `src/extension/runtime/wasm/mod.rs`, `src/capability/zahir_facade.rs`.

Record:

- Which existing object owns provider state and generation.
- Which existing object owns the WASM runtime, loaded map, binding and mount identity.
- Where activation transitions to Starting/Ready and where stop transitions to Stopping/Stopped/Disabled/Failed.
- Where call admission checks current generation and where stale completion/call is rejected.
- Which unmount/disposer path removes the loaded map entry and which remaining `Arc` holders can delay exact drop.
- Whether `RuntimeOwner` already contains enough information; avoid adding a seam until the census proves it is needed.
- Existing tests and baseline counts. Save evidence under `.superpowers/sdd/2026-10-29-gateway-stage-f-provider-boundary/`.

**Exit:** source-backed ownership graph and baseline command ledger. No production edits yet.

## Task 2 — Write provider-boundary RED contracts

Extend existing B3/provider tests and Stage E fixture helpers. Do not create a second lifecycle harness.

Required contracts:

1. **Cold:** provider declaration/list/descriptor/projection is `Cold`, provider generation remains undispatched, `ProviderHandle` is not Ready, loader probe reports zero guest-byte reads and zero instance create, and WASM runtime remains lazy.
2. **Activation:** existing provider transition reaches the current Ready generation; one real guest read/create occurs; payload A executes through the existing binding/canonical path; call admission accepts only that generation.
3. **Stop:** transition to stopping/stop closes admission; old binding and old provider handle reject calls; after existing unmount/dispose returns and all retained owner/binding/identity Arcs are dropped, the exact old instance receipt is dropped once.
4. **Replacement:** old generation and binding reject after replacement; new activation gets a new generation and payload B; old instance drops before the new instance is allowed to drop; no stale disposer removes the successor.
5. **Failure:** invalid bytes or failed build may record a byte read but cannot record create/drop or a clean Ready outcome; any existing residual is asserted as residual.
6. **Notifications:** if provider broadcast is exercised, assert it is notification-only and does not become a recovery or lifecycle authority.

Run the new tests before implementation where possible. A compile/import/setup error is not a valid RED contract.

## Task 3 — Implement the smallest provider composition

Expected production changes are limited to existing provider/lifecycle integration points. Reuse Stage E receipt plumbing.

- Thread no new owner map or cleanup scheduler.
- Use existing `ProviderHandle::transition`, generation and admission methods.
- Reuse existing `RuntimeOwner` and `ExtensionManager` disposer paths.
- If a test-only adapter is necessary, make it a thin composition that delegates to existing authorities and cannot independently decide enabled/mounted/provider state.
- Keep test-only counters and receipts behind `#[cfg(test)]`.
- Preserve `Wasm::data` and explicit byte read; do not reintroduce `Wasm::file`.
- Keep failures fail-closed and retain real residual evidence.
- Stop and request architecture direction if the implementation requires public lifecycle changes, production mount, global engine/cache, native unload or process isolation.

## Task 4 — Verification

Every Cargo command must be immediately preceded by a separate:

```bash
python qa/browser_live/check_memory.py
```

Require exit 0 and `ALLOW`, and save each result/log.

Minimum matrix:

```bash
cargo test -p alephcore --lib <new-provider-boundary-filter> -- --nocapture  # three serial runs
cargo test -p alephcore --lib extension::runtime::wasm:: -- --nocapture
cargo test -p alephcore --lib extension::lifecycle::tests::b3_ -- --nocapture
cargo test -p alephcore --lib capability:: -- --nocapture
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix:: -- --nocapture
cargo test -p alephcore --lib gateway::channel_health_monitor::tests:: -- --nocapture
cargo test -p alephcore --lib tools::registry::tests:: -- --nocapture
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo fmt --all --check
git diff --check
```

Add `cargo check -p alephcore`, relevant integration targets, feature/default-feature checks or workspace checks when the actual diff changes those surfaces. A zero-test filter must be reported as zero evidence, not as a pass.

## Task 5 — Independent review and closure

Dispatch a fresh read-only reviewer after the implementation and saved verification logs exist. Require:

- Critical/Important/Minor findings with exact file:line references.
- Ready: Yes/No.
- Explicit checks for duplicated provider/generation/cleanup authority, stale Arc holders, failed activation residuals, provider-state versus guest-instance conflation, and native/JIT/RSS overclaims.

Resolve all Critical/Important findings and re-review. If the review route fails, preserve the complete error and run the checklist manually without claiming independent approval.

After GREEN and review:

- Write `docs/superpowers/runs/2026-10-29-gateway-stage-f-provider-boundary-run.md` in Chinese and English.
- Update this plan and add `FEATURE_LOCATOR.md` §3.5j only if the bounded contract is actually proved.
- Update `PLUGIN_SYSTEM.md` or `GATEWAY.md` only if the final behavior changes their documented boundary.
- Commit code and docs separately.
- Confirm `main`/development branch status as appropriate; do not merge, push, clean worktrees or start another stage without explicit user instruction.

## Acceptance criteria / 验收标准

Stage F is GREEN only if all are true:

- Cold provider composition has zero guest-byte reads, zero instances and no WASM runtime construction.
- Ready activation executes payload A through the existing provider/generation/admission path.
- Stop/replacement rejects old provider calls and bindings and executes payload B only through the successor.
- Exact old/new `LoadedWasmPlugin` drops are observed after all retained Arcs are released, with no double count.
- Failed build/activation does not fabricate instance creation, drop or clean success.
- No second lifecycle authority is introduced.
- All required memory gates, focused repeats, regression checks, static checks and independent review are GREEN.
- Documentation states what remains unproven and what provider/kind/production integration remains deferred.

## Deferred after Stage F / Stage F 之后仍后置

- Production Zahir/provider mount and any Feishu/R2/provider-kind wiring.
- Dynamic loading of built-in Gateway channels, terminal, memory graph or sandbox.
- Native compiled-code unloading, engine/JIT instrumentation, per-guest OS mapping attribution and RSS proof.
- Full unfiltered workspace test suite unless the actual diff expands to require it.
- Additional provider kinds and universal durable scheduler/recovery/effect claims.
