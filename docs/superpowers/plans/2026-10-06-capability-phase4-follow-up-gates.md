# Capability Phase 4 后续 Gate（E→F→G）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在已合并的 Capability Phase 4 基座（Gate A–D）之上，把 Gate E（concrete `ZahirFacade` backend wiring + `ResolveError` fail-closed lease）、Gate F（EffectClaim reducer fail-closed 保持 + `emit_batch` 原子边界审计 + producer seam 报告，**不**实现真实 producer）、Gate G（`CapabilityChangeStream` 桥接 live registry broadcast + projection 边界核验）从 deferred 推进到 committed。H/I（automatic Safe Replay、external-effect exactly-once、universal durable scheduler、完整 ACP server gate）保持 deferred。

**Architecture:** 全程不复制、不重定义既有事实源。`ToolHandlerRegistry`（`src/tools/registry.rs`）仍是 callable Tool 唯一事实源；`OwnershipTree`（`src/capability/ownership.rs`）是 owner generation / revoke / dispose 唯一事实源；`SessionEventStore`（`src/session/store.rs`）是 durable recovery 唯一 committed source。新增 concrete host 类型固定命名为 `ZahirFacade`（`src/capability/zahir_facade.rs`），只组合 `ToolBackendAdapter` + `Arc<OwnershipTree>`，不建第二套 registry/descriptor map/owner 计数。`CapabilityChangeStream` 保持纯 value 状态机（无 receiver / history）；registry broadcast 只是 live notification（256 槽、lag 可丢），不是 recovery source。

**Tech Stack:** Rust（`alephcore` workspace member）· tokio（`tokio::sync::broadcast`）· serde / serde_json · sha2（`Cargo.toml:113,261`，复用，不新增）· `cargo -p alephcore`（**不**用 `--workspace`）

