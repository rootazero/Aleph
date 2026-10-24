# Aleph Phase 2 B 续跑 Prompt
# Copyable Resume Prompt — Canonical Plugin Producer

你是 Aleph 模块化第二阶段 B 的主 Agent / Lead Orchestrator。请在 `.` 继续执行 **Phase 2 B：canonical plugin producer、per-entry provider/owner fencing、long-lived canonical projection**。先做 plan review，再实施；持续推进到 B 验收完成或遇到真实阻塞。不要自动 commit、push、reset 或覆盖共享工作区。

## 先读与起点

先读取：

- `AGENTS.md`
- `docs/research/aleph_modularization_phase2b_plan.md`
- `docs/research/aleph_modularization_phase2_plan.md`
- `docs/designs/runtime-owner-lazy-activation.md`
- 相关 `docs/reference/REDLINES.md`、`docs/reference/HARNESS_PHILOSOPHY.md`、`docs/reference/SUBSYSTEM_ROUTING.md`、`docs/reference/DEVELOPMENT.md`、`docs/reference/STYLE.md`、`docs/reference/TOOL_SYSTEM.md`、`docs/reference/SECURITY.md`

先执行只读检查：

```bash
pwd
git rev-parse --show-toplevel
git status --short
git diff --stat
git log -1 --oneline
```

当前 A 变更是共享工作区的未提交输入。不得 reset、checkout 旧提交、覆盖他人修改或 `git add -A`。不要修改 `src/harness/`。

## 已冻结的 A 事实

- `RuntimeOwner` 是 WASM runtime side-effect 的唯一 owner；`ProviderHandle` 是实际状态和 generation 权威。
- tool-only WASM discovery 进入 `Cold`，查询、catalog、diagnostic、describe、resolve 不启动 VM。
- authority 只能由 sealed canonical dispatch verdict 派生；不能从 plugin id、角色、`TurnContext`、metadata 或 direct RPC 合成。
- compatibility inner plugin 没有真实 canonical identity 时必须继续 fail-closed。
- direct gateway/plugin RPC 没有 authority 时只能调用已 mounted/Ready runtime。
- stop 先关闭新 admission、等待 in-flight permit、join retained operation、dispose exact scope；失败保留 Failed/residual。
- `Views::after_transition` 及其 lifecycle 委托是 plugin projection 的唯一发布边界。
- `reconcile_plugin_tools()` 在 B 接线前保持 `plugin producer reconciliation is not mounted` 的 fail-closed 语义。

## 本阶段目标

完成并证明：

1. 只有真实 Ready `RuntimeOwner`、真实 `WasmMountIdentity`、当前 provider/owner generation 和真实 handler binding 才能产生 canonical plugin entry。
2. canonical entry 使用现有 `ToolHandlerRegistry` authority，不新建第二 callable registry。
3. entry、lease、registration handle 和 disposer 均绑定 plugin id、mount identity、provider generation、owner generation、descriptor identity 和 registry revision。
4. stop/reload/replacement 先撤回旧 entry，再清理旧 scope；旧 entry、handler、lease、binding、late completion 和旧 disposer 不能影响新实例。
5. 既有 canonical permission/visibility/trust/confirmation/actor gates 真实涵盖 plugin entry；仅在 gate scope 中产生 `PluginActivationAuthority`。
6. 已存在的 facade/catalog/observer 在 publish、revoke、replace 后从真实 snapshot/notification 看到变化；不伪造 canonical revision，不把 broadcast 当 durable recovery。

## B0 必须先完成的架构决策

先提交一份短 review，再写 producer：

- 画出 `ExtensionManager → RuntimeOwner → ToolHandlerRegistry → ScopedToolService → canonical dispatch` 的真实调用图。
- 说明 producer 输入、entry 生命周期、注册事务、撤回顺序和各 revision 的权威。
- 明确 Cold 是否允许通过 canonical activation proxy 首次触发。
- 默认采用保守方案：Cold 不进入可执行 canonical registry，producer 只发布 Ready、identity-fenced handler；如果要改变这个边界，先停止并报告需要单独确认的权限、visibility、confirmation、lease 和 stop 语义。
- 说明同名冲突、partial registration、provider mismatch、disposer failure、late completion 的 fail-closed 结果。

B0 未冻结前，不让 `reconcile_plugin_tools()` 成功，不修改 producer 成功路径。

