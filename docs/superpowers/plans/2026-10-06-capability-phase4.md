# Capability Phase 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Aleph 的能力面从「单个 Tool 的注册 + 调用」扩展到 9 类 Capability 的统一描述与生命周期管理：新增 CapabilityDescriptor + Zahir facade + ownership tree + effect-claim 闭包 + projection-only 收紧，全部不回归 Phase 3 既有路径。

**Architecture:** 方案 B（spec §4）：静态 trait + 类型化 backend；`src/capability/` 新增 descriptor/facade/backend/ownership/effect_claim 五个纯增量文件，ToolHandlerRegistry 是本期唯一完整 backend（Tool kind）。三层事实源显式分层：Session event store（唯一恢复源）→ StateDatabase / ACP JSON / MCP JSON（projection-only）→ GlobalBus（notification-only）。ownership 为 Runtime → Session → Run → Task → EffectClaim 五层；VisibilityScope（ACL）与 LifetimeScope（RAII）分离。默认 VerifyOnly，不自动 replay。

**Tech Stack:** Rust Core（tokio + serde + schemars + sqlite-vec）· 既有 `arc_swap::ArcSwap` / `tokio::sync::broadcast`（ToolHandlerRegistry 快照/变更流）· `async_trait`（SessionEventStore/SessionService）· sha2（schema_fingerprint）。**不新增依赖**。

**Spec:** `docs/superpowers/specs/2026-10-06-capability-phase4-design.md`（基线 `a5300221d`；工作树已含其落地 commit `a7308823d`）

## Global Constraints

- 工作树：`/Volumes/TBU4/Workspace/Aleph-capability-phase4`，分支 `capability-phase4`。会话工作检出与编辑落点一律是此 worktree（AGENTS.md WORKSPACE 警告）。
- 事实源分层不变量（spec §4.1）：event store 是唯一 recovery source；StateDatabase / ACP JSON / MCP JSON 是 projection；GlobalBus / Gateway bus 是 notification。**不得**让 projection 或 bus 冒充 recovery source。
- 不得合并存储；不得建 universal scheduler；不引入 `Box<dyn Any>`、巨大 enum payload、或 `dyn Trait` 作为 descriptor 字段（spec §5.5）。
- `src/harness/` 不新增任何业务文件（R10 棘轮，spec §11.3）；Gate 由 review 检查 diff，不以文件计数守门。
- Phase 3 不回归：`src/tools/registry.rs` 既有 handler 注册契约、MCP builtin wire、markdown-skill builtin 注册路径都不得改动（spec §11.4）。
- 每个 task 结束必须 commit；每一步可独立 revert。
- 每个 task 的 TDD 顺序固定：**失败测试 → 确认失败 → 最小实现 → 定向通过 → diff/file boundary 审查 → commit**。deferred kind 必须不可执行（不可 invoke）；memo 缺失一律收敛 `Unknown`；必须有「no automatic replay」测试。
- 默认 VerifyOnly：replay 必须显式 `ReplayPermit`；本期不开启 automatic Safe Replay。
- 验证：`cargo check` / `cargo test` / `cargo clippy`；不运行 `cargo` 之外的 gate。
- 中文文档为主，代码/命令/符号英文。

## Review Focus

1. **stale revision / owner**：一个 BackendLease / snapshot 在其 owner `generation` bump 或 `revoke` 之后仍被当作有效消费。期望：`resolve`/`describe` 返回的租约携带 `owner_generation`，与当前 owner generation 不一致时立即失效（fail-closed），不返回陈旧 handler。→ 测试落在 Task 2。
2. **cursor + generation resync**：订阅者在收到 `Invalidated` 后从 0 重订，放大失效窗口并丢失去重保障。期望：保留原 `committed_cursor`，用 snapshot + delta 从该 cursor 重同步，按 `OwnerGeneration` / `CapabilityRevision` 去重，丢弃跨 owner/跨 generation 的错位事件。→ 测试落在 Task 4。
3. **duplicate claim / request**：同一个 `request_id` 出现第二个 active `EffectClaim`（跨 owner 重复 claim），或同一 intent 被重复 commit。期望：`EffectClaimReconciliation` 拒绝第二个 active claim 并记 `Unknown`；prepared→claimed→invoking 链上不重复 commit。→ 测试落在 Task 3。
4. **projection failure**：ACP/MCP JSON 与 event store 不一致时被当作事实消费。期望：fail-closed——以 event store 为准，projection 写失败不影响 event store，下次 recovery 重投影。→ 测试落在 Task 4。
5. **approval / hook durable memo**：hook 实现重启后 memo 丢失导致决策不可 audit。期望：approval memo（决策+request_id+fence+时间+approver）与 hook memo（phase+capability+跳过/通过+时间）都入 event store，重放 memo 而不重跑 hook 决策。→ 测试落在 Task 3。
6. **compatibility wrappers**：`to_metadata_form` 被用来做 facade 的 canonical 序列化或发生 side-effect。期望：`to_metadata_form` 仅作为旧 Tool wrapper（字段复制 / schema 序列化 / version·generation 序列化；真实签名 `pub fn to_metadata_form(defs: &[ToolDefinition]) -> Arc<[crate::tool_metadata::ToolDefinition]>`，`src/tools/service.rs:450`，不改名不改参、不扩展为 canonical facade API），无 side-effect、无 RPC、无 event store mutation、无 capability 字段写入；facade 只用 ProjectionMetadata。→ 测试落在 Task 4。

---

## File Structure

