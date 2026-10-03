# Capability Tool Descriptor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将现有 `ToolHandlerRegistry` 收敛为 Tool Capability descriptor/handler 的唯一运行时事实源，并把注册、替换、注销、模型投影、MCP 投影和 Unsafe recovery 语义连成一条可验证主链。

**Architecture:** 新增轻量 `ToolCapabilityDescriptor`，由 `ToolHandlerRegistry` 与 handler 原子绑定并产生单调 revision；`ToolCatalog` 只保留 UI/routing/conflict/visibility 语义，模型/协议公共字段从 descriptor 投影，`LoopToolRegistry` 只作为执行索引。新增独立 `ToolRegistrationScope`，复用现有 `Disposer` 类型但不改造插件专用 `EffectScope`。本期 descriptor 暴露 `ReplayPolicy::{Unsafe, Safe}`，但 recovery 只执行默认 `Unsafe` 的现有未知结果路径；调用时 descriptor snapshot 与自动 Safe replay 是下一期，避免伪造尚未持久化的调用时版本。

**Tech Stack:** Rust, `serde`/`serde_json`, `ArcSwap`, Tokio broadcast, existing `ToolHandler`, `ToolCatalog`, `Disposer`, cargo check/clippy/test.

**Spec:** `docs/superpowers/specs/2026-10-03-capability-tool-descriptor-design.md`

## Global Constraints

- 不修改 `src/harness/`。
- 不改造 `src/capability/mod.rs` 的进程级 `CapabilitySlot<T>`。
- `ToolHandlerRegistry` 是唯一 callable descriptor/handler 事实源；不得新增平行 Tool Registry。
- `ToolCatalog` 保留其 UI、slash、routing、conflict、visibility 独有字段，但不再作为 callable metadata 真源。
- `LoopToolRegistry` 保留执行索引职责，不拥有 replay、owner、visibility 或全局发现事实。
- descriptor 默认 `ReplayPolicy::Unsafe`；MCP/ACP 外部能力不得仅凭来源推断 Safe。
- 本期 recovery 不自动重放任何未落盘结果；Safe replay 需要调用时 descriptor snapshot，列入下一期。
- 注册、替换和注销必须保持已有调用持有旧 handler 的稳定引用；新调用才解析新版本。
- 不引入新重型依赖或独立持久化存储。
- 所有新增/删除结构必须在提交说明或代码注释中说明旧结构是否删除、降级为投影/执行索引或继续保留的原因。

## Review Focus

1. **descriptor 与 handler 定义不一致**：注册必须拒绝 name/schema/source/安全标志不匹配，测试覆盖结构化错误。
2. **替换期间的并发快照**：旧 snapshot 和已开始调用继续使用旧 handler，新 resolve 使用新 handler，且 revision/change feed 不出现半注册状态。
3. **scope 泄漏与重复 dispose**：MCP server 重同步、断开和重复注销都必须最终移除同一 server 的工具，disposer 只能生效一次。
4. **有损投影**：`requires_approval`、MCP/Extension source、idempotent、concurrent-safe 和 max duration 不能在 descriptor → model/protocol 投影时静默丢失。
5. **recovery 边界**：当前 `ToolCallRequested` 没有调用时 descriptor revision，因此本期不能宣称 Safe replay；所有 dangling call 仍生成未知结果/验证提示，且不按工具名添加特判。

---

### Task 1: 建立 Tool Capability Descriptor 契约

**Files:**
- Create: `src/tools/descriptor.rs`
- Modify: `src/tools/mod.rs:30-110`（导出 descriptor 模块及类型）
- Modify: `src/tools/service.rs:145-183,216-225`（将 `ToolSource` 兼容重导出到 descriptor；保留现有服务接口）
- Test: `src/tools/descriptor.rs` 内 `#[cfg(test)]` 模块

