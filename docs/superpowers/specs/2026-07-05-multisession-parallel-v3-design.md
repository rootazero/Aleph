# 多会话平行执行 v3 — 红点绝对同步 · 跨会话并行修复 · 版本化广播 · 上限热重载

- **日期**：2026-07-05
- **状态**：设计已批准（用户"确认"，§2 采用 Model A）
- **隔离**：worktree `.claude/worktrees/feat-multisession-parallel-v3`，分支 `feat-multisession-parallel-v3`（自 main `c6582536d` 切出）。严禁直接触碰 main；用户 review 后合并。
- **特征定位**：FEATURE_LOCATOR §4.10（多会话平行执行）
- **前置**：本功能已过两轮优化（per-session 互斥 `SessionRunRegistry`、global+per-agent 双信号量 `ConcurrencyLimiter`、`RunSlot` RAII 认领、`waiting` 队深、`running_sessions` 连线到 `gateway.metrics.run_concurrency`、Panel 侧栏红点雏形）。本 v3 在其上做红点正确性根治 + 跨会话并行 bug 修复 + 两项参考项目能力增强 + 熵减。

---

## 1. 背景与动机

用户核心诉求（三条追加指示）：**左侧会话列表标记"正在执行"的红点，必须与"是否真的在跑任务"绝对同步，绝不错误显示**（不该亮时亮 / 该亮时不亮都不行）。

审计发现的真实缺陷：

| # | 缺陷 | 证据 | 严重度 |
|---|------|------|--------|
| A | **假阳性（红点卡住不灭）** | `state/sessions.rs::is_running_session_key` 对本 Panel 追踪过的会话只认客户端 `running` refcount、故意不让权威 `server_running` 纠偏（`sessions.rs:318-320`）；漏收一个 `run` 完成事件 → `running[conv]>0` 永不清零 → 红点长亮 | HIGH |
| B | **假阴性（在跑却不亮）** | `run.session_updated` 仅在首条消息 start（`execute.rs:222`）与完成（`execute.rs:577`）emit，**非首 run 的 start 不 emit** → `server_running` 不刷新 → 别的接口在已存在会话上发起的第 2+ 个 run 不亮点 | HIGH |
| C | **持久化 state 死链** | `set_running`/`set_session_running` 全代码库零生产调用（`session_manager/ops/modify.rs:479`、`session_store/mod.rs:247`、`sqlite_backend/mod.rs:541`、`file_backend/mod.rs:1065`）；engine 只在完成时调 `set_idle`（`execute.rs:1100`），崩溃把 `sessions.list.state` 永久钉在 `Running` | MEDIUM（熵 + 隐患） |
| D | **跨会话并行在通道路径失效** | inbound busy 队列 `busy_queue.rs` 仍按 `agent_id` 分桶（`busy_queue.rs:45` + `executor.rs:406`）；Task 6 把 engine 闸迁到 per-session 时忘了同步迁移这里 → 同 agent 不同会话从通道进来被串行化，且 engine 的 Steer/Interrupt/Queue 分支被架空从不触发 | HIGH |
| E | **并发上限不可热重载** | `ConcurrencyLimiter` 信号量 boot 时定尺寸（`engine.rs:136`），无 watcher 重接；改 `[execution] max_runs_*` 需重启 | LOW（体验） |
| F | **`SimpleExecutionEngine` 静默 0/0** | 其 `ExecutionAdapter` impl（`simple.rs:441-464`）不覆写 `concurrency_snapshot`/`running_sessions` → 若当生产 adapter，metrics 静默返回全零，看起来正常的假 0 | LOW |
| G | **死变体** | `RunState::{Queued, Paused}`（`execution_engine/mod.rs:265-270`）零构造 | LOW（熵） |

## 2. 参考项目 Gap Analysis（架构映射，非复制）

对标 `T:\Github\{codex, kimi-cli, pi}`：

- **codex**：`AgentExecutionLimiter`（原子计数 + RAII `AgentExecutionGuard`，硬失败不排队）、`thread_created_tx: broadcast` 让客户端发现/挂入新会话、per-agent `watch::Receiver<AgentStatus>` 状态订阅、100ms 优雅中断窗。**无全局并发上限、无队深可观测**——Aleph 在这两点已领先。默认 busy-input = **steer-first**（`user_input_or_turn_inner` → `steer_input` → fallback `spawn_task`），Aleph 已默认 Steer，无需移植。
- **kimi-cli**：Python，**进程/会话各跑各**；`seq` 单调版本化 `SessionStatus` 状态机（`web/runner/process.py:133-176`）broadcast on transition + "join-while-running" replay-then-flush 让 mid-run 观察者不丢事件——**这是本 v3 版本化广播的直接映射源**。无全局并发上限（背景任务 `max_running_tasks=4` 硬拒绝）。
- **pi**：TypeScript，**进程-每-会话** + 薄 supervisor；双队列 `steeringQueue`（回合间注入）/`followUpQueue`（完成后续跑），各自 `all` vs `one-at-a-time` drain 模式；`queue_update` 事件带全量快照。**无全局并发上限、无跨会话公平、无聚合可观测**。

