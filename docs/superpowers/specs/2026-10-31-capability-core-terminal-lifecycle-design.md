# Capability Core 下一阶段设计：Terminal source-owned lifecycle boundary
# Capability Core Next-Phase Design: Terminal Source-Owned Lifecycle Boundary

日期 / Date: 2026-10-31
状态 / Status: 待下一会话 architecture/design approval

## Intent / 意图

当前 Generic Gateway 已完成 source-owned asynchronous `describe/list/project/resolve` 与 replacement fencing，但它仍是 source-private slice。下一阶段选择 Terminal，先关闭一个直接阻碍最终架构目标的真实生命周期缺口：`PtyManager` 的事件总线与 flush loop 目前在 `GatewayServer::build_router` 中无条件 attach/start，`start_flush_loop` 使用进程级 `AtomicBool` 只允许启动一次，无法表达 disabled boot、显式 stop、重新 start、generation 替换和 stale admission。

The next phase closes the terminal lifecycle boundary before attempting a universal mount. The target is a source-owned provider lifecycle slice over the existing `PtyManager`, with truthful disabled behavior and explicit start/stop/replacement fencing. The phase must preserve source ownership and must not turn the current synchronous Tool-only `ZahirFacade` into a Gateway-dependent async runtime.

最终架构目标仍是：

> Everything externally composable is a Capability.
>
> 一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。

本阶段只证明 Terminal provider 的 lifecycle/admission boundary；它不宣称 Terminal 已成为统一 Zahir backend，也不宣称 native process/thread/RSS reclamation。

## Goals / 目标

- 复用现有 `PtyManager` 作为唯一 Terminal session、owner retention、event-bus sink 和 cleanup authority。
- 复用 `ProviderHandle` 作为 Terminal provider state/generation/admission authority；ProviderHandle 仍只发布状态，不执行线程、PTY、bus 或 OS effect。
- disabled boot 不调用 `manager()`、不 attach event bus、不创建 flush task、不创建 PTY session。
- enabled activation 显式启动 event bus delivery/flush driver；stop 撤销 admission、关闭 sessions、等待 flush driver，并保留失败 cleanup evidence。
- restart/reenable 分配新的 provider generation；旧 generation 的 `pty.*` admission 和旧 flush completion fail closed。
- 保持现有 `policies.terminal.enabled` 为 enabled-state 配置 authority，不新增 enabled map 或 runtime table。
- 把当前 `start_flush_loop` 的永久单例语义改为可验证的 source-owned lifecycle driver。
- 为后续 Terminal capability descriptor/read model 留下 source-backed boundary，但本阶段不接入 `ZahirFacade`、MCP/ACP projection 或通用 Capability registry。

## Non-goals / 非目标

- 不新增 `CapabilityKind`。
- 不创建第二个 `PtyManager`、session registry、owner map、descriptor catalog、handler map、enabled-state map 或 durable store。
- 不把 `PtyManager::owners` 复制到 `OwnershipTree`；Terminal session owner 的现有 source-owned authority 保持单一。若未来要迁移到 `OwnershipTree`，必须另开 architecture gate。
- 不实现 Terminal `BackendLease`、Zahir route、MCP/ACP resource projection、Gateway subscription 或 durable recovery。
- 不迁移 Memory、Sandbox、Feishu 或其他 Gateway provider。
- 不声称关闭 PTY session 等于证明 OS child process、reader thread 或 native resource 已被完全回收；只能报告现有 `PtySession::kill` / join evidence 和结构化 residual。
- 不修改 `src/harness/**`。

## Current source facts / 当前源码事实

- `src/gateway/pty/manager.rs::GLOBAL` 是 process-global `LazyLock<PtyManager>`；`PtyManager` 持有 `Mutex<Inner>`、event bus 和 session cap。
- `src/gateway/pty/manager.rs::attach_event_bus` 写入 bus 后调用 `start_flush_loop`。
- `PtyManager::start_flush_loop` 使用 process-global `AtomicBool STARTED`，启动 `tokio::spawn` 后没有显式 stop/join path。
- `src/gateway/server/mod.rs::GatewayServer::build_router` 无条件调用 `crate::gateway::pty::attach_event_bus(self.event_bus.clone())`。
- `src/config/types/policies/terminal.rs::TerminalConfig` 已有 `enabled`，默认 true；配置注释要求关闭时终止 live sessions。
- `src/config/live_apply.rs` 在 `policies.terminal` disabled 时调用 `PtyManager::close_all`，但当前 live apply 没有对 flush driver 做 stop/restart fencing。
- `src/gateway/handlers/pty.rs` 在 `pty.spawn` admission 读取 live config；其他 addressed operations 通过 `PtyManager` session lookup。
- `src/capability/provider.rs` 的 `ProviderHandle` 具备 checked provider generation、state transition 和 exact-generation call admission，但自身不执行 runtime effect。

这些事实支持先修 Terminal lifecycle boundary。它们不足以证明一个通用 Terminal `BackendLease` 或 Zahir mount，因此这些内容留在后续 gate。

## Approaches / 方案比较

### A. 直接把 Terminal 接入当前 `ZahirFacade`

这会要求同步 `CapabilityBackend` / `Zahir` 直接读取 Tokio/PTY lifecycle，或者在 Core 内部阻塞等待 Gateway 状态；两者都会破坏当前依赖方向和 source-owned async boundary。它还会把 Terminal session owner 字符串、provider generation 和 `OwnershipTree` 绑定混成一个尚未批准的跨 authority contract。拒绝。

### B. 先迁移 Memory 或 Sandbox

Memory 的 `MemoryExtensionRegistry` 既是 dispatch registry，又有 MCP typed side-table 和 async hook execution；Sandbox 的 authority 分散在 factory、workspace、driver、approval 和 security hooks。两者都没有像 Terminal 一样清晰的单一 lifecycle driver 和 live enabled gate。现在迁移会先制造抽象，再发现 lifecycle 事实不完整。延期。

