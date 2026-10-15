# Capability Phase 4 — 只读 delta census（2026-10-06）

**性质**：Task 1-5 的文件边界权威表（覆盖 spec `docs/superpowers/specs/2026-10-06-capability-phase4-design.md` §4.1 + 第五章 + 第九章）。本表决定「哪些文件加 marker / 注释，哪些文件新增 / 改真实 wiring」。后续 Task 的 Files 块以此表为准。

**基线**：worktree `capability-phase4` HEAD = `3b5cea64f`（plan commit），含 spec commit `a7308823d` + projection-boundary commit `a5300221d`。

---

## 0. 不变量（红线）

- 不重写既有 `SessionEventStore` / `ReplayPermit` / `ResumeCoordinator` —— Task 3 仅在 `events.rs` 上新增变体 + 在既有 store / replay / resume 上加 marker 注释，不替换既有事实契约（spec §4.1 + §7.5 + §11.4）。
- 不接 `StateDatabase`：memo / effect-claim / capability descriptor 一律入 Session event store；StateDatabase 标 `projection_only`，recovery 时只重投影（spec §7.1）。
- memo 不接 `GlobalBus`：`approval memo` / `hook memo` 经 `SessionService::emit_event / emit_batch`（`src/session/service.rs:65/78`）入 event store；GlobalBus 标 `notification_only`，**不可**作为 memo 落点（spec §7.1 + §7.6）。
- memo 真实接线 `events.rs` 变体（非 marker）：`SessionEvent::EffectClaimClaimed` / `EffectClaimTerminal` / `ApprovalMemo` / `HookMemo` 新增 + serde + reduction + replay 消费；producer 来自 `call_log.rs:45` / `approval/policy.rs`+`types.rs` / `extension/hooks/executor.rs:1101/1339`。
- 禁止创建 `src/capability/phase4_baseline.rs` 或任何 golden JSON。
- 禁止 `src/harness/` 业务文件新增（R10 棘轮，spec §11.3）。

---

## 1. 9 处既有事实源