| 文件 | 责任 | 状态 |
| --- | --- | --- |
| `src/capability/mod.rs` | 既有 `CapabilitySlot`/`MutableCapabilitySlot`/`SlotStatus`/`ALL_SLOTS` 启动期固化能力槽 | **保留不动**，仅追加 `mod` 声明 |
| `src/capability/descriptor.rs` | `CapabilityKind`(9)、`CapabilityDescriptor`、`CapabilityId`、`CapabilityRevision`、`SchemaRef`、`ProjectionMetadata`、`Service` | 新增 |
| `src/capability/facade.rs` | `Zahir` trait（describe/resolve/subscribe/project）、`Scope`/`Reference`/`CapabilitySnapshot`/`BackendLease`/`CapabilityChangeStream`/`Projection` | 新增 |
| `src/capability/backend.rs` | `CapabilityBackend` trait（lookup/enumerate/generation）+ `ToolBackendAdapter`（唯一完整 backend） | 新增 |
| `src/capability/ownership.rs` | 纯基础类型 `OwnerRef`/`OwnerGeneration`/`VisibilityScope`/`LifetimeScope`/`FencingToken`/`EffectClaim`（Task 1 同 commit 落地）；行为 `OwnershipTree`（`bump`/`revoke`/`dispose`，Task 2） | 新增（Task 1 纯类型 + Task 2 行为） |
| `src/capability/effect_claim.rs` | `EffectClaimClosure` reducer + `EffectClaimReconciliation`（终态收敛） | 新增 |
| `src/tools/registry.rs` | `ToolHandlerRegistry`（backend #1 事实源，只加适配注释，不改契约） | 修改（仅注释/适配） |
| `src/session/events.rs` | `SessionEvent`（`:391`）新增 effect/approval/hook memo 变体 + serde + reduction；`SessionEventRecord`（`:690`）继续包装记录并参与序列化/回放（真实实现，非 marker） | 修改（真实实现） |
| `src/session/store.rs` / `replay.rs` / `service.rs` | 既有 recovery source，加 marker 注释，不改行为（**不重写 `SessionEventStore`/`ReplayPermit`/`ResumeCoordinator`**） | 修改（仅 marker） |
| `src/session/call_log.rs` | `emit_for_ambient_call`（`:45`，调用点 `src/tools/scoped/dispatch.rs:1290`）——approval memo 既有 seam 接线点 | 修改（真实 wiring） |
| `src/approval/policy.rs` / `types.rs` | `ApprovalPolicy`/`ApprovalDecision` 当前不写 SessionEvent——补 durable memo producer | 修改（真实 wiring） |
| `src/extension/hooks/executor.rs` | `execute_interceptors`（`:1101`）/`execute_observers`（`:1339`）当前无 store 句柄——注入句柄写 hook memo，replay 不重跑决策 | 修改（真实 wiring） |
| `src/gateway/resume_coordinator.rs` | 既有 resume 入口，加 marker 注释 | 修改（仅 marker） |
| `src/resilience/database/state_database/mod.rs` | `StateDatabase` 标 `projection_only` marker | 修改（仅 marker） |
| `src/acp/manager/persistence.rs` | ACP JSON 标 `projection_only` marker | 修改（仅 marker） |
| `src/mcp/tool_bridge.rs` | MCP **inbound bridge**（`spawn_tool_bridge` `:102`，既有）——**不是 projection，不改标** | 不动 |
| `src/mcp/types.rs` | `McpTool`（`:10`）——真实 MCP projection seam | 修改（`projection_only` marker） |
| `src/tools/mcp_scope_view.rs` | `McpScopedToolService`（`:14`）——真实 MCP projection seam | 修改（`projection_only` marker） |
| `src/tools/server/ops.rs` | `list_tools_arc_impl`（`:29`）——真实 MCP projection seam | 修改（`projection_only` marker） |
| `src/event/global_bus.rs` | `GlobalBus` 标 `notification_only` marker | 修改（仅 marker） |

> **既有事实源锚点**（census 已核对，不重写）：`ToolHandlerRegistry`（`src/tools/registry.rs:223`）、`ToolCapabilityDescriptor`（`src/tools/descriptor.rs:267`）、`SessionEventStore` trait（`src/session/store.rs:56`）、`SqliteEventStore`（`src/session/store.rs:685`）、`ReplayClaimResult`（`src/session/store.rs:454`，变体 `Claimed`/`HeadChanged`/`GenerationChanged`/`NotDangling`/`LeaseHeld`/`BudgetExhausted`）、`SessionEvent`（`src/session/events.rs:391`）、`SessionEventRecord`（`src/session/events.rs:690`）、`ReplayRequest`/`ReplayPrepare`/`ReplayPermit`（`src/session/replay.rs:19/33/51`）、`SessionService` trait（`src/session/service.rs:55`，`get_events`/`emit_event`/`emit_batch`）、`SessionRunRegistry`（`src/gateway/execution_engine/session_run_registry.rs:20`）、`ResumeCoordinator`（`src/gateway/resume_coordinator.rs:1103`）、`GlobalBus`（`src/event/global_bus.rs:169`）、`StateDatabase`（`src/resilience/database/state_database/mod.rs:47`）、`PluginRegistry`（`src/extension/registry/plugin_registry/mod.rs`）、ACP persistence（`src/acp/manager/persistence.rs`）、MCP inbound bridge（`src/mcp/tool_bridge.rs`，`spawn_tool_bridge` `:102`）、MCP projection seam（`src/mcp/types.rs:10` `McpTool`、`src/tools/mcp_scope_view.rs:14` `McpScopedToolService`、`src/tools/server/ops.rs:29` `list_tools_arc_impl`）、approval memo 既有 seam（`src/session/call_log.rs:45` `emit_for_ambient_call`，调用点 `src/tools/scoped/dispatch.rs:1290`）、hook memo 既有 seam（`src/extension/hooks/executor.rs:1101` `execute_interceptors` / `:1339` `execute_observers`）。

