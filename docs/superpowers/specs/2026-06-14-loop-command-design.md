# `/loop` 命令设计文档

**日期**: 2026-06-14
**分支**: `loop-command`
**状态**: 设计已确认，待写实现计划

## 1. 背景与动机

参考文章《Goal + Loop + Workflows 三大利器》把三个命令定义为三个**正交维度**：

| 命令 | 本质 | 由什么驱动 | 智能体数量 |
|------|------|-----------|-----------|
| `/goal` | 自主跑到目标达成为止 | 条件满足才停 | 1 个（当前会话，多轮） |
| `/loop` | 按节奏重复同一件事 | 时间间隔到点就跑 | 1 个（当前会话，多轮） |
| `/workflows` | 多智能体并行编排监控 | 看已启动 workflow 进度 | 多个（并行） |

`/goal` 管终点，`/loop` 管节奏，`/workflows` 管规模。`/goal` 和 `/loop` 都是**单会话、多轮、一个 Claude**，唯一区别是停止逻辑——`goal` 看目标（达标即停），`loop` 看时钟（到点就跑、永不自停）。

### Aleph 现状（Gap Analysis）

| 文章概念 | Aleph 现有基础设施 | 状态 |
|----------|--------------------|------|
| `/goal` | `goal` builtin tool + `goal_pursuit` + `spawn_continuation_run`（同会话续跑）+ `deadline_ms`/`pursuit_max_iterations`/gate/lessons | ✅ 已成熟 |
| `/loop` | `cron_manage` + `tasks/cron/*`（`ScheduleKind::Every` 间隔 / `Cron` 表达式 / `At` 一次性） | ⚠️ 形态错位 |
| `/workflows` | `teams` / 多 agent 系统 | ✅ 已存在 |

**核心缺口（错位而非缺失）**：Aleph 的 `cron` 是**重量级持久化作业**——存进 `tasks.db`、跨重启存活、在**独立 spawned session** 里跑、带 catchup / failure-alert / timezone。而文章的 `/loop` 是**轻量级、当前会话、随会话消亡**的"定时续跑"。它和 `/goal` 共享同一套"同会话多轮"机制（`spawn_continuation_run`），唯一区别是把"条件门"换成"时钟门"。

这是一个"连线优先"机会：`/loop` 复用 `goal` 的同会话续跑机器 + `cron` 的间隔语义，几乎不需要新建持久化层。

## 2. 已确认的设计决策

1. **形态 = 轻量·当前会话**：复用 `goal` 的 `spawn_continuation_run` 同会话续跑机器，把"条件门"换成"时钟门"，随会话消亡、**不进 `tasks.db`**（内存态）。
2. **停止语义 = 默认无限·保留可选闸**：贴合文章"只要不手动停就永远转"；默认 `loop(action='stop')` 才停。但复用 goal 的 `max_iterations` / `deadline_ms` / `token_budget` 作为**可选**安全网；unattended 无人值守场景自动注入软上限防烧爆。
3. **节奏 = 固定间隔 + 模型自定 两模式**：填了 `interval` 走固定间隔；不填则模型在某一轮调 `loop(action='update', next_wake='8m')` 自设下次延迟（结构化 tool call，不解析自由文本，贴合 P8）；不调则用 fallback 延迟。

## 3. 采用方案：内存 LoopRegistry + 复用续跑机器 + 延迟门

### 被否决的替代方案

- **方案 B（cron Every 门面）**：`/loop` 直接建轻量 cron job。复用最多，但 cron executor 跑在独立 spawned session，语义偏离"当前会话"；且要给 cron 加"非持久"特例污染 cron 子系统。被"形态=当前会话"决策排除。
- **方案 C（独立 LoopService + 自己的 tokio 定时循环）**：不复用续跑机器，loop 自起定时循环。完全独立但**重复**了续跑 / origin-fanout / unattended-tax / 事件广播一整套机器，违背连线优先、制造平行实现。

### 为何选 A

新增代码最小（1 个新模块 + 1 个 tool + 1 处 hook + 1 个参数），最大化复用 goal 已验证的续跑 / 广播 / fail-closed 链路，与 goal 形成干净的"条件门 vs 时钟门"对称。符合 R3（核心轻量化）/ R10（thin harness）/ P6（KISS & YAGNI）。

## 4. 架构详细设计

### 4.1 数据模型 `src/looping/types.rs`

> 模块名用 `looping`，因为 `loop` 是 Rust 关键字。用户面 tool / 命令仍叫 `loop`。

