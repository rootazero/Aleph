# Aleph Architecture

> Complete system architecture overview

> **Terminology:** See [GLOSSARY.md](./GLOSSARY.md) for canonical Anthropic-aligned definitions of Harness, Sandbox, Session, Tools, Orchestrator, and AcpAdapter.

---

## System Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           INTERFACE LAYER (I/O)                               │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐      │
│  │ macOS    │  │  Tauri   │  │   CLI    │  │ Telegram │  │ Discord  │      │
│  │  App     │  │   App    │  │          │  │Interface │  │Interface │      │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘      │
│       │             │             │             │             │             │
│       └─────────────┴─────────────┴─────────────┴─────────────┘             │
│                                   │                                          │
│                          WebSocket (JSON-RPC 2.0)                           │
│                          ws://127.0.0.1:18790/ws                             │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                              GATEWAY LAYER                                   │
│                         (Control Plane + Routing)                           │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Router    │  │  Session    │  │   Event     │  │  Security   │        │
│  │  (JSON-RPC) │  │  Manager    │  │    Bus      │  │  (Auth)     │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │ Interface   │  │   Config    │  │  Webhooks   │  │    Cron     │        │
│  │  Registry   │  │ Hot Reload  │  │             │  │  Scheduler  │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                              AGENT LAYER                                     │
│                    (Orchestrator → Harness → Think → Act)                   │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        Orchestrator                                  │   │
│  │  Resolves AgentDef + FlowSpec, builds HarnessDeps, dispatches       │   │
│  └───────────────────────────────┬──────────────────────────────────────┘   │
│                                  │ HarnessRunner::run                       │
│  ┌───────────────────────────────▼──────────────────────────────────────┐   │
│  │                         AgentHarness                                 │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐   │   │
│  │  │ Context │→ │  Think  │→ │   Act   │→ │Stop-Hook│→ │ Compact │   │   │
│  │  │ (Budget)│  │(Thinker)│  │ (Tools) │  │  Check  │  │ (Budget)│   │   │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘  └─────────┘   │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                      │
│  │  ToolService │  │    Guards    │  │   Overflow   │                      │
│  │  (Tool Exec) │  │  (Safety)    │  │  Detector    │                      │
│  └──────────────┘  └──────────────┘  └──────────────┘                      │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                            EXECUTION LAYER                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │   Thinker   │  │  Executor   │  │    Tool     │  │    Exec     │        │
│  │ (LLM Call)  │  │ (Tool Run)  │  │   Server    │  │  (Shell)    │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐        │
│  │  Providers  │  │  Builtin    │  │    MCP      │  │  Extension  │        │
│  │ (AI APIs)   │  │   Tools     │  │   Client    │  │  (Plugins)  │        │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘        │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │
┌───────────────────────────────────┴─────────────────────────────────────────┐
│                            STORAGE LAYER                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────┐  ┌────────────────┐  ┌─────────────────┐          │
│  │  Memory (SQLite+vec0)│  │ Resilience     │  │  Config Store   │          │
│  │  ┌──────┐ ┌───────┐  │  │   (SQLite)     │  │  ┌─────┐┌────┐ │          │
│  │  │Facts │ │ Graph │  │  │  ┌──────────┐  │  │  │TOML ││Keys│ │          │
│  │  │+Vec  │ │ Nodes │  │  │  │  State   │  │  │  │File ││    │ │          │
│  │  │+FTS  │ │ Edges │  │  │  │ Database │  │  │  └─────┘└────┘ │          │
│  │  └──────┘ └───────┘  │  │  └──────────┘  │  └─────────────────┘          │
│  └──────────────────────┘  └────────────────┘                               │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Capability Plane

Capability is a semantic contract, not another plugin or dependency-injection container. In the first converged slice, a callable Tool is represented by one `ToolCapabilityDescriptor` and one live handler in `ToolHandlerRegistry`. The descriptor is the source for discovery and projection; the handler is the execution implementation. `LoopToolRegistry`, provider metadata, ToolCatalog, MCP bridge, and recovery are consumers of that contract, not parallel registries.

```text
Tool registration
  -> ToolCapabilityDescriptor + live handler (one atomic generation)
  -> entries_snapshot() / revision change feed
  -> model metadata | ToolCatalog | MCP bridge | Loop execution index
  -> EffectScope-compatible registration disposer
  -> recovery ReplayPolicy lookup (fail-closed until call-time revision exists)
```

A capability may also declare orthogonal behavior facets in `CapabilityDescriptor::facets`: `Callable`, `EventSource`, `Subscription`, `ProviderLifecycle`, and `ExternalEffect`. `CapabilityKind` remains a primary classification, while facets describe composable behavior; a future Gateway provider can therefore combine inbound events, outbound calls, lifecycle control, and external effects without masquerading as `Tool` or adding another kind. `Scope::facet` filters discovery, projection, and live change visibility through the same descriptor predicate. Facets are descriptive metadata only: provider instance/generation, owner generation, descriptor revision, leases, effect claims, and durability remain owned by their existing authorities. The current Tool backend declares `Callable` only; Feishu has no descriptor, resolver, or production mount in this change.

能力还可以在 `CapabilityDescriptor::facets` 中声明正交行为 facet：`Callable`、`EventSource`、`Subscription`、`ProviderLifecycle` 与 `ExternalEffect`。`CapabilityKind` 仍是主分类，facet 描述可组合行为；未来 Gateway provider 可以组合入站事件、出站调用、生命周期控制与外部 effect，而不必伪装成 `Tool` 或新增 kind。`Scope::facet` 通过同一个 descriptor predicate 过滤 discovery、projection 与 live change visibility。facet 只是描述性元数据：provider instance/generation、owner generation、descriptor revision、lease、effect claim 与 durability 仍由既有 authority 单独拥有。本次修改中 Tool backend 只声明 `Callable`；Feishu 仍没有 descriptor、resolver 或 production mount。


### B4 plugin authority chain / 插件权限链

B4 (`d53821d7f`) binds canonical plugin execution to a captured
`CanonicalEntryLease` and the exact `DispatchVerdict` from existing scoped gates.
`PluginActivationAuthority` derives only from that task-local sealed verdict plus
registry-owned `ReadySnapshot` publication. `PluginCanonicalHandler` and
`ExtensionManager::call_plugin_tool_with_authority` revalidate provider/owner
generations, mount identity, binding, descriptor/caller/input, and publication
before/after waits and immediately before guest admission; stale, replaced, or
ownerless compatibility paths fail closed.