---

## Task 0: 只读 delta census / baseline contract

**Files:**
- Create: `docs/superpowers/notes/2026-10-06-capability-phase4-census.md`（只读枚举表，非 golden JSON，非 `.rs` baseline）
- 不改任何 `src/` 文件，不运行 cargo。

**Interfaces:**
- Consumes: 无（只读 spec + 上表锚点）
- Produces: 一张「已存在 vs 待新增」表——每行 = 一个事实源文件 + 其既有契约（类型/方法名）+ phase4 将「新增」还是「marker 标注」+ 所属 Gate。Task 1-5 的 Files 块以此表为准。

- [ ] **Step 1: 写 census 表**

列出 9 处既有事实源（registry / plugin_registry / session_run_registry / session store+service+replay+**events** / state_database / acp persistence / global_bus / capability::CapabilitySlot / resume_coordinator）+ 3 处 MCP projection seam（`src/mcp/types.rs` `McpTool` / `src/tools/mcp_scope_view.rs` `McpScopedToolService` / `src/tools/server/ops.rs` `list_tools_arc_impl`，**非** `src/mcp/tool_bridge.rs` inbound bridge）+ 2 处 memo seam（`src/session/call_log.rs:45` / `src/extension/hooks/executor.rs:1101/1339`）与 5 处待新增文件，注明「不重写 store/replay/resume（含 `SessionEventStore`/`ReplayPermit`/`ResumeCoordinator`）；不接 StateDatabase；memo 不接 GlobalBus；memo 真实接线 `events.rs` 变体」。禁止创建 `src/capability/phase4_baseline.rs` 或任何 golden JSON。

- [ ] **Step 2: 核对锚点仍存在**

Run: `grep -n "pub struct ToolHandlerRegistry" src/tools/registry.rs && grep -n "pub enum ReplayClaimResult" src/session/store.rs && grep -n "pub struct ReplayPermit" src/session/replay.rs && grep -n "pub trait SessionEventStore" src/session/store.rs && grep -n "pub enum SessionEvent" src/session/events.rs && grep -n "pub struct SessionEventRecord" src/session/events.rs && grep -n "pub struct McpTool" src/mcp/types.rs && grep -n "pub struct McpScopedToolService" src/tools/mcp_scope_view.rs && grep -n "fn list_tools_arc_impl" src/tools/server/ops.rs && grep -n "emit_for_ambient_call" src/session/call_log.rs`
Expected: 各自命中（223 / 454 / 51 / 56 / 391 / 690 / 10 / 14 / 29 / 45 附近），无 MISS。

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/notes/2026-10-06-capability-phase4-census.md
git commit -m "docs: census capability phase 4 delta"
```

---

## Task 1: Gate A — CapabilityDescriptor + Zahir facade + backend trait

**Files:**
- Create: `src/capability/descriptor.rs`, `src/capability/facade.rs`, `src/capability/backend.rs`, `src/capability/ownership.rs`（**仅纯 ownership 基础类型** `OwnerRef`/`OwnerGeneration`/`VisibilityScope`/`LifetimeScope`/`FencingToken`/`EffectClaim`——descriptor 编译所需，与 Task 1 同 Gate/同 commit；`OwnershipTree` 行为留 Task 2）
- Modify: `src/capability/mod.rs`（追加 `pub mod descriptor; pub mod facade; pub mod backend; pub mod ownership;`；**CapabilitySlot 及 ALL_SLOTS 一律不动**）
- Test: `src/capability/descriptor.rs` 与 `src/capability/backend.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `ToolHandlerRegistry`（`new`/`descriptor`/`descriptor_snapshot`/`revision`）、`ToolCapabilityDescriptor`（字段：`name: String, kind: ToolKind, schema_version: u32, description: String, input_schema: serde_json::Value, source: ToolSource, replay_policy: ReplayPolicy, requires_confirmation: bool, idempotent: bool, concurrent_safe: bool, max_duration_ms: Option<u64>, revision: u64, implementation_contract: Option<ImplementationContract>`）。
- Produces（后续 task 依赖的精确签名）:

```rust
// ownership.rs（纯基础类型，与 descriptor 同 Gate/同 commit）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub struct OwnerGeneration(pub u64);
pub enum LifetimeScope { Runtime, Session, Run, Task, External }
pub struct VisibilityScope { /* principal/agent + workspace/channel + session + allowed kinds/ids + permission/approval context；各字段 Option，None=不限 */ }
pub enum OwnerRef { Runtime, Session(SessionKey), Run(RunId), Task(TaskId) } // SessionKey/RunId 以实现时对既有类型对齐
pub struct FencingToken(/* 不透明，owner 层签发；每次 claim 新增 */);
pub struct EffectClaim { pub request_id: String, pub fence: FencingToken, pub owner: OwnerRef }

// descriptor.rs
pub enum CapabilityKind { Tool, Skill, Agent, Task, Resource, EventSource, Subscription, Plugin, Hook }
pub struct CapabilityId { pub namespace: String, pub name: String } // e.g. aleph/tools/<name>
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub struct CapabilityRevision(pub u64);
pub struct SchemaRef { pub version: u32, pub fingerprint: [u8; 32] } // sha2::Sha256
pub struct CapabilityDescriptor {
    pub id: CapabilityId, pub kind: CapabilityKind, pub schema: SchemaRef,
    pub revision: CapabilityRevision, pub owner_generation: crate::capability::ownership::OwnerGeneration,
    pub lifetime: crate::capability::ownership::LifetimeScope,
    pub visibility: crate::capability::ownership::VisibilityScope,
    pub owner: crate::capability::ownership::OwnerRef,
    pub services: Vec<Service>, pub metadata: ProjectionMetadata,
}

// facade.rs
pub trait Zahir {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> BackendLease;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}

// backend.rs
pub trait CapabilityBackend: Send + Sync {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor>;
    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor>;
    fn generation(&self) -> OwnerGeneration;
}
pub struct ToolBackendAdapter { registry: crate::tools::registry::ToolHandlerRegistry }
```

