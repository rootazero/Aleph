# Stage F Provider Boundary Composition Design
# Stage F Provider Boundary Composition 设计

- Date / 日期: 2026-10-29
- Scope / 范围: bounded provider-boundary evidence for the existing WASM plugin lifecycle
- Branch / 分支: `stage-f-provider-boundary`

## Intent / 意图

Stage E proved an Extension-owned Extism instance boundary: a real test guest can be read, instantiated, replaced, rejected through a stale binding, and dropped exactly once after retained owners release it. The first contract still open is whether provider state, generation, admission, and guest instance ownership are constrained by the same existing lifecycle path.

本阶段把 Stage E 的证据推进到现有 `ProviderHandle` / `RuntimeOwner` / `ExtensionManager` provider boundary。目标是证明 provider-facing activation and calls use the same lifecycle authority that owns the guest instance, while keeping the evidence bounded to the existing test composition.

## Goals / 目标

1. Prove that disabled, discovered, and Cold provider observation through list/descriptor/projection paths performs zero guest-byte reads and creates zero runtime/instance objects.
2. Prove that activation through the existing enable/ensure-loaded path uses `ProviderHandle` transitions and generation/admission, reads and executes a real guest payload A, and returns a provider-facing result through the real published canonical handler.
3. Prove that provider calls admit only `Ready` with the current provider generation; Stopping, Stopped/Disabled, Failed, and stale generations are rejected.
4. Prove that replacement follows the existing unmount/dispose then new activation path, executes payload B, rejects the old binding/provider call, and observes the exact old `LoadedWasmPlugin` drop after all retained owner, binding, and identity `Arc`s are released.
5. Prove that failed read/build/publication activation preserves the existing residual/error semantics and never reports a fabricated clean instance creation or drop.
6. Keep every lifecycle receipt tied to a real `LoadedWasmPlugin` incarnation. Provider transitions and projections may be recorded as supporting evidence only; they are not a second guest lifecycle authority.

## Non-goals / 非目标

- No production `ZahirFacade` mount.
- No Feishu, R2, Terminal, Memory, Sandbox, or built-in channel integration.
- No public plugin lifecycle API expansion.
- No new registry, owner map, generation map, loader cache, cleanup authority, runtime, dependency, process isolation, or native code unloading.
- No claim that dropping an Extism `Plugin` proves engine-wide JIT, anonymous mappings, executable pages, threads, or RSS reclamation.
- No claim that a single test guest proves dynamic loading for all providers or channels.

## Constraints / 约束

- `plugin_state.rs` and `ExtensionManager` remain the durable enabled/disabled, discovery, mount/unmount authorities.
- `ProviderHandle` / `ProviderLifecycle` remain the provider state, generation, call-admission, and transition-notification authority.
- `RuntimeOwner` remains the existing composition owner and retains `ProviderHandle`, `owner_generation`, `WasmMountIdentity`, bindings, and the existing disposer.
- `WasmBinding`, `WasmMountIdentity`, provider generation, and the current disposer remain the stale-call and replacement判据.
- The lazy `Option` WASM runtime and Stage E loader probe/receipt are reused.
- Production code comments remain English; new documentation is bilingual.
- Every Cargo command is preceded by `python qa/browser_live/check_memory.py` and must receive exit 0 / `ALLOW`.
- Focused provider-boundary tests run serially three times, followed by the required extension/WASM, provider/capability, B3, Stage C/D narrow regressions, check, clippy, fmt, and diff checks.

## Approaches considered / 已考虑方案

### A. Existing lifecycle composition — chosen / 方案 A：组合现有生命周期 — 已选择

Drive the evidence through the real `ExtensionManager` enable/ensure-loaded path, the existing `RuntimeOwner::activate_owner`, canonical `PluginBatch` handler publication, `ProviderHandle` admission, and existing stop/replacement disposer. Add only the smallest `cfg(test)` observation seam if the current loader receipt cannot expose an already-owned fact.

