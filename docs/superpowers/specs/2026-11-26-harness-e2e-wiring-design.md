# Harness 八场景端到端验证与连线修复 / Harness E2E Verification & Wiring

- 状态 / Status: 草案，待用户批准 / Draft, awaiting approval
- 分支 / Branch: `harness-e2e-wiring`（worktree `.worktrees/harness-e2e-wiring`，基于 main `961f38dc1`）
- 参考 / Reference: `deepseek-harness`（下称 dsh）的同名场景实现，仅借鉴纪律，不搬工具

## 1. 目标 / Goal

Aleph 作为长时间运行的 agent，八个用户可感知场景的零件大多已在源码中，但缺少"从用户动作到可见结果"的真机证明。
本轮用真实服务 + 真实客户端逐场景走一遍；**哪里断了修哪里**，修复必须先有失败的测试。

The parts mostly exist; what is missing is end-to-end proof that they connect. This round drives each scenario on a real server
with a real client, fixes only what the drive shows broken, and lands every fix behind a failing test first.

非目标 / Non-goals:
- 不新增第二份状态（不引入 dsh 的 `todo`、`exit_plan_mode`、双队列工具）——会与 scratchpad、`ExecTier::Plan`、`busy_queue` 重复。
- 不做统一"线程状态 Capability"（方案 B）；仅当某场景测出两份互相矛盾的状态时，才在该处单独提案。
- 不越过 `src/harness/` 的 R10 棘轮（`src/harness/tests/budget.rs` `CEILING = 5535`）。

## 2. 假设 / Assumptions (my calls, reversible)

- **F8 多端语义**：用户未作答，沿用当前设计——两个终端都可写同一线程，由 gateway actor 串行化；不改成 dsh 的"单写者文件锁"。
- 真机装置一律复用 `qa/` 现有骨架（scratch `HOME/ALEPH_HOME`、mock provider、先构建后重定向 HOME、禁止 `SKIP_BUILD` 下的结论）。

## 3. 场景映射 / Scenario map

| # | 用户动作 → 期望可见结果 | 现有锚点 | 现有证明 | 本轮补的证明 |
|---|---|---|---|---|
| F1 线程持久化 | 设好模式/模型/思考/记忆后关掉终端、重启 gateway，`--continue` 回来值都在，token 用量回显 | `src/session/events.rs` 快照字段；`interfaces/tui/src/lib.rs:289`；`session.snapshot` / `session.usage` RPC | `qa/resume_boundary knobs`（只证明**崩溃后续跑**沿用快照） | 新阶段 `qa/thread_continuity reopen`：正常退出 + 重启后读 `session.snapshot`、`session.usage`，与关之前逐字段比对 |
| F2 任务清单 | agent 写清单、逐项勾选；有未勾项时不准收尾；重开后清单还在 | `src/builtin_tools/scratchpad.rs`（`PlanSnapshot` 随 `tool_call_completed` 与 `RunSummary` 下发）；`ScratchpadGoalVerifier`；TUI `tasks_panel`、Panel `todo_panel` | 单测 | 新阶段 `qa/thread_continuity checklist`：mock 模型留未勾项尝试收尾 → 被否决；重开后 `chat.history` 能重建清单（TUI 在 `replaying_trace` 时跳过，需确认重建路径） |
| F3 插话/排队/改道 | 运行中发新消息：本步插入（steer）或下一轮排队；中止后改道 | `src/session/steer_signal.rs`；`src/gateway/busy_queue/`（含 `durable.rs`） | `qa/busy_input burst-drain / interrupt / queue` | 先重跑基线；补 `steer` 阶段（本步插入在同一 run 内被模型看到，且不另起一轮） |
| F4 等人工具 | `ask_user` 有人时停下等；无人（CI/定时）时明确失败；终端可纯文本作答 | `src/clarification/ask.rs:133` `HEADLESS_DENIAL`；`src/builtin_tools/ask_user.rs` | 单测 | 新阶段 `qa/human_gate ask`：有人作答 / 纯文本回退 / unattended 立即拒绝且不挂起 |
| F5 计划→执行 | 计划层级下改动被拒；`scratchpad request_approval` 获批后自动回到执行层级，模型同一轮继续执行 | `ExecTier::Plan`；`PlanGate::restore_to`；`src/executor/builtin_registry/builder/constructor/mod.rs` | `qa/plan_handoff`（**已过期**，驱动的是已删除的 `plan_phase` 设计） | 按 `qa/plan_handoff/run.sh` 头注释的迁移清单复活 `handoff / deny / floor` 三个阶段 |
| F6 审批链 | 放行顺序确定且可解释 | `src/tools/scoped/gate_chain.rs`（`GateRule` 首个匹配）；`src/tools/scoped/dispatch.rs:1728-1789`（unattended 拒绝 → 常驻授权 → 询问） | 单测（如 `a_session_grant_does_not_survive_into_an_unattended_run`） | 一张顺序对照表单测（见 §4）；真机复用 F5 的 `floor` 阶段 |
| F7 子代理 | 隔离评审不带父会话框架；fork 复用前缀缓存 | `src/agents/subagent_spawner/fork.rs` | 单测 | 真机：mock provider 记录请求字节，断言同一轮 K 个 fork 兄弟的 `system + messages` 前缀逐字节一致、隔离子代理不含父转录 |
| F8 崩溃/多端 | 两个终端同时往同一线程发消息，不丢、不覆盖、顺序确定；崩溃后续跑 | `src/session/in_process.rs`；`src/utils/instance_lock.rs`；`busy_queue/durable.rs` | `qa/resume_boundary`、`qa/session_order` | 新阶段 `qa/thread_continuity two_clients`：两个 WS 客户端同 `session_key` 并发 `chat.send`，断言两条都入转录、各得一次回复、序号连续 |