- `Service` = relation（`requires/provides/conflicts`，不持数据、不可 invoke）。`CapabilityKind` 只有本文件这一处新定义——`plugin_trust.rs` **当前没有** `CapabilityKind` enum（仅历史文档曾提及该名，spec §5.1 的引用是历史文档），本期不产生第二定义、也不声称「删除过 enum」。

- [ ] **Step 1: 写失败测试（Tool round-trip）**

```rust
// backend.rs tests
#[test]
fn tool_name_round_trips_to_descriptor() {
    let reg = ToolHandlerRegistry::new();
    let desc = ToolCapabilityDescriptor::from_definition(&fake_def("hello"), 0);
    reg.register(desc, Arc::new(fake_handler("hello"))).unwrap();
    let backend = ToolBackendAdapter { registry: reg };
    let d = backend.lookup(&CapabilityId { namespace: "aleph/tools".into(), name: "hello".into() }).unwrap();
    assert_eq!(d.kind, CapabilityKind::Tool);
    assert_eq!(d.revision.0, 1);
}
```

- [ ] **Step 2: 确认失败**

Run: `cargo test -p aleph-server capability::backend --no-fail-fast 2>&1 | tail -20`
Expected: FAIL — `ToolBackendAdapter` / `CapabilityKind` 未定义（E0433/E0412）。

- [ ] **Step 3: 最小实现四文件（含 ownership 纯类型）**

`ownership.rs` 定义纯基础类型 `OwnerRef`/`OwnerGeneration`/`VisibilityScope`/`LifetimeScope`/`FencingToken`/`EffectClaim`（无行为，`OwnershipTree` 留 Task 2）；`descriptor.rs` 定义 `CapabilityKind`/`CapabilityDescriptor`/`CapabilityId`/`CapabilityRevision`/`SchemaRef`/`Service`/`ProjectionMetadata`（引用 `ownership::*`）；`facade.rs` 定义 `Zahir` trait 与 `Scope`/`Reference`/`CapabilitySnapshot`/`BackendLease`/`Cursor`/`CapabilityChangeStream`/`TransportTarget`/`Projection`（snapshot 内容在持有期间不可变；lease 在 owner revoke/generation bump 时失效）；`backend.rs` 定义 `CapabilityBackend` trait 与 `ToolBackendAdapter`，`lookup` 转 `registry.descriptor(name)`，`revision` 透传 `ToolCapabilityDescriptor.revision`，`generation` 返回当前 `OwnerGeneration`。**8 个其它 kind 只定义 trait 且 deferred kind 不可执行（不可 invoke），不实现、不挂载、不在报告里宣称「已挂载」**。

- [ ] **Step 4: 定向通过**

Run: `cargo test -p aleph-server capability::backend --no-fail-fast 2>&1 | tail -20`
Expected: PASS；同时 `cargo check` 全绿，Phase 3 既有 test suite 无 regression。

- [ ] **Step 5: diff / file boundary 审查**

`git diff --stat`：确认改动限于 `src/capability/mod.rs` + `descriptor.rs` + `facade.rs` + `backend.rs` + `ownership.rs`（纯类型）；无 harness 业务文件、无越界改动。

- [ ] **Step 6: Commit**

```bash
git add src/capability/mod.rs src/capability/descriptor.rs src/capability/facade.rs src/capability/backend.rs src/capability/ownership.rs
git commit -m "feat(capability): add CapabilityDescriptor, Zahir facade, backend trait, ownership base types (Gate A)"
```

---

## Task 2: Gate B — Ownership tree + scope separation