**Interfaces:**
- Produces `pub enum ToolKind { Tool }`。
- Produces `pub enum ReplayPolicy { Unsafe, Safe }`，实现 `Default` 为 `Unsafe`，并派生 `Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq`。
- Produces `pub struct ToolCapabilityDescriptor`，字段固定为：
  `name: String`, `kind: ToolKind`, `schema_version: u32`, `description: String`, `input_schema: serde_json::Value`, `source: ToolSource`, `replay_policy: ReplayPolicy`, `requires_confirmation: bool`, `idempotent: bool`, `concurrent_safe: bool`, `max_duration_ms: Option<u64>`, `revision: u64`。
- Produces `ToolCapabilityDescriptor::from_definition(definition: &crate::tools::service::ToolDefinition, revision: u64) -> Self`。
- Produces `ToolCapabilityDescriptor::validate(&self) -> Result<(), DescriptorError>`：拒绝空名、超过现有 provider 名称限制的名字、非 object input schema、非正 `schema_version`/`revision`；不根据工具名推断 replay。
- Produces `ToolCapabilityDescriptor::matches_definition(&self, definition: &ToolDefinition) -> bool`，用于 registry 配对校验。
- `ToolSource` 的定义只保留一份；`src/tools/service.rs` 通过 `pub use crate::tools::descriptor::ToolSource` 保持现有调用方路径兼容。

- [ ] **Step 1: Write the failing tests**

在 `src/tools/descriptor.rs` 添加：

```rust
#[test]
fn default_replay_policy_is_unsafe() {
    assert_eq!(ReplayPolicy::default(), ReplayPolicy::Unsafe);
}

#[test]
fn descriptor_rejects_empty_name_and_non_object_schema() {
    let descriptor = descriptor_with("", serde_json::json!([]));
    assert!(descriptor.validate().is_err());
}

#[test]
fn descriptor_round_trip_preserves_source_and_safety_fields() {
    let descriptor = descriptor_with_source(ToolSource::Mcp { server_id: "srv".into() });
    let encoded = serde_json::to_string(&descriptor).unwrap();
    let decoded: ToolCapabilityDescriptor = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, descriptor);
}
```

并为 builtin、MCP source 和 `matches_definition` 添加最小断言。

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tools::descriptor -- --nocapture`
Expected: FAIL because `tools::descriptor` and its contract do not exist.

- [ ] **Step 3: Implement the descriptor contract**

实现字段、serde defaults 和 validation；`from_definition` 从现有 `ToolDefinition`/`ToolDefinitionMetadata` 复制字段，`replay_policy` 固定采用 `ReplayPolicy::Unsafe`，不要从 `idempotent` 或 MCP source 推断 Safe。移动 `ToolSource` 定义后只保留一个定义，并逐一修复 imports。

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tools::descriptor -- --nocapture`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/tools/descriptor.rs src/tools/mod.rs src/tools/service.rs
git diff --cached --check
git commit -m "feat: define tool capability descriptor contract"
```

---

### Task 2: 收敛 ToolHandlerRegistry 与 revision/change feed

**Files:**
- Modify: `src/tools/registry.rs:1-130`（保存 handler + descriptor，增加 revision、replace、descriptor snapshot、关闭状态）
- Modify: `src/tools/service.rs:1-80`（增加结构化 registry/descriptor 错误变体）
- Modify: `src/tools/handlers/builtin.rs:1-90`、`src/tools/handlers/mcp.rs:1-190`、`src/mcp/tool_bridge.rs:338-365`（迁移注册调用）
- Modify: `src/tools/handlers/registration.rs:68-180`（MCP 注册使用同一 descriptor）
- Modify: `src/tools/adapters/mcp_adapter.rs:35-85`（从 descriptor 取公共字段）
- Test: `src/tools/registry.rs` 内 registry tests

**Interfaces:**
- `RegistryEntry { handler: Arc<dyn ToolHandler>, descriptor: Arc<ToolCapabilityDescriptor> }` 为 registry 内部值。
- `RegistryChange` 改为携带 `revision: u64`、`name: String`、`source: ToolSource`，事件种类为 `Registered`, `Replaced`, `Unregistered`。
- `pub struct RegistrationHandle`：保存 registry weak handle、name 和注册 revision；`pub fn dispose(&self) -> bool` 幂等，只移除仍匹配该 revision 的 entry。
- `ToolHandlerRegistry::register(&self, descriptor: ToolCapabilityDescriptor, handler: Arc<dyn ToolHandler>) -> Result<RegistrationHandle, ToolError>`。
- `ToolHandlerRegistry::replace(&self, descriptor: ToolCapabilityDescriptor, handler: Arc<dyn ToolHandler>) -> Result<RegistrationHandle, ToolError>`。
- `ToolHandlerRegistry::resolve(&self, name: &str) -> Option<Arc<dyn ToolHandler>>`。
- `ToolHandlerRegistry::descriptor(&self, name: &str) -> Option<Arc<ToolCapabilityDescriptor>>`。
- `ToolHandlerRegistry::snapshot(&self) -> Arc<HashMap<String, Arc<dyn ToolHandler>>>` 保持兼容；新增 `descriptor_snapshot(&self) -> Arc<HashMap<String, Arc<ToolCapabilityDescriptor>>>`。
- `ToolHandlerRegistry::revision(&self) -> u64`、`ToolHandlerRegistry::close(&self)`、`ToolHandlerRegistry::is_closed(&self) -> bool`。
- `ToolError` 新增 `InvalidDescriptor`, `DescriptorMismatch`, `RegistryClosed`, `UnknownRevision`，所有错误包含可诊断 name/revision/reason。

- [ ] **Step 1: Write the failing tests**

扩展 `src/tools/registry.rs`：

```rust
#[test]
fn registration_assigns_monotonic_revision_and_emits_it() { /* assert 1 then 2 */ }

