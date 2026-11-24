# Stage F Provider Boundary Composition Run
# Stage F Provider Boundary Composition 运行记录

- Date / 日期: 2026-10-29
- Branch / 分支: `stage-f-provider-boundary`
- Base / 基线: `04fba22290b891c4fa4db28033e22fb91cc9a1bc`
- Implementation baseline commit / 实现基线提交: `21b919e74`
- Final amended branch commit / 最终 amend 分支提交: current `HEAD` after review fixes and documentation
- Scope / 范围: bounded provider-facing composition evidence for the existing WASM plugin lifecycle

## Contract / 合约

Stage E proved the Extension-owned Extism instance boundary. Stage F closes the first remaining bounded contract: the existing provider state/generation/admission path and the guest instance ownership path are exercised together through `ExtensionManager`, `RuntimeOwner`, `ProviderHandle`, canonical publication, `PluginLoader`, and the existing disposer. The tests do not create a second lifecycle authority and do not claim production capability mounting.

Stage E 已证明 Extension-owned Extism instance boundary。Stage F 补齐第一条剩余的有界 contract：通过 `ExtensionManager`、`RuntimeOwner`、`ProviderHandle`、canonical publication、`PluginLoader` 和现有 disposer，共同验证 provider state/generation/admission 路径与 guest instance ownership 路径。测试没有新增第二套 lifecycle authority，也没有宣称 production capability mount。

## Evidence / 证据

### Cold, discovered, disabled / Cold、discovered、disabled

`stage_f_cold_provider_surfaces_do_not_read_or_create_guest` uses the real discovered fixture and observes discovered IDs, `PluginStatus::Cold`, an existing Cold `RuntimeOwner`, empty plugin tool listing, empty canonical publication, and the existing `ZahirFacade::describe/project` surfaces. It then disables the same declaration through `set_plugin_enabled`. Both Cold and Disabled observations have zero guest-byte reads, zero instance creation, zero drops, and an inactive lazy WASM runtime.

该测试使用真实 discovery fixture，观察 discovered ID、`PluginStatus::Cold`、现有 Cold `RuntimeOwner`、空 plugin tool listing、空 canonical publication，以及现有 `ZahirFacade::describe/project`。随后通过 `set_plugin_enabled` 禁用同一声明。Cold 与 Disabled 两种观察均为零 guest-byte read、零 instance create、零 drop，lazy WASM runtime 保持未激活。

### Activation / activation

`stage_f_provider_activation_executes_real_guest_through_current_generation` activates through `b3_enable_cold_owner` → `ExtensionManager::set_plugin_enabled`, reaches `ProviderState::Ready`, captures the current provider generation, verifies the canonical publication carries that generation, and invokes the real guest payload A through `b3_call_canonical`. The loader probe records exactly one guest-byte read and one instance create, with no drop while the owner/publication remain retained.

该测试通过 `b3_enable_cold_owner` → `ExtensionManager::set_plugin_enabled` 激活，进入 `ProviderState::Ready`，读取当前 provider generation，验证 canonical publication 携带同一 generation，并通过 `b3_call_canonical` 执行真实 guest payload A。loader probe 记录恰好一次 guest-byte read 与一次 instance create；owner/publication 保留期间没有 drop。

### Replacement and exact drop / replacement 与 exact drop

`stage_f_provider_replacement_fences_old_generation_and_drops_exact_owner` retains the old canonical lease, provider handle, binding, owner, and old receipt. Existing `reload_plugin` withdraws the old publication and makes old provider admission, canonical validation, and `WasmBinding::call_bound` fail. Releasing the retained old references then produces exactly one old drop. The replacement is activated through the same production enable path, executes payload B, has provider generation `1` on its new `ProviderHandle`, and has a larger existing `RuntimeOwner.owner_generation`. The successor receipt has a distinct ID; final drop IDs are exactly `[old, new]`.

该测试保留旧 canonical lease、provider handle、binding、owner 和 old receipt。现有 `reload_plugin` 撤回旧 publication，并使旧 provider admission、canonical validation 与 `WasmBinding::call_bound` 失败。释放旧引用后恰好产生一次 old drop。replacement 通过同一 production enable path 激活，执行 payload B；新 `ProviderHandle` 的 provider generation 为 `1`，而现有 `RuntimeOwner.owner_generation` 单调增加。successor receipt 拥有不同 ID；最终 drop ID 恰为 `[old, new]`。