This does not unify direct mounted-only RPC with raw `tools.invoke` compatibility
semantics or grant Cold tool-only WASM an activation proxy at discovery time.
B5 (2026-10-23) adds the explicit enable-driven activation path without changing
the callable authority; B6 adds the real boot activation pass and Tool/Plugin lifecycle acceptance; see the B6 addendum below. See
[Tool System — B4](TOOL_SYSTEM.md#b4-canonical-plugin-dispatch--规范插件派发) for the dispatch boundary.

B4（`d53821d7f`）把 canonical plugin 执行绑定到捕获的 `CanonicalEntryLease`
与既有 scoped gates 形成的精确 `DispatchVerdict`。`PluginActivationAuthority`
只从该 task-local sealed verdict 与 registry-owned `ReadySnapshot` publication
派生。`PluginCanonicalHandler` 与 `ExtensionManager::call_plugin_tool_with_authority`
在等待前后及 guest admission 紧前复检 provider/owner generation、mount identity、
binding、descriptor/caller/input 与 publication；陈旧、被替换或无 owner 的兼容路径
fail-closed。

这不统一 direct mounted-only RPC 与 raw `tools.invoke` 的兼容语义，也不赋予 Cold
tool-only WASM 在 discovery 期的 activation proxy。B5（2026-10-23）补上显式 enable
驱动的激活路径，且不改变 callable 权威；B6 现在补上真实 boot activation pass 与
Tool/Plugin lifecycle 验收；派发边界详见上方 Tool System 链接。

### B5 plugin activation lifecycle / 插件激活生命周期

B5 (2026-10-23) wires exactly one thing: the explicit activation of eligible tool-only WASM through the existing plugin-control path. Discovery declares `Cold` and nothing else — no VM, no published tool surface, no registry revision change. The single production entry is `ExtensionManager::set_plugin_enabled(id, true)` → `reconcile_plugin_control_locked` (under `load_guard`) → capture the exact `RuntimeOwner` incarnation → release the guard → `RuntimeOwner::activate_owner` → re-acquire the guard → re-verify current owner (`Arc::ptr_eq`), provider `Ready`, registry row `Loaded`, identity and receipt present, scope still held. Only then does the outcome become `Mounted`; anything else is `Failed` at the Reconcile phase.

The process-global manager Arc installs a `Weak` activation context in `init_extension_manager` only; the retained activation task never strongly holds the manager, and an unattached context fails closed. `ToolHandlerRegistry` remains the sole callable authority, with the existing exact receipt / revision / generation fencing on Ready publication, revoke, and replacement. Disable revokes publication, drains and joins in-flight guest calls, then unloads and disposes the scope; stale owner, provider generation, mount identity, and lease all fail closed. Zahir observers subscribe before the snapshot, treat the snapshot as their anchor, receive live broadcast only, and resync through `Invalidated` on lag — there is no durable registry replay, and `SessionEventStore` remains the only session durability source.

Historical B5 validation: `cargo fmt --all --check` passed; `cargo check -p alephcore -p aleph-slot-api --all-targets` passed; targeted lib tests passed — extension lifecycle 57, tool registry 53, Zahir facade 23, session 352; `cargo test -p alephcore --lib` passed with 21488 passed, 0 failed, 20 ignored; `cargo clippy --workspace --all-targets` passed with warnings (a large `Result` `Err` variant at `src/extension/plugin_ops.rs:877` plus two pre-existing registry test lints). Historical B5 deferred scope: boot auto-activation, MCP lazy semantics, EffectClaim producer, automatic Safe Replay, universal durable scheduler, remaining capability kinds, durable external-effect exactly-once, and `src/harness/`.

B5（2026-10-23）只接一件事：eligible tool-only WASM 的**显式**激活，走既有 plugin control 路径。discovery 只声明 `Cold`：不启动 VM、不发布 tool surface、registry revision 不变。唯一生产入口是 `ExtensionManager::set_plugin_enabled(id, true)` → `reconcile_plugin_control_locked`（在 `load_guard` 内）→ 捕获 exact `RuntimeOwner` incarnation → 释放 guard → `RuntimeOwner::activate_owner` → 重新取回 guard → 复检 current owner（`Arc::ptr_eq`）、provider `Ready`、registry row `Loaded`、identity 与 receipt 齐备、scope 仍持有；全部成立才是 `Mounted`，否则 `Failed`（Reconcile 阶段）。

进程级 manager Arc 只在 `init_extension_manager` 装 `Weak` 激活上下文；retained activation task 不强持 manager；未 attach 一律 fail-closed。`ToolHandlerRegistry` 仍是唯一 callable authority，Ready publication / revoke / replacement 沿用既有 exact receipt / revision / generation fencing。disable ⇒ revoke publication ⇒ drain/join 在途 guest 调用 ⇒ unload/dispose scope；陈旧 owner、provider generation、mount identity、lease 一律 fail-closed。Zahir observer 先 subscribe 再取 snapshot，snapshot 是锚点，broadcast 只是 live notification，lag 时以 `Invalidated` 触发 resync；**没有** durable registry replay，durable recovery 只认 `SessionEventStore`（session durability）。

B5 历史基线验证：`cargo fmt --all --check` 通过；`cargo check -p alephcore -p aleph-slot-api --all-targets` 通过；定向 lib 测试通过——extension lifecycle 57、tool registry 53、Zahir facade 23、session 352；`cargo test -p alephcore --lib` 通过：21488 passed / 0 failed / 20 ignored；`cargo clippy --workspace --all-targets` 通过但带 warnings（`src/extension/plugin_ops.rs:877` 的 large `Result` `Err` 变体，另有两处既有 registry 测试 lint）。deferred：boot 期 auto-activation、MCP lazy semantics、`EffectClaim` producer、automatic Safe Replay、universal durable scheduler、其余 Capability kinds、durable external-effect exactly-once、`src/harness/`。

### B6 Tool/Plugin boot activation / B6 Tool/Plugin 启动激活

B6 connects enabled, eligible tool-only WASM to the real `ExtensionManager::ensure_loaded` path. Discovery remains Cold under `load_guard`; exact owners activate outside that guard; a shared proof checks current owner incarnation, provider `Ready`, row `Loaded`, mount identity, receipt, and effect scope before publishing `cache_state.loaded=true`. `boot_guard` serializes the whole pass, and public `load_all`/`reload` remain discovery-only primitives that are mutually exclusive with boot. Per-plugin failures close admission, leave the provider `Failed`, record the owner/row failure, and allow other owners to settle. Read-side catalog/describe/resolve/`plugins.list`/observer/doctor paths do not activate. Stage H makes the distinction explicit: `ensure_discovered()` is declaration-only discovery, while `ensure_loaded()` and explicit enable/reload/control remain activation boundaries. Startup catalog and agent-run paths remain activation boundaries only where they invoke the existing load/activation path; `describe`, `resolve`, `project`, and `ProjectionHost` read-side calls do not start providers.

D-A selects the `ensure_loaded` two-phase entry; D-B is the user-confirmed per-plugin fail-closed policy; D-C lists the remaining legal Cold paths; D-D keeps public load/reload discovery-only. Real WASM tests cover single and concurrent boot ensure effects, while existing B5 lifecycle tests cover publication, revoke, drain/join/dispose, replacement, stale handles, and observer behavior. B6 remains limited to the Tool/Plugin slice; the B6 historical scope left channel, terminal, memory, MCP, replay, scheduler, EffectClaim, external exactly-once, and other Capability kinds deferred. The B7 text below is a dated historical record of one Feishu provider slice; it did not wire Feishu into Zahir. Current B8.2 status keeps Feishu → Zahir and the other provider/kind integrations **NO-GO / deferred**. The current validation is recorded in `docs/reference/FEATURE_LOCATOR.md` §3.5f.

B6（Tool/Plugin 启动激活）把 enabled 且 eligible 的 tool-only WASM 接入真实
`ExtensionManager::ensure_loaded`：discovery 在 `load_guard` 内保持 Cold，exact owner
在 guard 外激活，共享 proof 统一检查 current owner incarnation、provider `Ready`、row
`Loaded`、mount identity、receipt 与 effect scope，全部成立后才发布
`cache_state.loaded=true`。`boot_guard` 串行化整个 pass；public `load_all`/`reload` 仍是
与 boot 互斥的 discovery-only 原语。单插件失败会 close admission、使 provider 进入
`Failed`、记录 owner/row failure，并继续 settle 其他 owner。catalog/describe/resolve/
`plugins.list`/observer/doctor 等读侧不触发 activation。Stage H 进一步明确：
`ensure_discovered()` 只做 declaration-only discovery，而 `ensure_loaded()` 与显式
enable/reload/control 仍是 activation boundary。startup catalog 与 agent-run 路径只有在
调用既有 load/activation 路径时才是 activation boundary；`describe` / `resolve` /
`project` / `ProjectionHost` 的 read-side 调用不会启动 provider。

D-A 选择 `ensure_loaded` 两段式入口；D-B 是用户确认的 per-plugin fail-closed；D-C
列出仍可见 Cold 的合法路径；D-D 保持 public load/reload discovery-only。真实 WASM 测试
覆盖单次与并发 boot ensure 的效果，既有 B5 lifecycle 测试覆盖 publication、revoke、
drain/join/dispose、replacement、stale handle 与 observer。B6 只覆盖 Tool/Plugin slice；
B6 的历史范围把 channel、terminal、memory、MCP、replay、scheduler、EffectClaim、external exactly-once
与其他 Capability kinds 留在 deferred。下方 B7 是一条 Feishu provider slice 的带日期历史记录，
并不表示 Feishu 已接入 Zahir；当前 B8.2 仍将 Feishu → Zahir 与其他 provider/kind 接入标为
**NO-GO / deferred**。当前验证见 `docs/reference/FEATURE_LOCATOR.md`
§3.5f。

### Stage H declaration-only plugin reads / Stage H 插件只读声明发现

Stage H keeps the B5/B6 owner and publication authorities while splitting the existing `ExtensionManager` discovery walk into a declaration-only read path. `ensure_discovered()` scans, parses and admits installed extensions, publishes declaration rows, and declares eligible tool-only WASM as `RuntimeOwner(Cold)`; it does not unmount existing owners, clear the owner table, mount deferred providers, read guest bytes, create provider resources, or publish callables. `plugins.list`, plugin-management read actions and Hub installed reconciliation use this path. `ensure_loaded()` and explicit enable/reload/control calls remain the activation boundaries and continue through the existing owner and canonical `ToolHandlerRegistry` path.

The `discovered` bit is a derived cache of completed declaration work. Reload invalidates it; it is not durable enabled state, provider state, a generation map or a second owner authority. Stage H is limited to ExtensionManager-managed tool-only WASM. Channel, PTY, memory, MCP and other provider kinds remain deferred, and this slice makes no claim about RSS, thread, engine/JIT, executable-page or OS-mapping reclamation.

Stage H 在保留 B5/B6 owner 与 publication authority 的前提下，把既有 `ExtensionManager` discovery walk 分成 declaration-only read path。`ensure_discovered()` 扫描、解析并 admission 已安装 extension，发布 declaration row，并将 eligible tool-only WASM 声明为 `RuntimeOwner(Cold)`；它不卸载现有 owner、不清空 owner table、不 mount deferred provider、不读取 guest bytes、不创建 provider resource，也不发布 callable。`plugins.list`、plugin management 的读 action 与 Hub installed reconciliation 使用这条路径；`ensure_loaded()` 与显式 enable/reload/control 仍是 activation boundary，继续走既有 owner 与 canonical `ToolHandlerRegistry` 路径。

`discovered` 只是已完成 declaration work 的派生缓存；reload 会使它失效。它不是 durable enabled state、provider state、generation map 或第二份 owner authority。本 Stage H 只覆盖 ExtensionManager 管理的 tool-only WASM；channel、PTY、memory、MCP 与其他 provider kind 仍 deferred，也不声明 RSS、thread、engine/JIT、executable page 或 OS mapping reclamation。

### Current Generic Gateway source-owned capability read/lease slice / 当前 Generic Gateway 源所有 capability read/lease 切片（2026-10-30）

The bounded Generic Gateway implementation now has a source-owned asynchronous read/lease surface over existing `ChannelRegistry` Bound entries. `describe_bound_capabilities`, `list_bound_capabilities`, and `project_bound_capabilities` share one source publication cut. `resolve_bound_capability` captures source evidence, validates provider generation and ownership without holding source locks, then revalidates the source cut before returning `BoundChannelCapture`. This is a derived read model over `ChannelRegistry`, `ProviderHandle`, and `OwnershipTree`; it is not a second registry, descriptor catalog, runtime table, `CapabilityBackend`, or universal `ZahirFacade` mount.

当前有界 Generic Gateway 实现已在既有 `ChannelRegistry` Bound entry 之上提供 source-owned 异步 read/lease surface。`describe_bound_capabilities`、`list_bound_capabilities`、`project_bound_capabilities` 共享一个 source publication cut。`resolve_bound_capability` 捕获 source evidence，在不持有 source lock 的情况下校验 provider generation 与 ownership，再复验 source cut 后返回 `BoundChannelCapture`。这是对 `ChannelRegistry`、`ProviderHandle` 与 `OwnershipTree` 的 derived read model，不是第二 registry、descriptor catalog、runtime table、`CapabilityBackend` 或 universal `ZahirFacade` mount。

Task 1 keeps `source_revision` and descriptor revision independent and serializes all source publication paths with checked exhaustion before the no-await commit section. Task 3 rejects a capture that claims a future source revision. The lower-level `validate_bound_capture` rule preserves a current entry capture across unrelated later publications, while `resolve_bound_capability` deliberately requires its captured source cut to remain unchanged and fails closed if that cut changes. Retirement commit sections validate source/entry identity, retirement marker, active work, and publication revisions; provider generation and ownership binding are validated separately by capability resolution and execution admission. The read surface is proven no-start/no-auth/no-network for cold and disabled declarations: it does not start or authenticate providers/channels, refresh tokens, open listeners, spawn tasks/drivers, or mount webhooks. Provider call admission and ownership binding remain separate use-time fences.

Task 1 保持 `source_revision` 与 descriptor revision 独立，并让所有 source publication path 经过 checked exhaustion 与 no-await commit section。Task 3 拒绝声称 future source revision 的 capture。较低层的 `validate_bound_capture` 保留当前 entry capture 跨越无关后续 publication 的有效性；而 `resolve_bound_capability` 有意要求自身捕获的 source cut 保持不变，发生变化时 fail-closed。Retirement commit section 校验 source/entry identity、retirement marker、active work 与 publication revision；provider generation 与 ownership binding 则由 capability resolve 和 execution admission 分别校验。Cold 与 Disabled declaration 的 read surface 已证明 no-start/no-auth/no-network：不启动或认证 provider/channel、不刷新 token、不打开 listener、不 spawn task/driver、不 mount webhook。Provider call admission 与 ownership binding 仍是独立的 use-time fence。

This closes only the bounded Generic Gateway read/lease contract. It does not close Feishu B8.2-A/B/C/D, production Bound ingress/runtime, Zahir routing, subscriptions, durable/crash recovery, external exactly-once effects, all-channel compatibility, Terminal/Memory/Sandbox migration, EffectClaim production, Safe Replay, universal scheduling, or remaining capability kinds. The current implementation evidence is in `src/gateway/channel_registry/{capability.rs,binding.rs,resource_matrix.rs}`; focused capability/resource/ownership tests and `cargo check -p alephcore --tests` passed. The provider-neutral Tool host remains the only production Zahir/ProjectionHost mount.

本切片只闭合 Generic Gateway 有界 read/lease contract，不闭合 Feishu B8.2-A/B/C/D、production Bound ingress/runtime、Zahir routing、subscription、durable/crash recovery、external exactly-once effects、all-channel compatibility、Terminal/Memory/Sandbox migration、EffectClaim production、Safe Replay、universal scheduling 或其余 capability kind。当前实现证据在 `src/gateway/channel_registry/{capability.rs,binding.rs,resource_matrix.rs}`；定向 capability/resource/ownership tests 与 `cargo check -p alephcore --tests` 已通过。provider-neutral Tool host 仍是唯一 production Zahir/ProjectionHost mount。

### B8.2 current Feishu lifecycle re-gate / B8.2 当前 Feishu 生命周期复闸（2026-10-25）

The current worktree closes the Feishu **provider lifecycle slice** around Gateway authority; it does not mount Feishu into Zahir. `ChannelRegistry` owns channel instances, desired intent, lifecycle serialization, replacement preflight, and webhook mounts (`src/gateway/channel_registry.rs:187-220,295-433,600-688,1460-1504`). Disabled boot records `Stopped` and is excluded from `start_all`; `create_channel` derives intent from `ChannelConfig.enabled`; restart publishes `Running` only after a complete stop/start/mount refresh.

`FeishuChannel` owns an in-process `ProviderHandle` for state/generation admission, while `api_handle` is a derived, instance/generation-fenced cache (`src/gateway/interfaces/feishu/mod.rs:51-183,334-405`; `src/gateway/interfaces/feishu/api_handle.rs:1-104`). Start commits Ready and derived publication together. Stop revokes first, cleans up refresh/WS/webhook tasks, drains admitted API work, and retains serve/join residuals. `FeishuApi::drain` proves admitted-call and inbound enqueue quiescence; Axum accepted-handler ownership is not independently held, so `HandlersUnjoined` remains a real residual (`src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:29-120`).

Source-backed path: boot builder → `ChannelRegistry::register_for_boot` → `start_all` → `start_channel_inner` → `FeishuChannel::start` → runtime → `api_handle::publish` → `ProviderHandle::with_ready_commit`; outbound calls pass `admit_call` → `ProviderHandle::acquire_call` → `FeishuApi::admit`; inbound webhook/WS forwarding uses the same admission boundary. Stop/replacement follows registry serialization → revoke/withdraw → cleanup → API drain → mount removal after successful stop.

B8.2-A descriptor/kind, B8.2-B lease/generation/replacement, B8.2-C callable/effect, and B8.2-D observer/recovery are all **NO-GO / deferred**. The production `ProjectionHost`/`ZahirFacade` path is Tool-only evidence. There is no Feishu descriptor adapter, Feishu `BackendLease` route, Feishu callable resolve path, Feishu Capability subscription, or durable provider lifecycle producer. The Feishu webhook remains a Gateway-owned external effect; no Feishu `EffectClaim` producer, Safe Replay, scheduler, or external exactly-once path exists. Production Feishu → Zahir mount is prohibited until the first unclosed gate is closed. See `docs/research/aleph_modularization_phase2b_b8_2_closeout.md` for the current failure matrix, authority mapping, and focused evidence.

### B7 historical Feishu provider vertical slice / B7 历史 Feishu provider 垂直切片

> Historical B7 baseline only. B8.2 above is the current status and re-gate; B7 validation text below is not current B8.2 evidence.

B7 (2026-10-24, uncommitted) reuses the frozen provider lifecycle contract for exactly one Gateway provider: the Feishu channel (`src/gateway/interfaces/feishu/**`, plus `src/gateway/inbound_router/executor.rs`). It is a provider slice, not a unified Gateway.

`src/gateway/channel_registry.rs::ChannelRegistry` stays the only Gateway lifecycle authority. The channel owns an internal `ProviderHandle` (provider id `gateway.feishu:<channel id>`, `ProviderTransport::InProcess`) that acts as an admission fence over `ProviderState` and generation — not a second registry, not a second enabled decision, not a replacement for the registry. `src/gateway/interfaces/feishu/api_handle.rs` is a derived cache: a `Weak` client plus config plus the provider instance and generation it was published under; it is not an authority.

Lifecycle: `Cold → Starting → Ready → Stopping → Stopped/Failed`, with a generation increment on every new `Starting`. `Ready` and the cache publication commit together under `with_ready_commit`; a failed start revokes, cleans up, leaves the provider `Failed` and returns the error; stop revokes, signals webhook shutdown, awaits the `axum::serve(...).with_graceful_shutdown(...)` future, then aborts and awaits the token-refresh / ws tasks, drains in-flight outbound HTTP, and reports `Stopped`. Serve/join failures remain a structured shutdown residual. Every outbound channel action and every outbound HTTP call is admitted against the current generation (the API read guard spans the whole call, including the token await, so the drain really waits); inbound webhook and WS forwarding are admitted too (an unadmitted webhook request gets `503`). The read side never starts or authenticates anything: `api_handle::get` only validates the provider instance and generation, and `try_create_feishu_emitter` no longer rebuilds a config plus a second `TokenManager` per inbound message. An outgoing generation cannot withdraw its replacement, and the ws channel identity is still `config.app_id`.

Static in-process boundary (D-B): B7 proves authority and runtime-effect revocation, not code unmapping or hot unload. B7 is **not** a live Zahir facade (no capability descriptor / lease / subscribe / project for Feishu), **not** hot-unload, and **not** EffectClaim producers, automatic Safe Replay, a universal durable scheduler, durable external-effect exactly-once, MCP lazy, or the remaining Gateway channels / terminal / memory providers. Known residuals: stop now signals the webhook server and awaits Axum's graceful-shutdown serve future, so Axum waits for the response tails visible to that future; FeishuChannel still does not directly own each accepted handler task, so this is not an independent per-handler ownership/join proof. A serve or join error is retained as `WebhookShutdownResidual` and blocks an unproven restart/disposal. `FeishuApi::admit_inbound()` forms the enqueue quiescence point via the existing calls read guard: after a revoke, newly arriving inbound is fail-closed, while already-admitted sends can still be waited on by `drain()`. Also, `qa/channels` needs a bare-environment Python `websockets` package that is not installed by default. Validation in the B7 implementation round: cargo fmt/test/check/clippy/build passed (only pre-existing clippy warnings) and `qa/channels/run.sh all` returned rc=0 (phase 1 reachability 18/18, phase 2 Lark failure paths 6/6, phase 3 approval 9/9). Full historical decisions and validation are in `docs/reference/FEATURE_LOCATOR.md` §3.5g-historical; current B8.2 status is in `docs/research/aleph_modularization_phase2b_b8_2_closeout.md`.

B7（2026-10-24，未提交）把已冻结的 provider lifecycle contract 复用到**一条** Gateway provider：Feishu channel（`src/gateway/interfaces/feishu/**`，外加 `src/gateway/inbound_router/executor.rs`）。它是 provider slice，**不是** Gateway 已统一。

`src/gateway/channel_registry.rs::ChannelRegistry` 仍是唯一的 Gateway lifecycle authority。channel 内部持有一个 `ProviderHandle`（provider id `gateway.feishu:<channel id>`，`ProviderTransport::InProcess`），在 `ProviderState` 与 generation 上做 admission fence——它不是第二个 registry、不是第二份 enabled 判定、也不能替代 registry。`src/gateway/interfaces/feishu/api_handle.rs` 是 derived cache：`Weak` client + config + 发布时的 provider 实例与 generation；它不是权威。

生命周期：`Cold → Starting → Ready → Stopping → Stopped/Failed`，每次新的 `Starting` 递增 generation。`Ready` 与 cache publication 在 `with_ready_commit` 里同一次提交落定；start 失败 ⇒ revoke → cleanup → provider `Failed` 并返回错误；stop ⇒ revoke → abort + await webhook / token-refresh / ws 任务 → 排空在途 outbound HTTP → `Stopped`。channel 的每个 outbound 动作与 `FeishuApi` 的每个出站 HTTP 调用都按当前 generation 准入（API 的 read guard 覆盖整段调用，含 token await，所以 drain 真的等得住）；入站 webhook 与 WS 转发同样准入（未准入的 webhook 请求返回 `503`）。读侧不启动也不认证：`api_handle::get` 只校验 provider 实例与 generation，`try_create_feishu_emitter` 不再为每条入站消息重建 config + 第二个 `TokenManager`。旧 generation 撤不掉继任者；ws channel identity 仍是 `config.app_id`。

静态 in-process 边界（D-B）：B7 证明的是 authority 与 runtime effect 的撤销，不是代码页未映射、也不是热卸载。B7 **不等于** Zahir live facade（Feishu 没有 capability descriptor / lease / subscribe / project）、**不等于**热卸载，也**不等于** EffectClaim producer、automatic Safe Replay、universal durable scheduler、durable external-effect exactly-once、MCP lazy，或其余 Gateway channel / terminal / memory provider。已知残余：stop 只 abort + await 顶层 webhook accept task；没有显式跟踪/等待每个已接受的 webhook handler，因此**不能**宣称完整的 graceful handler drain。`FeishuApi::admit_inbound()` 以现有 calls read guard 形成入队静止点：revoke 之后新入站的 inbound 是 fail-closed 的，而已准入的 send 仍可被 `drain()` 等待。另外，`qa/channels` 依赖默认未安装的裸环境 Python `websockets` 包。B7 实施轮验证：cargo fmt/test/check/clippy/build 通过（clippy 仅既有 warnings），`qa/channels/run.sh all` rc=0（phase 1 reachability 18/18、phase 2 Lark failure paths 6/6、phase 3 approval 9/9）。完整历史 decisions、残余与验证见 `docs/reference/FEATURE_LOCATOR.md` §3.5g-historical；当前 B8.2 状态见 `docs/research/aleph_modularization_phase2b_b8_2_closeout.md`。

### Provider lifecycle / Provider 生命周期

`src/capability/provider.rs` supplies the shared lifecycle contract for an in-process provider and a future Bridge provider. `ProviderLifecycle` is a side-effect-free state machine: `Discovered`, `Cold`, `Disabled`, `Starting`, `Ready`, `Stopping`, `Stopped`, and `Failed`. Every new `Starting` transition increments the provider generation. `ProviderHandle` only publishes committed state and live notifications; it does not start threads, open IPC, load native libraries, or replace `ExtensionManager`.

`ToolHandlerRegistry` owns the shared provider handle for its callable surface. While the provider is not `Ready`, callable snapshots and resolution are empty while the registered state is retained. `BackendLease` records both the ownership generation and provider generation, so a stop or restart makes an old lease fail closed as `StaleProvider`. `ExtensionManager` and `plugin_state.rs` remain the discovery, effect, and durable enabled-state authorities. In the current bounded production slice, `ExtensionManager` / `RuntimeOwner` drive the provider transitions for ExtensionManager-managed tool-only WASM; `ProjectionHost` / `ZahirFacade` remain derived read/projection/notification surfaces and do not own that lifecycle. A future Bridge supervisor may drive the same contract around a sidecar; native Rust dynamic-library unloading is deliberately outside this foundation. This does not imply a general Gateway or all-capability-kind wiring; see the current Stage H and B8.2 sections above.

`src/capability/provider.rs` 为进程内 Provider 与未来 Bridge Provider 提供共享生命周期契约。`ProviderLifecycle` 是无副作用状态机，覆盖 `Discovered`、`Cold`、`Disabled`、`Starting`、`Ready`、`Stopping`、`Stopped`、`Failed`；每次新的 `Starting` 都递增 Provider generation。`ProviderHandle` 只发布已提交状态和 live notification，不启动线程、不建立 IPC、不加载原生动态库，也不替代 `ExtensionManager`。

`ToolHandlerRegistry` 为自己的 callable surface 持有共享 Provider handle。Provider 未处于 `Ready` 时，callable snapshot 和 resolve 为空，但已注册状态保留；`BackendLease` 同时记录 ownership generation 与 provider generation，因此停止或重启会让旧 lease 以 `StaleProvider` fail-closed。发现、效果执行和持久化 enabled 状态仍由 `ExtensionManager` 与 `plugin_state.rs` 负责。在当前有界生产切片中，`ExtensionManager` / `RuntimeOwner` 驱动 ExtensionManager-managed tool-only WASM 的 provider transition；`ProjectionHost` / `ZahirFacade` 仍是派生的 read/projection/notification surface，不拥有该 lifecycle。未来 Bridge supervisor 可以围绕同一契约驱动 sidecar；原生 Rust 动态库卸载不属于本基础切片。这不表示 Gateway 或所有 capability kind 已统一接线；当前 Stage H 与 B8.2 状态见上文。

Terminal now has a separate bounded source-owned lifecycle in `src/gateway/pty/manager.rs`: the existing `PtyManager` owns sessions, retained owners, bus and one joinable flush driver; one embedded pure `ProviderHandle` fences state/generation/admission. The existing shared `Config::policies.terminal.enabled` controls boot and live apply. Disabled boot does not initialize the manager. Stop revokes admission before checked session cleanup and driver join; failure and cancellation retain source evidence. The synchronous live-apply boundary returns non-live/restart for a pending join or missing cold-boot bus. Re-enable with the existing bus uses a new generation. Operations and screen/exit publication and forwarding reject stale generations while preserving `created_by` ownership. This does not mount a Terminal capability backend or change the Tool-only `ZahirFacade`; kill return and driver join do not prove native unload, child/reader-thread/RSS reclamation or process isolation. See [Terminal lifecycle run](../superpowers/runs/2026-10-31-terminal-lifecycle-run.md).

Terminal 当前在 `src/gateway/pty/manager.rs` 具有独立的受限 source-owned lifecycle：现有 `PtyManager` 拥有 session、retained owner、bus 与一个可 join 的 flush driver，一个内嵌纯 `ProviderHandle` 负责 state/generation/admission fence。既有 shared `Config::policies.terminal.enabled` 决定 boot 与 live apply；disabled boot 不初始化 manager。Stop 在 checked session cleanup 与 driver join 前撤销 admission，失败与取消保留 source evidence。同步 live-apply 对 pending join 或 cold boot 无 bus 如实返回 non-live/restart；已有 bus 的 re-enable 使用新 generation。操作及 screen/exit publication/forwarding 拒绝旧 generation，`created_by` ownership 保持不变。这没有挂载 Terminal capability backend，也不改变 Tool-only `ZahirFacade`；kill 返回与 driver join 不证明 native unload、child/reader-thread/RSS reclamation 或 process isolation。详见上述 run record。

Recovery deliberately does not infer replay safety from a tool name, source, or idempotence flag. `ReplayPolicyLookup` reads the current descriptor. Phase 2A now persists an optional call-time `ToolCallIdentity` on `ToolCallRequested` and carries it through `DanglingCall`; old rows decode to `None`, and unknown nested policy values fail closed. Recovery classifies a call as replay-eligible only when stored and current identities are both `Safe` with equal schema version and a matching nonempty `replay_contract_fingerprint`; `revision` is only a process-local equality/TOCTOU guard, not a cross-restart eligibility criterion. Phase 2A still does not execute eligible tools: it emits the existing unknown-outcome verification event. Safe execution remains a separate Phase 2B decision requiring an executor and external-effect/idempotency contract.

Phase 2B has shipped an inert-restricted replay *preparation* slice (commits `3490103d4` and `7d3bf4a5a` on `feature/capability-phase2-durable`; spec `docs/superpowers/specs/2026-10-04-capability-phase2b-safe-replay-design.md`, plan `docs/superpowers/plans/2026-10-04-capability-phase2b-safe-replay.md`). The shipped surface is: a `replay_contract_fingerprint` carried cross-restart on `ToolCallIdentity`; a private, non-`Copy` `ReplayPermit` produced by a `ReplayPreparer` whose registry-derived handler/descriptor/closed-state are captured from one immutable `ToolHandlerRegistry::snapshot_state()`; the original `call_id`, `tool_name`, `turn_id`, `effective_input`, and stored `ToolCallIdentity` are not registry-derived inputs; a durable expected-head/serialized claim and unanswered-call outcome precondition (`SessionEventStore::claim_replay_call` / `commit_replay_outcome`); and a `ResumeCoordinator::replay_dangling_calls` ordering — prepare/refuse → fresh head/generation fences → claim → invoke → commit → fresh reduction → VerifyOnly. The seam in `src/session/replay.rs` imports no `ToolHandlerRegistry`, `ScopedToolService`, MCP client, or platform API; no second registry, scheduler, ledger, or wire-protocol event is added; the `ReplayInvocation::NoEffect` path that leaked active cursor rows after a successful claim is removed (`7d3bf4a5a`).

Production replay is **inert and VerifyOnly** today. `ToolCallRequested` is emitted before guardrail/sanitization in `src/harness/agent/act.rs:484-509` (serial) and `:821-833` (parallel), so the post-guardrail effective input is not durably captured; `ResumeCoordinator::replay_dangling_calls` keeps `effective_input = None`, and the gate (Option + mandatory `Refused` on `None`) forces every dangling call onto the existing unknown-outcome verification path. **No handler is invoked and no `ToolResult`/`ToolError` is appended in production.** The semantics are restricted at-least-once — original request count, effect invocation count, and durable receipt count are three independent numbers; this is not exactly-once, and the duplicate-effect tradeoff is the Safe registration owner's obligation. Delegated `repair_and_close_abandoned` (team/cron/heartbeat abandonment) remains VerifyOnly with no replay adapter and no permit capability. The post-guardrail durable `ToolCallEffectiveInput` marker, paired with a `release_replay_claim` store operation, is the explicit deferred work that, when landed, will let `effective_input` carry a non-`None` value and unlock actual handler invocation through this seam.

The two real `ToolCallRequested` producers in `src/harness/agent/act.rs` receive only a read-only lookup through `HarnessDeps`. The startup path passes the existing MCP `ToolHandlerRegistry` explicitly; fixtures and builtin paths without a registry remain `None` and therefore fail closed. This is wiring, not recovery policy: the harness does not decide replay or execute recovery.


---


```
                    ┌─────────────┐
                    │   gateway   │ ← Entry point (chat ingress, protocol adapters)
                    └──────┬──────┘
                           │ FlowRequest
          ┌────────────────┼────────────────┐
          │                │                │
          ▼                ▼                ▼
    ┌───────────┐    ┌───────────┐    ┌───────────┐
    │interfaces │    │  routing  │    │orchestrator│
    └───────────┘    └───────────┘    └─────┬─────┘
                                            │ HarnessRunner::run
                           ┌────────────────┼────────────────┐
                           │                │                │
                            ▼                ▼                ▼
                      ┌───────────┐    ┌───────────┐    ┌───────────┐
                      │  harness  │    │  memory   │    │   exec    │
                      └─────┬─────┘    └───────────┘    └───────────┘
                            │
           ┌────────────────┼────────────────┐
           │                │                │
           ▼                ▼                ▼
     ┌───────────┐    ┌───────────┐    ┌───────────┐
     │  thinker  │    │tool_service│   │  engine   │
     └─────┬─────┘    └───────────┘    └─────┬─────┘
           │                                  │
           ▼                                  ▼
     ┌───────────┐                      ┌───────────┐
     │ providers │                      │   tools   │
     └───────────┘                      └─────┬─────┘
                                              │
                            ┌─────────────────┼─────────────────┐
                            │                 │                 │
                            ▼                 ▼                 ▼
                      ┌───────────┐     ┌───────────┐     ┌───────────┐
                      │  builtin  │     │    mcp    │     │ extension │
                      │   tools   │     │  client   │     │ (plugins) │
                      └───────────┘     └───────────┘     └───────────┘
```

---

## Data Flow

### Request Processing

```
Client Request (JSON-RPC)
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Gateway: InboundRouter                              │
│   • Parse JSON-RPC message                          │
│   • Route to appropriate handler                    │
│   • Authentication check (if enabled)              │
│   • Construct FlowRequest { agent_id, input, ... }  │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Orchestrator                                        │
│   • Resolve AgentDef + FlowSpec                     │
│   • Build HarnessDeps (SessionService, ToolService, │
│     Sandbox, AiProvider)                            │
│   • Dispatch to HarnessRunner::run                  │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ AgentHarness: Think → Act loop                      │
│   1. Context: Apply budget, preflight prep          │
│   2. Think: Call Thinker (LLM) for decision         │
│   3. Act: Execute tools via ToolService             │
│   4. Stop-hook: Evaluate completion / guards        │
│   5. Compact: If overflow, compact history          │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ FlowOutcome → Gateway renders response              │
│   • Stream events via TraceSink / EventBus          │
│   • Final response as JSON-RPC result               │
└─────────────────────────────────────────────────────┘
```

### Tool Execution Flow

```
Thinker Decision (tool_use)
    │ validate non-empty, response-unique call_id
    │ persist AssistantMessage before side effects
    ▼
┌─────────────────────────────────────────────────────┐
│ Dispatcher                                          │
│   • Analyze tool request                            │
│   • Check permissions (ToolFilter)                  │
│   • Risk evaluation                                 │
│   • Confirmation flow (if needed)                   │
└─────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────┐
│ Executor                                            │
│   • Lookup tool (Builtin / MCP / Extension)         │
│   • Deserialize arguments                           │
│   • Execute with timeout                            │
│   • Capture output                                  │
└─────────────────────────────────────────────────────┘
    │
    ├─── Builtin Tool (AlephTool trait)
    │       • Direct Rust execution
    │
    ├─── MCP Tool (Model Context Protocol)
    │       • JSON-RPC to external process
    │
    └─── Extension Tool (WASM / Node.js)
            • Plugin runtime execution
```

`call_id` is the correlation key across the provider response, session log,
approval identity, in-flight cancellation, tool result, trace, and UI projection.
The harness rejects empty or duplicate IDs before persisting the assistant event;
it never repairs IDs heuristically. The process-wide in-flight registry is installed
before its Gateway RPC handlers, independently of the optional result store. MCP
restart failures emit `ServerCrashed`, allowing the existing bridge to remove stale
handlers before a later turn snapshots the tool surface.

Exec-class tools (`code_exec`, `bash_exec`) route through an additional
**Sandbox layer** (`src/sandbox/`) between the tool and process execution.
The sandbox owns per-session workspace provisioning
(`~/.aleph/workspaces/{session_id}/`), capability enforcement, and OS-level
seatbelt isolation via `OsSandboxDriver`. Tool-level permissions
(`SmartFilter`) gate *whether* a call runs; the sandbox's capability check
gates *what the subprocess can do* once it is allowed to run. See
[SANDBOX.md](./SANDBOX.md) for the six-step execute pipeline and testing
pattern.

---

## Core Modules Summary

| Module | Path | Purpose |
|--------|------|---------|
| **gateway** | `src/gateway/` | WebSocket server, JSON-RPC routing, interfaces |
| **orchestrator** | `src/orchestrator/` | Resolve FlowSpec, build HarnessDeps, dispatch |
| **harness** | `src/harness/` | Think→Act loop, stop-hooks, context budget, compaction |
| **agents** | `src/agents/` | Agent runtime, subagent spawning, team coordination |
| **thinker** | `src/thinker/` | LLM interaction, prompt building, streaming |
| **dispatcher** | `src/dispatcher/` | Task orchestration, tool filtering |
| **engine** | `src/engine/` | Tool execution engine |
| **providers** | `src/providers/` | AI provider implementations |
| **tools** | `src/tools/` | AlephTool trait, tool server |
| **builtin_tools** | `src/builtin_tools/` | Built-in tool implementations |
| **memory** | `src/memory/` | Facts DB, hybrid retrieval (SQLite + sqlite-vec + markdown notes + wikilink graph) |
| **extension** | `src/extension/` | Plugin system (WASM, Node.js) |
| **exec** | `src/exec/` | Shell execution, approval system, OS-native sandboxing (`OsSandboxDriver`) |
| **sandbox** | `src/sandbox/` | `Sandbox` trait + `WorkspaceSandbox` — per-session workspace, capability ledger, `ApprovalGate`-backed escalation ([SANDBOX.md](./SANDBOX.md)) |
| **mcp** | `src/mcp/` | MCP client implementation |
| **routing** | `src/routing/` | Session key resolution |
| **config** | `src/config/` | Configuration management |
| **runtimes** | `src/runtimes/` | Capability ledger — probe, bootstrap, persist external tool status |
| **session** | `src/session/` | Append-only session event log + in-process actor (see below) |
| **a2a** | `src/a2a/` | A2A protocol adapter |
| **acp** | `src/acp/` | ACP protocol implementation |
| **approval** | `src/approval/` | Approval system |
| **arena** | `src/arena/` | Arena functionality |
| **browser** | `src/browser/` | Browser automation |
| **capability** | `src/capability/` | Capability system |
| **cluster** | `src/cluster/` | Single-center node federation — reverse RPC, node registry, `node_invoke`/`node_file`, approval routing ([CLUSTER.md](./CLUSTER.md)) |
| **components** | `src/components/` | Shared components |
| **compressor** | `src/compressor/` | Context compression |
| **core** | `src/core/` | Core types and primitives |
| **discovery** | `src/discovery/` | Service discovery |
| **event** | `src/event/` | Event system |
| **generation** | `src/generation/` | Media generation |
| **group_chat** | `src/group_chat/` | Group chat management |
| **hub** | `src/hub/` | Aleph Hub — extension catalog + install pipeline ([ALEPH_HUB.md](./ALEPH_HUB.md)) |
| **intent** | `src/intent/` | Intent recognition |
| **logging** | `src/logging/` | Logging infrastructure |
| **loop_graph** | `src/loop_graph/` | Loop-governance topology (who watches/audits/anchors whom) — scaffolding only, adjudication stays in LLM turns ([GRAPH_LAYER.md](./GRAPH_LAYER.md)) |
| **markdown** | `src/markdown/` | Markdown processing |
| **media** | `src/media/` | Media processing |
| **metrics** | `src/metrics/` | Metrics collection |
| **permission** | `src/permission/` | Permission system |
| **pii** | `src/pii/` | PII detection and handling |
| **prompt** | `src/prompt/` | Prompt management |
| **resilience** | `src/resilience/` | State management (SQLite) |
| **search** | `src/search/` | Search providers |
| **secrets** | `src/secrets/` | Secret management |
| **security** | `src/security/` | Security utilities |
| **skill** | `src/skill/` | Skill system |
| **supervisor** | `src/supervisor/` | Execution supervision |
| **tasks** | `src/tasks/` | Task management |
| **teams** | `src/teams/` | Team coordination |
| **tool_output** | `src/tool_output/` | Tool output handling |
| **utils** | `src/utils/` | Utilities |
| **vision** | `src/vision/` | Vision processing |
| **wizard** | `src/wizard/` | Wizard flows |

---

## Agent execution

### Session Service

Agent execution's authoritative session state lives in
[`SessionService`](./SESSION_SERVICE.md) (`src/session/`). The
`InProcessActorSessionService` spawns one tokio task per session and
persists each event synchronously to the `session_events` table.
`AgentHarness` reads and writes history exclusively through
`SessionService`. Gateway `session.*` RPC methods continue to use the
legacy `SessionManager`. **There is no dual-write shim.** This paragraph
used to name `src/session/shim.rs`: that file does not exist, and the
mirroring it described was removed when `session_events` became the SSOT.
`MessageProjector` (`src/gateway/session_projector.rs`) materialises
`messages` from `session_events` asynchronously, and since 2026-09-13 it is
the table's **only production writer**, pinned to the tree by
`session_projector::tests::the_projector_is_the_only_production_writer_of_the_messages_table`.
The two direct writers that used to bypass the log —
`AgentInstance::add_message` and the boot orphan notice — are gone (T17, T16);
rows they left carry a NULL `source_seq`, which `map_message_row`
(`src/gateway/session_manager/ops/crud.rs`) reads back as "not event-sourced,
leave it alone". See SESSION_SERVICE.md.

---

## Design Patterns

Aleph employs several key design patterns to ensure code quality, type safety, and maintainability:

### Context Pattern

Groups related function parameters into dedicated structs, reducing parameter count and improving API ergonomics.

**Example: `HarnessDeps` / `FlowRequest`**
```rust
// Before: 7 parameters passed directly
legacy_agent_loop.run(request, context, tools, identity, callback, abort_signal, initial_history).await

// After: structured deps + FlowRequest
let deps = HarnessDeps { session, tool_service, sandbox, provider, trace_sink };
let flow = FlowRequest::new(agent_id, input, identity)
    .with_abort_signal(abort_rx);
HarnessRunner::run(flow, deps).await
```

**Benefits:**
- Extensibility: Add parameters without breaking changes
- Readability: Clear parameter grouping
- Type Safety: Compile-time validation
- Ergonomics: Builder pattern for optional parameters

### Newtype Pattern

Wraps primitive types in distinct structs for type safety and semantic clarity.

**Example: ID Types**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExperimentId(String);

impl ExperimentId {
    pub fn new(id: impl Into<String>) -> Self { Self(id.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl Deref for ExperimentId {
    type Target = str;
    fn deref(&self) -> &Self::Target { &self.0 }
}
```

**Newtype Catalog:**
- **IDs**: `ExperimentId`, `VariantId`, `ContextId`, `TaskId`, `SubscriptionId`
- **Collections**: `Ruleset` (permission rules)
- **Values**: `Answer` (question responses)

**Benefits:**
- Type Safety: Prevents mixing different ID types
- Self-Documentation: Clear semantic meaning
- Encapsulation: Controlled access to inner value
- Extension Points: Add methods without modifying primitives

### FromStr Trait Pattern

Provides consistent parsing interface across the codebase.

**Example:**
```rust
impl FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            _ => Err(format!("Invalid TaskStatus: {}", s)),
        }
    }
}

// Usage
let status: TaskStatus = "pending".parse()?;
```

**Implemented for:** `FactType`, `HookKind`, `DeviceType`, `TaskStatus`, `RuntimeKind`, and 10+ other types.

### Complete Documentation

For detailed information on design patterns, implementation guidelines, and migration guides, see:
- **[DESIGN_PATTERNS.md](DESIGN_PATTERNS.md)** - Complete design patterns reference

---

## Providers

### Protocol Adapter Architecture

Aleph uses a layered protocol adapter system supporting multiple AI provider protocols:

**Layer 1: Built-in Protocols** (Compiled Rust)
- `OpenAiProtocol` - OpenAI-compatible APIs
- `AnthropicProtocol` - Claude/Anthropic APIs
- `GeminiProtocol` - Google Gemini APIs
- `OllamaProvider` - Local Ollama (native implementation)

**Layer 2: Configurable Protocols** (YAML-based, hot-reload)
- Minimal configuration mode - Extend existing protocols with differences
- Full template mode - Completely custom protocol implementations
- Loaded from `~/.aleph/protocols/` directory
- Changes detected within 600ms (500ms debounce + processing)

**Layer 3: Extension Protocols** (Future)
- WASM/Node.js plugin protocols
- Independent process protocols (MCP/gRPC)

#### Protocol Resolution Flow

```
User config.protocol
    ↓
ProtocolRegistry.get(name)
    ↓
├─> Dynamic protocols (YAML-loaded) ───> ConfigurableProtocol
│   ├─> Minimal mode: base + differences
│   └─> Custom mode: template rendering
├─> Built-in protocols ───> OpenAi/Anthropic/Gemini
└─> Not found ───> Error with available list
```

#### Hot Reload Mechanism

1. `notify-debouncer-full` watches `~/.aleph/protocols/`
2. File change detected (Create/Modify/Delete)
3. YAML parsed into `ProtocolDefinition`
4. `ConfigurableProtocol` created
5. Registry updated atomically
6. New requests use updated protocol

See `docs/PROTOCOL_ADAPTER_USER_GUIDE.md` for user documentation.

---

## Feature Flags

所有生产功能始终编译，无需 feature flags。仅保留测试用 features：

```toml
[features]
default = []
loom = ["dep:loom"]       # 并发测试
test-helpers = []          # 集成测试工具
```

通道在运行时通过 `aleph.toml` 配置启用/禁用。

---

## Platform Architecture

### Desktop Bridge

The desktop bridge provides native OS capabilities to the core through IPC:

```
desktop/
├── shared/                   # DesktopCapability trait + IPC protocol
├── macos/                    # macOS native implementation (AppKit, Vision)
├── linux/                    # Linux native implementation
└── windows/                  # Windows native implementation
```

## Swift Helper Process (macOS)

aleph-server spawns `AlephBridge` (Swift) as a long-lived child process for
native macOS API access. The process-level split of responsibilities:

- **Swift owns**: AVFoundation (camera, audio), Vision (OCR),
  Accessibility (AX tree), SFSpeechRecognizer (speech-to-text)
- **Rust owns**: IOKit (`IOPMAssertion` for the sleep inhibitor), TCC
  framework status checks via `objc2-av-foundation` / `objc2-speech` in
  `permission.rs`, all business logic, vault, and sessions

This satisfies architectural redline R1 (Brain–Limb separation): the Rust
core defines `DesktopCapability` traits; the helper provides physical
implementations over JSON-RPC 2.0 stdio. See
[DESKTOP_BRIDGE.md](DESKTOP_BRIDGE.md) for the full protocol specification,
method reference, error envelope, and debugging procedures.

### Web Chat Interface

```
interfaces/webchat/          # Web-based chat interface (Leptos/WASM panel)
```

### Terminal Interface (TUI)

```
interfaces/tui/              # Full-screen terminal chat (ratatui + crossterm)
```

A thin, remote client: it speaks JSON-RPC to the Gateway over a WebSocket via
`aleph-client` + `aleph-protocol` and **must not depend on alephcore** (enforced
in its `Cargo.toml`). Like the CLI it holds no agent/tools/memory in-process —
it is a pure I/O interface (redline R4): user input → JSON-RPC request, and
`StreamEvent`s → rendered output. See
[FEATURE_LOCATOR.md](FEATURE_LOCATOR.md) §5.13 for the file-level map.

### Browser Live Viewer (Panel browser workspace)
<!-- browser-live-docs:begin -->
Browser execution stays in the Gateway/Core (`src/browser/`, `crates/aleph-cdp`); the Panel Browser tab and the TUI launcher never speak CDP. The Gateway issues one-shot viewer grants and carries the viewer WebSocket (`src/gateway/server/browser_live_runtime.rs`, `src/gateway/handlers/browser_viewer.rs`); Panel renders and collects local input only (R2/R4), and `interfaces/tui/src/viewer_launcher.rs` is a disabled-by-default local launch seam. Layers, call chain and status table: [BROWSER_LIVE_VIEWER.md](BROWSER_LIVE_VIEWER.md).

浏览器执行留在 Gateway/Core；Panel / TUI 不碰 CDP。

- **共同边界 / Shared boundary**：Gateway 独占 CDP（Gateway owns CDP）；Panel/TUI 仅 I/O only 呈现；仅显式合法 `[gateway] browser_viewer_origin` 才 opt-in，默认（None / 非法）deny-only；producer 的 `Safe` 被钳为 `Pending`，`Safe` 只来自用户经 `browser.viewer.resource.decide` 的 revision CAS；绑定走精确 binding / owner / generation；#27 TUI paired/browser 已接线，Windows Credential Manager standalone probe 1/1 已通过；macOS/Linux provider 与真实 pairing/browser E2E 仍 blocked；`NetworkContextGrant` / proxy 未触碰；**not production ready**。
Gateway owns CDP; Panel/TUI are I/O only; opt-in only for an explicit valid `[gateway] browser_viewer_origin`, default deny-only; producer `Safe` is clamped to `Pending` and `Safe` comes only from the user's decision CAS; exact binding/owner/generation; #27 TUI paired/browser wiring is implemented; the Windows Credential Manager standalone probe passed 1/1; macOS/Linux providers and real pairing/browser E2E remain blocked; network grants untouched; not production ready.
<!-- browser-live-docs:end -->

---

## Identity Context Flow

### Overview

IdentityContext is an immutable identity snapshot that flows through the entire execution chain, enabling identity-based permission enforcement at the tool execution level.

### Flow Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                    Identity Context Flow                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  1. Session Creation                                             │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  SessionManager                                           │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ Owner Session:                                      │  │  │
│  │  │   metadata = SessionIdentityMeta {                  │  │  │
│  │  │     role: Owner,                                    │  │  │
│  │  │     identity_id: \"owner\",                           │  │  │
│  │  │     scope: None                                     │  │  │
│  │  │   }                                                 │  │  │
│  │  │                                                     │  │  │
│  │  │ Guest Session:                                      │  │  │
│  │  │   metadata = SessionIdentityMeta {                  │  │  │
│  │  │     role: Guest,                                    │  │  │
│  │  │     identity_id: \"guest-123\",                       │  │  │
│  │  │     scope: Some(GuestScope {                        │  │  │
│  │  │       allowed_tools: [\"translate\"],                │  │  │
│  │  │       expires_at: Some(1735689600)                  │  │  │
│  │  │     })                                              │  │  │
│  │  │   }                                                 │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  2. Request Processing                                           │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  ExecutionEngine                                          │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ let identity = session_manager                      │  │  │
│  │  │     .get_identity_context(&session_key, \"gateway\")  │  │  │
│  │  │     .await?;                                        │  │  │
│  │  │                                                     │  │  │
│  │  │ // IdentityContext {                               │  │  │
│  │  │ //   request_id: \"req-456\",                        │  │  │
│  │  │ //   session_key: \"session-123\",                   │  │  │
│  │  │ //   role: Guest,                                  │  │  │
│  │  │ //   identity_id: \"guest-123\",                     │  │  │
│  │  │ //   scope: Some(GuestScope { ... }),              │  │  │
│  │  │ //   created_at: 1735689000,                       │  │  │
│  │  │ //   source_channel: \"gateway\"                     │  │  │
│  │  │ // }                                               │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  3. Harness Execution                                            │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  HarnessRunner::run(                                      │  │
│  │      FlowRequest {                                        │  │
│  │          agent_id,                                        │  │
│  │          input,                                           │  │
│  │          identity,  // ← IdentityContext passed here     │  │
│  │          abort_signal,                                    │  │
│  │      },                                                   │  │
│  │      HarnessDeps { session, tool_service, sandbox, ... } │  │
│  │  )                                                        │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  4. Tool Execution                                               │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  Executor::execute(&action, &identity)                    │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ // Normalize tool name                             │  │  │
│  │  │ let normalized = normalize_tool_name(tool_name);   │  │  │
│  │  │                                                     │  │  │
│  │  │ // Check permission                                │  │  │
│  │  │ let result = PolicyEngine::check_tool_permission(  │  │  │
│  │  │     &identity,                                     │  │  │
│  │  │     &normalized                                    │  │  │
│  │  │ );                                                 │  │  │
│  │  │                                                     │  │  │
│  │  │ match result {                                     │  │  │
│  │  │     Allowed => execute_tool(...),                  │  │  │
│  │  │     Denied { reason } => ToolError { error: reason }│  │  │
│  │  │ }                                                  │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  5. Permission Check                                             │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  PolicyEngine::check_tool_permission                      │  │
│  │  ┌────────────────────────────────────────────────────┐  │  │
│  │  │ match identity.role {                              │  │  │
│  │  │     Role::Owner => Allowed,                        │  │  │
│  │  │                                                     │  │  │
│  │  │     Role::Guest => {                               │  │  │
│  │  │         // Check scope                             │  │  │
│  │  │         if scope.is_none() {                       │  │  │
│  │  │             return Denied { \"no scope\" };          │  │  │
│  │  │         }                                          │  │  │
│  │  │                                                     │  │  │
│  │  │         // Check expiration                        │  │  │
│  │  │         if is_expired() {                          │  │  │
│  │  │             return Denied { \"expired\" };           │  │  │
│  │  │         }                                          │  │  │
│  │  │                                                     │  │  │
│  │  │         // Check tool permission                   │  │  │
│  │  │         if allowed_tools.contains(tool_name) {     │  │  │
│  │  │             Allowed                                │  │  │
│  │  │         } else {                                   │  │  │
│  │  │             Denied { \"not in scope\" }              │  │  │
│  │  │         }                                          │  │  │
│  │  │     }                                              │  │  │
│  │  │                                                     │  │  │
│  │  │     Role::Anonymous => Denied { \"auth required\" }  │  │  │
│  │  │ }                                                  │  │  │
│  │  └────────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Example: Owner Request

```rust
// 1. Owner creates session (default)
let session_key = "owner-session-1";
let metadata = SessionIdentityMeta::owner("gateway");

// 2. Request arrives
let identity = session_manager.get_identity_context(session_key, "gateway").await?;
// identity.role = Role::Owner

// 3. Execute tool
let action = Action::ToolCall {
    tool_name: "shell_exec".to_string(),
    arguments: json!({"command": "ls"}),
};

let result = executor.execute(&action, &identity).await;
// Result: ToolSuccess (Owner bypasses all checks)
```

### Example: Guest Request

```rust
// 1. Guest activates invitation
let invitation = manager.create_invitation(CreateInvitationRequest {
    guest_name: "Alice".to_string(),
    scope: GuestScope {
        allowed_tools: vec!["translate".to_string()],
        expires_at: Some(now + 3600),
        display_name: Some("Alice".to_string()),
    },
})?;

let guest_token = manager.activate_invitation(&invitation.token)?;

// 2. Guest creates session
// SessionManager stores metadata with guest scope

// 3. Request arrives
let identity = session_manager.get_identity_context(session_key, "gateway").await?;
// identity.role = Role::Guest
// identity.scope = Some(GuestScope { allowed_tools: ["translate"], ... })

// 4. Execute allowed tool
let action1 = Action::ToolCall {
    tool_name: "translate".to_string(),
    arguments: json!({"text": "Hello"}),
};

let result1 = executor.execute(&action1, &identity).await;
// Result: ToolSuccess (tool in allowed_tools)

// 5. Execute denied tool
let action2 = Action::ToolCall {
    tool_name: "shell_exec".to_string(),
    arguments: json!({"command": "ls"}),
};

let result2 = executor.execute(&action2, &identity).await;
// Result: ToolError { error: "Tool 'shell_exec' not in guest 'guest-123' scope" }
```

### Key Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| **IdentityContext** | `shared/protocol/src/auth.rs` | Immutable identity snapshot |
| **SessionIdentityMeta** | `src/gateway/session_manager/` | Persistent identity metadata |
| **PolicyEngine** | `src/gateway/security/policy_engine.rs` | Stateless permission checker |
| **InvitationManager** | `src/gateway/security/invitation_manager.rs` | Guest invitation lifecycle |
| **SessionManager** | `src/gateway/session_manager/` | Identity construction |
| **Orchestrator** | `src/orchestrator/` | Identity injection into FlowRequest |
| **AgentHarness** | `src/harness/` | Identity propagation through Think→Act loop |
| **Engine** | `src/engine/` | Permission enforcement |

### Security Properties

1. **Immutability**: IdentityContext cannot be modified after creation
2. **Frozen Permissions**: Guest scope is frozen at session creation time
3. **Stateless Checks**: PolicyEngine has no mutable state
4. **Certificate of Authority**: Identity is constructed once and passed down
5. **Fail-Safe**: Missing or invalid metadata defaults to Owner (backward compatible)

---

## Prompt System

The prompt system lives entirely in `src/thinker/`. The sole public entry point is `thinker::PromptBuilder`, which wraps a `PromptPipeline`. The old `agent_loop::PromptBuilder` was removed during the Harness migration (Phase 6/7).

### PromptBuilder

```rust
// Standard usage — identity is a single file-based source of truth:
// agent-dir SOUL.md (persona), AGENTS.md (project context), etc. ride in via
// `with_identity_files`; `SoulLayer` / `ProfileLayer` render them.
let builder = PromptBuilder::new(config).with_identity_files(identity_files);

// Production main-loop entry point — two-part split (cacheable stable prefix +
// per-request dynamic suffix) on `AssemblyPath::Cached`. This is what the
// harness bridge calls.
let parts = builder.build_system_prompt_cached_with_mode(&tools, mode);

// Sub-agent inline prompt — one flat string, no cache split, on
// `AssemblyPath::Basic`. This is what `subagent_spawner` calls.
let prompt = builder.build_system_prompt(&tools);
```

> **Two entry points, two assembly paths, and nothing else.** The legacy
> `SoulManifest`→prompt builders (`build_system_prompt_with_soul` /
> `build_for_agent` / `build_for_agent_basic` / `build_with_budget` /
> `build_system_prompt_with_mode` / `build_system_prompt_with_full_context` /
> `build_system_prompt_with_context`) and the fork-snapshot cluster
> (`capture_snapshot` / `execute_stable_only` / `build_from_snapshot`) were all
> removed as dead entry points. Identity flows exclusively from the agent-dir
> files threaded via `with_identity_files`; the `SoulManifest` struct survives
> only as the parser behind the `identity.get` RPC.

### Layers

**The registration block in `PromptPipeline::default_layers()` is the only
authoritative list** — layer set, priorities, and assembly order all live there,
and each layer self-declares its `priority()`, `stability()`, `paths()` and
`supports_mode()`. This document deliberately does **not** reproduce that list.

`stability()` has **no default body** (2026-08-04) — a new layer that says nothing
about whether its bytes change per turn does not compile. It used to default to
`Stable`, which meant omission silently placed the layer in the provider-cached
prefix; `ToolRuntimeStateLayer` rode that default until a person reading the code
noticed a 30-second health probe was re-keying whole sessions. The question is
asked at the one moment its author knows the answer. See FEATURE_LOCATOR §2.19 ⑥.

A hand-maintained priority table used to live here, and by 2026-07-26 it named
eight layers that no longer existed (`HydratedToolsLayer`, `HeartbeatLayer`,
`ResponseFormatLayer`, `SkillModeLayer`, `InboundContextLayer`,
`MemoryAugmentationLayer`, and more), three assembly paths that had been
deleted, and a `TokenBudget` mechanism that had been replaced. The same table
was removed from `prompt_pipeline.rs` years earlier for the same reason. A
duplicated list of a thing that changes is a list that will be wrong.

To see the current set, with sizes, run:

```bash
aleph-server prompt-size --path cached --paradigm background
```

It prints every contributing layer (bytes / chars / tokens / zone / priority)
plus, by name, every layer that stayed silent and why that is possible.

**Invariants** (each locked by a test, not by prose):

| Invariant | Guard |
|---|---|
| Priorities are unique and ascending | `default_layers_have_unique_priorities`, `test_default_layers_sorted` |
| Every Stable layer precedes every Dynamic one (this boundary *is* the prompt-cache breakpoint) | `stable_layers_come_before_dynamic` |
| Stable + Dynamic concatenate back to the full assembly | `stable_plus_dynamic_reconstructs_full` |
| Every layer can actually speak in production, or is declared conditionally-silent with a reason | `prompt_contract::reachable_layers` |
| The fixed scaffold does not grow | `prompt_contract::scaffold_bytes_ratchet` |
| No sentence is emitted by two layers | `prompt_contract::no_sentence_is_stated_twice` |

### Assembly Paths

Exactly one variant per entry point — a path no caller requests is a trap,
because a layer listing only that path renders nowhere and the omission is
invisible. Three such phantoms (`Context`, `Hydration`, `Soul`) have been
removed for precisely that reason.

| Path | Entry point |
|------|-------------|
| `Basic` | `build_system_prompt` — sub-agent inline prompt, one flat string |
| `Cached` | `build_system_prompt_cached_with_mode` — main loop, stable/dynamic split |

### Prompt Modes

`[execution] prompt_mode` picks the verbosity tier; each layer opts out via
`supports_mode()`.

| Mode | Behavior |
|------|----------|
| `Full` (default) | Every layer participates |
| `Compact` | Sheds the heavy guidance layers (see `compact_mode_excludes_heavy_layers`) |
| `Minimal` | Persona / curated memory / language only (see `minimal_mode_only_core_layers`) |

### Stable-Prefix Reuse

The prompt is partitioned by each layer's `LayerStability` into a cacheable
**Stable** prefix (persona, security, skills …) and a per-request **Dynamic**
suffix (session / memory / runtime context).
`build_system_prompt_cached_with_mode()` returns
`[SystemPromptPart { cache: true, .. }, SystemPromptPart { cache: false, .. }]`,
so the provider (e.g. Anthropic) caches the prefix; the breakpoint sits exactly
at the Stable→Dynamic boundary.

A fresh `PromptBuilder` (and pipeline) is constructed per build, so there is no
in-pipeline mutable cache to invalidate. Layer renders are deterministic
functions of their inputs, so rebuilding stays byte-stable when nothing changed
— which is what keeps the provider-side prefix cache warm. (An earlier
name-keyed `execute_cached()` and a session-level stable-prompt LRU were both
removed: keyed by layer name with no input fingerprint, they could only ever
serve stale sections.)

**Corollary that governs what may be added:** anything whose bytes change per
run must NOT enter the system prompt, or it re-keys the whole conversation
prefix. Per-run content travels as a transient trailing message instead
(`HarnessDeps::recall_context`). This is why `RuntimeContextLayer` coarsens its
timestamp to the hour and why per-query memory recall left the prompt entirely.

### AgentRoleLayer

Replaces the old `prompt_sections::resolve()` function. When
`LayerInput.agent_def` is set, this layer injects:
- Role header from `AgentDef.role`
- Protocol blocks declared in `AgentDef.prompt_sections` (e.g.
  `explore_constraints`, `coder_guidelines`, `researcher_protocol`,
  `verify_protocol`, `plan_protocol`)

### What belongs in the system prompt

Two filters, both enforced by tests rather than by review discipline
(see [HARNESS_PHILOSOPHY.md §8](HARNESS_PHILOSOPHY.md)):

1. **Runtime fact, not instruction.** Time, cwd, active goal, security posture,
   identity files — things the model cannot know. Not "how to think".
2. **No single tool owns it.** If one tool owns the sentence, it belongs in that
   tool's `DESCRIPTION`, which ships with its schema and only reaches requests
   that can actually call it. The system prompt carries only what no tool can
   state: cross-tool routing, runtime facts, safety boundaries.

### TokenBudget

`TokenBudget` caps the assembled prompt in characters (`max_total_chars`,
window-scaled via `window_char_budget`). Enforcement is `fit_dynamic_suffix`:
the **stable prefix is a protected floor** and is never trimmed, so the cache
breakpoint stays valid; only the per-request dynamic suffix is head/tail
truncated, with a model-visible notice appended. A no-op for prompts under
budget.

This is an overflow backstop, not a leanness measure — the fixed scaffold is
two orders of magnitude below the cap, so only
`prompt_contract::SCAFFOLD_CEILING_BYTES` can detect prompt bloat. See
FEATURE_LOCATOR §1.2 for the distinction.

---



## Gateway source-owned lifecycle boundary / Gateway source-owned 生命周期边界

For the verified Stage B Generic Webhook/loopback composition, Gateway association remains in the existing `ChannelRegistry` map and entry. Entry-local runtime state records only retirement markers, admitted work, task/forwarder handles, and cleanup residuals. Provider instance/state/generation stays in `ProviderHandle`; owner/lifetime/generation and irreversible binding state stays in `OwnershipTree`. As of 2026-10-30, the bounded source-owned read/lease surface in `src/gateway/channel_registry/capability.rs` derives `describe_bound_capabilities`, `list_bound_capabilities`, `project_bound_capabilities`, and `resolve_bound_capability` from those same authorities. It is an asynchronous source read model, not a production mount or a claim that all externally composable capabilities have already migrated.

对已验证的 Stage B Generic Webhook/loopback 组合，Gateway association 仍由既有 `ChannelRegistry` map 与 entry 持有。entry-local runtime state 只记录 retirement marker、admitted work、task/forwarder handle 与 cleanup residual。provider instance/state/generation 仍由 `ProviderHandle` 持有；owner/lifetime/generation 与不可逆 binding state 仍由 `OwnershipTree` 持有。2026-10-30 起，`src/gateway/channel_registry/capability.rs` 中的有界 source-owned read/lease surface 从这些相同 authority 派生 `describe_bound_capabilities`、`list_bound_capabilities`、`project_bound_capabilities` 与 `resolve_bound_capability`。它是异步 source read model，不是 production mount，也不表示所有 externally composable capability 已迁移。

Retirement uses a short source-gate fence and final publication commit; real channel, transport, stop, close, forwarder, and join I/O stays outside the gate. Revision exhaustion fails closed before destructive retirement or final publication. R1 remains NO-GO/deferred pending the broader resource and compatibility evidence.

retirement 使用短 source-gate fence 与最终 publication commit；真实 channel、transport、stop、close、forwarder 和 join I/O 均在 gate 外执行。revision exhaustion 在 destructive retirement 或最终 publication 前 fail-closed。R1 仍为 NO-GO/deferred，等待更广泛的 resource 与 compatibility 证据。

- [Agent System](AGENT_SYSTEM.md) - Agent loop internals
- [Gateway](GATEWAY.md) - WebSocket protocol and RPC methods
- [Cluster](CLUSTER.md) - Single-center node federation (center/node, reverse RPC, `node_invoke`/`node_file`)
- [Tool System](TOOL_SYSTEM.md) - Tool development guide
- [Memory System](MEMORY_SYSTEM.md) - RAG and retrieval
- [Extension System](EXTENSION_SYSTEM.md) - Plugin architecture
- [Security](SECURITY.md) - Exec approval and permissions
- [Sandbox](SANDBOX.md) - `Sandbox` trait, `WorkspaceSandbox`, capability pipeline
- [Skill Sandboxing](SKILL_SANDBOXING.md) - OS-native sandboxing for evolved skills
- [Skill Model Taxonomy](SKILL_MODEL_TAXONOMY.md) - Four-layer skill type map (Phase 1 of unification 2026-05-20)
- [Markdown Skill Authoring](MARKDOWN_SKILL_AUTHORING.md) - SKILL.md frontmatter + sandbox modes + host-mode contract