```rust
/// 节奏：固定间隔 or 模型自定
pub enum Cadence {
    Fixed { interval_ms: u64 },
    ModelPaced { fallback_ms: u64 },
}

pub enum LoopStatus { Active, Stopped }

/// 单会话的循环状态（内存态，随会话消亡）
pub struct LoopState {
    session_id: String,
    prompt: String,             // 每 tick 原样重注入的固定 prompt
    cadence: Cadence,
    next_wake_ms: Option<u64>,  // ModelPaced 下模型设定的下次延迟（绝对 epoch ms 或相对，实现时定）
    iterations_used: u32,
    max_iterations: Option<u32>, // 可选闸
    deadline_ms: Option<u64>,    // 可选闸（绝对 epoch ms）
    token_budget: Option<u64>,   // 可选闸
    status: LoopStatus,
    created_at_ms: u64,
}
```

- **不可变更新风格**：`with_*` 方法返回新副本，照搬 `goal/types.rs` 已建立范式（P 不可变性）。
- `serde` + 旧 payload 兼容测试（虽是内存态，但保持与 goal 一致的序列化纪律，便于将来若需 status 查询走 JSON）。

### 4.2 内存注册表 `src/looping/mod.rs`

镜像 `goal/mod.rs` 的 `OnceCell<Arc<...>>` 全局单例，但后端是**纯内存**：

```rust
static GLOBAL: OnceCell<Arc<LoopRegistry>> = OnceCell::new();
// LoopRegistry 内部 = Mutex<HashMap<String /*session_key*/, LoopState>>

pub fn init_global(registry: Arc<LoopRegistry>);  // boot 时调用
pub fn global() -> Option<Arc<LoopRegistry>>;     // None 时整个 loop 子系统休眠
```

- daemon 重启即清空 = "随会话消亡"语义的物理保证。
- 锁安全：`.lock().unwrap_or_else(|e| e.into_inner())`（P7）。

### 4.3 续跑门 `src/looping/pursuit.rs`

镜像 `tasks/goal_pursuit.rs`，但更轻（纯函数，约 100 行）：

- `should_fire(&LoopState, now_ms, now_tokens) -> bool` — 未 Stopped 且未超任何已设 cap。
- `exhausted(&LoopState, now_ms, now_tokens) -> bool` — 达到某个 cap（用于标 Stopped + 记原因）。
- `tick_delay_ms(&LoopState, now_ms) -> u64` — `Fixed` 取 `interval_ms`；`ModelPaced` 取 `next_wake_ms` 相对值，无则 `fallback_ms`。
- `tick_prompt(&LoopState) -> String` — 固定 `prompt` + 一行系统提示："这是定时循环第 N 轮；要改节奏调 `loop(action='update', next_wake='8m')`，要停调 `loop(action='stop')`。"
- `cap_reached_note(&LoopState) -> String` / `deadline_reached_note(...)` — 镜像 goal，停止时给用户的原因。

### 4.4 Builtin 工具 `src/builtin_tools/loop_manage.rs`

注册名 `loop`，经 `ToolCatalog` 自动成为 `/loop` 斜杠命令（R8 工具即一切）。镜像 `cron_manage.rs` / `goal.rs` 的 schema 风格。

**actions**：

- `start`：`interval`（人类格式 `"5m"`/`"30s"`/`"2h"`，**省略 → ModelPaced**）、`prompt`（必填）、可选 `max_iterations` / `timeout_minutes` / `token_budget`。在本 session 注册一个 Active LoopState。
- `stop`：标本 session loop 为 Stopped。
- `status`：返回当前 loop 状态（节奏、已跑轮数、caps、下次唤醒）。
- `update`：模型自设 `next_wake`（仅 ModelPaced 有意义），或调整 caps。

工具描述要讲清与 goal 的对照：**到点就跑、永不自停、要 stop 才停**；并说明 ModelPaced 模式下应在每轮结束前调 `update` 设下次节奏。

**人类间隔解析**：`"5m"`→`300_000ms` 等。复用或镜像 cron 已有的间隔校验（`every_ms < 1000` 拒绝，见 `cron_manage.rs:103`）。

### 4.5 接线 hook `src/gateway/execution_engine/execute.rs`

在现有 goal 续跑 hook（约 619–766 行）**旁边**加 loop 分支，结构对称：

```text
run 完成
  └─ if let Some(cont_deps) = self.continuation_deps.get():
       ├─ [现有] goal 分支：goal::global() → should_continue → spawn_continuation_run(delay=None)
       └─ [新增] loop 分支：looping::global() → 查本 session LoopState
            ├─ should_fire → iterations_used+1 持久化 → spawn_continuation_run(
            │      prompt = tick_prompt(),
            │      delay_ms = Some(tick_delay_ms()))
            └─ exhausted → 标 Stopped + 记 cap/deadline note
```

复用 goal 已有的 origin-fanout（多端推送）/ unattended（无人值守安全税）/ fail-closed（续跑失败处理）链路——**零重复**。

> **goal 与 loop 互斥性**：同一 session 同时有 goal 和 loop 的情况极少。实现时 loop 分支独立判断，不与 goal 分支冲突（两者都可各自 spawn 续跑，但实践上用户只会用其一）。实现计划阶段确认是否需要"同 session 二选一"护栏。

