# 多会话平行 · 第二次优化 · P0+P1 地基 (Multi-Session Parallel — Second Optimization, P0+P1 Foundation)

- **日期**: 2026-07-04
- **状态**: 已确认（brainstorming 四节 A/B/C/D 逐节批准）
- **性质**: 深度架构重构（后端并发模型）+ 前置去险
- **本轮范围**: 仅 P0+P1（整条路线图已设计，P2–P6 各自后续独立 spec）
- **对应审计**: 前端 SessionMap 审计 + 后端并发两轮审计（2026-07-04，3 路 Explore）
- **参考项目**: codex（`ThreadManager`/steer-interrupt-new-turn）、pi（process-per-session）、kimi（background task lifecycle）

---

## 0. 背景与定位

"多会话平行"第一次优化交付了 Panel 侧的多会话基础设施：`SessionMap`（ConvId 键控）+ 每个后台会话常驻 `ChatState` 被全局 dispatcher 喂事件（切走不冻结、token 无损）。实际 UI 面是左侧 `ChatSidebar` 会话列表（后端 `sessions.list` 驱动），**无顶部 tab strip**（`sessions.rs` 文档注释描述的 tab strip / Cmd+N 尚不存在），仅 wide/tablet 有，手机端无。

**第二次优化的核心架构发现（前后端审计一致）**：

> **"多会话平行"的并行度受限于"不同 agent 数量"，而非"会话数量"。**

后端并发模型是三道 throttle 叠加，但只有一道真正限住*运行中*的 agent：
1. **Lane 信号量**（`lane.rs`）——全局，Execute 分 desktop(4)/shared(3)。但许可在 **dispatch 时释放**（`handler.rs:640-643` 只在 `process_request().await` 期间持有，而 `chat.send`/`agent.run` 在 `tokio::spawn` 后立即返回 `handlers/agent.rs:423-439`）→ 限的是"请求受理并发"不是"并发 agent"（审计 1.4）。
2. **per-AGENT 闸**（`AgentInstance.state` 经 `try_start_run`，`agent_instance.rs:360-375`）——真正的限流：**一 agent 同时最多一个 run**，跨该 agent 所有会话（审计 1.1）。`AgentRegistry` 一 agent 一 `AgentInstance`（`agent_registry.rs`）。
3. **`max_concurrent_runs = 5`**——per-agent backstop，文档自承不可达（`mod.rs:70-76`）且未 config 连线（死旋钮，审计 1.3）。

**无 per-session lane**。同 agent 多会话完全串行；更糟——Panel 路径**根本没有 busy queue**：在 `main` 开两 tab，A 跑时 B 发消息 → B 的 run 直接 `Failed`（`handlers/agent.rs:423-433` 的 `Err → RunStatus::Failed`，`execute.rs:190-206` 的 `Steer` 分支对不同会话的同 agent 冲突返回 `AgentBusy`）（审计 1.2）。

**选定方向**：真·per-session 并行（大重构）——把 run 闸从 per-agent 移到 per-session，同 agent 的 N 个会话可并行。

---

## 1. 架构红线与不变量

### R10 归属
`SessionRunRegistry` 与新准入闸落在 `src/gateway/`（编排层），**不进 `src/harness/`**——不吃 harness 12 文件 / ~4900 行预算。后端审计已确认："any new per-session scheduler must live outside `src/harness/`（it would be gateway orchestration, which is where it currently correctly sits）"。

### INV-ISO（隔离不变量，设计红线）
> per-session 并行**只改"何时可开跑"（闸），绝不改"run 看见什么身份/记忆/config"**。每个 run 继续用**自己的 `SessionKey`**（含 `agent_id` + 会话）经 task-local `TURN_CONTEXT` 携带身份；记忆写入继续按 `agent_id` 物理分区（`note/{agent_id}/…` 布局 + `(agent_id, …)` 表键）。**禁止任何 agent 级共享可变的"当前会话/agent"态被 run 环境式读取。** 不同 agent 的并发 run 绝不跨写记忆；同 agent 不同会话的并发 run 共享该 agent 记忆域（正确）但存储须容忍并发写。

**为何 INV-ISO 已基本成立（P1 的任务是保住，不是新建）**：
- `TURN_CONTEXT` 是 tokio `task_local!`（`src/tools/turn_context.rs:63`，`.scope()`/`.sync_scope()`/`.try_with()`）——每个 run 一个独立 async task（`handlers/agent.rs:423` `tokio::spawn`），各自持有自己的 TURN_CONTEXT，天然不共享。
- `note_manage.resolve_agent_id`（`note_manage.rs:376`）：agent 分区来自 `args.agent_id` → 否则读**任务级** turn context 的活跃会话 agent；存储物理分区按 `agent_id`。