**结论**：Aleph 的并发核心原语（全局+per-agent 双上限 + await 不拒绝 + 队深可观测）**领先三库**。可移植的高价值项 = kimi/codex 的**版本化状态广播**（服务红点绝对同步）+ **并发上限热重载**（自 Aleph 既有 §5.8 hot-reload 链）。**不移植**（YAGNI）：codex LRU 驻留淘汰（大子系统，无内存痛点）、codex `MailboxDeliveryPhase` fold-vs-defer（过度设计）、pi 每队列 drain 模式（UX 细节，价值小）。

## 3. 守住的不变量（不动）

- **INV-SEQ**：一会话至多一个 Running run（`SessionRunRegistry` 权威）。
- **INV-ISO**：并行 run 间记忆/存储按 `agent_id` 物理隔离。
- **两条身份轴**：session=并行/红点/transcript 单元；agent=记忆/存储/子上限单元。**别混。**
- **RAII 认领**：`RunSlot` 认领即构造、drop 即释放（第二轮成果，不回退到"先 try_claim 后建 slot"）。
- **session-ssot 单写者**：`session_events` 唯一写者不新增第二个（见 `[[session-ssot-single-writer]]`）。

---

## 4. 设计

### §4.1 版本化运行态广播（地基，先建）

`SessionRunRegistry` 增单调 `seq: AtomicU64`，**每次 `try_claim` 成功与每次 `release` 生效各 bump 一次**。注册表在状态变更后经既有事件总线推一个轻量事件：

```
run.running_set { seq: u64, running: Vec<String>, concurrency: ConcurrencySnapshot }
```

- 单一真源 = registry 内存态（非持久 store 那个 crash 会过期的 cosmetic 标记）。
- 补齐当前"只在首条 start + 完成时 emit"的缺口：**非首 run 的 start 现在也推**（根治缺陷 B）。
- `seq` 单调，消费端保留"忽略更旧 seq"守卫，漏收的中间事件在下一个事件到达时**自愈**。
- 对标 kimi `seq` 状态机 + codex `thread_created` 广播，映射到 Aleph 事件总线 + Rust `AtomicU64`。

**连线点**：`SessionRunRegistry` 需要一个 emit 回调/事件 sink 引用（构造时注入，保持 registry 对 gateway 事件层零硬依赖——传入 `Arc<dyn Fn(RunningSet)>` 或复用 engine 现有 emitter）。emit 时机在 `try_claim` 返回 `true` 后、`release` 实际 remove 后。

### §4.2 红点绝对同步（核心，Model A）

侧栏每会话红点 `is_running_session_key` 改为**最新 seq 服务端运行集的纯函数**：

- **只读 `server_running`**（由 §4.1 事件实时刷新，`SessionMap` 增 `server_seq: RwSignal<u64>`，`set_server_running` 带 seq 守卫忽略乱序旧包）。
- **不再用客户端 `running` refcount 判红点**（删除 `is_running_session_key` 的 tracked 分支对 `running` 的依赖）。
- `running` refcount + `route`（run_id→ConvId）**保留**给 chunk 路由与活跃视图乐观态——**外科手术式改动只碰侧栏红点入口 `is_running_session_key`**，不动 `bind_run`/`settle_run`/`route`/`is_running(conv)`。
- 根治**假阳性 A**：release 一定推事件 → 红点由权威集合清掉，没有客户端 refcount 能钉住。
- 根治**假阴性 B**：任何接口（daemon/Telegram/别的 Panel/非首 run）claim 都推事件 → 亮点。
- 代价：用户按下发送到红点亮，有 §4.1 事件一个来回（本地 IPC ~毫秒级）的极小延迟——可接受，换"构造上不会错"。

**慢速 poll 兜底保留**：Usage 视图 `RunSlotsCard` 与侧栏 seeding 之外，保留一条慢速 poll 作 belt-and-suspenders（并顺带修 `usage.rs` `RunSlotsCard` 从不刷新的问题——connect 时一次性拉取改为随 `run.running_set` 事件更新或慢速 poll）。

