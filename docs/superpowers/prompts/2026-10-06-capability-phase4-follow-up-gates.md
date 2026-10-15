# Aleph Phase 4 后续 Gate 执行 Prompt

你正在继续 Aleph 第四期架构重构。不要从头重做已经合并的 Phase 4 基座；从当前 `main` 的真实代码和提交继续推进后续 gate。

## 当前仓库状态

- canonical checkout：`/Volumes/TBU4/Workspace/Aleph`
- 当前分支：`main`
- 当前 HEAD：`1ea05b1ce86d35b64384d8157f015fd68224cdf6`
- Phase 4 基座已从 `capability-phase4` fast-forward 合并到 `main`。
- 合并前后都已验证主 checkout clean；所有后续源码修改必须在新的 worktree 和 feature branch 中完成，不能直接修改 `main`。
- 基线提交：`a5300221d27c7a916407d8b96cadccb91cde1bbf`
- 已批准的书面 spec：`docs/superpowers/specs/2026-10-06-capability-phase4-design.md`
- 已批准的 implementation plan：`docs/superpowers/plans/2026-10-06-capability-phase4.md`
- 实施 ledger 和 reports：`.superpowers/sdd/2026-10-06-capability-phase4/`

开始工作前必须读取：

1. `/Volumes/TBU4/Workspace/Aleph/AGENTS.md`
2. `/Volumes/TBU4/Workspace/Aleph/docs/reference/FEATURE_LOCATOR.md`
3. `docs/superpowers/specs/2026-10-06-capability-phase4-design.md`
4. `docs/superpowers/plans/2026-10-06-capability-phase4.md`
5. `.superpowers/sdd/2026-10-06-capability-phase4/task-*-report.md`
6. 当前 `git status --short --branch`、`git log --oneline -12` 和 `git diff a5300221d..HEAD --stat`

不要假设历史 report 等于当前源码事实；读取实际文件和符号核验。

## 已完成并已验证的基座

### Gate A：Capability contract

已新增并提交：

- `src/capability/descriptor.rs`
- `src/capability/facade.rs`
- `src/capability/backend.rs`
- `src/capability/ownership.rs`
- `src/capability/mod.rs`

当前语义：

- `CapabilityKind` 的唯一当前定义点在 `src/capability/descriptor.rs`，包含 Tool、Skill、Agent、Task、Resource、EventSource、Subscription、Plugin、Hook。
- `CapabilityRevision` 表示 descriptor/registry revision。
- `OwnerGeneration` 表示 lifecycle owner generation；禁止把 Tool registry revision 当成 owner generation。
- `ToolBackendAdapter` 是本期唯一完整 live backend，包装现有 `ToolHandlerRegistry`。
- 其它 kind 只有 contract/deferred 状态，不能宣称已经挂载统一 registry。
- `CapabilitySlot` / `ALL_SLOTS` 仍是启动期 first-writer-wins slot，不能改造成动态 registry。

### Gate B：Ownership

已有 `OwnershipTree` 和 typed owner 类型：

```text
Runtime -> Session -> Run -> Task -> EffectClaim
```

`VisibilityScope` 与 `LifetimeScope` 正交。`SessionRunRegistry` 仍然是进程内 active admission index/cache，不是跨崩溃 recovery source。

注意：当前 `ToolBackendAdapter::generation()` 仍返回稳定的 `OwnerGeneration(0)`，这是基座占位，不是完整 runtime ownership wiring。

### Gate C：durable claim / memo contract

已新增或接入：

- `src/capability/effect_claim.rs`
- `SessionEvent::EffectClaimPrepared`
- `SessionEvent::EffectClaimClaimed`
- `SessionEvent::EffectClaimInvoking`
- `SessionEvent::EffectClaimTerminal`
- `SessionEvent::ApprovalMemo`
- `SessionEvent::HookMemo`
- `src/session/call_log.rs`
- `src/extension/hooks/executor.rs`
- `src/tools/scoped/dispatch.rs`
- `src/gateway/mcp_face/mod.rs`

当前语义：

- Effect claim reducer 对合法状态迁移进行纯函数 reconciliation。
- malformed identity、duplicate active claim、非法迁移、terminal 后再次迁移都 fail-closed 为 `Unknown`。
- approval decision + `ApprovalMemo` 通过一次 `SessionService::emit_batch` 提交。
- Hook memo 使用 per-call cloned executor + session-specific sink，避免不同 session 共用可变 memo 状态。
- MCP scoped dispatch 已补 per-call `CallIdentity`，避免 approval/memo 在该路径静默丢弃。
- 已有 approval batch guard：`session::in_process::tests::approval_memo_batch_is_one_contiguous_commit`。