只要 P1 不引入 agent 级共享可变的 ambient "当前会话/agent" 态、且 TURN_CONTEXT 在 run 边界用该 run 自己的 `SessionKey` scope，并发 run（哪怕不同 agent）就不会串写记忆。

### INV-SEQ（单写者不变量）
> 每会话同一时刻只有一个逻辑写者，且一会话一 run。seq 单调性由"一会话一 actor"（`in_process.rs:65-98`）+ 会话互斥锁共同保证。

---

## 2. P0 — 地基去险（放松粗锁前必做）

### P0-a. `execute.rs` 巨石拆分（审计 4.3，纯行为保持）
`execute.rs` = 1657 行 / 87 KB，单个 `execute()` ~1100 行（`:108-1250`）内联了：准入闸 + busy 分支 + spawn run + 收尾续跑（topic-gen / 压缩 / goal-loop / strategy）。这是改 run 生命周期最大的耦合/可观测性风险。

**拆分（按职责）**：
- `execution_engine/gate.rs` — 准入闸（会话 claim + 并发许可 + busy 分支派发）＝ **P1 新双闸的落点**
- `execution_engine/spawn.rs` — spawn 脱离式 run task
- `execution_engine/post_run.rs` — 收尾续跑（topic-gen / 压缩触发 / goal-loop / strategy）
- `execute.rs` — 瘦成薄编排器调用以上

**纪律**：纯行为保持；现有 `execution_engine` 测试全绿；零新语义；不改公共签名除非拆分必需。符合 P2 高内聚 + R10 可导航。

### P0-b. seq 竞态修复（审计 4.1，放锁前堵损坏源）
**病灶**：两个独立 seq 分配器——actor 内存 `head_seq+1`（`actor.rs:94-115`）与直写 `load_head_seq()+1`（`resume_coordinator.rs:322-343`、`backfill.rs:50,94-99`）。表主键 `(session_id, seq)`（`store.rs:121-128`）→ 碰撞是 INSERT 报错；actor append 出错**不 resync `head_seq`**（`actor.rs:112-114`）→ 永远算出同一撞键 seq → **该会话 actor 写入永久卡死**。今天靠 per-agent 粗锁掩盖，P1 放松后必现（backfill 已用 `service.detach()` 防御 `backfill.rs:61-63`，证明隐患真实且已知；但 detach 与直写间仍有 TOCTOU 窗口——steering `emit_event` 可重生 actor）。

**修复（分层，两者都做）**：
1. **自愈（必做兜底）**：actor append 收到唯一键冲突 → 从 store `load_head_seq()` **resync 后有界重试一次**（把"永久卡死"变自愈）。
2. **单写者纪律**：run 热路径全部走 actor（`in_process.rs:65-98` 已保证一会话一 actor）；收紧剩余合法直写路径（backfill）的 detach/reattach 纪律，堵住 TOCTOU 窗口。

### P0 顺序
`P0-a 拆分`（隔离出闸点）→ `P0-b seq 自愈+单写者`（放锁前堵损坏）→ 进入 P1。

---

## 3. P1 — 后端 per-session 并行核心

### 3.1 双闸模型
把"能否开跑"拆成两个正交关注点（审计原话"两道互不知情的闸"，现让它们各司其职）：

**闸① 会话互斥锁（正确性）** — 新建 `SessionRunRegistry`，键 = 完整 `SessionKey`：
- `try_claim(session_key) -> bool`：原子 Idle→Running。**只保证"一会话同时最多一个 run"**（守 INV-SEQ / 审计 4.2，防同会话事件交错损坏 transcript）。
- `release(session_key)`：run 结束（complete/error/cancel）释放。
- 第二条**同会话**消息仍走现有 `BusyInputMode`（Steer/Interrupt/Queue，`execution_engine/{mod,execute→gate}.rs`），但 busy 判定从 **per-agent 改为 per-session**。

**闸② 并发上限许可（资源/公平）** — cap 信号量，**许可持有到 run 生命周期结束**（修审计 1.4）：
- run spawn 前 acquire，run 终态（complete/error/cancel）drop。
- 三层上限见 §3.3。

