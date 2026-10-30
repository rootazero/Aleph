# Stage H Prompt: Provider-Neutral Lazy Activation and Activation Ownership Readiness
# Stage H Prompt：Provider-Neutral Lazy Activation 与 Activation Ownership 就绪性

## Mission / 任务

Continue Aleph toward the North Star:

> Everything externally composable is a Capability.
>
> 一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。

Evolve Aleph from a process that initializes many integrations into a lightweight Capability Core with providers that can be discovered, started on demand, stopped, replaced, and retired without pulling disabled resources into the main process path.

继续推动 Aleph 实现 North Star：把 Aleph 从“一个集成很多功能的进程”演进成“轻量 Capability Core，加上一组可以按需发现、启动、停止、替换和退出的能力提供者”，使关闭的能力不会被主线程提前拉入内存或初始化资源。

This is Stage H. Stage F already proved provider state, generation, admission, and exact Extism guest ownership through the existing Extension lifecycle. Stage G is complete and merged into `main`: it proved that production already mounts the provider-neutral `ProjectionHost` from the canonical `ToolHandlerRegistry` and one production `OwnershipTree`, without adding a second authority or a concrete channel.

本阶段是 Stage H。Stage F 已通过既有 Extension lifecycle 证明 provider state、generation、admission 与 Extism guest ownership。Stage G 已完成并合并到 `main`：证明 production 已使用 canonical `ToolHandlerRegistry` 与唯一 production `OwnershipTree` 挂载 provider-neutral `ProjectionHost`，没有新增第二套 authority，也没有接入具体 channel。

## First contract / 第一条 contract

Stage H must determine whether the existing production lifecycle can expose lazy provider activation without introducing a second lifecycle authority.

The only acceptable activation authority is the existing owner that already owns the resource:

- `plugin_state.rs` and `ExtensionManager` own discovery and durable enabled/disabled state;
- `RuntimeOwner` owns plugin runtime identity, effect scope, exact registration receipt, task, disposer, and residual failure;
- `ProviderHandle` and `ProviderLifecycle` own provider state, generation, and call admission;
- `ToolHandlerRegistry` remains the canonical callable publication source;
- `OwnershipTree` remains the owner/revoke/dispose source;
- `ProjectionHost` and `ZahirFacade` remain projection, subscription, and host-local worker composition.

Stage H must not make a read-side projection, descriptor snapshot, `describe`, `resolve`, `project`, or broadcast notification start a provider.

Stage H 必须判断现有 production lifecycle 是否能在不引入第二套 lifecycle authority 的情况下暴露 lazy provider activation。

唯一允许的 activation authority 是已经拥有对应资源的既有 owner：

- `plugin_state.rs` 与 `ExtensionManager` 负责 discovery 和 durable enabled/disabled state；
- `RuntimeOwner` 负责 plugin runtime identity、effect scope、exact registration receipt、task、disposer 与 residual failure；
- `ProviderHandle` 与 `ProviderLifecycle` 负责 provider state、generation 与 call admission；
- `ToolHandlerRegistry` 继续是 canonical callable publication source；
- `OwnershipTree` 继续是 owner/revoke/dispose source；
- `ProjectionHost` 与 `ZahirFacade` 继续负责 projection、subscription 与 host-local worker composition。

Stage H 不得让 read-side projection、descriptor snapshot、`describe`、`resolve`、`project` 或 broadcast notification 启动 provider。

## Required first steps / 必须先做的步骤

1. Verify the actual repository root, `HEAD`, branch, worktree, remote relationship, and current prompt/plan. Historical hashes in this prompt are orientation only.
2. Read `AGENTS.md`, the relevant redlines and references, the Stage F/G design and run records, and the current source before proposing code.
3. Perform a production ownership census for:
   - `ToolHandlerRegistry::new()` and `new_with_provider` callers;
   - `ExtensionManager`, `RuntimeOwner`, `ProviderHandle`, `ProviderLifecycle` and their construction/transition paths;
   - `ProjectionHost`, `ZahirFacade`, and the canonical registry handoff;
   - `ChannelRegistry`, `PtyManager`, `MemoryExtensionRegistry`, and other provider-local authorities.
4. Identify whether a real existing runtime owner can trigger activation while the canonical Tool projection remains derived from the same source.
5. Present two or three real approaches in a separate message, recommend one, and obtain the user’s choice before writing a design.
6. Write a bilingual design under `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` and obtain explicit approval before changing production or test implementation.
7. After design approval, write the implementation plan and follow the executing-plans/TDD workflow.

如果 production `new_with_provider` 无法接入既有 owner，或必须新增 activation coordinator、provider catalog、第二个 enabled/disabled authority、generation map、runtime table 或 cleanup authority，立即输出 NO-GO 与具体架构决策请求，停止实现。

## Stage H bounded objective / Stage H 有界目标

Prove or implement the smallest provider-neutral lazy activation path that satisfies all of the following:

1. **Cold discovery:** discovery, list, descriptor, `describe`, `project`, and the corresponding ProjectionHost read-side surfaces do not read guest bytes, create a guest instance, transition a provider to `Starting`, open provider resources, or create provider threads/connections/databases/caches.
2. **Explicit activation:** only an explicit call from the existing runtime owner may request activation. Activation is serialized or single-flight for the same provider and uses the existing `ProviderHandle`/`ProviderLifecycle` transition path.
3. **Ready publication:** activation reaches `Ready` with a new generation and publishes the canonical callable surface through the existing `ToolHandlerRegistry`; the ProjectionHost sees the resulting publication as a derived view.
4. **Admission:** calls are admitted only for the current Ready generation. A non-Ready provider cannot expose a usable callable lease or route.
5. **Failure:** failed read, build, start, publication, or disposer operations settle in the existing Failed/residual model. Tests must preserve the residual and must not report clean Ready/create/drop success.
6. **Replacement and stop:** stop or replacement closes admission, fences the old generation and binding, revokes the exact registration, drains admitted work, and retains the existing exact receipt/disposer semantics.
7. **Restart:** a subsequent explicit activation can publish a successor generation through the same canonical registry and ProjectionHost, without reviving stale handles or creating a second authority.