**Spec:** `docs/superpowers/specs/2026-10-06-capability-phase4-follow-up-gates-design.md`（同目录）。spec commit `15032e2679ee3c3abdf95eb07e123e0881932351`；用户 m00471 已确认 spec。

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up`，分支 `capability-phase4-follow-up`。所有命令从该 worktree root 显式 `cd` 后运行；**不是** `/Volumes/TBU4/Workspace/Aleph` 的 `main`。

---

## Global Constraints

- **R1（脑体分离）**：改动全部落在 Rust Core（`src/capability/`、`src/tools/descriptor.rs`、`src/session/`）与 `docs/`；不调平台 API、不引入 platform-binding 依赖。
- **R3（核心轻量化）**：不新增三方依赖；fingerprint 复用已有 `sha2`（`Cargo.toml:113,261`）。
- **R4（Interface 层禁止业务逻辑）**：projection/notification boundary 只加 doc 锁注释，不加业务逻辑。
- **R7（LLM 主权）**：不引入意图分类器 / dispatcher / 确定性 replay 策略。
- **R8（工具即一切）**：capability 是 Tool 的真 backend；不另立 Tool 抽象。
- **R10（薄 Harness）**：不修改 `src/harness/`。
- **不复制事实源**：`ToolHandlerRegistry`/`RegistryChange`/`RegistryShared{change_tx, mutation_lock}`/`subscribe()` 只在 `src/tools/registry.rs`；`Zahir`/`BackendLease`/`Cursor`/`CapabilityChange`/`CapabilityChangeStream`/`CapabilitySnapshot`/`Projection`/`TransportTarget` 只在 `src/capability/facade.rs`。**禁止**新建 `src/capability/registry.rs`、`src/capability/change_stream.rs`。
- **不发明类型/方法名**：禁止 `BackendLease.fence` / `BackendLease::is_valid` / `BackendLease::invalidate` / `StaleLease` / `RegistryView` / `Zahir::list()` / `EffectClaimProducer` / `IntegrationGateDeferred`。真实 `BackendLease` 只有 `descriptor + owner_generation`；真实 `Zahir` 只有 `describe/resolve/validate_lease/subscribe/project`。
- **F 边界（不得臆造 producer）**：全仓**没有** EffectClaim producer 构造点（grep `EffectClaimPrepared/Claimed/Invoking/Terminal` 仅 `src/session/events.rs` 定义 + `src/capability/effect_claim.rs` 测试，无生产 `SessionEvent::EffectClaim*` 构造）。本 plan 只做 reducer fail-closed 保持测试 + `emit_batch`/`append_batch` 原子边界 contract audit + producer seam 报告；**不**实现真实 producer（不臆造 dispatch fire-and-forget、SessionId 转换、fence 来源、`EffectClaimProducer` 类型）。真实 producer integration 需下一次独立 architecture approval。
- **H/I deferred 不得改实**：automatic Safe Replay、external-effect exactly-once、universal durable scheduler、完整 ACP server gate、其余 `CapabilityKind` 的 concrete backend。
- **低内存前置检查**：每次跑 `cargo` 前先取 macOS `vm_stat` 的 **free + inactive + speculative** 三页和（页面大小 4 KiB）：
  ```bash
  FREE=$(vm_stat | awk '/Pages free/{print $3}' | tr -d '.')
  INACTIVE=$(vm_stat | awk '/Pages inactive/{print $3}' | tr -d '.')
  SPECULATIVE=$(vm_stat | awk '/Pages speculative/{print $3}' | tr -d '.')
  MEM_KIB=$(( (FREE + INACTIVE + SPECULATIVE) * 4 ))
  echo "MemKib(free+inactive+speculative): ${MEM_KIB}"
  ```
  阈值 4 GiB = `4194304` KiB。**低于阈值即 STOP/WAIT，不执行任何 cargo 命令**（不降级跑、不先释放 target 缓存）。
- **格式命令**：仓库唯一格式工具是 `cargo fmt`（rustfmt，STYLE.md：4-space / 100 列宽）。全仓 `cargo fmt --check` 会被既有无关文件 drift 阻断 → 只对本次改动 `.rs` 文件核验格式，并把既有 baseline drift 单独记录（不顺手修无关文件）。**不得声称 `cargo fmt -p`**（仓库无 per-package fmt wrapper）。
- **commit message**：`<scope>: <description>` 英文（AGENTS.md 规范），无 emoji / 中文。
- **clippy**：`cargo -p alephcore clippy --all-targets -- -D warnings` 必须绿。
- **测试**：`cargo -p alephcore test --lib` 必须全绿。
- **判据自查**：每条 commit 自查 CLAUDE.md §0–§19 适用项（§1 两份表述、§2 恒真谓词、§8 fail-closed、§10 wire 契约两边、§11 no-op、§16 孪生子系统、§17 展示用能指出渲染行）。

---

## Review Focus

（spec 暗示但最易咬人的 6 个输入/失败模式）

1. **`describe` 与 `resolve` 同代** — facade 若 describe 走 `descriptor_snapshot()`、resolve 走 `lookup()` 会代际撕裂。Task 5 新增 `ToolBackendAdapter::snapshot_capabilities()`（单次 `snapshot_state()` 读，返回 `(Vec<CapabilityDescriptor>, u64)`），`describe`/`subscribe` 的 descriptor 集与 `committed_cursor` 来自同一 registry 代；`resolve`/`validate_lease` 走单次 `lookup()`（单读，绝不把 `descriptor_snapshot()` 与另一次 `revision()` 配对）。
2. **stale owner generation 必须 fail-closed** — `BackendLease` 只有 `descriptor + owner_generation`，无 stale 标志；`validate_lease` 是唯一 use-time 校验入口，消费者不得自行比较 generation。Task 5 落地三道 fail-closed 判据（binding 定位 / owner generation / descriptor freshness）。
3. **`resolve` 构造点不可能 `StaleOwner`** — `resolve` 当场从 `OwnershipTree::generation()` 铸造 lease，`owner_generation` 就是当前值；`StaleOwner` 只在 `validate_lease`（对先前发放、之后 bump 的 lease）出现。Task 5 测试钉死这个不对称。
4. **`emit_batch` 单事务原子 + 空 batch 拒绝** — Task 6 审计：`SessionService::emit_batch` 默认 `Err`、`InProcessActorSessionService`（`src/session/in_process.rs:293`）是唯一生产 override；`SessionEventStore::append_batch`（`src/session/store.rs:79`）单事务、空 events 无 retire 拒绝（`src/session/store.rs:1231`）。不引入 batch-id durable set / duplicate-ignore。
5. **registry in-process broadcast ≠ recovery source** — broadcast 无 durable cursor、256 槽 lag 可丢；recovery 只认 `SessionEventStore`。Task 5 的 `subscribe` 先 `registry.subscribe()` 拿 receiver、再读 snapshot（receiver 先于 snapshot 存在，故 snapshot 后的 drain 不丢窗口事件）；lag → snapshot + `Invalidated`，不伪装逐条投递。输入 `cursor` 只是过滤/去重 hint，不声称断线恢复；`cursor > snapshot revision` 时 fail-closed 返回 snapshot cursor（不宣称恢复，测试钉死）。
6. **projection boundary 不维护第二 identity/revision** — `StateDatabase` / ACP JSON / MCP view / `GlobalBus` 不得授权、不得 resolve handler、不得决定 replay、不得维护第二份 generation。Task 7 加 boundary 锁注释。
7. **`Scope.visibility` 必须影响 describe/project/subscribe** — `ToolBackendAdapter::enumerate` 只过滤 kind/namespace；host 按同一 `(id, visibility)` binding 过滤（`OwnershipTree::generation(...).is_some()`）。Task 5 测试分离 default visibility 与 custom visibility。

---

## File Structure

### New Files
- `src/capability/zahir_facade.rs` — concrete `ZahirFacade` host：组合 `ToolBackendAdapter` + `Arc<OwnershipTree>`；`describe/resolve/validate_lease/subscribe/project` 的 live 实现（Task 5）。
- `.superpowers/sdd/2026-10-06-capability-phase4-follow-up-gates/task-6-producer-seam-report.md` — Gate F producer seam 审计报告（Task 6 产出，非产品代码）。

### Modified Files（按独占归属）
- `src/capability/ownership.rs` — 新增只读 `generation()`（Task 1 独占）。
- `src/capability/backend.rs` — `to_descriptor` fingerprint（Task 2）→ 删除 `CapabilityBackend::generation()` + `owner_generation` 行（Task 3，顺序承接 Task 2）→ 新增只读 `snapshot_capabilities()`（Task 5，顺序承接 Task 3）。
- `src/tools/descriptor.rs` — `canonicalize_json` 私有改 `pub(crate)`（Task 2 独占）。
- `src/capability/descriptor.rs` — 删除 `CapabilityDescriptor.owner_generation` 字段（Task 3 独占）。
- `src/capability/facade.rs` — `ResolveError`、`Scope`/`Reference` 加 `visibility`、`Zahir` trait 改 `resolve -> Result` + 新增 `validate_lease`（Task 4 独占）。
- `src/capability/mod.rs` — 注册 `pub mod zahir_facade;`（Task 5）。
- `src/capability/effect_claim.rs` — 仅补 fail-closed 语义保持测试（Task 6 独占）。
- `docs/reference/FEATURE_LOCATOR.md` / `AGENTS.md` / `CLAUDE.md` — E/F/G committed、H/I deferred 同步（Task 7 独占）。

### 明确不碰
- `src/harness/`（R10）、`src/tools/service.rs:453` 的 `to_metadata_form`（旧 compatibility wrapper，不删不迁）、`src/session/events.rs`（wire 身份定义，只读）、`src/session/store.rs` / `src/session/service.rs`（只读审计，不改 public API）。

---

## Task Dependency & File Exclusivity

| Task | Files | Depends on | 独占说明 |
|---|---|---|---|
| 1 E generation | `ownership.rs` | — | 独占 `ownership.rs` |
| 2 E fingerprint | `backend.rs`（`to_descriptor`）、`tools/descriptor.rs` | — | 独占 `tools/descriptor.rs`；`backend.rs` 只改 `to_descriptor` 的 `fingerprint` 行 |
| 3 E remove authority | `backend.rs`（`generation()` + `owner_generation` 行）、`descriptor.rs` | 2（`backend.rs` 顺序承接） | 独占 `descriptor.rs`；`backend.rs` 只删 `generation()` 与 `owner_generation` 行 |
| 4 E facade contract | `facade.rs` | 1, 3 | 独占 `facade.rs` |
| 5 E+G ZahirFacade | `zahir_facade.rs`（新）、`backend.rs`（`snapshot_capabilities`）、`mod.rs` | 1, 2, 3, 4 | 独占 `zahir_facade.rs`、`mod.rs`；`backend.rs` 顺序承接 Task 3（只加 `snapshot_capabilities`） |
| 6 F reducer/atomic audit | `effect_claim.rs`（测试）、report 文件 | 4, 5 | 独占 `effect_claim.rs`、report 文件 |
| 7 docs | FEATURE_LOCATOR / AGENTS / CLAUDE | 1–6 | 独占 docs |
| 8 final verification | —（命令） | 1–7 | 无文件 |

> `backend.rs` 被 Task 2 → Task 3 → Task 5 顺序承接（非并行）；每个 Task 在前一个 Task commit 之后开始，不并发编辑。其余文件严格独占，无编辑竞争。

---

## Task 1: Gate E — `OwnershipTree::generation()` 只读 accessor

**Files:** Modify `src/capability/ownership.rs`

**Public signature（新增，紧邻 `resolve`（:272）之后插入）:**

```rust
    /// Read the current owner generation of one `(CapabilityId, VisibilityScope)`
    /// binding without mutating it.
    ///
    /// Returns `None` when no such binding exists. Never bumps the nonce, never
    /// touches claims, never registers a binding. Generation is per-binding
    /// (`(CapabilityId, VisibilityScope)`-level), NOT a global registry revision.
    #[must_use]
    pub fn generation(
        &self,
        capability: &CapabilityId,
        visibility: &VisibilityScope,
    ) -> Option<OwnerGeneration> {
        let inner = self.inner.lock().expect("ownership mutex poisoned");
        let key = Self::key(capability, visibility);
        inner.bindings.get(&key).map(|binding| binding.generation)
    }