### C. 先关闭 Terminal source-owned lifecycle boundary（推荐）

在 `PtyManager` 内保留 session/bus authority，在同一 source owner 内增加现有 `ProviderHandle` 的 provider state/generation，并把 flush loop 变成可 start/stop/join 的 source-owned driver。`GatewayServer` 和 live config apply 只调用这个 source-owned lifecycle API，不新增中心 registry。这样先证明 disabled/no-start、stop/drain、restart fencing 和 residual handling，再决定是否需要一个 source-neutral async projection contract。

推荐理由不是抽象更少，而是它直接覆盖最终目标中最容易被错误声称的资源边界：当前代码确实无条件启动了 Terminal driver，而这一步可以在不改 Core→Gateway 依赖方向的情况下修复并验证。

## Contract / 契约

### Authority map / 权威分工

| Fact / 事实 | 唯一 authority | 只读者 | 修改者 |
|---|---|---|---|
| Terminal enabled intent | `Config::policies.terminal.enabled` | server boot, live apply, handlers | existing config patch/reload path |
| PTY session map, owner retention, event bus sink | `PtyManager` | handlers, lifecycle driver | `PtyManager` methods |
| provider state/generation/admission | one `ProviderHandle` owned by `PtyManager` | handlers/driver | source lifecycle driver via ProviderHandle |
| flush driver task ownership | `PtyManager` | lifecycle tests/diagnostics | `start` / `stop` source methods |
| session child/process cleanup | existing `PtySession` and `PtyManager::close/close_all` | stop/recovery paths | existing session cleanup methods |
| durable PTY journal | existing process journal | status/recovery | existing journal writer |

The plan must not add a second row owner for any fact.

### Lifecycle / 生命周期

`Discovered → Cold → Starting → Ready → Stopping → Stopped` is the successful path. A failed activation or cleanup reaches `Failed` and retains residual evidence. Every `Starting` transition allocates a checked new provider generation. `ProviderHandle` state publication is separate from the effect driver: the driver starts/stops the flush task and closes sessions; the handle only fences admission.

Allowed read behavior:

- `Discovered/Cold/Disabled/Stopped`: descriptor/provider status may be read without starting a driver.
- `Starting/Stopping/Failed`: reads may report state; new `pty.*` execution admission fails closed unless the existing operation explicitly allows cleanup.
- `Ready`: new session operations require the current provider generation; existing admitted work follows the existing session cleanup contract.

### Disabled boundary / disabled 边界

When `TerminalConfig.enabled == false` at router construction, the boot path must not call `manager()`, attach the bus, create the flush driver, or create a PTY session. Configuration validation and handler registration may remain code-level declarations; they must not instantiate the process-global runtime owner.

When live apply changes the gate to disabled, the source lifecycle path must revoke new admission, close sessions, stop/join the flush driver, and return a truthful result. Re-enable must use a new provider generation and a new driver handle. A completed failed driver may be cleared only when the provider is in `Failed` and the failure evidence is retained; other incomplete/failed cleanup remains fail-closed.

## Required proof / 必须证明

- disabled boot is externally and internally observable as no manager initialization, no bus attachment, no flush task, and no PTY session;
- enabled start produces a real output frame through the existing event bus;
- stop closes sessions and joins the flush driver, or returns a typed residual without claiming stopped;
- re-enable produces a new provider generation and old generation admission fails;
- concurrent start/stop/disable does not create two flush drivers or lose cleanup evidence;
- existing owner filtering and `PtyManager` session map remain the only Terminal session authority;
- `ProviderHandle` remains side-effect-free;
- no claim is made about native unload, OS process/thread reclamation, RSS reduction, or process isolation.

## Gate / 闸

**GO for a bounded Terminal lifecycle implementation** only if the plan can pass the disabled boot, driver ownership, stop/join, generation fencing, and residual tests without adding a second authority or changing Core to depend on Gateway.

**NO-GO / deferred** if implementation requires moving `PtyManager::owners` into a parallel map, inventing a new `CapabilityKind`, changing `Zahir`/`BackendLease` to depend on Gateway, using a global timer/task registry, or asserting process/thread/RSS reclamation beyond observed cleanup evidence.

## Approval boundary / approval 边界

This document is a design proposal for the next session. It requires explicit approval before production Rust, tests, configuration wiring, or implementation skill invocation. The implementation plan below must remain subordinate to this design.

本文是下一会话的 design proposal。生产 Rust、测试、配置 wiring 或 implementation skill invocation 前必须得到明确批准。下面的 implementation plan 必须从属于本文。

## Explicit non-actions / 明确不做

- No Feishu mount or route.
- No Generic Gateway Zahir mount.
- No Terminal MCP/ACP projection.
- No Memory/Sandbox migration.
- No new kind, second registry, durable scheduler, EffectClaim producer, Safe Replay, external exactly-once, or harness change.
- No push, remote merge, deployment, or external action from this planning session.

- 不做 Feishu mount/route、Generic Gateway Zahir mount、Terminal MCP/ACP projection、Memory/Sandbox migration、新 kind、第二 registry、durable scheduler、EffectClaim producer、Safe Replay、external exactly-once 或 harness change。
- 本 planning session 不 push、不做 remote merge、deployment 或 external action。

## Validation not run / 未运行验证

No production implementation or Cargo check is run while writing this design package. The next implementation session must run the focused checks listed in the plan and must record any unrun workspace-wide checks explicitly.

本 design package 编写阶段不实现 production code，也不运行 Cargo check。下一实现会话必须运行 plan 中的 focused checks，并明确记录未运行的 workspace-wide checks。