**事件消费连线**：webchat 侧订阅 `run.running_set`（`interfaces/webchat/src/api/` + dispatcher），转 `SessionMap::set_server_running(seq, keys)`。

### §4.3 跨会话并行修复（FIFO 重键 `agent_id` → `session_key`）

`busy_queue.rs` 本身 key 无关（吃 `&str`），改动**外科式**：

- `executor.rs` 的三个调用点（register `:415` / is_front `:425` / remove `:459`，key 来自 `:406` `agent_key = request.session_key.agent_id()`）把键换成 `request.session_key.to_key_string()`。
- 订正 `busy_queue.rs` 头部与 `:29-32` 那段 Task-6 后过期的模块文档（"per-agent" → "per-session"，"try_start_run gate" → "SessionRunRegistry gate"）；`MAX_QUEUED_PER_AGENT` 语义改为 per-session（常量名可保留或重命名 `MAX_QUEUED_PER_SESSION`，二选一，倾向重命名以名副其实）。
- **效果**：同 agent 不同会话从通道进来不再互相串行阻塞 → §4.10 "真并行"对通道路径成立；同会话消息仍 FIFO 保序；engine 的 Steer/Interrupt/Queue 分支对同会话并发消息真正生效（此前被 FIFO 推迟到 post-completion 从不触发）。
- per-agent 并发上限（3）的背压由 `ConcurrencyLimiter::acquire().await` 承接，与本队列正交，不冲突。
- **回归测试**：同 agent 两不同会话互不阻塞（两 `is_front` 各自为 true）；同会话 FIFO 保序（现有测试改键后仍过）。

### §4.4 并发上限热重载

`ConcurrencyLimiter` 加 `reconfigure(global_cap, per_agent_cap)`：

- global 信号量：`Arc<Semaphore>` 改 `Mutex<Arc<Semaphore>>`（或 `arc_swap::ArcSwap`）**整体换新**（tokio 1.35 信号量只能 `add_permits` 加、不能缩，rebuild-swap 最稳且版本安全）。
- `global_total` / `per_agent_cap` 改 `AtomicUsize`。
- 清空 `per_agent` map，让各 agent 信号量按新 cap 懒重建。
- 在飞的旧 permit 持旧 `Arc<Semaphore>` 存活到 drop → 过渡期总量至多"旧在飞 + 新 cap"、很快收敛（文档写明这个瞬态过冲是有意接受的）。
- `acquire`/`try_acquire`/`snapshot` 改为通过 `Mutex<Arc<Semaphore>>`/`AtomicUsize` 读当前值（读一次克隆 `Arc` 再 await，避免持锁跨 await）。
- `reload_impact.rs`：把 `execution` 段从默认 Restart 归类到 **Live**；订阅既有 `ConfigChanged` 广播（§5.8 `handle_patch_config` 发的）→ 调 `ExecutionEngine` 暴露的 `reconfigure_concurrency(global, per_agent)` → `limiter.reconfigure(...)`。
- 改 `[execution] max_runs_*` 不再需要重启（下轮 admission 即用新上限）。

### §4.5 熵减

- 删死变体 `RunState::{Queued, Paused}`（`execution_engine/mod.rs:265-270`，零构造）。
- 删死的 `set_running`/`set_session_running` 链（缺陷 C）：`session_manager/ops/modify.rs`、`session_store/mod.rs`、`sqlite_backend/mod.rs`、`file_backend/mod.rs` 四处 `set_running`，及其 trait 声明。`set_idle` 保留（`execute.rs:1100` 在用）。红点唯一 SSOT = 内存 registry；`sessions.list.state` 若仅为红点服务则一并停止序列化该字段（`query.rs:79/290`），否则保留但文档标注它非红点数据源。
- `SimpleExecutionEngine`（`simple.rs:441-464`）覆写 `concurrency_snapshot`/`running_sessions`（与主 engine 一致），杜绝缺陷 F 的假 0；若 Simple 无真实 limiter，则返回明确的"unsupported"信号而非全零。

---

## 5. 变更面（文件清单）