## 4. F6：用户期望顺序 vs Aleph 实际顺序 / Approval order

用户列的顺序：逐工具拒绝 → YOLO → 会话授权 → 类别策略 → 询问。

Aleph 实际顺序（源码）：
1. `PolicyDeny`（逐工具 `deny`）——直接拒绝。
2. 分类：`GateRule` 按固定顺序取首个匹配（`ToolDeclared` 等不可降低的底线 → 参数级 `DestructiveArguments` → 操作员覆盖 → 层级 `TierRaised` / `PlanMode` …）。未匹配即放行。
3. unattended 运行 → 直接拒绝（**刻意排在会话授权之前**，见 `dispatch.rs` 注释）。
4. 常驻授权（会话级 / 持久级，`GrantStore::granted_within`）→ 放行并记账。
5. 询问。

两处有意偏离，本轮**只记录不改**：
- **没有能越过底线的 YOLO**：`exec_tier = "full"` 不会降低 `requires_confirmation` 声明的底线（如 `vault_store`）。
- **类别策略先于会话授权参与判定**：分类先决定"要不要问"，授权只用来满足"要问"的调用。

交付：一张表驱动单测，按用户的五段列出每段的输入与 Aleph 的实际结果，作为这个顺序的回归锚点。

## 5. F7 的边界 / F7 boundary

`fork.rs:27` 已写明：fork 出来的子代理**不复用父会话的缓存**（父会话走 `AssemblyPath::Cached`，子代理走 `AssemblyPath::Basic`，system 块字节不同）。
能命中的是**同一轮 fork 兄弟之间**以及**同一角色跨轮次**的前缀。本轮只验证这两点；要让子代理命中父会话缓存，就得改子代理的 prompt 组装方式，这是另一个设计问题（见 §8）。

## 6. 实施顺序 / Order of work

1. **基线**：构建一次，重跑现有装置——`busy_input`（3 个阶段）、`resume_boundary knobs / holes / unanswered`、`session_order`、`run_halt crash / cap`——记录 main 上的红绿状态。
2. **复活 `qa/plan_handoff`**（F5、F6 真机部分）。
3. **新装置 `qa/thread_continuity`**：`reopen`（F1）、`checklist`（F2）、`two_clients`（F8）；`qa/busy_input` 增加 `steer`（F3）；新装置 `qa/human_gate ask`（F4）；在 `qa/` 现有 mock provider 上加请求录制（F7）。
4. **逐个修复**：每个红点先写最小失败测试（单测或集成测试），修复，再让真机阶段转绿。发现的重复实现一并删除并在提交说明中列出。
5. **F6 顺序单测**。
6. **文档**：校对并补全 `docs/reference/FEATURE_LOCATOR.md` §3 Harness 与相关 reference，登记新装置到 `qa/README.md`；写回执 `docs/superpowers/runs/2026-11-26-harness-e2e-wiring-receipt.md`。

## 7. 取舍 / Trade-offs

- 由证据驱动意味着产出不可预知：如果大部分场景本来就通，本轮主要交付测试与文档，新功能很少。
- 真机装置贵且慢（每个阶段都要起服务）；它们是手动触发的，不进 CI，因此可能再次过期（`plan_handoff` 就是先例）。缓解：每个新阶段的关键断言同时落一条可进 CI 的集成测试。
- 沿用多写者语义（F8）意味着要证明 actor 串行化在并发下成立；若测出交错写入，修复会触及 `src/session/in_process.rs`，风险高于单写者锁。

## 8. 待用户决定 / Open questions

1. **F8**：确认"多端都可写、gateway 排队"的语义（当前假设）。
2. **F7**：是否需要子代理命中**父会话**的缓存？若需要，单独立项（改子代理 prompt 组装方式），本轮不做。
3. **F6**：是否接受"没有能越过底线的 YOLO"这一偏离？本轮默认接受。