优点是 provider state/generation/admission and guest ownership stay on one existing path, and the test exercises the same stale binding and disposer boundaries that production code already uses. The trade-off is that tests need to discover the published handler and may be less locally diagnostic than a direct helper.

### B. Minimal test-only call seam / 方案 B：最小测试调用 seam

Add a `cfg(test)` helper under `RuntimeOwner` that performs provider admission and invokes a real `WasmBinding`.

This would make the state matrix explicit, but creates an additional lifecycle entry point that could drift from the canonical handler path. It is retained only as a fallback if the existing published handler cannot expose the required provider-facing call without changing production behavior.

### C. New production provider API / 方案 C：新增生产 provider API

Create a new production-level provider-facing guest-call interface for tests and future channels.

This would make a future boundary explicit, but expands the public plugin lifecycle contract and would require architecture review. It is outside this bounded evidence stage and is rejected.

## Chosen design / 选定设计

Use Approach A. The test fixture will create the existing eligible real WASM plugin and observe the existing loader counters/receipts. It will exercise:

1. discovery/list/projection while disabled and Cold, asserting no guest-byte read and no runtime/instance creation;
2. existing enable/ensure-loaded activation, then lookup of the real published canonical handler and invocation of payload A;
3. provider admission against the current `ProviderHandle` generation and rejection of stale or non-Ready states;
4. existing stop/revoke/drain/join/dispose followed by replacement activation and payload B;
5. release of test-held handler, binding, mount identity, and owner references before asserting the exact old `LoadedWasmPlugin` drop;
6. failed read/build/publication cases, asserting real residuals and the absence of fabricated clean receipts.

The test must record these evidence classes separately: manifest read, guest-byte read, instance create/drop, provider state/generation, OS mapping, and RSS. Provider state or transition receipts are never interpreted as instance creation/drop evidence.

## Builder-check resolutions / Builder check 处理结果

The independent builder check raised three scope questions. They are resolved from the request and existing authority as follows:

1. “discovered / disabled / Cold” covers both the authoritative `ProviderState` and the existing plugin registry/projection rows. The test will use real discovery and declaration paths; it will not synthesize provider states. It will assert the declared Cold owner and the corresponding disabled/discovered view separately because those are different representations.
2. Read, build, and publication failures each need their own stage-specific residual/error and loader receipt assertions. A failed activation with no committed publication has no old canonical handler to invoke, so the test will not invent an old-handler rejection requirement; stale-call rejection is covered by the successful replacement path.
3. “list/descriptor/projection” means the existing representative public surfaces that expose these facts: plugin registry listing/status, canonical capability descriptor/handler publication, and the existing capability/provider projection path. The test will not enumerate every accessor or create a new observation API merely to widen the matrix.

These are builder-level scope calls and do not change the chosen approach or lifecycle authority.

## Builder responsibilities / Builder 负责

- Locate the existing real published handler lookup and invocation path without introducing a new production authority.
- Add behavior RED tests first against the current composition.
- Reuse the Stage E loader probe and exact `LoadedWasmPlugin` receipt.
- Keep failed activation residuals in the existing `RuntimeOwner`/`ExtensionManager` error semantics.
- Remove or avoid any temporary seam that is not needed after the behavior is proven.
- Update the bilingual run record, Stage F plan, and `FEATURE_LOCATOR.md` only after GREEN and independent review.

## Acceptance evidence / 验收证据

The final run must answer:

- What each disabled/discovered/Cold observation, activation, and replacement read or created.
- Which existing authority produced state, generation, and admission decisions, and whether any second lifecycle authority exists.
- Which retained provider/binding/identity/owner `Arc`s had to be released before the exact instance drop became observable.
- The exact failed-activation residual and whether any clean result was fabricated.
- Which OS mapping, engine/JIT, thread, and RSS facts remain unproven.
- Confirmation that no production channel or Zahir mount was connected.
- Actual commands, memory gates, test counts, reviewer availability, and deferred provider/kind/production integrations.
- Destructive or external actions not performed.