**后端 `alephcore`：**
- `src/gateway/execution_engine/session_run_registry.rs` — 加 `seq` + emit 回调（§4.1）。
- `src/gateway/execution_engine/concurrency.rs` — `reconfigure` + 内部可变 caps（§4.4）。
- `src/gateway/execution_engine/engine.rs` — 暴露 `reconfigure_concurrency`；registry 构造注入 emit sink。
- `src/gateway/execution_engine/execute.rs` — 确认 emit 覆盖非首 run start（§4.1）；不新增 session_events 写者。
- `src/gateway/execution_engine/mod.rs` — 删 `RunState::{Queued, Paused}`（§4.5）。
- `src/gateway/execution_engine/simple.rs` — 覆写 adapter 两方法（§4.5 F）。
- `src/gateway/inbound_router/{busy_queue.rs, executor.rs}` — FIFO 重键 + 文档订正（§4.3）。
- `src/gateway/handlers/gateway_metrics.rs` — `run.running_set` 事件形状（若复用 metrics 出口）。
- `src/config/reload_impact.rs` — `execution` → Live（§4.4）。
- `src/gateway/session_manager/ops/modify.rs`、`session_store/{mod,sqlite_backend/mod,file_backend/mod}.rs`、`handlers/session/db_handlers/query.rs` — 删 `set_running` 死链（§4.5 C）。
- 配置热重载订阅连线点：`src/bin/aleph-server/commands/start/builder/agent_init/` 或既有 `ConfigChanged` 订阅者。

**前端 `aleph-panel`（webchat）：**
- `interfaces/webchat/src/state/sessions.rs` — `is_running_session_key` 改纯服务端权威 + `server_seq` 守卫（§4.2）。
- `interfaces/webchat/src/api/system.rs` + dispatcher — 订阅 `run.running_set` 事件 → `set_server_running(seq, keys)`。
- `interfaces/webchat/src/components/chat_sidebar.rs` — seeding 改由事件驱动（保留慢速 poll 兜底）。
- `interfaces/webchat/src/platform/wide/views/usage.rs` — `RunSlotsCard` 随事件刷新（修从不刷新）。

## 6. 验证口径（Windows）

- 后端 scoped 测试：
  - `session_run_registry`：seq 单调、claim/release 各 bump、emit 触发。
  - `concurrency`：`reconfigure` 扩/缩 cap 生效、瞬态过冲收敛、既有 4 测不回归。
  - `busy_queue`：同 agent 两会话不互阻、同会话 FIFO 保序、overflow/GC 不回归。
  - `gate`：`run_slot_releases_session_claim_on_drop_without_permit` 等不回归。
- `cargo check -p alephcore --lib` clean。
- `cargo check -p aleph-panel --target wasm32-unknown-unknown` clean（红点纯函数 + 事件订阅）。
- 排除 `aleph-desktop-macos` / `aleph-desktop-linux`（Windows verify scope）。
- `rustfmt` **只格式化实际改动文件**（`rustfmt <files>`，勿用会顺 `mod` 递归卷入兄弟文件的 `rustfmt mod.rs`；repo 有既存 fmt drift）。
- 遵守 `alephcore` 构建吃内存教训：scoped test，`CARGO_PROFILE_TEST_DEBUG=line-tables-only` 防 lib-test OOM，不全量 test。
- E2E 待定：真实 Panel 观察红点在跨接口/并发/漏事件下的同步（本地部署验证）。

## 7. 明确不做（YAGNI / 越界）

- codex V2Residency 空闲会话 LRU 淘汰到磁盘（大子系统，无内存痛点 → 另开项目）。
- codex `MailboxDeliveryPhase` CurrentTurn↔NextTurn fold-vs-defer（过度设计）。
- pi 每队列 `all` vs `one-at-a-time` drain 模式（UX 细节）。
- 全局"最大会话数"上限（Aleph 已有 run 级双上限，会话数无界是有意设计）。
- 跨会话优先级/加权公平调度（三个参考项目均无，非本次目标）。

## 8. 风险与开放项

- **§4.4 信号量 rebuild-swap 的瞬态过冲**：文档已声明为有意接受；若需精确无过冲，未来可升 tokio 到有 `forget_permits` 的版本再优化——本次不做。
- **§4.1 emit sink 注入方式**：registry 应保持对 gateway 事件层的低耦合。实现时二选一：(a) 构造注入 `Arc<dyn Fn>`；(b) engine 在 claim/release 后主动 emit（registry 只暴露 seq + running_keys）。倾向 (b) —— registry 保持纯数据结构，emit 归 engine，连线更清晰、测试更易。
- **continuation-vs-steering-rescue 竞争**（`execute.rs:535` drop → 后续 continuation/rescue 抢 freed slot）：现状 benign-by-swallow，本次不改，但在触及 lifecycle 时补一条不变量注释。
- **持久化 `sessions.list.state` 去留**：若别处（非红点）仍消费该字段，则保留字段但停止把它当红点源；实现时 grep 确认消费者再决定删字段还是仅改文档。