### 4.6 `spawn_continuation_run` 签名扩展（熵减点）

`execute.rs:868` 现有函数加 `delay_ms: Option<u64>` 参数：

```rust
fn spawn_continuation_run(
    /* …现有参数… */,
    delay_ms: Option<u64>,   // 新增
) {
    // …
    tokio::spawn(async move {
        if let Some(d) = delay_ms {
            tokio::time::sleep(Duration::from_millis(d)).await;
        }
        // …现有 execute 逻辑不变…
    });
}
```

- goal 的两处 callsite（gate-failure 续跑、should_continue 续跑）传 `None` —— **行为完全不变**。
- loop 传 `Some(interval)`。
- **收敛"立即 vs 延迟续跑"两条路进单函数**，消除潜在的平行 spawn 样板。这是本特性唯一的重构动作。

### 4.7 安全网（unattended 软上限）

unattended 续跑（`metadata["unattended"]="true"`）里若 loop 未设任何 cap，默认注入软 `max_iterations`（镜像 goal 的 unattended 处理）。防 24/7 daemon + 无人值守自主路径叠加烧爆 token。

### 4.8 boot 接线

daemon 启动时（`constructor.rs` 附近，与 `goal::init_global` 同址）调 `looping::init_global(Arc::new(LoopRegistry::default()))`。`continuation_deps` 已有，loop 复用同一 `ContinuationDeps`。

## 5. 清理旧代码（熵减）

- 本特性**以新增为主**。grep 确认无遗留 loop stub / 死代码。
- 唯一重构：`spawn_continuation_run` 的"立即 vs 延迟"两路收敛进单函数（4.6），消除潜在平行 spawn 样板。
- 无需删除现有 cron / goal 任何代码——loop 与它们正交共存。

## 6. 测试策略

纯函数为主，**不依赖真 daemon**：

- `looping/types.rs`：`with_*` 不可变更新、序列化兼容、caps 字段默认值。
- `looping/pursuit.rs`：`should_fire` / `exhausted` / `tick_delay_ms` 边界（零间隔、超 max_iterations、过 deadline、ModelPaced 有/无 next_wake 的 fallback、token 超 budget）。
- `loop_manage.rs`：各 action 单测（start 注册、stop 转 Stopped、status 返回、update 设 next_wake、间隔解析 `"5m"`→ms、sub-second 拒绝）。
- hook 集成点：尽量以 pursuit 纯函数覆盖判定逻辑，spawn 路径靠现有 goal 续跑测试间接覆盖。

## 7. 架构红线核对

- **R3 核心轻量化** ✅：loop 只加调度门，不引重依赖、不搬砖。
- **R7 LLM 主权** ✅：loop 不做意图判断；模型自设节奏走 tool call。
- **R8 工具即一切** ✅：loop 是 builtin tool，`/loop` 自动派生；start/stop/update 全是自然语言可驱动的工具操作。
- **R10 薄 Harness / 笨循环** ✅：loop 的调度门放在 `src/looping/` 与 `execute.rs` 续跑 hook，**不进 `src/harness/`**（与 goal 同址，不增 harness 12 文件 / ~4900 行预算）。
- **P6 KISS & YAGNI** ✅：内存态、复用续跑机器、不预留持久化。

## 8. 文件清单

**新增**：
- `src/looping/mod.rs`（内存注册表 + 全局单例）
- `src/looping/types.rs`（LoopState / Cadence / LoopStatus）
- `src/looping/pursuit.rs`（续跑门纯函数）
- `src/builtin_tools/loop_manage.rs`（`loop` 工具）

**修改**：
- `src/gateway/execution_engine/execute.rs`（加 loop 续跑分支 + `spawn_continuation_run` 加 `delay_ms`）
- `src/builtin_tools/mod.rs`（注册 `loop` 工具）
- `src/lib.rs`（`pub mod looping;`）
- boot 接线点（`constructor.rs` 附近，`looping::init_global`）

## 9. 验收标准

1. `loop(action='start', interval='5m', prompt='检查部署状态，有变化就告诉我')` → 本 session 每 5 分钟自动续跑该 prompt，永不自停。
2. `loop(action='start', prompt='盯着 CI', max_iterations=20)` 无 interval → ModelPaced，模型每轮可调 `update` 设下次节奏；20 轮后自动 Stopped。
3. `loop(action='stop')` → 立即停止（下次 tick 不再 fire）。
4. `loop(action='status')` → 返回节奏、已跑轮数、caps、下次唤醒。
5. daemon 重启 → loop 状态清空（随会话消亡验证）。
6. goal 现有续跑行为零回归（`spawn_continuation_run` 加参后两处 callsite 传 `None`）。