### Failure residuals / 失败 residual

The Stage F matrix now covers four real failure phases:

1. missing guest file through production enable: `PluginRuntimeOutcome::Failed`, `PluginControlPhase::Reconcile`, actual state `failed`, error containing `wasm_module`, retained owner `ProviderState::Failed`, no loaded plugin, zero successful instance create/drop;
2. invalid guest bytes through production enable: one guest-byte read with exact invalid byte length, no instance create/drop, and retained Failed owner;
3. publication collision: incumbent canonical publication/revision remains intact, the loser owner is `ProviderState::Failed` with `OwnerFailure::Publication(PluginPublicationError::ScopeOccupied)`, and its loader is not loaded;
4. disposer failure: unmount returns `RuntimeOwnerResidual`, the owner remains current and Failed, admission is closed, the receipt remains retained, the canonical entry is withdrawn, and the existing `OwnerFailure::Dispose` records the `service` failure.

Stage F 失败矩阵现在覆盖四个真实 failure phase：缺失 guest 文件、非法 guest bytes、canonical publication collision，以及 disposer failure；每项均检查现有错误/residual、ProviderState、publication 或 loader 结果，以及真实 instance create/drop 边界，没有伪造 clean Ready/create/drop。

## Authority / authority

Durable plugin enabled/disabled and discovery/mount/unmount authority remains `plugin_state.rs` plus `ExtensionManager`. Provider state, per-`ProviderHandle` generation, admission, and transition notification remain `ProviderHandle`/`ProviderLifecycle`. Cross-incarnation monotonic identity remains the existing `RuntimeOwner.owner_generation`; provider generation intentionally restarts at `1` for a newly constructed handle. Canonical publication and `WasmBinding`/`WasmMountIdentity` provide the existing stale-call tuple. No second owner map, generation map, registry, cleanup authority, or receipt registry was introduced.

## Limits / 限制

The exact receipt proves drop of the real Extism `LoadedWasmPlugin` instance. It does not prove engine-wide JIT reclamation, anonymous mappings, executable OS pages, thread shutdown, or RSS reduction. ProviderState, generation, projections, and notifications are not used as substitutes for exact instance drop. This remains a bounded test composition: no production `ZahirFacade::new` mount, Feishu/R2/Terminal/Memory/Sandbox integration, built-in channel dynamic loading, or universal provider/kind rollout was performed.

## Verification / 验证

Every Cargo command below had a separate preceding `python qa/browser_live/check_memory.py` run returning exit 0 and `ALLOW`.

- Stage F focused filter: 7 passed, 0 failed, three final serial runs after review fixes.
- Stage E lifecycle: 3 passed.
- B3 lifecycle: 12 passed.
- WASM runtime: 60 passed.
- Capability: 222 passed.
- Gateway resource matrix: 6 passed.
- Channel health monitor: 14 passed.
- Tools registry: 54 passed.
- `cargo check -p alephcore --tests`: passed.
- `cargo clippy -p alephcore --tests -- -D warnings`: passed.
- `cargo fmt --all --check`: passed.
- `git diff --check`: passed.

The first independent reviewer request with `openai/gpt-6.1-sol` failed before reading because no OpenAI API key was available. A fallback read-only reviewer completed with SIGTERM after producing a substantive review: no Critical; one Important and one Minor. Both findings were fixed with focused regression tests and the required green verification. The completed review result is saved in the delegate output path and summarized in `.superpowers/sdd/2026-10-29-gateway-stage-f-provider-boundary/reviewer-availability.md`; no independent second re-review was dispatched because the execution workflow requires one fix pass after the whole-branch review.

## Deferred / deferred

OS/native unloading, global engine/cache, process isolation, new runtime/dependency, production Zahir mount, Feishu/R2/Terminal/Memory/Sandbox wiring, all built-in channel dynamic loading, universal provider/kind rollout, and RSS/mapping/thread claims remain deferred.

## Actions not performed / 未执行的动作

No merge, push, publication, production mount, external service call, destructive cleanup, worktree deletion, or branch reset was performed.