**Files:**
- Modify: `src/capability/ownership.rs`（纯类型已在 Task 1 落地，此处追加 `OwnershipTree` 行为：`bump`/`revoke`/`dispose`）
- Test: `src/capability/ownership.rs` 内 `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `OwnerGeneration`/`VisibilityScope`/`LifetimeScope`/`OwnerRef`/`EffectClaim`/`FencingToken`（Task 1 纯类型）、`CapabilityDescriptor` / `CapabilityId`（Task 1）。
- Produces:

```rust
// 纯类型（OwnerRef/OwnerGeneration/VisibilityScope/LifetimeScope/FencingToken/EffectClaim）已在 Task 1 的 ownership.rs 落地，此处仅追加行为。
pub struct OwnershipTree { /* 五层 Runtime→Session→Run→Task→EffectClaim */ }
impl OwnershipTree {
    pub fn bump(&self, level: LifetimeScope) -> OwnerGeneration; // 子层失效，订阅者收 Invalidated
    pub fn revoke(&self, id: &CapabilityId) -> bool;             // 阻止后续 resolve/claim，active claim 置 Unknown
    pub fn dispose(&self, scope: LifetimeScope) -> bool;         // 不可逆释放该层及所有下层
}
```

- **不得**合并 `SessionRunRegistry` 与 `CapabilitySlot`：二者职责不同（Run 互斥 gate vs 启动期固化槽），保持独立。

- [ ] **Step 1: 写失败测试（generation bump 使子层失效 + 同 id 不同 scope 共存 + revoke/dispose 不可逆）**

```rust
#[test]
fn bump_invalidates_child_claims() { /* bump(Session) 后 Run/Task 层 active EffectClaim 变 Unknown */ }
#[test]
fn same_id_two_scopes_coexist() { /* 同一 CapabilityId 在 Run 与 Session 两个 VisibilityScope 下都可 describe */ }
#[test]
fn revoke_and_dispose_are_irreversible() { /* revoke 后 resolve 返回 None 且二次 revoke 仍不可恢复；dispose 后新 claim 被拒 */ }
```

- [ ] **Step 2: 确认失败**

Run: `cargo test -p aleph-server capability::ownership --no-fail-fast 2>&1 | tail -20`
Expected: FAIL — `OwnershipTree` 未定义。

- [ ] **Step 3: 最小实现 `OwnershipTree` 行为（纯类型已在 Task 1）**

五层 `OwnershipTree`：每层 owner 签发 `OwnerGeneration`，`bump` 单调且使子层失效（subscriber 收 `Invalidated`，snapshot 仍按 committed cursor 可读）；`revoke`/`dispose` 不可逆（dispose 与 revoke 对 active `EffectClaim` 语义一致，区别仅在是否彻底释放该层资源）；`VisibilityScope`（ACL）与 `LifetimeScope`（RAII）正交分离；`External` 与本地层在 lifetime 维度对齐 RAII 而不假装进程内回收。

- [ ] **Step 4: 定向通过**

Run: `cargo test -p aleph-server capability::ownership --no-fail-fast 2>&1 | tail -20`
Expected: PASS；`cargo check` 全绿。

- [ ] **Step 5: diff / file boundary 审查**

`git diff --stat`：确认改动限于 `src/capability/ownership.rs`（仅追加 `OwnershipTree` 行为，纯类型已在 Task 1）；无 harness 业务文件、无越界改动。

- [ ] **Step 6: Commit**

```bash
git add src/capability/ownership.rs
git commit -m "feat(capability): add ownership tree with scope separation (Gate B)"
```

---

## Task 3: Gate C — EffectClaim closure + reconciliation + durable memo（真实 wiring）

**Files:**
- Create: `src/capability/effect_claim.rs`（纯 reducer + 对账收敛，无 store I/O）
- Modify（真实实现，非 marker）: `src/session/events.rs`（`SessionEvent` `:391` 新增 effect/approval/hook memo 变体 + serde + reduction；`SessionEventRecord` `:690` 继续包装记录并参与序列化/回放 + producer wiring）、`src/session/call_log.rs`（`emit_for_ambient_call` `:45` 补 approval memo producer，经 `SessionService::emit_event/emit_batch` 入 event store，不接 GlobalBus）、`src/approval/policy.rs` + `src/approval/types.rs`（`ApprovalPolicy`/`ApprovalDecision` 当前不写 SessionEvent——补 decision 入 store）、`src/extension/hooks/executor.rs`（`execute_interceptors` `:1101` / `execute_observers` `:1339` 当前无 store 句柄——注入 store 句柄写 hook memo，重放时 replay memo 不重跑决策）
- Modify（仅 doc 注释，不改行为）: `src/session/store.rs`、`src/session/replay.rs`、`src/gateway/resume_coordinator.rs`（加「recovery source / VerifyOnly 默认」doc 注释）——**明确不重写 `SessionEventStore` / `ReplayPermit` / `ResumeCoordinator`**，只在其上追加事件变体与 replay 消费
- Test: `src/capability/effect_claim.rs` 内 `#[cfg(test)] mod tests`（reducer / property / crash）+ `src/session/events.rs` / `src/approval/` / `src/extension/hooks/executor.rs` 内 memo 正/负测试

**Interfaces:**
- Consumes: `ReplayClaimResult`（`src/session/store.rs:454` 既有，变体 `Claimed`/`HeadChanged`/`GenerationChanged`/`NotDangling`/`LeaseHeld`/`BudgetExhausted`，语义「这次 claim 是否被采纳」，**不是**对账结果）、`ReplayPermit`（`src/session/replay.rs:51`）/`ReplayPreparer`（`:41`）、`SessionService::emit_event/emit_batch`（`src/session/service.rs:65/78`）、`SessionEvent`/`SessionEventRecord`（`src/session/events.rs:391/690`）。
- Produces:

```rust
pub enum EffectClaimState { Prepared, Claimed, Invoking, Succeeded, Failed, Unknown }
pub struct EffectClaimReconciliation { pub request_id: String, pub terminal: EffectClaimState } // 语义：按闭包对账后收敛到哪个终态
pub fn reconcile_effect_claim(events: &[/* claim 事件流 */]) -> EffectClaimReconciliation;
// events.rs 新增变体（示意，以源码核对为准）：
//   SessionEvent::EffectClaimClaimed { request_id, owner_generation, .. }
//   SessionEvent::EffectClaimTerminal { request_id, state, .. }
//   SessionEvent::ApprovalMemo { request_id, fence, decided_at, approver, decision, .. }
//   SessionEvent::HookMemo { phase, capability, skipped/passed, at, .. }
```

- **区分**：`EffectClaimReconciliation`（本期新增）≠ 既有 `ReplayClaimResult`（claim 是否被采纳）。`EffectClaimClosure` ≠ `marker_balance.rs` 的 `RunStarted/RunFinished` 配对。闭包规则见 spec §7.5（唯一活跃 claim / 合法前驱 / 终态唯一 / 失败闭合），任何不变量失败 fail-closed 为 `Unknown`。**durable memo 是真实 implementation gate**：新 SessionEvent 变体 + serde + reduction + replay + producer wiring，复用既有 `emit_batch` 追加事件；`ReplayPermit` 与 `EffectClaimReconciliation` 仍分离；**automatic replay 保持关闭**（默认 VerifyOnly）。

