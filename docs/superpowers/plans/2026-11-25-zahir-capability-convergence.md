# Zahir Capability 收敛 实施计划 / Zahir Capability Convergence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 删除 `src/capability` 中重复或未挂载的权威，让依赖不可用的工具调用返回结构化「暂时不可用」答案，并让单插件重载在新版本无效时保留旧版本。/ Remove duplicate or unmounted capability authorities, return a structured "temporarily unavailable" answer when a tool's dependency is down, and keep the old plugin version when a reload's new version is invalid.

**Architecture:** 四段提交在同一分支：C 收敛（纯删除 + `PolicyAttachment` 内一个 `AtomicBool`），D 在 `ZahirFacade::resolve_resource` 读 `ProviderRegistry` 状态并新增一个 `ResolveError` 变体，S 把 `reload_plugin` 改为「先验后拆」，F 更新文档。不新建 registry、trait 或 supervisor。/ Four commits on one branch; no new registry, trait or supervisor.

**Tech Stack:** Rust（tokio、serde、thiserror），crate `alephcore`，extism WASM 运行时。

**Spec:** `docs/superpowers/specs/2026-11-25-zahir-capability-convergence-design.md`（已批准；段 S 的修正见下文 §S0，待用户确认）

## Global Constraints

- 不改 `src/harness/`（R10 棘轮）。/ Do not touch `src/harness/`.
- 不新增依赖；不新建 registry / trait / supervisor（P6、R3、反目标）。
- 会话中工具列表与 schema 不变（研究文档 §10）。
- 代码注释英文；文档中英双语；回执中文。
- 模型可见文本固定为：``memory.runtime` 当前不可用（状态 Stopped），稍后重试或请用户启用``（状态名随实际 `ProviderState` 的 `Debug` 输出变化）。
- 事件标签固定为 `dependency_unavailable`。
- 被删测试若仍保护生产行为，迁到生产模块的测试里，不随文件删除（设计 §7）。
- 每段提交前：`cargo check --workspace`、`cargo clippy -p alephcore --all-targets -- -D warnings`、受影响模块 `cargo test -p alephcore --lib <module>`。
- 只在 worktree `.worktrees/zahir-convergence`（分支 `zahir-capability-convergence`）工作；不动 main，不推送。

## Review Focus

1. 依赖在 resolve 与 admit 之间停止（竞态）：调用必须 fail-closed，不得执行；`admit_memory` 路径维持现有 `canonical memory admission is stale` 拒绝 —— Task 5 加竞态测试。
2. 终端会话的 `NotVisible → Unknown` 折叠不得吞掉 `DependencyUnavailable`，也不得把它变成可枚举会话的侧信道 —— Task 4 测试。
3. 删除 `activation.rs` 后，已撤销的 policy attachment 仍以 `runtime_lifecycle` 拒绝，且撤销幂等 —— Task 3 测试。
4. 插件重载时新 manifest 无效或 WASM 无法编译：旧版本工具仍可调用、插件行状态不变成 Error —— Task 7 测试。
5. 重载预检通过但新版本挂载在拆除旧版后失败：行为与今天一致（插件不可用、`core_panel_lifecycle_failed_reload_never_serves_old_publication` 保持绿）—— Task 7 不改此路径，回归测试保持。

---

## 段 C：收敛 / Convergence

### Task 1: 删除 Legacy handler registry 与 ambient

**Files:**
- Modify: `src/capability/backend.rs:21,33-38,57-270,478-576`（删 `special_backend` 别名、同名重导出、`Legacy*` 类型与 `impl CapabilityBackend`、legacy 测试；保留 `ToolBackendAdapter` 及 `:612` 起的工具测试）
- Modify: `src/capability/special.rs:193,943,962`（删 `SpecialRegistration::legacy`、`register_legacy`、`dispatch_legacy`）
- Delete: `src/capability/ambient.rs`
- Modify: `src/capability/mod.rs`（去掉 `pub mod ambient`）
- Modify: `src/capability/events_tests.rs:3-5,450-…`（`handler_invocation_success_and_failure_are_audited_without_payloads`）

**Interfaces:**
- Produces: `crate::capability::special::CapabilityHandlerRegistry` 为唯一 handler registry；`crate::capability::backend` 不再导出 `CapabilityHandlerRegistry` / `CapabilityHandler` / `CapabilityDispatchError`。

- [ ] **Step 1: 迁移仍保护生产行为的测试** —— `handler_invocation_success_and_failure_are_audited_without_payloads` 改用 `special::CapabilityHandlerRegistry` 经生产 `dispatch_admitted`（`special.rs:1013`）发出 `HandlerResult::Success` / `HandlerResult::Failure` 事件，断言事件记录不含 handler payload 字符串。
- [ ] **Step 2: 运行该测试确认在删除前为绿** —— `cargo test -p alephcore --lib capability::events_tests::handler_invocation` → PASS。
- [ ] **Step 3: 删除上列代码与文件。**
- [ ] **Step 4: 验证** —— `cargo check --workspace` 无错误；`rg -n 'LegacyCapability|register_legacy|dispatch_legacy|capability::ambient' src` 无输出；`cargo test -p alephcore --lib capability::` PASS。
- [ ] **Step 5: 提交** —— `git commit -m "refactor(capability): remove legacy handler registry and unmounted ambient"`

### Task 2: 删除 audit 与 grant

**Files:**
- Delete: `src/capability/audit.rs`, `src/capability/grant.rs`, `src/capability/grant_integration_tests.rs`
- Modify: `src/capability/mod.rs`，`src/capability/projection_host.rs:858`（删 `audit_collector`）
- Modify: `src/capability/events_tests.rs:641-692`（删 audit 模块）
- Modify: `src/capability/events_scoped_tests.rs:184,305,500,525`

**Interfaces:**
- Consumes: `InMemoryEventStream::snapshot()`、`EventFilter`、`TrustedEventScope::admits`（均保留）。

- [ ] **Step 1: 迁移 scoped 事件测试** —— 四处 `AuditCollector::query_scoped(...)` 改为 `stream.snapshot()` → 按原 `EventFilter` 过滤 → `scope.admits(record)`；断言与原测试相同的可见/不可见记录集合。
- [ ] **Step 2: 运行** —— `cargo test -p alephcore --lib capability::events_scoped_tests` → PASS。
- [ ] **Step 3: 删除上列文件与函数**；`src/capability/policy.rs:365` 的跨会话授权推迟注释保留，指向设计 §9。
- [ ] **Step 4: 验证** —— `rg -n 'AuditCollector|GrantRegistry|capability::(audit|grant)' src` 无输出；`cargo test -p alephcore --lib capability::` PASS。
- [ ] **Step 5: 提交** —— `git commit -m "refactor(capability): remove unmounted audit and grant authorities"`

### Task 3: 以 `revoked` 标志取代 lifecycle/activation 状态机

**Files:**
- Modify: `src/capability/policy_runtime.rs:129-200`（`AttachmentState`）
- Delete: `src/capability/lifecycle.rs`, `src/capability/activation.rs`
- Modify: `src/capability/events.rs:20-30,44-…,918-996`（删 `ActivationEventHook`、`CapabilityEvent::CapabilityAttached` / `CapabilityDetaching`、`CapabilityEventType::Attached` / `Detaching`）
- Modify: `src/tools/scoped/mod.rs:121-125`, `src/tools/scoped/builder.rs:33-34,89-111`, `src/tools/scoped/dispatch.rs:~1531-1560`（删兼容激活入口 `capability_activations`、`capability_constraint_context`、`with_capability_activation`、`with_capability_constraint_context`、`validate_compatibility_activation`）
- Modify: `src/tools/scoped/tests.rs:5094-5270`, `src/tools/scoped/memory_runtime_tests.rs:203,231,254`, `src/capability/events_tests.rs:2,300-380`
- Modify: `src/capability/ownership.rs:5`（过时注释改为描述现有 `OwnershipTree`）

**Interfaces:**
- Produces: `AttachmentState { binding, fact_key, facts, revoked: AtomicBool, active_calls }`；`AttachmentState::revoke(&self)` 幂等；`admit` 在 `revoked` 时返回 `denied("runtime_lifecycle")`。保留 `admit` 中已有的 generation 比较（若有）。

- [ ] **Step 1: 写失败前置测试**（在 `src/capability/policy_runtime.rs` 测试模块）：
  ```rust
  #[test]
  fn revoked_attachment_denies_with_runtime_lifecycle_and_revoke_is_idempotent() {
      // build an attachment through the existing test helper, revoke twice
      // assert: second revoke does not panic; admit(...) == Err(denied("runtime_lifecycle"))
  }
  ```
  另迁移 `runtime_constraint_violation_is_audited`：改为经 `PolicyAttachment` 生产路径（`policy_runtime.rs:519`）触发 `CapabilityEvent::ConstraintViolation { constraint_type == "resource" }`。
- [ ] **Step 2: 在旧实现上运行，两测试为绿**（行为锁定）—— `cargo test -p alephcore --lib capability::policy_runtime` → PASS。
- [ ] **Step 3: 实现替换并删除上列代码**；删除 `memory_runtime_tests.rs` / `tests.rs` 中只验证兼容入口的测试。
- [ ] **Step 4: 验证** —— `rg -n 'CapabilityActivation|LifecycleState|ActivationEventHook|CapabilityAttached|CapabilityDetaching|capability_activations' src` 无输出；`cargo test -p alephcore --lib capability:: tools::scoped::` PASS；census 守卫 `cargo test -p alephcore --lib capability::census` PASS。
- [ ] **Step 5: 段 C 闸门** —— `cargo check --workspace`、clippy 全绿；`git diff --shortstat a4dac8db4 -- src` 记录净删除行数（预期约 −3.4K）写入提交信息。
- [ ] **Step 6: 提交** —— `git commit -m "refactor(capability): replace lifecycle/activation state machine with revoked flag"`

## 段 D：依赖不可用 / Dependency availability

### Task 4: `ResolveError::DependencyUnavailable` 与资源判定

**Files:**
- Modify: `src/capability/facade.rs:164`（`ResolveError`）
- Modify: `src/capability/zahir_facade.rs:87`（`terminal_denial`），`:1468-1474`（事件标签），`:1671-1687`（`resolve_resource`）
- Modify: 编译器报出的其余 `match ResolveError`（`src/gateway/capability_ui.rs:546-550`、`src/capability/facade.rs:610`、`src/tools/registry.rs:1804-1849`）
- Test: `src/capability/zahir_facade_memory_tests.rs`

**Interfaces:**
- Produces: `ResolveError::DependencyUnavailable { target: CapabilityId, state: ProviderState }`（`ProviderState` 来自 `crate::capability::provider`）。
- `resolve_resource`：`self.provider_adapter.entry(&reference.id)` 不存在 → `Unknown`；`entry.provider.state() != ProviderState::Ready` → `DependencyUnavailable { target: reference.id.clone(), state }`；`Ready` → 现有 memory 捕获（`MemoryRuntime::capture` 的 `Running` 检查保留为二次栅栏）。

- [ ] **Step 1: 写失败测试**（复用 `memory_runtime_restart_captures_new_generation_and_fences_old_lease` 的夹具）：
  ```rust
  #[tokio::test]
  async fn tool_resolve_reports_dependency_unavailable_while_memory_is_cold() {
      // runtime never started
      assert_eq!(facade.resolve(reference).unwrap_err(),
          ResolveError::DependencyUnavailable { target: crate::memory::runtime::resource_id(), state: ProviderState::Cold });
  }
  #[tokio::test]
  async fn tool_resolve_reports_dependency_unavailable_after_memory_stops() {
      // start, stop → state: ProviderState::Stopped
  }
  #[test]
  fn terminal_denial_keeps_dependency_unavailable() {
      let error = ResolveError::DependencyUnavailable { target, state: ProviderState::Stopped };
      assert_eq!(terminal_denial(error.clone()), error);
  }
  ```
- [ ] **Step 2: 运行确认失败** —— `cargo test -p alephcore --lib capability::zahir_facade_memory_tests` → 编译失败（变体不存在）。
- [ ] **Step 3: 实现** 变体、`resolve_resource` 判定、事件标签 `ResolveError::DependencyUnavailable { .. } => "dependency_unavailable"`，并逐个处理其余 match（不得落入 `_ =>` 通配被折叠成 `source_unavailable`）。
- [ ] **Step 4: 运行** —— 上述测试与原 `memory_runtime_restart_*` PASS；`cargo test -p alephcore --lib capability::` PASS。
- [ ] **Step 5: 提交**（与 Task 5 合并为段 D 一次提交亦可）。

### Task 5: 工具调用路径的模型可见文本

**Files:**
- Modify: `src/tools/scoped/dispatch.rs:473`（`resolve_with_capture_for_call` 的 `map_err`）
- Test: `src/tools/scoped/memory_runtime_tests.rs`

**Interfaces:**
- Consumes: Task 4 的 `ResolveError::DependencyUnavailable { target, state }`。
- 映射：`DependencyUnavailable` → `ToolError::Execution { name, cause: format!("`{}` 当前不可用（状态 {:?}），稍后重试或请用户启用", target.name, state) }`；其他变体维持现有 `PermissionDenied { reason: "canonical capability lease cannot be resolved: {error:?}" }`。`:1253` 的 `admit_memory` 映射不变（竞态由二次栅栏 fail-closed）。

- [ ] **Step 1: 写失败测试**：
  ```rust
  #[tokio::test]
  async fn memory_tool_call_while_runtime_stopped_returns_dependency_unavailable_text() {
      // scoped service with canonical facade + memory runtime started then stopped
      let error = service.call("memory_fixture", json!({})).await.unwrap_err();
      assert_eq!(error, ToolError::Execution { name: "memory_fixture".into(),
          cause: "`memory.runtime` 当前不可用（状态 Stopped），稍后重试或请用户启用".into() });
  }
  #[tokio::test]
  async fn memory_stopping_between_resolve_and_admit_fails_closed() {
      // stop runtime after resolve, before admission → Err(PermissionDenied) and handler not invoked
  }
  ```
- [ ] **Step 2: 运行确认第一个失败** —— `cargo test -p alephcore --lib tools::scoped::memory_runtime_tests` → FAIL（得到 PermissionDenied）。
- [ ] **Step 3: 实现映射。**
- [ ] **Step 4: 运行** —— 两测试 PASS；`cargo test -p alephcore --lib tools::scoped::` PASS；工具列表快照测试未变。
- [ ] **Step 5: 段 D 闸门与提交** —— check/clippy 全绿；`git commit -m "feat(capability): structured DependencyUnavailable for unavailable tool dependencies"`

## 段 S：插件重载「先验后拆」/ Preflight-before-teardown reload（可单独回退）

### S0：相对设计 §6.3 的修正（待用户确认）

源码复核（`src/extension/lifecycle.rs:290-305,907-1228`，`src/extension/loader.rs:200-248,622-650`）表明设计 §6.3 的双实例提交不成立：

- hooks 与 panel 不是按插件替换，而是 `after_transition` 从 registry 整体重建；`HookRegistry::replace_users`（`hooks/registry.rs:542`）只替换用户 hooks。
- `PluginLoader::load_plugin` 与 `WasmRuntime`、`runtime_owners`、`scopes` 均以插件 id 为键；`load_plugin` 遇到已加载 id 直接跳过（`loader.rs:201-203`）。
- `mount_parsed` 会启动 MCP 服务器与 services（外部进程），新旧两份同时运行会冲突。

要实现设计原文需要重设计 loader/WASM/scope 的键与 MCP/service 的双开语义，远超本期。修正方案：**把所有可失败且无副作用的步骤移到拆除旧版之前**。新版本 manifest 无效、身份不符、被策略拒绝或 WASM 编译失败时，旧版本原样服务（G3 的主体）；预检通过后才走现有 unmount → mount。拆除后的副作用步骤失败仍与今天一致。已准入调用在旧实现上跑完由现有 `finish`（`lifecycle.rs:1613`，`acquire_many_owned(MAX_IN_FLIGHT)`）承担，`with_admitted_completion` 仍无消费者，留在 §9 推迟项。双实例切换写入 §9，触发条件：出现无法预检的高频重载失败。

### Task 6: 拆分 `mount_inner` 为准备与提交

**Files:**
- Modify: `src/extension/lifecycle.rs:907-1010`

**Interfaces:**
- Produces: `struct PreparedMount { output: AdapterOutput, record: PluginRecord, manifest: Option<PluginManifest> }`；`async fn prepare_mount(&self, id: &str) -> Result<PreparedMount, MountError>`（原 `:925-985`：定位目录、admit、解析、身份检查、再次 admit、`build_record`；错误时写行状态的现有行为保留）；`async fn commit_mount(&self, prepared: PreparedMount) -> Result<PluginStatus, MountError>`（`eligible` → `declare_cold`，否则 `mount_parsed`）。`mount_inner` = 已挂载检查 + `prepare_mount` + `commit_mount`。

- [ ] **Step 1: 运行现有挂载/重载测试作为基线** —— `cargo test -p alephcore --lib extension::lifecycle` → PASS。
- [ ] **Step 2: 重构（纯移动，不改行为）。**
- [ ] **Step 3: 再次运行同一命令** → PASS。

### Task 7: `reload_plugin` 先验后拆

**Files:**
- Modify: `src/extension/lifecycle.rs:290-305`
- Modify: `src/extension/loader.rs`（新增预检函数）
- Test: `src/extension/lifecycle.rs` 测试模块（与 `reload_plugin_is_unmount_then_mount_and_reload_rebuilds_everything` 同处）

**Interfaces:**
- Consumes: Task 6 的 `prepare_mount` / `commit_mount`。
- Produces: `pub(crate) fn preflight_wasm_module(manifest: &PluginManifest, settings: &serde_json::Value) -> ExtensionResult<()>`（在一次性 `WasmRuntime` 中 `load_plugin_with` 后丢弃，不写入任何 loader 状态）。
- `reload_plugin`：`prepare_mount(id)`（manifest 解析必须绕过 `parse_manifest_from_dir_cached_global` 的缓存或使其失效，确保读到新内容）→ WASM 插件再 `preflight_wasm_module` → 任一失败：返回原 `MountError`，`reason` 追加 ` (previous version is still running)`，插件行状态不写 Error、不调用 `unmount_inner` → 通过后 `unmount_inner` → `commit_mount(prepared)`。

- [ ] **Step 1: 写失败测试**：
  ```rust
  #[tokio::test]
  async fn reload_with_invalid_manifest_keeps_previous_version_serving() {
      // mount plugin v1, overwrite manifest with invalid TOML/JSON, reload_plugin(id)
      // assert: Err(MountError::Parse { reason }) with reason.ends_with("(previous version is still running)")
      // assert: v1 tool still listed and callable; row status still Loaded
  }
  #[tokio::test]
  async fn reload_with_uncompilable_wasm_keeps_previous_version_serving() {
      // replace .wasm with garbage bytes; same assertions
  }
  ```
- [ ] **Step 2: 运行确认失败** —— `cargo test -p alephcore --lib extension::lifecycle::tests::reload_with` → FAIL（插件已被拆除）。
- [ ] **Step 3: 实现。**
- [ ] **Step 4: 运行** —— 新测试 PASS；回归 `reload_plugin_is_unmount_then_mount_and_reload_rebuilds_everything`、`stage_e_wasm_reload_drops_old_instance_once_before_new_instance_can_drop`、`b4_production_canonical_reload_rejects_admitted_a_before_guest_and_runs_b`、`core_panel_lifecycle_failed_reload_never_serves_old_publication`、`g2_reload_is_a_fixed_point_of_the_surfaces` PASS；`cargo test -p alephcore --lib extension::` PASS。
- [ ] **Step 5: 段 S 闸门与提交** —— check/clippy 全绿；同时把 §S0 修正写回设计文档 §5（第 3 条）与 §9；`git commit -m "feat(extension): preflight plugin reload before tearing down the running version"`

## 段 F：文档 / Docs

### Task 8: FEATURE_LOCATOR、docs/reference 与回执

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（「当前状态」:22-58 更新到本期；登记 Phase 6–8 模块现状与被删模块的 git 找回方式 `git show a4dac8db4:src/capability/<file>`；§3.5 闭环追加本期条目）
- Modify: `docs/reference/{ARCHITECTURE,TOOL_SYSTEM,PLUGIN_SYSTEM,GLOSSARY}.md` 中提到被删模块、激活、插件重载的段落
- Modify: `AGENTS.md` 与 `CLAUDE.md` 的「Capability Phase 4 入口」若引用被删类型则同步（两份保持一致）
- Create: `docs/superpowers/runs/2026-11-25-zahir-convergence-receipt.md`（各段提交、净删除行数、测试命令与结果、未做事项）

- [ ] **Step 1: 列出引用** —— `rg -n 'LegacyCapability|ambient|AuditCollector|GrantRegistry|CapabilityActivation|LifecycleState|reload_plugin' docs/reference AGENTS.md CLAUDE.md`，逐条按代码校对。
- [ ] **Step 2: 编辑文档**；证据层（`docs/superpowers/` 等）旧记录不改写。
- [ ] **Step 3: 验证** —— 文档中引用的每个 `path:line` 用 `rg`/`sed -n` 抽查存在；`rg` 对被删类型在 `docs/reference` 中只出现在「已删除」说明里。
- [ ] **Step 4: 提交** —— `git commit -m "docs: align FEATURE_LOCATOR and reference docs with Zahir convergence"`

### Task 9: 全分支验证与独立审阅

- [ ] **Step 1:** `cargo check --workspace`、`cargo clippy -p alephcore --all-targets -- -D warnings`、`cargo test -p alephcore --lib` 全绿，结果写入回执。
- [ ] **Step 2:** 以不同模型做只读整分支审阅（SPEC → QUALITY），处理阻断项后在回执列出未做事项。