证明或实现最小 provider-neutral lazy activation path，并满足以上七项：Cold discovery 不初始化资源；只有既有 runtime owner 能显式启动；Ready publication 复用 canonical registry；non-Ready fail closed；failure 保留 residual；stop/replacement 保持 generation、binding、receipt/disposer fencing；restart 通过同一 registry/ProjectionHost 发布 successor，旧 handle 不复活，也不创建第二套 authority。

## Preferred evidence / 首选证据

Use an existing real WASM Extension provider fixture and the existing `ExtensionManager` activation path as the first bounded evidence. Reuse Stage E/F/G probes and helpers where they already observe guest-byte reads, instance creation/drop, provider generation, canonical publication, stale bindings, and exact receipts.

Do not generalize one WASM fixture into proof that all providers are dynamically loaded. Feishu, other Gateway channels, Terminal, Memory Graph, Sandbox, and remaining capability kinds require separate gates.

首选使用既有真实 WASM Extension provider fixture 与现有 `ExtensionManager` activation path，复用 Stage E/F/G 已有的 guest-byte read、instance create/drop、provider generation、canonical publication、stale binding 与 exact receipt probes。

不要把一个 WASM fixture 的证据扩大成“所有 provider 都支持动态加载”。Feishu、其他 Gateway channel、Terminal、Memory Graph、Sandbox 与其余 capability kind 必须分别过 gate。

## Hard boundaries / 硬边界

- Do not add a production mount, wrapper, adapter registry, provider catalog, activation coordinator, owner map, generation map, runtime table, or cleanup authority.
- Do not make `ProjectionHost`, `ZahirFacade`, a snapshot, or a broadcast channel own enabled/disabled state or provider lifecycle.
- Do not make `ChannelRegistry`, `PtyManager`, or `MemoryExtensionRegistry` a universal backend.
- Do not mount Feishu or any concrete Gateway channel into Zahir during this stage.
- Do not add `unsafe`, `dlopen`/`dlclose`, `libloading`, Wasmtime, a new runtime, a new dependency, native unloading, process isolation, detached cleanup, or hidden read-side start.
- Do not use ProviderState, generation, projection disappearance, or notification delivery as a substitute for exact instance/receipt/drop evidence.
- Do not claim engine/JIT reclamation, OS mapping or executable-page reclamation, thread shutdown, RSS reduction, or universal resource savings.
- Keep production dependency direction and the Capability redlines intact.

## Expected NO-GO conditions / 预期 NO-GO 条件

Stop and request an architecture decision if any of these are true:

- `new_with_provider` cannot be connected to an existing resource owner without a second authority;
- activation requires a new enabled/disabled source of truth or a facade-owned state machine;
- discovery or projection must start providers to expose descriptors;
- concurrent activation cannot be serialized by an existing owner boundary;
- non-Ready resolution would need to return a usable lease;
- stop/replacement cannot revoke the exact registration and preserve residual failure;
- production wiring would change public plugin lifecycle solely to make the test path possible;
- the only available tree is channel-local and would need to become universal.

## Verification discipline / 验证纪律

- Every Cargo command must be immediately preceded by `python qa/browser_live/check_memory.py`, with exit 0 and `ALLOW` recorded.
- Focused Stage H tests must run serially three times with real non-zero test counts.
- Re-run the narrow Stage E/F/G, B3, ProjectionHost, capability, gateway and WASM regressions justified by the changed path.
- Run `cargo check -p alephcore --tests`, `cargo clippy -p alephcore --tests -- -D warnings`, `cargo fmt --all --check`, and `git diff --check` as applicable.
- New documentation is bilingual; code comments are English.
- Dispatch an independent read-only reviewer over the complete diff. If unavailable, preserve the exact failure and perform the manual authority checklist without claiming independent approval.

## Required final report / 最终报告必须回答

1. Which existing owner triggers activation, and why is it the only authority?
2. What do discovery, cold read-side projection, explicit activation, replacement, stop, and restart read or create?
3. Where is provider state/generation/admission derived, and is there any second lifecycle authority?
4. Which retained facade/provider/binding/lease/identity/owner references must be released before exact receipt and instance drop are observable?
5. What residuals remain after failed read/build/start/publication/dispose?
6. What resource classes remain unproven: OS mapping, engine/JIT, threads, RSS, or process isolation?
7. Which concrete providers and capability kinds remain deferred?
8. If NO-GO, which production owner or contract is missing, what architecture decision is required, and what is the smallest next choice?
9. Which commands ran, which memory gates passed, and was the independent reviewer available?
10. Which destructive, external, merge, push, or publication actions were not performed?

Do not implement Stage H merely because this prompt exists. The prompt is an architecture/readiness task; implementation starts only after approach selection and explicit design approval.

不要因为存在这份 prompt 就直接实现 Stage H。本 prompt 首先是 architecture/readiness task；只有完成方案选择并获得 design approval 后，才能开始实现。