- [ ] **Step 1: 写失败测试（闭包 reducer + 非法迁移反例 + memo 持久性）**

```rust
#[test]
fn legal_chain_prepared_claimed_succeeded() { /* Prepared→Claimed→Invoking→Succeeded 收敛为 Succeeded */ }
#[test]
fn invoking_without_claimed_is_unknown() { /* 无 Claimed 直接 Invoking → Unknown */ }
#[test]
fn duplicate_active_claim_is_rejected() { /* 同一 request_id 两个 active claim → Unknown */ }
#[test]
fn succeeded_then_claimed_is_illegal() { /* 终态后再迁移 → Unknown（不可再次写入） */ }
#[test]
fn approval_memo_survives_crash() { /* emit_event 成功 → 崩溃（write-before/after）→ 重放读回 memo，不重跑决策 */ }
#[test]
fn memo_missing_converges_unknown() { /* 无 memo → 对账收敛 Unknown（fail-closed） */ }
#[test]
fn duplicate_memo_producer_rejected() { /* 同 request_id 二次 producer → 拒绝 / Unknown */ }
#[test]
fn no_automatic_replay() { /* 默认 VerifyOnly：无 ReplayPermit 时绝不 replay */ }
```

- [ ] **Step 2: 确认失败**

Run: `cargo test -p aleph-server capability::effect_claim --no-fail-fast 2>&1 | tail -20`
Expected: FAIL — `reconcile_effect_claim` 未定义。

- [ ] **Step 3: 最小实现 reducer + 事件变体 + producer/consumer wiring**

`reconcile_effect_claim` 按 spec §7.5 迁移集合判定（`Prepared→Claimed`、`Prepared→Unknown`、`Claimed→Invoking`、`Claimed→Unknown`、`Invoking→Succeeded`、`Invoking→Failed`、`Invoking→Unknown`；其余非法）；property 测试用随机事件流断言单值终态且不出现「同 intent 多终态 / 无前驱 active claim / 跨 owner 重复 active claim」；crash 测试在合法序列任意点注入 crash，恢复后收敛到闭包允许终态或 `Unknown`（memo 缺失→`Unknown`）。**durable memo 真实接线（非 marker）**：`src/session/events.rs` 新增 `SessionEvent` 变体 + serde + reduction；`SessionEventRecord` 继续包装记录并参与序列化/回放；`src/session/call_log.rs:45`/`src/approval/policy.rs`+`types.rs` 写 approval memo producer；`src/extension/hooks/executor.rs:1101/1339` 注入 store 句柄写 hook memo；memo 经 `SessionService::emit_event/emit_batch`（复用既有 append batch）入 event store，不接 GlobalBus；重放 replay memo 不重跑决策；`ReplayPermit` 与 `EffectClaimReconciliation` 分离，automatic replay 关闭。

- [ ] **Step 4: 定向通过**

Run: `cargo test -p aleph-server capability::effect_claim --no-fail-fast 2>&1 | tail -20`
Expected: PASS；既有 store/replay/resume 测试不回归（未重写 `SessionEventStore`/`ReplayPermit`/`ResumeCoordinator`）。

- [ ] **Step 5: diff / file boundary 审查**

`git diff --stat`：确认改动限于 `src/capability/effect_claim.rs` + `src/session/events.rs` + `src/session/call_log.rs` + `src/approval/policy.rs` + `src/approval/types.rs` + `src/extension/hooks/executor.rs` +（仅注释）`src/session/store.rs`/`replay.rs`/`resume_coordinator.rs`；无 harness 业务文件、无越界改动。

- [ ] **Step 6: Commit**

```bash
git add src/capability/effect_claim.rs src/session/events.rs src/session/call_log.rs src/approval/policy.rs src/approval/types.rs src/extension/hooks/executor.rs src/session/store.rs src/session/replay.rs src/gateway/resume_coordinator.rs
git commit -m "feat(capability): add effect claim closure, reconciliation, durable memo (Gate C)"
```

---

## Task 4: Gate D — projection / subscription cursor + compatibility wrapper

**Files:**
- Modify（补齐实现）: `src/capability/facade.rs` 的 subscription 实现（`CapabilityChangeStream` 的 snapshot+delta 重同步状态机）——归属 Task 1 已建文件，此处补齐 subscribe 语义。
- Modify（仅 marker + 适配）: `src/acp/manager/persistence.rs`（`projection_only` marker + 静态检查断言）、`src/resilience/database/state_database/mod.rs`（`StateDatabase` 标 `projection_only` marker）、`src/mcp/types.rs`（`McpTool` `:10`，`projection_only` marker——真实 MCP projection seam）、`src/tools/mcp_scope_view.rs`（`McpScopedToolService` `:14`，`projection_only` marker）、`src/tools/server/ops.rs`（`list_tools_arc_impl` `:29`，`projection_only` marker）、`src/tools/service.rs:450` 的 `to_metadata_form`（收窄为旧 Tool wrapper）、`src/event/global_bus.rs`（`notification_only` marker）
- **不改** `src/mcp/tool_bridge.rs`（MCP **inbound bridge** `spawn_tool_bridge` `:102`，**不是 projection**——落点若与 census 不符，先核对 `mcp/types.rs`/`mcp_scope_view.rs`/`server/ops.rs` 再定）。
- Test: `src/capability/facade.rs` 内 `#[cfg(test)] mod tests`（cursor resync）+ projection seam marker 断言