## 实施顺序

按以下顺序执行，每项独立验收：

- B1：冻结 entry/lease 契约；复用 `RegistryEntry`、`ToolCallIdentity`、`ProviderCallHandler`、`BackendLease` 和现有 registration handle。
- B2：实现 Ready-only canonical producer 的原子 register/replace/revoke；禁止旧 handle 撤销新 entry。
- B3：接入 RuntimeOwner Ready、stop、reload、replacement；保持 identity/generation fencing 和 exact scope disposer。
- B4：接入 canonical dispatch sealed authority；compatibility inner route 没有 canonical identity 时继续拒绝。
- B5：接入 long-lived snapshot/notification observer；lag/reconnect 从当前权威 snapshot 重建。
- B6：补真实 WASM、barrier、lease、replacement、observer 和 failure tests，之后运行 GA/GB 报告。

主 Agent 负责方案、writer 边界、diff 审查和验收；实现和大规模测试优先委派 subagent。多个 writer 不得同时修改同一文件。共享 target 的 cargo 串行运行。

## 必须覆盖的真实测试

禁止用 sleep、只数 mock 调用或只 grep 函数名作为证据。优先使用现有真实 guest fixture、`WasmMountIdentity`、`EffectScope`、Barrier/Notify/oneshot。

至少验证：

- Cold 查询不创建 VM、不发布 canonical executable entry、不改变 generation/revision。
- Ready owner 发布一个完整 identity/generation-fenced canonical entry。
- 真实 canonical gate 通过后执行真实 guest handler，并返回确定 payload。
- stop/reload/replacement 后旧 entry、旧 lease、旧 handler 和旧 binding fail-closed。
- 同名 replacement 的新 entry 不会被旧 disposer 或 late completion 删除。
- 并发首调和 stop/in-flight guest call 的安全边界。
- producer 注册失败、同名冲突、disposer error、provider mismatch 的结构化失败。
- 已存在 observer 看到 publish/revoke/replace；lag 后 snapshot 不复活旧实例。
- mixed WASM/MCP/static 既有语义保持不变。
- B 未接线时 producer-not-mounted 和 permission denied 仍 fail-closed。

## Cargo 与内存门

每一条 cargo 命令前都必须单独执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两个命令 exit 0 且 `available_bytes >= 4294967296` 才允许运行 cargo。不得并发 cargo；filter 0 tests 不算通过；未运行的测试必须写 UNRUN。

按实际 diff 选择并记录 exit code、有效测试数和 warning：

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

wire/shared 文件变化时补跑对应 shared tests。真实 fixture、工具链或内存不足时报告 BLOCKED/UNRUN，不以 mock 替代。

## 禁止与停止条件

禁止：

- 修改 `src/harness/`。
- 把全局 `aleph/tools` 绑定成单插件 provider。
- 新建第二 callable registry、第二 durable enabled store、第二 actual-state authority 或独立 disposer map。
- MCP lazy、sidecar、dylib、scheduler、Safe Replay、EffectClaim producer、跨 Capability kinds。
- 通过 metadata/catalog/role/TurnContext/name fallback 伪造 authority。
- 在 B0 未冻结前把 Cold 暴露为可执行 canonical entry。
- 虚增 registry revision、把 provider broadcast 当 durable recovery、吞掉 partial failure。
- 自动 commit、push、reset 或删除共享工作区内容。

遇到以下情况必须停下并报告证据：

- 需要改变 A 的 Cold/Ready/stop 权限语义。
- 需要 activation proxy 才能完成首次模型调用但尚未获得架构决定。
- 无法证明旧 entry/disposer/late completion 不影响新 entry。
- 只能绕过既有 permission/visibility/trust/confirmation 才能让测试通过。
- 内存门失败、真实 WASM 不可用、共享 writer 冲突或三次尝试没有实质进展。

## 最终交付

最终中文报告必须分别说明：

- B0 的架构决策与 trade-off。
- 修改文件和真实生产调用图。
- provider state、owner generation、mount identity、entry lease、metadata revision、canonical registry revision 的各自权威。
- register/replace/revoke/stop/reload 的真实顺序。
- 每条验证命令的 memory gate、exit code、有效测试数和 warning。
- A 保持的行为、B 完成的行为、仍 deferred 的行为。
- 残余风险、BLOCKED/UNRUN 项、未提交/未推送状态。
