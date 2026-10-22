# Aleph 模块化第二阶段 B 计划
# Phase 2 B Plan — Canonical Plugin Producer and Long-Lived Entry Fencing

## 1. 目标与边界 / Goal and Boundary

本工作包在 Phase 2 A 的 RuntimeOwner、Cold discovery、sealed admission、stop fencing 基础上，完成 **B：canonical plugin producer + per-entry provider fencing + long-lived canonical projection**。

最终目标是：经过真实验证并已 Ready 的 tool-only WASM plugin instance，能够以一组带 provider identity、owner generation、mount identity 和 registry revision 的 canonical `ToolHandlerRegistry` entry 进入既有 ToolService 调用面；实例停止、替换、reload 或 generation 变化时，旧 entry/handler/lease 必须 fail-closed 并被准确撤回，长期持有的 canonical facade/catalog/observer 能看到当前权威状态。

本阶段不重新实现 A 的 RuntimeOwner，不把 metadata/catalog 当 callable registry，不把 B 的局部接线说成所有插件能力完成。

## 2. 当前起点 / Starting State

- 工作目录：`/Volumes/TBU4/Workspace/Aleph`。
- A 的实现仍是共享工作区未提交变更；新 session 必须先读取 `git status --short` 和实际 diff，不能 reset、checkout 旧提交、覆盖共享修改或自动提交/推送。
- `RuntimeOwner` 已负责 tool-only WASM 的 Cold → Starting → Ready、single-flight activation、generation、WASM identity、admission permits、exact scope disposer 和 stop residual。
- `PluginActivationAuthority` 只能从 sealed canonical gate verdict 派生；不能从 plugin id、角色、`TurnContext`、metadata 或 direct RPC 合成。
- compatibility inner plugin 路径当前在没有真实 canonical identity 时 fail-closed；这是当前 B 的入口证据，不得绕过它。
- `ToolHandlerRegistry` 是 canonical callable authority；`PluginRegistry`、active tool metadata、catalog、diagnostic、provider broadcast 都是 projection 或 notification。
- `reconcile_plugin_tools()` 当前应保持 producer-not-mounted 的 fail-closed 行为，直到 B 的契约、事务和测试全部冻结。

## 3. B0：独立 gate 与架构决策 / Independent Gate

在修改 canonical producer 前，必须完成并保存一份短 plan review，至少回答：

1. 当前 `ToolHandlerRegistry`、`ProviderCallHandler`、`RegistryEntry`、`ToolCallIdentity`、`BackendLease`、`ZahirFacade` 和 `ExtensionManager` 之间的真实调用图。
2. producer 的唯一输入是什么：必须来自已验证 manifest/adapter declaration、当前 Ready `RuntimeOwner`、实际 mounted `WasmMountIdentity` 和真实 handler binding。
3. 一个 canonical entry 是否代表“当前已 Ready 的可执行 handler”，还是代表“会在调用时激活 Cold owner 的 proxy”。
4. 默认推荐：Cold 不进入可执行 canonical registry，producer 只发布 Ready 且 identity-fenced 的 handler；如果用户需要模型第一次调用即可激活 Cold，必须单独审定 activation proxy 的权限、visibility、confirmation、lease 和 stop 语义，不能暗中改变 A 的 Cold 契约。
5. 注册、替换、撤回、disposer、provider generation、owner generation、metadata revision、canonical registry revision 的权威和事务边界。
6. 失败顺序：producer 注册失败、同名冲突、owner stop 失败、旧 disposer 晚到、provider generation 不匹配时分别保留什么状态。
7. long-lived observer 如何通过真实 broadcast/notification 得到变化并从当前 snapshot 重建；不得承诺 durable replay，也不得虚增 revision。

B0 未通过前，不修改 `src/tools/registry.rs` 的 producer 成功路径。

## 4. 实施 DAG / Implementation DAG

```text
B0 plan review + canonical contract freeze
  → B1 producer input and entry lease contract
  → B2 atomic register/replace/revoke with provider and owner fencing
  → B3 real RuntimeOwner lifecycle integration
  → B4 canonical dispatch and sealed authority integration
  → B5 long-lived projections and observer recovery
  → B6 real WASM/effect/lease/concurrency acceptance
  → GB/B final report
```