**Interfaces:**
- Consumes: `Zahir::subscribe`/`project`（Task 1）、`Cursor`/`CapabilitySnapshot`、`to_metadata_form(defs: &[ToolDefinition]) -> Arc<[crate::tool_metadata::ToolDefinition]>`（`src/tools/service.rs:450`，真实签名**不改名不改参**，仅作旧 Tool wrapper、**不扩展为 canonical facade API**）、`GlobalBus`。
- Produces: `CapabilityChangeStream`（committed cursor 单调推进；generation bump 发 `Invalidated` 后保留原 cursor，snapshot+delta 去重重同步，**不从 0 重订**）。

**subscription 状态机（必须按此实现）**：
1. 首次订阅：**原子**返回完整 immutable snapshot + `committed_cursor`（一步到位，无中间可观测半态）。
2. 正常推进：按 committed 顺序逐条下发 delta。
3. generation bump：保存**最后 committed cursor** → 发 `Invalidated` → 以旧 cursor 读 delta → 与 snapshot 合并 → 按 `OwnerGeneration`/`CapabilityRevision` 去重（丢弃跨 owner/跨 generation 错位事件）→ **原子提交新 cursor**。
4. **永不从 0 重订**。
5. ACP/MCP 只读 projection；store 冲突一律 fail-closed（以 event store 为准）。

- [ ] **Step 1: 写失败测试（cursor resync 不从 0 + 去重 + projection fail-closed + wrapper 无副作用）**

```rust
#[test]
fn resync_keeps_cursor_and_dedups() { /* subscribe(cursor=5) → generation bump → 收到 Invalidated，重同步后 delta 从 5 继续，跨 owner 错位事件被丢弃 */ }
#[test]
fn first_subscribe_is_atomic_snapshot_plus_cursor() { /* 首次订阅一步返回 snapshot+committed_cursor，无半态 */ }
#[test]
fn resync_never_restarts_from_zero() { /* generation bump 后 cursor 永不回落 0 */ }
#[test]
fn projection_mismatch_fails_closed() { /* ACP/MCP JSON 与 event store 不一致时以 event store 为准 */ }
#[test]
fn to_metadata_form_is_pure() { /* 输入 defs，输出仅字段复制/schema 序列化，无 side-effect */ }
```

- [ ] **Step 2: 确认失败**

Run: `cargo test -p aleph-server capability::facade --no-fail-fast 2>&1 | tail -20`
Expected: FAIL — subscribe 重同步逻辑 / marker 断言未落地。

- [ ] **Step 3: 最小实现 subscribe 状态机 + projection marker + wrapper 收窄**

按上「subscription 状态机」实现 `CapabilityChangeStream`：首次原子 snapshot+committed cursor；正常按 committed 顺序；generation bump 保存最后 committed cursor → `Invalidated` → 以旧 cursor 读 delta → 合并 snapshot → 去重 → **原子提交新 cursor**；**永不从 0**。ACP/MCP（`src/mcp/types.rs`/`src/tools/mcp_scope_view.rs`/`src/tools/server/ops.rs`）/StateDatabase 加 `projection_only`、GlobalBus 加 `notification_only`。`to_metadata_form` 仅保留字段复制 / schema 序列化 / version·generation 序列化（真实签名 `pub fn to_metadata_form(defs: &[ToolDefinition]) -> Arc<[crate::tool_metadata::ToolDefinition]>`），禁止 side-effect / RPC / event store mutation / capability 字段写入（静态检查拒绝越界），**不扩展为 canonical facade API**。

- [ ] **Step 4: 定向通过**

Run: `cargo test -p aleph-server capability::facade --no-fail-fast 2>&1 | tail -20`
Expected: PASS；既有 ACP/MCP smoke 不回归。

- [ ] **Step 5: diff / file boundary 审查**

`git diff --stat`：确认改动限于 `src/capability/facade.rs` + `src/acp/manager/persistence.rs` + `src/resilience/database/state_database/mod.rs` + `src/mcp/types.rs` + `src/tools/mcp_scope_view.rs` + `src/tools/server/ops.rs` + `src/tools/service.rs` + `src/event/global_bus.rs`；`src/mcp/tool_bridge.rs` **不**在 diff 内（inbound bridge，非 projection）。

- [ ] **Step 6: Commit**

```bash
git add src/capability/facade.rs src/acp/manager/persistence.rs src/resilience/database/state_database/mod.rs src/mcp/types.rs src/tools/mcp_scope_view.rs src/tools/server/ops.rs src/tools/service.rs src/event/global_bus.rs
git commit -m "feat(capability): projection-only subscription cursor and wrapper narrowing (Gate D)"
```

---

## Task 5: 文档补全（FEATURE_LOCATOR + reference + Tier 1）

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（速查索引 + 新增「Capability Phase 4」词条，指向 spec + `src/capability/*`）
- Modify: `docs/reference/ARCHITECTURE.md`（Zahir facade / Session event store / ownership tree 三节）、`docs/reference/CODE_ORGANIZATION.md`（`src/capability/` 目录 + `src/session/store.rs`、`src/session/replay.rs`）、`docs/reference/DESIGN_PATTERNS.md`（projection-only / fail-closed marker / effect sandwich）、`docs/reference/DOMAIN_MODELING.md`（`CapabilityKind` 9 种 + ownership 关系图）、`docs/reference/SANDBOX.md`（effect sandwich 引用 fence/replay）、`docs/reference/REDLINES.md`（R8 下补 Zahir facade 边界注解）
- Modify: `AGENTS.md`、`CLAUDE.md`（Tier 1 增加「Capability 9 类」与「fact source = event store」指针，两份同步）

**Interfaces:**
- Consumes: 无（纯文档）
- Produces: 文档锚点与实现一致（路径、符号名从 Task 1-4 落地结果反查，不写不存在路径）。