### Gate D：projection/subscription

已完成：

- `CapabilityChangeStream` 的纯值状态机：initial snapshot+cursor、单调 cursor、Invalidated resync、delta 去重、cursor 不回零。
- ACP JSON、StateDatabase、MCP/tool list surfaces 的 projection-only 文档边界。
- GlobalBus 的 notification-only 文档边界。
- `to_metadata_form` 保持真实旧签名和纯 compatibility wrapper 语义。
- `src/harness/` 与 `src/mcp/tool_bridge.rs` 未修改。

必须牢记：当前 `Zahir` trait 没有完整 concrete facade/backend implementation；`CapabilityChangeStream` 是已测试的纯 value state machine，真实 committed registry snapshot/change-feed wiring 仍是后续 gate。

## 已验证结果

以下命令必须在 canonical checkout 或后续 worktree 中显式 `cd` 后运行：

```bash
cargo check -p alephcore
cargo test -p alephcore --lib capability::facade --no-fail-fast
cargo test -p alephcore --lib capability::effect_claim --no-fail-fast
cargo test -p alephcore --lib session::events --no-fail-fast
cargo test -p alephcore --lib session::in_process::tests::approval_memo_batch_is_one_contiguous_commit --no-fail-fast
cargo test -p alephcore --lib tools::service::metadata_form_tests --no-fail-fast
cargo test -p alephcore --lib tools::scoped::tests --no-fail-fast
cargo test -p alephcore --lib extension::hooks --no-fail-fast
```

最新已知结果：

- facade：5 passed
- effect claim：16 passed
- session events：28 passed
- approval batch：1 passed
- metadata wrapper：6 passed
- scoped tools：130 passed
- hooks：193 passed
- `cargo check -p alephcore`：passed
- `git diff --check`：passed

`cargo fmt --check` 目前会被仓库已有的多个无关文件格式漂移阻断，不能直接运行全仓格式化覆盖历史代码。若后续改动触及某文件，必须至少对改动文件单独核验格式，并在报告中区分 baseline drift 与本次变更。

## 后续 Gate 顺序

不要把以下工作一次性混成一个“大重构”。每个 gate 都必须有独立 brief、独立 subagent 任务、独立 review、独立测试和独立 commit。

### Gate E：concrete Zahir facade/backend wiring

目标：把当前纯 contract 接到既有 `ToolHandlerRegistry`，但不建立第二套 registry。

必须回答并实现：

- concrete facade 如何持有 Tool backend，而不复制 descriptor/handler truth；
- `describe(scope)` 如何从同一 atomic registry snapshot 生成 `CapabilitySnapshot`；
- `resolve(reference)` 如何返回带 `CapabilityRevision` 和 `OwnerGeneration` 的 lease，并在 stale revision/owner 时 fail-closed；
- `subscribe(scope,cursor)` 如何从真实 committed registry change feed 提供 ordered stream；
- facade projection 如何只读 canonical snapshot，不旁路 `ToolHandlerRegistry`；
- Tool descriptor 的 schema fingerprint 何时计算、由谁负责、如何测试；
- `OwnerGeneration(0)` 如何由真实 `OwnershipTree` 或明确 owner backend 接管。

禁止：

- `dyn Any`、巨大 payload enum、第二套 handler map；
- 把 `CapabilityRevision` 和 `OwnerGeneration` 合并；
- 把 `CapabilitySlot` 动态化；
- 修改 `src/harness/` 以承载 facade 业务；
- 通过 `to_metadata_form` 生成 canonical identity。

验收必须包含：snapshot/handler/descriptor 同代一致性、stale lease、replace/unregister、projection 与 dispatch 使用同一代、change cursor 恢复和无重复。

### Gate F：EffectClaim producer 与 recovery integration

当前 `effect_claim.rs` 主要是 reducer/adapter，automatic replay 仍关闭。下一步先做 design review，再决定是否实现 producer。

在写代码前必须明确：

- 哪些真实生产路径会产生 Prepared/Claimed/Invoking/Terminal event；
- claim、budget reservation、owner generation 和 fencing token 的事务边界；
- crash after intent/claim/invocation/outcome 的每个窗口；
- 哪些路径保持 VerifyOnly；
- `ReplayPermit` 与 EffectClaim reducer 如何保持职责分离；
- 外部 effect 不可观测时如何进入 Unknown；
- 如何避免把同一 effect 的 request、claim、outcome 计数混成 exactly-once。