### B1：producer 契约 / Producer Contract

允许优先修改：

- `src/tools/registry.rs`
- `src/capability/{backend,facade,zahir_facade}.rs`
- `src/extension/{runtime_owner,lifecycle,plugin_ops,projection}.rs`
- 必要的 `src/tools/descriptor.rs` 和现有 registry 类型

要求：

- 每个 producer entry 携带 plugin id、mount identity、provider generation、owner generation、handler identity 和真实 canonical descriptor。
- entry 的 executable admission 必须要求 provider `Ready`、owner current、generation 相等、WASM binding 当前且 desired/trust/visibility 仍允许。
- 不复制第二份 actual state、enabled store、callable registry 或 disposer map。
- `ToolCallIdentity` 必须由 canonical descriptor 派生；不能用 plugin metadata 名称代替。
- B 的 lease/entry 不能跨 provider restart 或同 id replacement 继续有效。

### B2：原子注册与撤回 / Atomic Publish and Revoke

- Ready transition 与 canonical entry publish 的顺序必须冻结：entry 只有在 scope、WASM identity、owner generation 和 callable admission 全部完成后才可见。
- stop/reload 先关闭新 admission，再撤回旧 canonical entry，等待现有 entry call 的安全边界，最后 dispose exact scope。
- register/replace/unregister 使用现有 registry revision 和 registration handle；旧 handle 不能撤销新 entry。
- 同名冲突、partial registration、disposer failure 必须 fail-closed，并保留结构化 diagnostic。
- 旧 entry 晚到的 completion/disposer 只能 no-op 或报告 stale，不能删除 replacement entry。

### B3：RuntimeOwner 接线 / Lifecycle Integration

- `RuntimeOwner` 仍是实际 WASM 生命周期的唯一 owner；producer 只消费它已证明的 Ready snapshot 和 binding。
- A 的 Cold discovery 不得因为 catalog、describe、resolve、projection 或 observer 查询而 activation。
- activation 成功后 producer publish；activation failure 不得留下 canonical entry。
- stop 成功撤回 entry、dispose scope、发布 Stopped/Disabled；stop residual 保留 Failed/residual 和旧资源，不伪报已撤回。
- reload/replacement 重新分配 generation 和 mount identity，禁止复用旧 lease。

### B4：canonical dispatch / Sealed Dispatch Integration

- 真实 canonical entry 通过既有 `ToolHandlerRegistry`/`ScopedToolService` permission、visibility、trust、confirmation 和 actor gates。
- 只有在 gate scope 内才能创建 `PluginActivationAuthority`；authority 绑定 canonical name、descriptor identity、input、call id、caller 和 actor。
- compatibility inner route 没有真实 canonical entry 时继续拒绝，不增加 name-based fallback。
- direct gateway RPC 不得伪造 canonical entry 或 authority；它只能调用已 mounted/Ready 的 owner path。
- 真实 canonical handler 调用后必须重新验证 provider/owner generation、mount identity 和 current entry，再接受 guest output。

### B5：long-lived projection / Observer Contract

- canonical registry snapshot、facade、catalog、MCP/tool observer 使用真实 registry revision；不得把 provider generation 或 metadata revision 改名为 canonical revision。
- registry broadcast 是 live notification，丢事件后从当前 canonical snapshot 重建；不添加假 replay。
- provider broadcast 只做通知，不能作为 durable recovery source；durable recovery 仍由既有 session event store 负责。
- observer 在不重建持有对象的情况下，必须看见 entry publish、撤回、replacement 和 stale 状态。
- 一个 plugin stop 不能撤回 builtin 或其他 plugin 的 entry。

## 5. 必须验证的真实效果 / Required Effect Evidence

优先复用现有真实 WASM fixture、`WasmMountIdentity` 测试、`EffectScope` disposer 和 barrier/oneshot，不使用 sleep 排序或 mock-only success。