#[test]
fn replacement_keeps_old_handler_alive_and_new_resolve_uses_new_handler() { /* Arc snapshot */ }

#[test]
fn stale_registration_handle_cannot_remove_replacement() { /* generation guard */ }

#[test]
fn close_rejects_register_and_dispose_is_idempotent() { /* structured error */ }

#[test]
fn descriptor_handler_mismatch_is_rejected() { /* no state mutation */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tools::registry -- --nocapture`
Expected: FAIL to compile against the new APIs/types.

- [ ] **Step 3: Implement atomic registry state**

在 `ArcSwap` 的单次 RCU 更新中同时写入 entry 与 revision；revision 只在成功的 register/replace/unregister 变更上递增。`replace` 保留旧 handler 的 Arc 给已有调用，新的 snapshot 只包含新 entry。`RegistrationHandle::dispose` 使用 name+revision 条件删除，重复 dispose 返回 `false` 且不发送重复事件。`broadcast::RecvError::Lagged` 不在 registry 伪装成无变化，由下一任务的 snapshot 重建消费者处理。

- [ ] **Step 4: Migrate all production and test call sites**

将 `src/tools/handlers/registration.rs`、`src/mcp/tool_bridge.rs`、builtin/MCP handlers 和 registry tests 统一改为先生成 descriptor，再调用 `register`/`replace`；保留 `ToolCatalog` 的额外 routing/UI registration，但不再从其字段反推 callable descriptor。

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tools::registry tools::handlers::registration -- --nocapture`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add src/tools/registry.rs src/tools/service.rs src/tools/handlers src/tools/adapters/mcp_adapter.rs src/mcp/tool_bridge.rs
git diff --cached --check
git commit -m "feat: make tool registry descriptor-backed"
```

---

### Task 3: 添加独立 ToolRegistrationScope 并接通 MCP 生命周期

**Files:**
- Create: `src/tools/registration_scope.rs`
- Modify: `src/tools/mod.rs:30-110`（导出 scope）
- Modify: `src/mcp/tool_bridge.rs:73-235`（按 server 保存 scope，重同步先 dispose 再注册）
- Modify: `src/tools/handlers/registration.rs:68-180`（接受/返回 scope handles）
- Test: `src/tools/registration_scope.rs`、`src/mcp/tool_bridge.rs` tests

**Interfaces:**
- `pub struct ToolRegistrationScope`：拥有 owner label 和 `Vec<Disposer>`，不复用插件 `EffectScope` 的 `PluginId`/固定 step labels。
- `ToolRegistrationScope::new(owner: impl Into<String>) -> Self`。
- `ToolRegistrationScope::track(&mut self, handle: RegistrationHandle)`：将幂等 handle 包装为现有 `Disposer`，逆序执行。
- `ToolRegistrationScope::dispose(self) -> ToolDisposeReport`：继续执行全部 disposer，记录每个失败，不因一个失败中止；`ToolDisposeReport::all_ok`, `failures`, `owner` 与 `steps` 可供 bridge 日志和测试使用。
- `register_mcp_tools(..., scope: &mut ToolRegistrationScope, ...) -> Vec<String>`：每个成功注册的 handler 同时被 scope track；返回值仍为新注册 names。
- `unregister_mcp_tools` 保留为兼容/紧急清理入口，但优先调用 scope dispose，避免重复维护两条正常生命周期路径。

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn scope_disposes_registrations_in_reverse_order_and_only_once() { /* assert */ }

#[tokio::test]
async fn mcp_server_resync_drops_removed_tools_without_leaking_old_entries() { /* assert */ }

#[tokio::test]
async fn disposer_failure_does_not_skip_later_registration_cleanup() { /* assert */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tools::registration_scope mcp::tool_bridge -- --nocapture`
Expected: FAIL because the scope and bridge integration do not exist.

- [ ] **Step 3: Implement scope using existing Disposer**

只复用 `src/extension/effects/disposer.rs` 的 `Disposer`、`DisposeOutcome` 和构造 helper；不要改写或泛化 `src/extension/effects/scope.rs`。scope dispose 必须消费 self、逆序运行、记录错误和捕获 disposer panic，重复 handle 不得删除替换后的 registration。

- [ ] **Step 4: Wire MCP bridge ownership**

在 bridge task 中以 `HashMap<String, ToolRegistrationScope>` 按 `server_id` 管理工具。`sync_server` 对已有 server scope 先 dispose，再基于当前 `tools/list` 建新 scope；stop/crash/remove 只 dispose 对应 scope。capability builtins 使用独立 owner scope，不混入 plugin `EffectScope`。

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tools::registration_scope mcp::tool_bridge -- --nocapture`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add src/tools/registration_scope.rs src/tools/mod.rs src/mcp/tool_bridge.rs src/tools/handlers/registration.rs
git diff --cached --check
git commit -m "feat: scope tool registrations"
```

---

### Task 4: 从 descriptor 统一 model-visible 与既有协议/目录投影

**Files:**
- Modify: `src/tools/service.rs:216-390`（增加 `ToolDefinition::from_descriptor`，修复投影）
- Modify: `src/tools/runtime.rs:29-70,300-330`（LoopTool 仅消费 descriptor-derived definition）
- Modify: `src/tools/adapters/mcp_adapter.rs:35-100`（删除重复公共字段复制）
- Modify: `src/tools/handlers/registration.rs:120-160`（UnifiedTool 从 descriptor 派生公共字段）
- Modify: `src/tool_metadata/types/definition.rs:1-60`（保持现有 `ToolDefinition` wire shape；source/category 映射不增加重复事实字段）
- Modify: `src/tool_metadata/registry/mod.rs:70-190`（保留 UI/routing/conflict/visibility，避免其成为 callable metadata 真源）
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:1786-1810`（用原子 registry-entry snapshot join MCP handler 与 descriptor，避免热替换时跨 snapshot 拼接不同代）
- Test: `src/tools/service.rs` projection tests、`src/tools/adapters/mcp_adapter.rs` tests、`src/tools/handlers/registration.rs` tests

**Interfaces:**
- `ToolDefinition::from_descriptor(descriptor: &ToolCapabilityDescriptor) -> Self`。
- `ToolHandlerRegistry::entries_snapshot() -> Arc<HashMap<String, RegistryEntry>>` 返回同一原子状态中的 handler+descriptor 对。
- `ToolCapabilityDescriptor::to_metadata_definition(&self) -> crate::tool_metadata::ToolDefinition`。
- `ToolCapabilityDescriptor::to_metadata_definition(&self) -> crate::tool_metadata::ToolDefinition`。
- `ToolCapabilityDescriptor::to_unified_tool(&self, id: String) -> UnifiedTool`，只设置 descriptor 拥有的公共字段；UI/routing/conflict 字段仍由 `ToolCatalog` 自己管理。
- `LoopToolRegistry::tool_definitions()` 不重新计算 replay/source/approval 元数据；只读取 registry descriptor projection 或保留执行专用运行时字段。
- MCP run-loop join 必须消费 `entries_snapshot()`；不得把 `snapshot()` 与 `descriptor_snapshot()` 分开读取后按 name 配对。

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn descriptor_projection_preserves_mcp_source_confirmation_idempotence_and_budget() { /* assert */ }

#[test]
fn model_and_catalog_projections_share_descriptor_name_schema_and_description() { /* assert */ }

#[test]
fn loop_registry_remains_execution_index_not_second_descriptor_source() { /* assert */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib tools::service tool_metadata::registry tools::adapters::mcp_adapter -- --nocapture`
Expected: FAIL on the currently lossy `to_metadata_form` and duplicated MCP metadata paths.

- [ ] **Step 3: Implement descriptor projections and narrow deletion**

删除 `to_metadata_form` 中将 MCP/Extension 强行变成 Builtin、丢弃 `requires_approval` 的逻辑；将公共字段改由 descriptor conversion 提供。不要删除 `ToolCatalog` 的 slash/UI/routing/conflict/visibility 字段，也不要删除 LoopTool 的执行 budget/concurrency adapter；这些是消费者职责。

- [ ] **Step 4: Verify existing MCP list/command behavior**

运行 MCP registration tests，确认 schema quarantine、annotation flags、health probe、slash resolution 和 unregister 行为不变；新增断言确认所有这些入口使用相同 name/schema/description/source/safety projection。

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib tools::service tool_metadata::registry tools::adapters::mcp_adapter tools::handlers::registration -- --nocapture`
Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add src/tools/service.rs src/tools/runtime.rs src/tools/adapters/mcp_adapter.rs src/tools/handlers/registration.rs src/tool_metadata/types/definition.rs src/tool_metadata/registry
 git diff --cached --check
git commit -m "refactor: project tool metadata from descriptors"
```

---

### Task 5: 固化 Unsafe recovery 接缝并记录 Safe replay 边界

**Files:**
- Modify: `src/session/boundary_repair.rs:70-220`（从 descriptor policy 读取但默认 Unsafe；不新增自动重放）
- Modify: `src/session/mod.rs:20-45`（导出最小 policy lookup 接口，如需要）
- Modify: `src/tools/registry.rs`（提供只读 `ReplayPolicy` lookup）
- Test: `src/session/boundary_repair.rs` tests、`src/session/reduction.rs` existing dangling tests
- Modify: `docs/superpowers/specs/2026-10-03-capability-tool-descriptor-design.md:4,8,10`（将“本期 Safe 自动重放”明确改为下一期；保留双 descriptor 条件作为 future contract）

**Interfaces:**
- `pub trait ReplayPolicyLookup { fn replay_policy(&self, name: &str) -> Option<ReplayPolicy>; }`，registry 实现只按 descriptor lookup，不按 name 特判。
- `boundary_repair_text`/`repairs_for` 可接受 `Option<&dyn ReplayPolicyLookup>`，缺失 lookup 或缺失 descriptor 一律走现有未知结果/验证提示。
- 本期不修改 `SessionEvent::ToolCallRequested`；不伪造 call-time revision，不修改 `src/harness/`。

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn missing_descriptor_keeps_unknown_outcome_repair() { /* exact existing wording */ }

#[test]
fn unsafe_descriptor_never_auto_replays_a_dangling_call() { /* assert verification prompt */ }

#[test]
fn replay_lookup_is_descriptor_based_not_tool_name_special_case() { /* same policy behavior for arbitrary names */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib session::boundary_repair -- --nocapture`
Expected: new lookup-aware tests fail to compile before the seam exists; existing tests provide the baseline.

- [ ] **Step 3: Implement the Unsafe-only seam**

让 recovery 读取 descriptor 的 `ReplayPolicy`，但 `Unsafe`、`Safe`、descriptor 缺失或 registry 不可用都维持当前“未知结果 + Verify current state”路径。不要基于工具名、MCP source、idempotent 或 read-only 标志绕过这个路径。同步更新 spec 的本期范围：调用时 descriptor revision、Safe 双重检查和自动重放留到后续 session-event 迁移。

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib session::boundary_repair session::reduction -- --nocapture`
Expected: PASS；既有 denied/parked precedence 不变。

- [ ] **Step 5: Commit**

```bash
git add src/session/boundary_repair.rs src/session/mod.rs src/tools/registry.rs docs/superpowers/specs/2026-10-03-capability-tool-descriptor-design.md
git diff --cached --check
git commit -m "feat: route unsafe recovery through tool descriptors"
```

---

### Task 6: 更新 FEATURE_LOCATOR 与架构文档，清点旧结构处理

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md:20-190`（速查索引新增 Tool Capability Descriptor）
- Modify: `docs/reference/FEATURE_LOCATOR.md:1010-1060`（Tool Registry 章节补充 descriptor 真源、projection 和 scope）
- Modify: `docs/reference/FEATURE_LOCATOR.md:1147-1180`（MCP 章节标记 MCP tool projection 与 scope 边界）
- Modify: `docs/reference/FEATURE_LOCATOR.md:4670-4740`（附录 A 增加本期完成/未完成体检）
- Modify: `docs/reference/ARCHITECTURE.md`（仅在现有章节有明确归属时补充 Tool descriptor 数据流）
- Test: 文档路径/锚点 grep 审核脚本或手工命令

**Interfaces:**
- 文档必须指向真实实现：`src/tools/descriptor.rs`、`src/tools/registry.rs`、`src/tools/registration_scope.rs`、`src/tools/service.rs`、`src/mcp/tool_bridge.rs`、`src/session/boundary_repair.rs`。
- 必须明确：本期纳入 Tool；Skill/Agent/Plugin/MCP/ACP 不宣称已经统一成 descriptor registry；`src/capability/mod.rs` 仍是 process-global boot slot；Safe replay 需要下一期调用时 snapshot。

- [ ] **Step 1: Add the feature locator entries**

在速查表、Tool Registry 章节、MCP 章节和附录 A 各增加一条互相链接的记录，写清事实源、投影消费者、scope disposer、默认 Unsafe 和本期未完成项。

- [ ] **Step 2: Validate anchors and implementation paths**

Run:

```bash
rg -n "Tool Capability Descriptor|ToolRegistrationScope|ReplayPolicy|descriptor.rs|registration_scope.rs|boundary_repair.rs" docs/reference/FEATURE_LOCATOR.md docs/reference/ARCHITECTURE.md
```

Expected: 每个文档声明都能映射到实际路径，不能出现“全仓 capability 已统一”的表述。

- [ ] **Step 3: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md docs/reference/ARCHITECTURE.md
git diff --cached --check
git commit -m "docs: document tool capability descriptor boundaries"
```

---

### Task 7: 全量门禁、差异清理与最终提交

**Files:**
- Review all changed files; no new product file is expected beyond tasks above.
- Verify `src/harness/` remains unchanged.
- Verify old duplicate callable metadata is removed or explicitly execution/UI-only.

**Interfaces:**
- No new API; this task is the merge gate.

- [ ] **Step 1: Verify worktree and scope**

Run:

```bash
git status --short
git diff --name-only ba786d5db...HEAD
 git diff --name-only ba786d5db...HEAD -- src/harness
```

Expected: only planned files are changed; the final command prints nothing.

- [ ] **Step 2: Run compiler and lint gates**

Run in order:

```bash
CARGO_BUILD_JOBS=2 cargo check -p alephcore --message-format short --all-targets
CARGO_BUILD_JOBS=2 cargo clippy -p alephcore --all-targets -- -D warnings
CARGO_BUILD_JOBS=2 cargo test -p alephcore --lib
```

Expected: all commands exit 0. If a compile error repeats twice, stop and report the exact short diagnostic rather than adding speculative compatibility layers.

- [ ] **Step 3: Run targeted behavior gates**

```bash
cargo test -p alephcore --lib tools::descriptor tools::registry tools::registration_scope mcp::tool_bridge session::boundary_repair -- --nocapture
```

Expected: descriptor, revision, replacement, scope disposal, MCP resync and Unsafe recovery tests all pass.

- [ ] **Step 4: Review deletions and documentation accuracy**

确认 `to_metadata_form` 的有损 source/approval 映射已删除或被 descriptor conversion 取代；确认 ToolCatalog 和 LoopToolRegistry 的保留理由写在代码/文档中；确认没有 `#[allow]`、新增生产 `unwrap()`、跳过测试或对 `src/harness/` 的修改。

- [ ] **Step 5: Commit final integration**

```bash
git add -A
git status --short
git diff --cached --check
git commit -m "refactor: establish tool capability descriptor mainline"
git status --short --branch
```

Expected: commit succeeds and worktree is clean.

## Deferred Follow-up: Safe Replay

下一期必须先设计并测试持久化调用时 descriptor snapshot/revision，再实现 `ReplayPolicy::Safe`：

1. 扩展 `SessionEvent::ToolCallRequested` 记录调用时 descriptor revision/schema version。
2. 在非 `src/harness/` 的 dispatch/logging seam 写入该字段，保持 harness 零改动约束或先单独审批边界变化。
3. recovery 同时验证调用时 descriptor 与当前 descriptor 都是 Safe 且 revision/schema 兼容。
4. descriptor 缺失、被替换、旧日志无字段时一律降级未知结果，不自动重放。
5. 增加 crash/recovery integration tests 后，才把 Safe 从“声明字段”提升为真实行为。

## Self-Review

- **Spec coverage:** Task 1 covers descriptor contract; Task 2 covers registry/revision/replacement/errors; Task 3 covers scoped disposal; Task 4 covers model/catalog/LoopTool projection; Task 5 covers the approved first-phase Unsafe recovery seam and explicitly defers the unimplementable call-time Safe contract; Task 6 covers FEATURE_LOCATOR/architecture documentation; Task 7 covers compiler, lint, tests and clean diff.
- **Step scan:** Every task has concrete files, named interfaces, failing tests, verification commands and a commit boundary. No step uses an undefined “appropriate handling” instruction.
- **Type consistency:** `ToolCapabilityDescriptor`, `ReplayPolicy`, `RegistrationHandle`, `ToolRegistrationScope`, `ReplayPolicyLookup` are introduced before their consumers; `ToolSource` has one owner and a compatibility re-export.
- **Review focus coverage:** Descriptor mismatch (Task 2), concurrent replacement (Task 2), scope leakage (Task 3), lossy projection (Task 4), and missing call-time replay snapshot (Task 5/deferred follow-up) each have explicit tests or a documented non-implementation boundary.
- **Proportion:** The plan is intentionally detailed because it changes a cross-module Rust boundary; it does not prescribe function bodies beyond the contracts needed to prevent independent incompatible implementations.

**Known limitation:** 本期没有实现 Safe 自动重放，也没有修改 `SessionEvent::ToolCallRequested`；因此“Safe 双 descriptor 检查”仍是下一期目标，不应在本期提交或文档中宣称已经完成。