只有在 effective input 已经于正确的 guardrail/cache/dispatch 边界 durable 记录，并且用户单独批准 Safe Replay gate 后，才允许实现真实 replay invocation。

### Gate G：committed projection/subscription wiring

将纯 `CapabilityChangeStream` 接到真实 source：

- atomic snapshot + cursor capture；
- registry mutation 与 committed event 的顺序；
- owner revoke/generation bump 的 Invalidated 语义；
- snapshot+delta resync 去重；
- overflow 时发送最新 snapshot 和新 cursor，不能伪装成完整逐条投递；
- GlobalBus 仍只做低延迟通知；
- StateDatabase/ACP JSON/MCP projections 不成为 recovery source。

必须为 ACP/MCP/model projection 分别证明：projection 不能授权、不能 resolve handler、不能决定 replay、不能维护第二份 identity/revision。

### Gate H：ACP server gate

这是独立高风险 gate，不要因为 ACP JSON projection 已有就声称 ACP server 完成。

必须单独审计：

- inbound command 是否经过 ACP transport/session manager 进入 Session/Run mutation line；
- ACP server 是否复用 Zahir projection 和 existing session/run ownership；
- 单 session 并发、cancel、quiescence、route snapshot；
- committed event projection、cursor、断线恢复；
- 权限/approval 不得由 protocol projection 偷换；
- 不得建立 ACP 自有 capability registry。

### Gate I：Safe Replay / external idempotency

只有新的 architecture/design approval 后才能推进。目标不是强行承诺 exactly-once，而是明确：

- provider 支持 idempotency key 时，request_id 如何传递；
- provider 不支持时，如何报告 at-least-once/Unknown；
- external effect ledger 是否必要；
- claim fencing、per-call budget、retry 和 duplicate suppression 的关系；
- approval/hook memo 的 policy fingerprint、scope、expiry、owner generation。

## 执行协议

- 中文沟通；代码注释用英文；文档按项目约定处理中英混排。
- 主代理只做扫描、架构判断、任务拆解、subagent 编排和验收；实现、批量修改、测试执行优先交给 subagent。
- 每个实现 gate 使用新 worktree；不得直接在 main 上写代码。
- 新 gate 开始前先读当前源码和已有报告，不把旧 report 当成事实。
- 每个子任务必须报告：修改文件、未修改文件、测试命令、真实结果、deferred 内容、commit hash。
- 合并前验证 staged tree：`git add` 后先检查 `git status` 和 `git diff --cached --check`，提交后再次确认 clean。
- 任何 reviewer 发现 Important/Critical finding 时，必须修复或明确升级为独立 gate，不能用文档措辞掩盖。
- 不要重跑或重写前三期 Tool canonical registry；优先复用 `ToolHandlerRegistry`、`ToolRegistrationScope`、`EffectScope`、`SessionService::emit_batch`、`ReplayPermit` 和现有 SQLite/session event infrastructure。

## 当前不变量

以下事实不能被后续 gate 破坏：

1. Tool registry 是 callable Tool 的唯一事实源。
2. Descriptor 与 handler 必须同代配对。
3. `CapabilityRevision` 不等于 `OwnerGeneration`。
4. VisibilityScope 不等于 LifetimeScope。
5. Projection 不授权，notification 不持久化。
6. Session event store 是 recovery-related committed source；StateDatabase/ACP JSON 是 projection。
7. Unknown、missing identity、stale owner、schema mismatch、invalid claim 都 fail-closed。
8. `to_metadata_form` 只做旧兼容 wrapper。
9. `src/harness/` 不扩张为第二个 runtime。
10. automatic Safe Replay、external exactly-once 和 universal scheduler 仍是未批准 gate。

## 新会话的第一轮工作

1. 检查 main/worktree/HEAD 是否与本 prompt 一致。
2. 读取 spec、plan、FEATURE_LOCATOR 和全部 Phase 4 reports。
3. 对 Gate E/F/G/H/I 做一次只读 gap scan，列出真实文件、现有接口、不可复用点和风险。
4. 选择一个最小 gate，给出 2–3 个方案和推荐方案。
5. 遵循 architectural design approval → written spec/plan → subagent implementation → review → verification 的流程；不要直接跳入 Safe Replay 或 ACP server 编码。
6. 将新的 gate 状态和 deferred decision 写入对应 report，避免下一会话重新猜测当前边界。