至少覆盖：

1. Cold discovery：重复 list/catalog/describe/resolve 不创建 VM、不发布 canonical executable entry、不递增 provider/registry generation。
2. Ready producer：真实 WASM mount 完成后发布一个带完整 identity/generation 的 canonical entry。
3. canonical call：通过真实 permission/visibility/confirmation gate，进入 `PluginActivationAuthority`，执行真实 guest handler 并返回确定 payload。
4. old lease rejection：stop、reload、replacement 后，旧 entry、旧 handler、旧 binding 和旧 lease 全部 fail-closed。
5. same-name replacement：新 entry 可用，旧 disposer/late completion 不会撤回新 entry。
6. concurrent calls：同一 provider generation 下并发调用正确共享 admission/lifetime；stop 不会在途调用仍使用资源时提前 unload。
7. producer failure：同名冲突、注册部分失败、disposer error、provider mismatch 都产生可查询失败状态，不报成功。
8. observer：已存在的 facade/catalog/observer 看见 publish/revoke/replace；lag/reconnect 从当前 snapshot 恢复，不复活旧 entry。
9. mixed/static regression：mixed WASM、MCP、static 继续使用各自原语义；本阶段只扩大明确批准的 Tool producer。
10. B boundary：未 Ready 或未 canonical producer 的 plugin 仍返回 producer-not-mounted/permission denied，不通过 metadata 旁路执行。

## 6. 资源与验证纪律 / Resource and Verification Discipline

每一条 cargo 命令之前都必须单独运行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有 self-test exit 0、探针 exit 0 且 `available_bytes >= 4294967296` 才允许运行 cargo。共享 target 不并发。不得把 filter 0 tests、旧日志、只编译成功或 subagent 摘要当成测试通过。

候选验证集按实际 diff 选择：

```bash
cargo fmt --all --check
cargo test -p alephcore --lib tools::registry::
cargo test -p alephcore --lib capability::zahir_facade::
cargo test -p alephcore --lib extension::runtime::wasm::
cargo test -p alephcore --lib extension::lifecycle::
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy --workspace --all-targets
```

wire/shared 变化必须补跑对应 shared tests；缺少真实 fixture 或工具链时记录 BLOCKED/UNRUN，不用 mock 替代。

## 7. 禁止事项与停止条件 / Prohibitions and Stop Conditions

禁止：

- 修改 `src/harness/`。
- 把全局 `aleph/tools` provider 改成单插件 provider。
- 新建第二 callable registry、第二 durable enabled store、第二 actual-state authority 或独立 disposer map。
- MCP lazy、sidecar、dylib、scheduler、Safe Replay、EffectClaim producer、跨 Capability kinds 扩展。
- 读侧触发 activation，或用 metadata/catalog/role/TurnContext 合成 authority。
- 在 B0 未冻结前把 Cold 暴露成可执行 canonical entry。
- 将 provider broadcast 当 durable recovery、虚增 canonical revision、吞掉 partial failure。
- reset、覆盖共享工作区、自动 commit、自动 push。

必须停止并报告证据的情况：

- 需要改变 A 已冻结的 Cold/Ready/stop 权限语义。
- 需要引入 activation proxy 才能完成首次模型调用但尚未获得该架构决策。
- 无法证明旧 entry/disposer/late completion 不会影响新 entry。
- 只能通过绕过既有 permission/visibility/trust/confirmation 或伪造 revision 使测试通过。
- 内存门失败、真实 WASM fixture 不可用、共享 writer 冲突或连续三次无实质进展。

## 8. 交付格式 / Deliverables

最终报告必须分别写明：

- B0 冻结的 canonical entry/lease/activation 选择及 trade-off。
- 实际修改文件和生产调用图。
- provider/owner/mount/entry/revision 各自的权威。
- register/replace/revoke/stop/reload 的真实顺序。
- 每条命令的 exit code、有效测试数和 memory gate 证据。
- A 保持的行为、B 完成的行为、仍 deferred 的行为。
- 残余风险、BLOCKED/UNRUN 项、未提交/未推送状态。