- [ ] **Step 1: 反查落地符号后写文档**

用 `grep -n "pub enum CapabilityKind" src/capability/descriptor.rs` 等反查确认真实符号与路径，再逐节补全；`AGENTS.md` 与 `CLAUDE.md` 同步修改。

- [ ] **Step 2: 校验链接有效**

Run: `grep -c "CapabilityKind" docs/reference/FEATURE_LOCATOR.md docs/reference/DOMAIN_MODELING.md AGENTS.md CLAUDE.md`
Expected: 各文件计数 > 0；所有 `docs/reference/*` 路径 `test -f` 存在。

- [ ] **Step 3: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md docs/reference/ARCHITECTURE.md docs/reference/CODE_ORGANIZATION.md docs/reference/DESIGN_PATTERNS.md docs/reference/DOMAIN_MODELING.md docs/reference/SANDBOX.md docs/reference/REDLINES.md AGENTS.md CLAUDE.md
git commit -m "docs: update reference docs for capability phase 4"
```

---

## Task 6: 整支验证与 review

**Files:**
- 不新增/不修改 `src/` 业务文件（review 检查 diff 无 harness 业务文件、无 `phase4_baseline.rs`、无 golden JSON）。

**Interfaces:**
- Consumes: Task 0-5 全部产物
- Produces: 一份「spec coverage + 非目标」review 结论。

- [ ] **Step 1: 全量验证**

Run: `cargo check && cargo test --no-fail-fast 2>&1 | tail -40 && cargo clippy --all-targets 2>&1 | tail -40`
Expected: 全绿；Phase 3 既有 test suite 无 regression。

- [ ] **Step 2: 逐项 review**

核对 Review Focus 六项各有测试；核对 spec §11 兼容边界（`to_metadata_form` 收窄 / `AlephToolServer` 非第二事实源 / `src/harness/` 零新增 / Phase 3 不回归）；核对 Deferred 清单未实现。

- [ ] **Step 3: 记录 review 结论并 commit（若有文档变更）**

```bash
git commit -am "docs: record capability phase 4 whole-branch review" || true
```

---

## Deferred（本期明确不做）

- external-effect ledger（外部 side-effect 总账）——Aleph 不假装能观测 receiver 不可见的 effect。
- 外部 effect 的 exactly-once 承诺——只保证 prepared→claimed→invoking 链上不重复 commit；外部副作用由 receiver 端幂等保证。
- automatic Safe Replay——默认 VerifyOnly，replay 必须显式 `ReplayPermit`。
- 完整 ACP server gate——本期只做 projection-only。
- universal durable scheduler——违反 R10 薄 harness 与 R7 LLM 主权。
- 其它 8 种 kind 的 backend 实现与挂载——只定义 trait + onboarding contract。
- `src/harness/` 扩张——本期零新增业务文件。
- `src/session/event_store/` 新目录或任何事件存储子模块——store/replay/resume 均为既有，只加 marker。

---

## Self-Review

**1. Spec coverage:** §5.1→Task 1（`CapabilityKind` 9 种）；§5.3/5.4→Task 1（`CapabilityDescriptor` / `Zahir`）；§5.6→Task 1（`ToolBackendAdapter` 唯一完整 backend）；§6→Task 2（ownership tree / scope 分离 / revoke/dispose）；§7→Task 3（effect sandwich / closure / durable memo 真实接线 / ReplayClaimResult 区分）；§8→Task 4（immutable snapshot + committed cursor / projection-only）；§11.1→Task 4（`to_metadata_form` 收窄）；§11.3→Task 6 review；§12→Deferred 清单；§13→Task 5 文档。无缺口。

**2. Step scan:** 每 step 单一动作 + 可核对结果；失败测试给出精确断言与函数名，实现 step 给出精确签名与落点文件，无「TBD / 处理边界情况」式空话；函数体仅对 spec 固定的算法（闭包迁移集合、cursor 重同步）给出规则，未抄写签名已决定的代码。

**3. Type consistency:** `CapabilityId`/`OwnerGeneration`/`CapabilityRevision`/`LifetimeScope`/`VisibilityScope`/`OwnerRef`/`EffectClaim`/`FencingToken`/`EffectClaimState`/`EffectClaimReconciliation` 在 Task 1/2/3 间签名一致；`CapabilityDescriptor` 引用 `ownership::{OwnerGeneration, LifetimeScope, VisibilityScope, OwnerRef}`——**Task 1 已把纯 ownership 基础类型与 descriptor 放同一 Gate/同一 commit（`ownership.rs` 同 commit 落地），消除 Task 1↔Task 2 的编译顺序依赖**；Task 2 仅追加 `OwnershipTree` 行为；`to_metadata_form` 沿用真实签名 `pub fn to_metadata_form(defs: &[ToolDefinition]) -> Arc<[crate::tool_metadata::ToolDefinition]>`（`src/tools/service.rs:450`），不改名不改参，仅旧 wrapper、不扩展为 canonical facade API。

**4. Review Focus:** 六项各自有 owning task 的测试：stale revision/owner→Task 2；cursor+generation resync→Task 4；duplicate claim→Task 3；projection failure→Task 4；approval/hook memo→Task 3；compatibility wrapper→Task 4。无空项。

**5. Proportion:** 计划以签名 + 测试名 + 断言为主，代码块仅出现在「spec 固定算法」处（闭包迁移集合、cursor 去重、descriptor 字段布局），未复写程序；每 task 的 Files/Interfaces 短于其代码量。既有事实源均标注「仅 marker / 注释，不改行为」，与 spec「不重写 store/replay/resume」一致。