| # | 路径（worktree 相对） | 既有契约（类型 / 关键方法 / 锚点行） | Phase 4 动作 | Gate |
|---|---|---|---|---|
| 1 | `src/tools/registry.rs` | `pub struct ToolHandlerRegistry` (`:223`)；backend #1 事实源；`ToolCapabilityDescriptor`（`src/tools/descriptor.rs:267`）含 `revision: u64`、`schema_version: u32`、`replay_policy: ReplayPolicy`；`new / register / descriptor(name) / descriptor_snapshot / revision` | **真实 wiring**（Task 1 适配层 + Task 4 适配注释）：作为 Zahir facade 的 Tool kind 唯一完整 backend；既有 handler 注册契约**不改** | A / D |
| 2 | `src/extension/registry/plugin_registry/mod.rs` | `pub struct PluginRegistry` (`:24`)；模块装载 + 钩子 + resource 类 | **不动**：本期不在 `plugin_registry` 内挂载 Plugin kind backend；仅保留 onboarding contract（spec §5.6 + Deferred） | — |
| 3 | `src/gateway/execution_engine/session_run_registry.rs` | `pub struct SessionRunRegistry` (`:20`)；Run ID 索引；互斥 gate 职责 | **不动**：不与 `OwnershipTree` 合并，二者职责不同（Run 互斥 vs 启动期固化槽） | — |
| 4a | `src/session/store.rs` | `pub trait SessionEventStore: Send + Sync + 'static` (`:56`)；`SqliteEventStore` (`:685`)；`pub enum ReplayClaimResult` (`:454`，变体 `Claimed` / `HeadChanged` / `GenerationChanged` / `NotDangling` / `LeaseHeld` / `BudgetExhausted`） | **仅 marker 注释**：标「唯一 recovery source」；不改契约、不重写 `SessionEventStore`；不接 `StateDatabase` | C |
| 4b | `src/session/service.rs` | `pub struct SessionService` (`:55`)；`emit_event` (`:65`) / `emit_batch` (`:78`) / `get_events` | **复用既有 append batch**：memo 与 effect-claim 事件经 `emit_event / emit_batch` 入 store | C |
| 4c | `src/session/replay.rs` | `pub struct ReplayPermit` (`:51`)；`ReplayRequest` (`:19`) / `ReplayPrepare` (`:33`) / `ReplayPreparer` (`:41`) | **仅 marker 注释**：标「VerifyOnly 默认」；不改契约、不重写 `ReplayPermit`；`ReplayClaimResult`（claim 是否被采纳）≠ `EffectClaimReconciliation`（对账终态） | C |
| 4d | `src/session/events.rs` | `pub enum SessionEvent` (`:391`)；`pub struct SessionEventRecord` (`:690`) | **真实 wiring**：新增 `EffectClaimClaimed` / `EffectClaimTerminal` / `ApprovalMemo` / `HookMemo` 变体 + serde + reduction + replay 消费；`SessionEventRecord` 继续包装记录并参与序列化 / 回放（spec §7.6） | C |
| 5 | `src/resilience/database/state_database/mod.rs` | `pub struct StateDatabase` (`:47`)；多张 SQLite 表 + vec index；操作型投影 | **仅 marker**：标 `projection_only`；recovery 时仅重投影；不接 effect-claim / memo | D |
| 6 | `src/acp/manager/persistence.rs` | ACP session JSON persistence（best-effort）；非事实源 | **仅 marker**：标 `projection_only`；ACP inbound 不允许绕过 Session / Run mutation line 自建 capability 注册表（spec §5.2） | D |
| 7 | `src/event/global_bus.rs` | `pub struct GlobalBus` (`:169`)；跨 region 推送；无 cursor / generation | **仅 marker**：标 `notification_only`；不接 memo / effect-claim；不可回答「现在是什么状态」 | D |
| 8 | `src/capability/mod.rs` | `pub struct CapabilitySlot<T: 'static>` (`:102`) / `MutableCapabilitySlot` / `SlotStatus` / `ALL_SLOTS`；启动期固化能力槽 | **保留不动**：仅追加 `pub mod descriptor; pub mod facade; pub mod backend; pub mod ownership; pub mod effect_claim;`；CapabilitySlot 与 ALL_SLOTS 一律不改 | A |
| 9 | `src/gateway/resume_coordinator.rs` | `pub struct ResumeCoordinator` (`:1103`) | **仅 marker 注释**：标「VerifyOnly 默认」；不改契约、不重写 `ResumeCoordinator` | C |

---

## 2. 3 处 MCP projection seam（非 `tool_bridge.rs` inbound bridge）

| # | 路径 | 既有契约 / 锚点行 | Phase 4 动作 | Gate |
|---|---|---|---|---|
| 1 | `src/mcp/types.rs` | `pub struct McpTool` (`:10`)；`McpToolFilter` (`:48`)；`McpToolResult` (`:113`) | **仅 marker**：标 `projection_only`；只读投影；不允许写回事件流 | D |
| 2 | `src/tools/mcp_scope_view.rs` | `pub struct McpScopedToolService` (`:14`) | **仅 marker**：标 `projection_only`；scope view 派生视图 | D |
| 3 | `src/tools/server/ops.rs` | `pub(super) async fn list_tools_arc_impl(tools: &ToolMap) -> Vec<Arc<dyn AlephToolDyn>>` (`:29`) | **仅 marker**：标 `projection_only`；只读列工具；不允许持有 mutation 状态 | D |

**显式排除**：`src/mcp/tool_bridge.rs` 的 `spawn_tool_bridge` (`:102`) 是 MCP **inbound bridge**（transport），**不是 projection**；Task 4 不改不改标（spec §5.2 inbound = transport 角色）。

---

## 3. 2 处 memo seam（真实接线点，非 marker）

| # | 路径 | 既有契约 / 锚点行 | Phase 4 动作 | Gate |
|---|---|---|---|---|
| 1 | `src/session/call_log.rs` | `pub async fn emit_for_ambient_call` (`:45`)；调用点 `src/tools/scoped/dispatch.rs:1290` | **真实 wiring**：approval memo 既有 seam 接线点；producer 经 `SessionService::emit_event / emit_batch` 入 event store，**不接 GlobalBus** | C |
| 2 | `src/extension/hooks/executor.rs` | `execute_interceptors` (`:1101`) / `execute_observers` (`:1339`)；当前无 store 句柄 | **真实 wiring**：注入 store 句柄写 hook memo（`phase` + `capability` + `skipped / passed` + `at`）；replay memo 不重跑决策；**不接 GlobalBus** | C |

---

## 4. 5 处待新增 capability 文件（全部为空目录 `/src/capability/` 下新增）

| # | 路径 | 责任 / 关键类型 | 关联既有锚点 | Gate |
|---|---|---|---|---|
| 1 | `src/capability/descriptor.rs` | `CapabilityKind`（9 种：`Tool / Skill / Agent / Task / Resource / EventSource / Subscription / Plugin / Hook`，spec §5.1 唯一新定义）；`CapabilityDescriptor` / `CapabilityId` / `CapabilityRevision` / `SchemaRef`（`sha2::Sha256` 指纹） / `Service`（relation，仅 `requires / provides / conflicts`，不持数据、不 invoke） / `ProjectionMetadata` | `ToolHandlerRegistry` (`:223`) + `ToolCapabilityDescriptor` (`src/tools/descriptor.rs:267`) 字段对齐 `schema_version` / `revision` | A |
| 2 | `src/capability/facade.rs` | `trait Zahir { fn describe / resolve / subscribe / project }`（spec §5.4 四方法）；`Scope` / `Reference` / `CapabilitySnapshot`（immutable，持有期间不可变） / `BackendLease`（owner generation bump / revoke 自动失效） / `Cursor` / `CapabilityChangeStream` / `TransportTarget` / `Projection` | `arc_swap::ArcSwap`（ToolHandlerRegistry 快照） + `tokio::sync::broadcast`（变更流） | A / D |
| 3 | `src/capability/backend.rs` | `trait CapabilityBackend: Send + Sync { fn lookup / enumerate / generation }`；`pub struct ToolBackendAdapter { registry: ToolHandlerRegistry }`（唯一完整 backend，spec §5.6）；其余 8 种 kind **只定义 trait 且 deferred kind 不可执行（不可 invoke）**，不实现、不挂载、不在 Gate 报告里宣称「已挂载 backend」 | `ToolHandlerRegistry::descriptor / descriptor_snapshot / revision` | A |
| 4 | `src/capability/ownership.rs` | Task 1 同 commit 落地纯基础类型 `OwnerGeneration(pub u64)` / `LifetimeScope { Runtime, Session, Run, Task, External }` / `VisibilityScope`（ACL，字段 `principal/agent + workspace/channel + session + allowed kinds/ids + permission/approval context`，各 `Option`，`None`=不限） / `OwnerRef { Runtime, Session(SessionKey), Run(RunId), Task(TaskId) }` / `FencingToken`（owner 层签发的不透明值） / `EffectClaim { request_id, fence, owner }`；Task 2 同模块追加 `OwnershipTree` 行为 `bump / revoke / dispose`（不可逆） | Task 1 descriptor 编译依赖；Task 2 五层 `Runtime → Session → Run → Task → EffectClaim` | A / B |
| 5 | `src/capability/effect_claim.rs` | `pub enum EffectClaimState { Prepared, Claimed, Invoking, Succeeded, Failed, Unknown }`；`pub struct EffectClaimReconciliation { request_id, terminal }`（语义「按闭包对账后收敛到哪个终态」，**≠** 既有 `ReplayClaimResult`「claim 是否被采纳」）；`pub fn reconcile_effect_claim(events: &[...]) -> EffectClaimReconciliation`；reducer 强制闭包 `Prepared→Claimed`、`Prepared→Unknown`、`Claimed→Invoking`、`Claimed→Unknown`、`Invoking→Succeeded`、`Invoking→Failed`、`Invoking→Unknown`，其余非法迁移一律 `Unknown`（spec §7.5） | `ReplayClaimResult` (`:454`) / `ReplayPermit` (`:51`) / `SessionEvent` (`:391`) 新变体 | C |

---

## 5. Task 0-5 文件落点反查（commit 路径前瞻）

| Task | 文件落点（git add 前瞻） | 动作类别 |
|---|---|---|
| 0 | `docs/superpowers/notes/2026-10-06-capability-phase4-census.md`（本文件） | 仅文档 |
| 1 | `src/capability/mod.rs`（追加 mod 声明）+ `descriptor.rs` + `facade.rs` + `backend.rs` + `ownership.rs`（纯类型） | 真实 wiring |
| 2 | `src/capability/ownership.rs`（追加 `OwnershipTree` 行为） | 真实 wiring |
| 3 | `src/capability/effect_claim.rs`（新增）+ `src/session/events.rs`（真实 wiring，新变体）+ `src/session/call_log.rs`（真实 wiring）+ `src/approval/policy.rs` + `src/approval/types.rs`（真实 wiring）+ `src/extension/hooks/executor.rs`（真实 wiring，注入 store 句柄）+ `src/session/store.rs`（仅 marker）+ `src/session/replay.rs`（仅 marker）+ `src/gateway/resume_coordinator.rs`（仅 marker） | 真实 wiring + marker |
| 4 | `src/capability/facade.rs`（补齐 subscribe 状态机）+ `src/acp/manager/persistence.rs`（marker）+ `src/resilience/database/state_database/mod.rs`（marker）+ `src/mcp/types.rs`（marker）+ `src/tools/mcp_scope_view.rs`（marker）+ `src/tools/server/ops.rs`（marker）+ `src/tools/service.rs`（`to_metadata_form` 收窄 wrapper）+ `src/event/global_bus.rs`（marker） | 真实 wiring + marker |
| 5 | `docs/reference/FEATURE_LOCATOR.md` / `ARCHITECTURE.md` / `CODE_ORGANIZATION.md` / `DESIGN_PATTERNS.md` / `DOMAIN_MODELING.md` / `SANDBOX.md` / `REDLINES.md` + `AGENTS.md` + `CLAUDE.md` | 仅文档 |

---

## 6. 锚点核对（grep 命中，无 MISS）

`grep -n "pub struct ToolHandlerRegistry" src/tools/registry.rs` → `:223`
`grep -n "pub enum ReplayClaimResult" src/session/store.rs` → `:454`
`grep -n "pub struct ReplayPermit" src/session/replay.rs` → `:51`
`grep -n "pub trait SessionEventStore" src/session/store.rs` → `:56`
`grep -n "pub enum SessionEvent" src/session/events.rs` → `:391`
`grep -n "pub struct SessionEventRecord" src/session/events.rs` → `:690`
`grep -n "pub struct McpTool" src/mcp/types.rs` → `:10`
`grep -n "pub struct McpScopedToolService" src/tools/mcp_scope_view.rs` → `:14`
`grep -n "fn list_tools_arc_impl" src/tools/server/ops.rs` → `:29`
`grep -n "emit_for_ambient_call" src/session/call_log.rs` → `:45`

10/10 命中，零 MISS，零 SPEC 9 错位（brief §0 预期行号全部对账）。