```

- [ ] **Step 1: 写 failing tests（在 `ownership.rs` 现有 `mod tests` 内追加）**

```rust
    #[test]
    fn generation_reads_binding_and_tracks_bump() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        assert_eq!(tree.generation(&cap, &vis), None);
        tree.register(
            cap.clone(),
            OwnerRef::Task(TaskId("t1".into())),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));
        let bumped = tree.bump(LifetimeScope::Task);
        assert_eq!(bumped, OwnerGeneration(1));
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(1)));
    }

    #[test]
    fn generation_is_none_after_revoke_or_dispose() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        tree.register(
            cap.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        assert_eq!(tree.generation(&cap, &vis), Some(OwnerGeneration(0)));
        assert!(tree.revoke(&cap));
        assert_eq!(tree.generation(&cap, &vis), None);

        let cap2 = cap_id("bar");
        tree.register(
            cap2.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register succeeds");
        assert!(tree.dispose(LifetimeScope::Task));
        assert_eq!(tree.generation(&cap2, &vis), None);
    }

    #[test]
    fn generation_does_not_mutate() {
        let tree = OwnershipTree::new();
        let cap = cap_id("foo");
        let vis = full_vis();
        tree.register(cap.clone(), OwnerRef::Runtime, LifetimeScope::Runtime, vis.clone())
            .expect("register succeeds");
        let before = tree.generation(&cap, &vis);
        let again = tree.generation(&cap, &vis);
        assert_eq!(before, again);
        assert_eq!(tree.resolve(&cap, &vis), Some(()));
    }
```

- [ ] **Step 2: 跑测试确认 RED**

```bash
cargo -p alephcore test --lib capability::ownership::tests::generation_reads_binding_and_tracks_bump
```
期望：编译失败（`generation` 方法不存在）。

- [ ] **Step 3: 实现 `generation()`（上面的签名体）**

- [ ] **Step 4: 跑测试确认 GREEN**

```bash
cargo -p alephcore test --lib capability::ownership
```
期望：`generation_reads_binding_and_tracks_bump` / `generation_is_none_after_revoke_or_dispose` / `generation_does_not_mutate` 及既有测试全 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/capability/ownership.rs
git commit -m "capability(e): add read-only OwnershipTree::generation accessor"
```

---

## Task 2: Gate E — `to_descriptor` schema fingerprint（sha2 + canonical JSON）

**Files:** Modify `src/tools/descriptor.rs`（`canonicalize_json` 改 `pub(crate)`）、`src/capability/backend.rs`（`to_descriptor`）

**事实锚点：** `src/tools/descriptor.rs:501` 已有私有 `fn canonicalize_json(value: &serde_json::Value, out: &mut Vec<u8>)`（稳定键序、无空白，`keys.sort_unstable()`），被 `canonical_replay_contract_preimage`（:444）复用。本 Task 只把它从 `fn` 改成 `pub(crate) fn`，**不**复用 harness 的 `crate::harness::agent::canonical_json_string`（`src/harness/agent.rs:1007`，R10 边界外）。

- [ ] **Step 1: 改 `src/tools/descriptor.rs:501` 可见性**

```rust
// 原文（private）:
fn canonicalize_json(value: &serde_json::Value, out: &mut Vec<u8>) {
// 改为:
pub(crate) fn canonicalize_json(value: &serde_json::Value, out: &mut Vec<u8>) {
```

- [ ] **Step 2: 写 failing test（在 `src/capability/backend.rs` 现有 `mod tests` 内追加）**

```rust
    #[test]
    fn to_descriptor_fingerprint_is_canonical_and_stable() {
        let reg = ToolHandlerRegistry::new();
        let a = ToolCapabilityDescriptor::from_definition(
            &ToolDefinition {
                name: "fp".to_string(),
                description: "fp".to_string(),
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"b":{"type":"string"},"a":{"type":"number"}}}"#).unwrap(),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            },
            0,
        );
        let b = ToolCapabilityDescriptor::from_definition(
            &ToolDefinition {
                name: "fp".to_string(),
                description: "fp".to_string(),
                input_schema: serde_json::from_str(r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"string"}}}"#).unwrap(),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            },
            0,
        );
        reg.register(a, Arc::new(fake_handler("fp"))).unwrap();
        let backend = ToolBackendAdapter { registry: reg };
        let da = backend
            .lookup(&CapabilityId { namespace: "aleph/tools".into(), name: "fp".into() })
            .unwrap();
        // Same schema, different key order → same fingerprint.
        let reg2 = ToolHandlerRegistry::new();
        reg2.register(b, Arc::new(fake_handler("fp"))).unwrap();
        let backend2 = ToolBackendAdapter { registry: reg2 };
        let db = backend2
            .lookup(&CapabilityId { namespace: "aleph/tools".into(), name: "fp".into() })
            .unwrap();
        assert_eq!(da.schema.fingerprint, db.schema.fingerprint);
        assert_ne!(da.schema.fingerprint, [0u8; 32]);
    }
```

- [ ] **Step 3: 跑测试确认 RED**

```bash
cargo -p alephcore test --lib capability::backend::tests::to_descriptor_fingerprint_is_canonical_and_stable
```
期望：失败（`fingerprint` 仍是 `[0u8; 32]` 占位，两条 schema 的 fingerprint 相等但等于零值，`assert_ne!([0u8;32])` 失败）。

- [ ] **Step 4: 实现 `to_descriptor` 的 fingerprint 计算**

在 `src/capability/backend.rs` 顶部补导入：

```rust
use sha2::{Digest, Sha256};
use crate::tools::descriptor::canonicalize_json;
```

将 `to_descriptor`（:92）改造如下——**只动 fingerprint 相关行，其余字段不复制进本 diff**：

```rust
// 顶部导入（新增）:
use sha2::{Digest, Sha256};
use crate::tools::descriptor::canonicalize_json;

// `to_descriptor` 函数体顶部新增（在 `CapabilityDescriptor {` 之前）:
    let mut schema_bytes = Vec::new();
    canonicalize_json(&tool.input_schema, &mut schema_bytes);
    let digest = Sha256::digest(&schema_bytes);
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&digest);

// `CapabilityDescriptor { ... }` 构造里唯一改动的行是 `schema`:
        schema: SchemaRef {
            version: tool.schema_version,
            fingerprint,          // 原 [0u8; 32] 占位 → 上面计算的 fingerprint
        },
// 其余字段（id / kind / revision / lifetime / visibility / owner / services /
// metadata）本 Task 一字不动，与当前源码 :92 逐字一致。
```

- [ ] **Step 5: 跑测试确认 GREEN**

```bash
cargo -p alephcore test --lib capability::backend
```
期望：新测试 PASS；既有 `tool_name_round_trips_to_descriptor` 仍 PASS（旧 generation 权威一致性测试由 Task 3 随字段删除一并移除）。

- [ ] **Step 6: Commit**

```bash
git add src/tools/descriptor.rs src/capability/backend.rs
git commit -m "capability(e): compute to_descriptor schema fingerprint via sha2"
```

---

## Task 3: Gate E — 删除旧 generation 权威（`CapabilityBackend::generation()` + `CapabilityDescriptor.owner_generation`）

**Files:** Modify `src/capability/backend.rs`、`src/capability/descriptor.rs`

**前置（承接 Task 2 的 `backend.rs` 状态；本 Task 顺序执行）。**

- [ ] **Step 1: 删除 `src/capability/backend.rs` 的 `generation()`**

- 从 `CapabilityBackend` trait（:25）删除 `fn generation(&self) -> OwnerGeneration;`。
- 从 `impl CapabilityBackend for ToolBackendAdapter`（:73 起）删除整个 `fn generation(&self) -> OwnerGeneration { ... OwnerGeneration(0) }`（含其长注释）。
- `to_descriptor`（:92）删除 `owner_generation: OwnerGeneration(0),` 行。
- 顶部导入 `use crate::capability::ownership::{LifetimeScope, OwnerGeneration, OwnerRef, VisibilityScope};` 去掉 `OwnerGeneration`（此时 backend.rs 已无使用）。
- 删除测试 `generation_matches_descriptor_owner_generation`（:178 起，它引用 `backend.generation()` 与 `d.owner_generation`，随字段/方法删除而失效）。

- [ ] **Step 2: 删除 `src/capability/descriptor.rs` 的 `owner_generation` 字段**

- `CapabilityDescriptor`（:95）删除 `pub owner_generation: OwnerGeneration,`。
- 顶部导入 `use crate::capability::ownership::{LifetimeScope, OwnerGeneration, OwnerRef, VisibilityScope};` 去掉 `OwnerGeneration`。

- [ ] **Step 3: 编译核验（确认无残留引用）**

```bash
cargo -p alephcore check --all-targets
```
期望：编译绿。若出现其它引用 `CapabilityDescriptor.owner_generation` 或 `backend.generation()` 的错误，逐一删除（grep 已确认全仓仅 `backend.rs` 与 `descriptor.rs` 引用，无生产消费方；`facade.rs` 的 `BackendLease.owner_generation` 是独立字段，**保留**）。

- [ ] **Step 4: 跑相关测试确认 GREEN**

```bash
cargo -p alephcore test --lib capability::
```
期望：`capability::backend`、`capability::descriptor`、`capability::facade` 测试全 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/capability/backend.rs src/capability/descriptor.rs
git commit -m "capability(e): drop CapabilityBackend::generation and descriptor owner_generation"
```

---

## Task 4: Gate E — facade contract（`ResolveError` + `Scope`/`Reference.visibility` + `Zahir` trait）

**Files:** Modify `src/capability/facade.rs`

**新增 `ResolveError`（闭集，紧邻 `BackendLease` 之后）:**

```rust
/// Closed error set for `Zahir::resolve` / `Zahir::validate_lease`.
///
/// `StaleOwner` is NOT a resolve-time error: a freshly minted lease carries the
/// generation read at resolve time, so it can never be stale at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// Lookup miss, binding missing (and not revoked), descriptor freshness
    /// mismatch, or any unclassifiable failure — fail-closed.
    Unknown,
    /// The caller's `Reference.visibility` has no binding for the id (resolve-only).
    NotVisible,
    /// The binding has been revoked.
    Revoked,
    /// The lease's `owner_generation` lags the current per-binding generation
    /// (validate-only).
    StaleOwner,
}
```

**改 `Scope`（:18）与 `Reference`（:27）:**

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub kind: Option<CapabilityKind>,
    pub namespace: Option<String>,
    pub visibility: VisibilityScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub id: CapabilityId,
    pub visibility: VisibilityScope,
}
```

**改 `Zahir` trait（:222-226）:**

```rust
pub trait Zahir {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError>;
    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError>;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}
```

**顶部导入补 `VisibilityScope`:** `use crate::capability::ownership::{OwnerGeneration, VisibilityScope};`

> 迁移范围已核验：`Scope`/`Reference` 在 `src/capability/facade.rs` 之外**无**生产 struct literal（grep 确认其它 `Scope`/`Reference` 匹配均为无关类型如 `McpServerSpec::Reference`、`FlowScope`、`GuestScope`）；`facade.rs` 既有测试模块不构造 `Scope`/`Reference`/`BackendLease`/`Zahir`，故加字段与改签名不破坏既有测试。

- [ ] **Step 1: 落实上述类型与 trait 改动**

- [ ] **Step 2: 编译核验**

```bash
cargo -p alephcore check --all-targets
```
期望：编译绿。`ResolveError` 有 4 变体但 `validate_lease` 尚无 impl（无 concrete `Zahir`），无死代码告警（trait 方法本身不触发 `-D warnings` 的 unused）。

- [ ] **Step 3: 跑既有 facade 纯值测试确认 GREEN**

```bash
cargo -p alephcore test --lib capability::facade
```
期望：既有 `from_snapshot_captures_cursor_and_empties_changes`、`append_only_accepts_strictly_monotonic_cursors`、`invalidate_and_resync_*` 全 PASS（`CapabilityChangeStream` 未改动）。

- [ ] **Step 4: Commit**

```bash
git add src/capability/facade.rs
git commit -m "capability(e): ResolveError + visibility on Scope/Reference + validate_lease"
```

---

## Task 5: Gate E+G — `ZahirFacade` concrete host + subscribe bridge

**Files:** Create `src/capability/zahir_facade.rs`；Modify `src/capability/backend.rs`（新增 `ToolBackendAdapter::snapshot_capabilities()`）、`src/capability/mod.rs`（注册 `pub mod zahir_facade;`）

> 本 Task 给的是**精确签名 + 算法**（不呈现一份无法逐字编译的"完整文件"）。实现时以真实源码为准：`ToolHandler` = `crate::tools::handlers::ToolHandler`（`src/tools/handlers/mod.rs:27`）；`ToolSource`/`ToolDefinition`/`ToolDefinitionMetadata`/`ToolError` = `crate::tools::service`（`ToolSource` 经 `service.rs:193` re-export）；`Arc` = `crate::sync_primitives::Arc`；`ToolOutput` = `crate::session::events::ToolOutput`；import 只保留实际使用项（按 clippy `-D warnings` 核验）。

**5.0 `ToolBackendAdapter::snapshot_capabilities()`（`backend.rs`，`impl ToolBackendAdapter` 内、`in_scope` 之后）** — 单次 `snapshot_state()` 读，不复制 map：

```rust
    /// `(descriptors, revision)` from ONE registry generation.
    ///
    /// Both halves come from a single `snapshot_state()` load, so a caller can
    /// never pair descriptors from generation N with a revision from N+1.
    /// `describe`/`subscribe` build their `CapabilitySnapshot` from THIS — never
    /// `enumerate` + a separate `revision()` read (two loads can tear).
    #[must_use]
    pub fn snapshot_capabilities(&self, scope: &Scope) -> (Vec<CapabilityDescriptor>, u64) {
        let snap = self.registry.snapshot_state();
        let capabilities = snap
            .entries()
            .values()
            .map(|entry| {
                let mut descriptor = to_descriptor(entry.descriptor.as_ref());
                // The registry descriptor is identity-level; the facade binds
                // the returned descriptor to the requested visibility scope.
                descriptor.visibility = scope.visibility.clone();
                descriptor
            })
            .filter(|d| Self::in_scope(&d.id, scope))
            .collect();
        (capabilities, snap.revision())
    }
```

**5.1 `zahir_facade.rs` — 精确签名 + 算法步骤（不附逐字编译的"完整文件"伪代码）**

```rust
/// The one live [`Zahir`] implementation. Holds NO second descriptor map,
/// handler map, or owner counter — the registry (inside `adapter`) is the
/// single callable-Tool source; the tree is the single owner/revoke/dispose
/// source.
pub struct ZahirFacade {
    adapter: ToolBackendAdapter,
    tree: Arc<OwnershipTree>,
}

impl ZahirFacade {
    /// No startup scan: bindings are registered lazily by
    /// `reconcile_registry_bindings()` on the first facade call, so a tool
    /// registered after construction is visible on the next call.
    #[must_use]
    pub fn new(registry: ToolHandlerRegistry, tree: Arc<OwnershipTree>) -> Arc<Self>;

    /// Idempotently register an ownership binding for every registry tool that
    /// has none (and is not revoked/disposed). Called at the top of
    /// `describe`/`resolve` (`subscribe`/`project` reach it via `describe`).
    pub fn reconcile_registry_bindings(&self);

    fn map_registry_change(&self, change: RegistryChange) -> (Cursor, CapabilityChange);
}

impl Zahir for ZahirFacade {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError>;
    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError>;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}
```

**算法步骤（实现时逐条落实；import 只保留实际使用项，按 clippy `-D warnings` 核验）:**

1. **`reconcile_registry_bindings`** — 单次 `self.adapter.registry.snapshot_state()`（`RegistrySnapshot`，字段经 `revision()`/`entries()`/`entry()` 访问）；对 `snap.entries().keys()` 的每个 `name` 构造 `CapabilityId { namespace: TOOL_NAMESPACE, name }`；仅当 `self.tree.resolve(&id, &VisibilityScope::default()).is_none()`（无 binding 且未 revoked/disposed）时 `self.tree.register(id, OwnerRef::Runtime, LifetimeScope::Runtime, VisibilityScope::default())`（`register` 是 `HashMap::insert` overwrite；revoked/disposed → `Err` → no-op）。已有 binding 跳过 → generation 不重置；`unregister` 不 revoke，后续 re-register 可 resolve。
2. **`map_registry_change`** — `RegistryChange::{Registered, Replaced, Unregistered}` 三臂：`name: String` + 原始 `u64` revision → `CapabilityId { namespace: TOOL_NAMESPACE, name }` + `CapabilityRevision(revision)`，返回 `(Cursor(revision), CapabilityChange::…)`；`source: ToolSource` 丢弃（spec §3.4）。
3. **`describe`** — 先 `reconcile_registry_bindings()`；`let (capabilities, revision) = self.adapter.snapshot_capabilities(&scope)`（同代，不经 `enumerate` + 单独 `revision()` 两次读）；helper 将返回 descriptor 的 `visibility` 设为 `scope.visibility`；再按 `(id, scope.visibility)` binding 过滤 `filter(|d| self.tree.generation(&d.id, &scope.visibility).is_some())`（visibility 过滤在 host 层，不在 backend `enumerate` 只过滤 kind/namespace）。返回 `CapabilitySnapshot { capabilities, committed_cursor: Cursor(revision) }`。
4. **`resolve`** — 先 `reconcile_registry_bindings()`；从 `self.adapter.lookup(&reference.id)` 取得 descriptor 后，先把返回副本的 `descriptor.visibility` 设为 `reference.visibility.clone()`（registry descriptor 仍是 identity-level source）；`self.tree.generation(&reference.id, &reference.visibility)`：`Some(g)` → `Ok(BackendLease { descriptor, owner_generation: g })`；`None` + `self.tree.is_revoked(&reference.id)` → `Revoked`；`None` → `NotVisible`。构造点 generation 即当前值，**不可能** `StaleOwner`。`validate_lease` 使用 lease descriptor 上的 visibility，因此 custom binding 不会退回 default binding。
5. **`validate_lease`** — 三道 fail-closed 判据：① binding 定位（`self.tree.generation(&lease.descriptor.id, &lease.descriptor.visibility)`，`None` → `is_revoked` ? `Revoked` : `Unknown`）；② owner generation（`current != lease.owner_generation` → `StaleOwner`）；③ descriptor freshness（`self.adapter.lookup(&lease.descriptor.id)` 比对 `(revision, schema.fingerprint)`，不匹配 → `Unknown`）。全过 → `Ok(())`。
6. **`subscribe`** — ① **先** `let mut receiver = self.adapter.registry.subscribe()` **再** `let snapshot = self.describe(scope.clone())`（receiver 先于 snapshot 存在，snapshot→drain 窗口不丢事件）；② `cursor` 是去重/过滤 HINT（同步纯 value API 无 durable history，不声称断线恢复）；`cursor > snapshot.revision` 时 `from_snapshot` 把 `committed_cursor` 锚定到 snapshot revision（不伪造未来 cursor，记录 source 不可恢复，测试钉死）；③ `try_recv` 同步 drain：`map_registry_change` → `stream.append(cursor, change)`（`append` 拒绝 `cursor <= committed_cursor`，snapshot 已含事件自动丢弃）；`Lagged(_)` → `self.describe(scope)` 重新 snapshot + `stream.invalidate_and_resync(fresh, Vec::new())`（`Invalidated`，不伪装逐条投递）；`Empty`/`Closed` → break。
7. **`project`** — `let snapshot = self.describe(scope)`（projection 与 dispatch 同代）；返回 `Projection { target, metadata: ProjectionMetadata::default(), capabilities: snapshot.capabilities.iter().map(|d| d.id.clone()).collect() }`；不重算 fingerprint、不 resolve handler、不决定 replay。

**`src/capability/mod.rs` 注册:** 在 `pub mod ownership;` 后加 `pub mod zahir_facade;`

- [ ] **Step 1: 写 failing tests（在 `zahir_facade.rs` 底部加 `#[cfg(test)] mod tests`）**

> `backend.rs` 的测试 helper 是 private（`fn fake_def` `src/capability/backend.rs:128`、`struct FakeHandler` `:138`、`impl ToolHandler for FakeHandler` `:143`、`fn fake_handler` `:155`，均在 `mod tests` 内、无 `pub`），不能跨模块 `use`。Task 5 在自身 `#[cfg(test)] mod tests` 内定义同形 helper（body 与 `backend.rs:128-159` 逐字一致），exact signatures 如下（不贴完整实现）：

```rust
use super::*; // 带入 parent 的 LifetimeScope/OwnerRef/VisibilityScope/CapabilityId/TOOL_NAMESPACE 等

fn fake_def(name: &str) -> ToolDefinition;      // { name, description: format!("desc {name}"),
                                                //   input_schema: json!({"type":"object","properties":{}}),
                                                //   source: ToolSource::Builtin, metadata: ToolDefinitionMetadata::default() }
struct FakeHandler { name: String }
// impl ToolHandler for FakeHandler:
//   invoke(_input) -> Ok(ToolOutput { value: json!({"tool": self.name}), metadata: Default::default() })
//   definition() -> fake_def(&self.name)
fn fake_handler(name: &str) -> FakeHandler;     // FakeHandler { name: name.to_string() }
fn host_with(tools: &[&str]) -> Arc<ZahirFacade>; // new ToolHandlerRegistry; register 每个 fake_def(t)（rev 0）+ fake_handler(t); ZahirFacade::new(reg, Arc::new(OwnershipTree::new()))
fn ref_for(id: &CapabilityId) -> Reference;      // Reference { id: id.clone(), visibility: VisibilityScope::default() }
fn scope_all() -> Scope;                         // Scope { kind: None, namespace: None, visibility: VisibilityScope::default() }
```

**测试清单（测试名 → 精确断言）：**

1. `resolve_mints_a_fresh_lease_with_current_generation` — `host_with(&["a"])`；`resolve(ref_for(&id))` 成功且 `lease.owner_generation == OwnerGeneration(0)`（构造点 generation 即当前值，绝非 resolve-time `StaleOwner`）；`validate_lease(&lease) == Ok(())`。
2. `resolve_fails_closed_for_unknown_not_visible_revoked` — lookup miss → `Err(Unknown)`；`VisibilityScope{workspace: Some("w"), ..default()}` 无 binding → `Err(NotVisible)`；`tree.revoke(&id)` 后 → `Err(Revoked)`。
3. `validate_lease_detects_stale_owner_after_bump` — resolve 得 lease 后 `tree.bump(LifetimeScope::Runtime)`；`validate_lease(&lease) == Err(StaleOwner)`。
4. `validate_lease_detects_descriptor_freshness_after_replace` — resolve 得 lease 后 `registry.replace(改 schema 的 desc, ...)`；`validate_lease(&lease) == Err(Unknown)`（descriptor freshness，非 StaleOwner）。
5. `describe_and_resolve_share_one_registry_source` — `host_with(&["a","b"])`；`describe(scope_all())` → `committed_cursor == Cursor(2)`、`capabilities.len() == 2`、全部 `kind == CapabilityKind::Tool`。
6. `subscribe_returns_pure_stream_starting_at_snapshot` — `host_with(&["a"])`；`subscribe(scope_all(), Cursor(0))` → `committed_cursor == Cursor(1)`、`changes.is_empty()`。
7. `register_after_construction_is_visible_on_next_describe` — 构造后 `resolve(id_b) == Err(Unknown)`；`registry.register(fake_def("b"), ...)` 后 `describe(scope_all()).capabilities.len() == 2` 且 `resolve(id_b).is_ok()`。
8. `unregister_then_reregister_resolves` — resolve "a" ok；`registry.unregister("a").is_some()` 后 `resolve == Err(Unknown)`（unregister 不 revoke binding）；`registry.register(fake_def("a"), rev 1)` 后 `resolve.is_ok()`（旧未 revoke binding 使 resolve 再成功）。
9. `describe_filters_by_visibility_binding` — `host_with(&["a","b"])`；default visibility `describe.len() == 2`；`workspace` visibility 无 binding → `len() == 0`；`tree.register(id_b, ..., ws)` 后 `describe(ws).len() == 1` 且 `capabilities[0].id == id_b`、`capabilities[0].visibility == ws`；`resolve(Reference { id: id_b.clone(), visibility: ws.clone() })` 返回 lease 的 `descriptor.visibility == ws` 且 `validate_lease(&lease) == Ok(())`；`project(AcpJson, ws).capabilities == vec![id_b]`。
10. `subscribe_cursor_ahead_of_snapshot_fails_closed_to_snapshot` — `host_with(&["a"])`；`subscribe(scope_all(), Cursor(99))` → `committed_cursor == Cursor(1)`（fail-closed clamp 到 snapshot cursor，不伪造 99）、`changes.is_empty()`。

- [ ] **Step 2: 跑测试确认 RED**

```bash
cargo -p alephcore test --lib capability::zahir_facade
```
期望：编译失败（`zahir_facade` 模块不存在 / `impl Zahir` 不存在）。

- [ ] **Step 3: 创建 `zahir_facade.rs` + 注册 `mod.rs`（按 5.0 `snapshot_capabilities` 与 5.1 `zahir_facade.rs` 签名 + 算法步骤及 `mod.rs` 注册实现；测试按 Step 1 的测试清单）**

- [ ] **Step 4: 跑测试确认 GREEN**

```bash
cargo -p alephcore test --lib capability::zahir_facade
cargo -p alephcore check --all-targets
```
期望：本模块定义的 10 个测试全部 PASS（`zahir_facade` 是新模块，无既有测试），编译绿。

- [ ] **Step 5: Commit**

```bash
git add src/capability/zahir_facade.rs src/capability/mod.rs
git commit -m "capability(e): ZahirFacade concrete host + subscribe broadcast bridge"
```

---

## Task 6: Gate F — reducer fail-closed 保持 + 原子边界审计 + producer seam 报告

**Files:** Modify `src/capability/effect_claim.rs`（仅测试）；Create `.superpowers/sdd/2026-10-06-capability-phase4-follow-up-gates/task-6-producer-seam-report.md`

**事实锚点（审计依据，只读）:**
- `src/capability/effect_claim.rs:67` `reconcile_effect_claim(&[EffectClaimEvent]) -> EffectClaimReconciliation` 已实现 fail-closed：duplicate active claim / 非法迁移 / terminal 后再迁移 / identity 不一致 / 空 identity（0 generation、0 fence）→ `Unknown`。
- `src/session/service.rs:78` `SessionService::emit_batch` 默认 `Err("this SessionService cannot append atomically (only InProcessActorSessionService can)")`；生产唯一 override `InProcessActorSessionService::emit_batch`（`src/session/in_process.rs:293`）。
- `src/session/store.rs:79` `SessionEventStore::append_batch` 单事务；空 events 无 retire 拒绝（`src/session/store.rs:1231`）。

- [ ] **Step 1: 审计 producer 构造点（只读，grep 枚举，不臆造）**

```bash
grep -rn "EffectClaimPrepared\|EffectClaimClaimed\|EffectClaimInvoking\|EffectClaimTerminal" src/
```
期望：命中仅在 `src/session/events.rs`（wire 定义 :710-744 + 测试 :1102-1133）与 `src/capability/effect_claim.rs`（reducer 投影 + 测试）。**结论**：全仓无生产 `SessionEvent::EffectClaim*` 构造点，无 `EffectClaimProducer` 类型，无 dispatch fire-and-forget 发射路径。记录进报告，**不**在本 Task 造 producer。

- [ ] **Step 2: 审计 `emit_batch` / `append_batch` 原子边界（只读，不改 public API）**

确认三条 contract（记录进报告）：
1. claim + budget reservation + owner generation + fence 必须经**同一次** `emit_batch` 提交（禁拆两次 batch）。
2. `emit_batch` 默认 `Err`；只有 `InProcessActorSessionService` 原子 override。
3. `append_batch` 空 events 无 retire → `Err`（判据 §11 no-op 防御）；`(session_id, seq)` 主键保证重复 append 冲突失败（非幂等落盘）。

- [ ] **Step 3: 补一条 fail-closed 语义保持测试（在 `effect_claim.rs` 现有 `mod tests` 内追加）**

```rust
    #[test]
    fn duplicate_claim_and_illegal_migration_remain_fail_closed() {
        // Already-pinned semantics must not regress when producer integration
        // lands later. Duplicate active claim and terminal-then-migrate both
        // close to Unknown — never "ignore", never a second-idempotent result.
        assert_eq!(
            reconcile_effect_claim(&[
                event(EffectClaimState::Prepared),
                event(EffectClaimState::Claimed),
                event(EffectClaimState::Claimed),
            ])
            .terminal,
            EffectClaimState::Unknown
        );
        assert_eq!(
            reconcile_effect_claim(&[
                event(EffectClaimState::Prepared),
                event(EffectClaimState::Claimed),
                event(EffectClaimState::Invoking),
                event(EffectClaimState::Succeeded),
                event(EffectClaimState::Succeeded),
            ])
            .terminal,
            EffectClaimState::Unknown
        );
    }
```

- [ ] **Step 4: 跑测试确认 GREEN（保持语义，不新增语义）**

```bash
cargo -p alephcore test --lib capability::effect_claim
```
期望：现有 fail-closed 测试与新增 `duplicate_claim_and_illegal_migration_remain_fail_closed` 全 PASS。

- [ ] **Step 5: 写 producer seam 报告**

报告内容（`.superpowers/sdd/.../task-6-producer-seam-report.md`）必须覆盖：
- 现状：无 EffectClaim producer 构造点；reducer 已 fail-closed；`emit_batch`/`append_batch` 原子边界已存在且正确。
- **本期交付**：reducer fail-closed 保持测试 + 原子边界 contract audit（上面 Step 1–3）。
- **本期明确不交付**：真实 producer integration（dispatch fire-and-forget 发射 `EffectClaim*`、SessionId→owner 转换、fence 来源、`EffectClaimProducer` 类型）。
- **与 spec 关系**：spec §3.3 Gate F 的 producer 语义边界是「若实现」的前置设计，本 plan 只落地 reducer + atomic audit；真实 producer 需下一次独立 architecture approval。spec 的 F 验收（reducer 闭包 + atomic boundary）已覆盖，**不声称 F producer committed**。automatic replay / external exactly-once / ACP server gate（H/I）保持 deferred。
- **下期前置清单**：producer seam 审计与设计（枚举 scoped dispatch `src/tools/scoped/dispatch.rs` 与 MCP 面 `src/gateway/mcp_face/mod.rs` 的真实发射路径、确定 fence 来源与 SessionId 转换）作为独立 approval 输入。

- [ ] **Step 6: Commit（只 stage `src/capability/effect_claim.rs`）**

`.superpowers/` 被 `.gitignore:107` 忽略（`.superpowers/sdd/.gitignore` = `*`），`git add .superpowers/...` 会失败。报告写入 ignored ledger 文件并由 `progress.md` ledger 记录内容结论，**不作为 git commit**；commit 只 stage 产品代码：

```bash
git add src/capability/effect_claim.rs
git commit -m "test(capability): pin effect-claim fail-closed and audit producer seam"
```

---

## Task 7: 文档同步（E/F/G committed、H/I deferred）

**Files:** Modify `docs/reference/FEATURE_LOCATOR.md`（§3.5d）、`AGENTS.md`、`CLAUDE.md`（Capability Phase 4 入口段，Tier 1 同步）

- [ ] **Step 1: 更新 `FEATURE_LOCATOR.md` §3.5d**

精确引用真实路径：`src/capability/zahir_facade.rs`（`ZahirFacade`）、`src/capability/facade.rs`（`ResolveError`/`validate_lease`）、`src/capability/ownership.rs`（`generation()`）、`src/capability/backend.rs`（`snapshot_capabilities()`）、`src/tools/registry.rs`。E/G 标 committed；F 只标 **reducer fail-closed 保持测试 + `emit_batch`/`append_batch` 原子边界 audit committed**——**producer integration deferred**（真实 `EffectClaim*` 构造点本期不实现，下期独立 approval）。H/I（automatic Safe Replay、external-effect exactly-once、universal durable scheduler、完整 ACP server gate）仍 deferred。

- [ ] **Step 2: 同步 `AGENTS.md` 与 `CLAUDE.md`**

两处「Capability Phase 4 入口」段保持一致：E/G committed；F = reducer fail-closed + atomic audit committed、producer integration deferred；H/I deferred；`to_metadata_form` 仍是旧 compatibility wrapper。

- [ ] **Step 3: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md AGENTS.md CLAUDE.md
git commit -m "docs: sync capability phase 4 E/F/G and producer/H-I deferred"
```

---

## Task 8: 最终验证、低内存检查、格式核验与提交

**Files:** 无（仅命令）

- [ ] **Step 1: 低内存检查（free+inactive+speculative，阈值 4 GiB）**

```bash
FREE=$(vm_stat | awk '/Pages free/{print $3}' | tr -d '.')
INACTIVE=$(vm_stat | awk '/Pages inactive/{print $3}' | tr -d '.')
SPECULATIVE=$(vm_stat | awk '/Pages speculative/{print $3}' | tr -d '.')
MEM_KIB=$(( (FREE + INACTIVE + SPECULATIVE) * 4 ))
echo "MemKib: ${MEM_KIB}"
```
期望：`MEM_KIB >= 4194304`。低于阈值 → STOP/WAIT，不执行后续 cargo。

- [ ] **Step 2: 定向 check**

```bash
cargo -p alephcore check --all-targets
```

- [ ] **Step 3: clippy**

```bash
cargo -p alephcore clippy --all-targets -- -D warnings
```

- [ ] **Step 4: test**

```bash
cargo -p alephcore test --lib --no-fail-fast
```

- [ ] **Step 5: 格式核验（只对改动文件，区分 baseline drift）**

```bash
git diff --name-only HEAD | grep '\.rs$' | xargs -r rustfmt --edition 2021 --check
```
期望：本次改动文件 0 告警；若 `xargs` 因路径含空格出错，逐文件 `rustfmt --edition 2021 --check <file>`。既有无关文件的 drift 单独记录为 baseline，不顺手修。

- [ ] **Step 6: `git diff --check`**

```bash
git diff --check
```
期望：exit 0，无 whitespace error。

- [ ] **Step 7: 最终确认**

```bash
git status --short --branch
git log --oneline -10
```
期望：`git status` 干净（或仅剩本 plan 未跟踪文件之外无残留修改）；log 显示 Task 1–7 对应 commit 按序排列。

---

## What This Plan Does Not Do（Negative 边界）

- 不实现 automatic Safe Replay / external-effect exactly-once / universal durable scheduler / 完整 ACP server gate（H/I deferred）。
- **不实现真实 EffectClaim producer**（不臆造 `EffectClaimProducer`、dispatch fire-and-forget、SessionId 转换、fence 来源）；只做 reducer fail-closed 保持 + 原子边界 audit + seam 报告。真实 producer integration 需下一次独立 architecture approval。
- 不实现其余 `CapabilityKind` 的 concrete backend。
- 不复制/重定义 `ToolHandlerRegistry` / `RegistryChange` / `CapabilityChangeStream` / `Zahir` / `BackendLease`。
- 不发明 `BackendLease.fence` / `is_valid` / `invalidate` / `StaleLease` / `RegistryView` / `Zahir::list()` / `IntegrationGateDeferred` / batch-id durable set / duplicate "ignore"。
- 不删除/迁移 `to_metadata_form`（`src/tools/service.rs:453`）。
- 不新增三方依赖（sha2 复用 `Cargo.toml:113,261`）。
- 不修改 `src/harness/`（R10）、不动 UI（R2）、不动 platform-binding API（R1）。
- 不跑 `cargo --workspace` / 不跑全仓 `cargo fmt --check` / 不声称 `cargo fmt -p`。
- 不在 commit message 用 emoji / 中文。

---

## Self-Review（writing-plans 自检）

- **Spec 覆盖**：E1（resolve→Result）/E2（validate_lease 三道判据）/E3（ResolveError 闭集）/E4（BackendLease 两字段保持）/E5（generation accessor）/E6（删 generation 权威）/E7（visibility 字段）/E8（host 不变量 + reconcile 幂等）/E9（fingerprint 单源）→ Task 1–5；F（reducer fail-closed + atomic audit，producer integration deferred）→ Task 6；G（subscribe 桥接 + broadcast≠recovery + cursor fail-closed）→ Task 5；H/I deferred + 文档 → Task 7。
- **Step granularity**：每个 Task 有 RED（failing test）→ 实现 → GREEN → commit 的独立步骤；Task 5 分「signatures + algorithms」与「测试」两段，不呈现一份无法逐字编译的"完整文件"。
- **类型一致**：Task 5 的 `ZahirFacade` 引用 Task 4 的 `ResolveError`/`validate_lease`/`Scope.visibility`/`Reference.visibility`、Task 1 的 `OwnershipTree::generation()`、Task 2 的 `SchemaRef.fingerprint`、Task 5 新增的 `ToolBackendAdapter::snapshot_capabilities()`；`BackendLease.owner_generation` 全程保留。测试 helper 与 `backend.rs` 测试模块同形（`fake_def`/`FakeHandler`/`fake_handler` 在 `src/capability/backend.rs:128,138,155` 为 private，Task 5 在自身 `mod tests` 重定义同形 helper），引用真实定义（`ToolHandler` = `crate::tools::handlers::ToolHandler`、`ToolOutput` = `crate::session::events::ToolOutput`、`ToolSource` 经 `crate::tools::service` re-export）。
- **Review focus**：可见性过滤、cursor fail-closed、reconcile 幂等三项已进入 Review Focus；producer 边界措辞精确为「reducer/atomic audit committed、producer integration deferred、H/I deferred」。