### 3.2 退休 per-agent 闸
- `AgentInstance.state: Arc<RwLock<AgentState>>`（Idle/Running）不再作并发闸。`try_start_run` 的语义迁到 `SessionRunRegistry::try_claim(session_key)`。
- **审计项**：`AgentInstance` 除 `state` 外若还有"假设一 agent 一 run"的可变字段（缓存/累加器/last-run），一并 per-session 化或加锁——否则同 agent 并发 run 会在这些字段上竞争（隐性破坏 INV-ISO）。P1 实现前先枚举 `AgentInstance` 全部可变字段并逐一判定。
- Subagent 注意：`session_key.rs:227-236` 的 `Subagent{parent_key}.agent_id()` 返回**父** agent_id——迁移后 subagent 的会话互斥要按其**自身 session_key** claim，不要退回父 agent 粒度（否则又串行化了 subagent 与父）。

### 3.3 并发上限（config 连线，修死旋钮 C5/1.3）
三层，全部连进 `[gateway]`（对齐 §5.6 `[gateway.delivery_queue]` 的 `*TomlConfig → to_runtime()` 模式，坏配置 clamp 兜底）：

| 层级 | 语义 | 默认 | 可配 |
|---|---|---|---|
| per-session | 一会话最多几个 run | **1** | 否（硬不变量） |
| per-agent | 一 agent 最多几个并发会话 run | **3** | 是 |
| global | 全局最多几个并发 run | **8** | 是 |

默认保守可调。透出 "**N / M 槽在用**" 到 Panel（修审计 3.4 无 per-session 计费；先复用 `gateway.metrics.lanes` 快照的扩展）。

### 3.4 公平与排队（修 1.2 Panel 无队列 + C4 只 team 有公平）
- **cap 满时不 Fail 而是排队**：修掉审计 1.2 最刺眼的"Panel 第二 tab 直接 Failed"。P1 给 Panel 路径补**准入等待队列**（对齐 channel 路径已有的 `busy_queue`，但键从 agent_id 调整为准入语义）；不同会话各排各的。
- **per-agent sub-cap + FIFO 准入**避免单 agent 头阻塞。**不**引入 team-dispatcher 的完整 per-owner round-robin（YAGNI；per-agent sub-cap 已廉价封住饿死；真需要再升级）。
- **背压类型化**：cap 满 → 给 UI 可区分的"**已排队 / 队列位置 N**"信号（替代现泛化 `INTERNAL_ERROR "Service congested"` `handler.rs:644-648`）。run 不失败，排队等槽。

### 3.5 与前端路由的范围边界（重要）
P1 让**后端正确且并行**，但**帧级 `session_key` 盖章 + run_id 统一 + fan-out 隔离是 P2**，本轮不做。约束：**P1 不得回归现有路由**。
- 现状：除 `RunAccepted`/`SessionUpdated` 外，所有 run 中途帧只带 `run_id`（`events/frame.rs`）；前端靠 `RunAccepted` 建 `run_id→conv` map。
- 对"本会话内从 Panel 发起的并发 run"：run_id 唯一 + map 键为 run_id → **今天的路由已能正确区分两个同 agent 并发 run**。所以 P1 的并行**在主流程可观测正确**；只有"重连 / 订阅晚于 run 开始"的可靠性边缘留给 P2。

---

## 4. 数据流与依赖方向

```
chat.send / agent.run (RPC)
  → gate.rs:
      ① SessionRunRegistry.try_claim(session_key)   ← 会话互斥（正确性）
         ├ 已 Running → BusyInputMode(per-session): Steer/Interrupt/Queue
         └ Idle → 继续
      ② cap 许可 acquire(global, per-agent)          ← 并发上限（资源）
         ├ 满 → 准入队列排队，回吐"queued, pos N"（类型化背压）
         └ 得槽 → 继续
  → spawn.rs: tokio::spawn(run task)
      run task 内 TURN_CONTEXT.scope(该 run 的 SessionKey) { agent loop }
      所有 session_events append 走该会话 actor（INV-SEQ）
  → run 终态: release 会话互斥锁 + drop cap 许可 + 出队下一个
  → post_run.rs: topic-gen / 压缩 / goal-loop / strategy
```

依赖方向：`RPC → gate → registry/semaphore → spawn → actor(每会话) → post_run`。全在 `src/gateway/` 编排层，不反向依赖 harness/domain。

---

## 5. 测试与验证策略（TDD 先红后绿）

1. **单元 · `SessionRunRegistry`**：claim/release；同会话二次 claim 被拒；跨会话 claim 放行；cap 许可持有到 run complete（非 dispatch）——钉死 1.4。
2. **单元 · seq 自愈**：模拟撞键直写 → 断言 actor resync+重试自愈而非卡死——钉死 4.1。
3. **重构安全 · `execute.rs` 拆分**：现有 `execution_engine` 测试全绿（纯行为保持）。
4. **并发**：可行处用 `loom` feature 测 registry 闸 + seq 路径；否则 tokio 多线程压力测（同 agent 不同会话两并发 run）。
5. **隔离回归（INV-ISO）**：
   - ①不同 agent 并发 run 各写 note → 各落各自 agent 分区，零串写。
   - ②同 agent 不同会话并发 run → transcript 不交错（seq 每会话单调、无跨会话事件）+ 两条记忆写入落同分区不丢不死锁。
6. **行为集成**：两个 Panel tab 同 agent → 双 run 并行、都不 Failed（修 1.2）；cap 满 → 排队非失败；"N/M 槽"透出。

---

## 6. 非目标（本轮明确 OUT，各为已命名的下一 spec）

- **P2 事件路由 SSOT**：帧级 `session_key` 盖章 + run_id 统一（审计 2.1/2.2：一 run 现有三个 run_id）+ 重连重建 map RPC + per-session fan-out/projector 隔离（3.1 共享 1024 广播 Lagged 丢全会话帧；3.2 projector 单 drain + 4096 共享队列丢跨会话投影）。
- **P3 前端正确性**：SessionMap bug 集群——bind_run 双计数竞态（永久幽灵红点）、channel-run 劫持前台 tab（#2）、删会话不调 `close()` 泄漏、close/switch 死代码、Todo/plan 切换丢失、abandoned run GC、无持久化/重连恢复。
- **P4 前端 UX 上浮**：切换器 + per-tab agent 色 + running/done 徽章 + 状态计数头、键盘导航（Cmd+N/W/1-9）、有界事件缓冲+切换重放+草稿保存、后台会话审批上浮、jump-to-running。
- **P5 新能力**：自动标题、scope/搜索切换器、从某轮 fork/分支、原地 rewind/checkpoint、资源仪表盘、启动 reconcile 崩溃恢复、有界并发 shutdown。
- **P6 手机端多会话**（当前完全无；遵手机导航法则——无左右分屏、下钻式）。
- **不动**：agent 记忆分区模型、身份/soul、per-agent provider/config（INV-ISO 全部保住）。

---

## 7. 纪律

- **R10**：新机件全在 `src/gateway/`，不进 `src/harness/`。
- **cargo 节制**：至多在 P1 闸合并这一高风险点跑一次 `cargo check --lib`，其余靠定向单测。
- **提交**：English commit messages，`<scope>: <description>`（如 `gateway: per-session run registry replacing per-agent run gate`）。
- **单分支**：直接在 main。

---

## 附录 A. 关键代码锚点（起步导航）

| 关注点 | 锚点 |
|---|---|
| per-agent 闸（待退休） | `src/gateway/agent_instance.rs:360-375`（`try_start_run`）、`:120-131`（`AgentState`）、`:144`（`state`）、`agent_registry.rs:735-782` |
| 准入闸（待拆分+改造） | `src/gateway/execution_engine/execute.rs:108-1250`（`execute()` 巨石）、`:123-208`（gate + busy 分支）、`mod.rs:66-76`（死 count 限） |
| Panel 无队列 → Failed | `src/gateway/handlers/agent.rs:423-433`、`execute.rs:190-206` |
| Lane 许可 dispatch 即释放 | `src/gateway/lane.rs:16-22`、`server/handler.rs:640-643` |
| seq 双分配器竞态 | `src/session/actor.rs:94-115`、`:112-114`、`resume_coordinator.rs:322-343`、`harness_bridge/backfill.rs:50,61-63,94-99`、`session/store.rs:121-128`、`in_process.rs:65-98` |
| busy_queue（channel 侧参考） | `src/gateway/inbound_router/busy_queue.rs:40-63`、`executor.rs:402-462` |
| 隔离依据 | `src/tools/turn_context.rs:63`（task-local）、`src/builtin_tools/note_manage.rs:376`（resolve_agent_id） |
| subagent agent_id 陷阱 | `src/routing/session_key.rs:227-236` |
| config 连线模式参考 | `[gateway.delivery_queue]`（§5.6 FEATURE_LOCATOR）、`start/mod.rs:160`（`[gateway.lane]`） |
