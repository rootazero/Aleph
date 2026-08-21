# Per-Principal Spend Budget — Design Spec
# 按主体的花费预算 — 设计规格

- **Date**: 2026-08-21
- **Status**: Approved design (brainstorming complete; awaiting implementation plan)
- **Branch**: `multiuser-round7`, worktree `/Volumes/TBU4/Workspace/Aleph-mu7`, based on `multiuser-round6` (unmerged)
- **Reference**: qm (`/Volumes/TBU4/Github/qm`) — `src/ratelimit/budget.ts` + `src/ratelimit/postgres-budget.ts`
- **Origin**: 多用户 §5.22 round-6 的显式遗留项。Round-6 修的是**请求**轴（`RateLimitLayer` 从字面量 `"rpc"` 改成 per-principal）；花费轴当时刻意未做，因为它需要新配置 + 聚合 + 拒绝路径，是独立一轮。

---

## 1. 问题陈述 (Problem)

一台多用户 Aleph 上，**没有任何东西限制一个主体能花多少钱**。请求轴（round-6）限的是「每分钟几次调用」，那约束不了成本：一次 `chat.send` 可以是 500 token，也可以是一个跑 200 轮、扇出十个子代理、每轮 200k 上下文的 run。两者在请求轴上完全等价。

后果是不对称的：**API 账单挂在机主头上，而花钱的可以是任何一个成员**，且机主既没有上限也没有事后按人拆账的读取面。单机形态同样缺一半——一个跑飞的 loop 可以在一夜之间烧掉任意金额，没有刹车。

## 2. 已有的零件（这一轮是接线，不是从零造）

| 零件 | 位置 | 状态 |
|---|---|---|
| **价格表** | `src/pricing.rs`（2804 行；长上下文分档、cache read/write 分价、vendor 回退） | `estimate(provider, model, &TokenBreakdown) -> CostEstimate { usd, status }`。⚠️ 模块 doc 逐字写着 **"pricing is best-effort, never a gate"** |
| **唯一计量漏斗** | `MeteringProvider::record_usage`（`src/providers/metering.rs:61`） | `process()` 与 `execute_streaming_dyn()` 共用它。子代理、MoA 每 advisor、compactor 各自独立 wrap（源码注释自陈「wrapping that one again would double-count」）⇒ **每一分钱都从这一个函数流过** |
| **施动者** | `scope::current_room_author()` ← `AUTHOR_USER_KEY` | 已在 `CarriedAttribution` 的六个 task-local 里，**跨 `tokio::spawn` 自带**；生产者有 census 测试 `scope_stamping_producers_are_all_accounted_for` |
| **主体登记表** | `users`（`SecurityStore`，与 round-6 的 `security_audit_log` 同库） | 账本的天然邻居 |
| **单一错误呈现源** | `ExecutionError::user_receipt()`（`src/gateway/execution_engine/mod.rs:445`） | in-engine `RunError` 帧、`agent.run`/`chat.send` RPC、channel 错误回复三个面都经它 |
| **准入闸** | `ExecutionEngine::admit_run`（`gate.rs:89`）；两个引擎的共用 helper 先例 `run_loop::ensure_session_under_request_scope` | 排队在**上游** `busy_queue`，所以引擎内的检查天然发生在**执行时刻** |
| **既有 run 级上限族** | `TerminateReason::BudgetExhaustedPartialResult { reason }` + cron carry-over | 本轮**不动** |

**明确不能用的**：`sessions.estimated_cost_usd`。它按 `owner_user_id` 键控，而那是**行主**——共享房间里所有成员的花费都记在创建者名下。（判据：「『谁拥有』回答不了『谁在问』」。）

**顺带确认的事实**：`RateLimitConfig` 全仓每个构造点都是 `::default()`，**round-6 修好了轴，限额本身至今不可配**。所以本轮的新配置没有现成的 `[gateway]` 限额段可挂。

## 3. 已锁定的四个决策 (Settled Decisions)

| # | 问题 | 决策 |
|---|------|------|
| D1 | 闸在哪一层咬合 | **两层**：准入给话术 + 每次 LLM 调用是不可绕过的地板。两处调**同一个**谓词 |
| D2 | 未定价模型（`CostStatus::Unknown`） | **单独计数并在每张回执上出声**。usd 一个字节不动，`unpriced_calls += 1` |
| D3 | 窗口语义 | **日历周期**（`"month"` / `"day"`，到点归零） |
| D4 | 层数 | **per-principal + 全局总额**，双层，各自是**有名字的枚举**而非共用两个数字字段 |

### 3.1 为什么不照抄 qm

qm 的 `budget.ts` 是同一问题的最小实现，但有四处本仓判据清单点过名的形状，逐条不移植：

1. **`estimateCostUsd(inputTokens, DEFAULT_AGENT_INPUT_USD_PER_MTOK)`** —— 只按 input token、一个默认单价，output token 免费。本仓有逐模型的 input/output/cache/reasoning 分价表。判据：「能被精确回答的数字别用常量猜」。
2. **`check()` 在个人额度通过后，把 org 的数字塞进同名字段返回** —— 调用方打印 `budget exceeded ($X of $Y)` 却不知道说的是哪一层。判据：「一个布尔够挡住调用，不够解释调用」。
3. **`record()` 是 `void` 的 fire-and-forget**，postgres 版 `catch` 后 `console.error` —— 写失败＝静默少记＝闸形同虚设。
4. **内存版重启即归零**，且它是无 `databaseUrl` 时的默认实现。判据：「纯内存的注册表在进程消失后不是『空了』，是『撒谎了』」——对预算而言，重启就是赦免。
5. **`record()` 同时写 `principalId` 与 `@org` 两行** —— 同一事实两个写者。本设计里全局总额是 `SUM` 派生的。

---

## 4. 设计 (Design)

### 4.1 命名与落点

仓里已有两个 `budget`：`src/context/budget/`（上下文 token 预算）与 `TerminateReason::BudgetExhaustedPartialResult`（run 级迭代/输出上限）。**本轮的东西一律叫 `spend`**：

```
src/spend/
  mod.rs      SpendLedger trait · Verdict/Limit/Spent 类型 · install()/global() · SpendPolicy 解析
  sqlite.rs   耐久实现（SecurityStore 同一个库）
  period.rs   日历周期边界（period_start / period_end）
```

配置段 `[policies.spend]`。不共用 `budget` 这个词，省得三个月后有人把两件事读成一件。

### 4.2 账本（聚合）

**按周期一行，不是逐事件一行**：

```sql
CREATE TABLE IF NOT EXISTS spend_ledger (
  principal_id    TEXT    NOT NULL,
  period_start    INTEGER NOT NULL,   -- epoch ms，周期起点（含）
  usd             REAL    NOT NULL DEFAULT 0,
  unpriced_calls  INTEGER NOT NULL DEFAULT 0,  -- CostStatus::Unknown
  partial_calls   INTEGER NOT NULL DEFAULT 0,  -- CostStatus::PartialMissingPrice（值是下界）
  updated_at      INTEGER NOT NULL,
  PRIMARY KEY (principal_id, period_start)
);
```

- 日历周期让聚合塌成一次 UPSERT（`usd = usd + ?`），**不需要逐事件行、不需要滑动裁剪**。
- **周期边界按本机本地时区计算**，因为「这个月」是一句人话，而账单周期也是人在读。`period.rs` 是这件事的唯一答案，`period_start_ms` / `period_end_ms` 一起由它算出并随每个 `Verdict`、每次 `spend.query` 一起出线——**不让任何消费者自己再算一次边界**（第二个算法就是第二个真源）。已知取舍：跨时区搬机器或 DST 切换会让某个周期长一小时或短一小时；这写进 `period.rs` 的 doc，不当它不存在。
- **全局总额是 `SELECT SUM(usd) FROM spend_ledger WHERE period_start = ?`，派生的**。不写第二行（qm 的 `@org` 行是同一事实的第二个写者）。
- 保留策略：一次开机 sweep 删掉 `period_start` 早于 N 个周期的行，形状对齐 `security_audit_log` 的 30 天清理。N 默认 12（一年的月）。
- 热路径（每次 LLM 调用一次 check）走**进程内缓存 + 写穿**：`Mutex<HashMap<(principal, period_start), Spent>>`，check 与 record 在同一把锁里完成，SQL 在同一临界区内发出。启动时不预热，**按主体惰性从 SQLite 载入当期行**；跨周期时旧键自然失效。
- **非 journaling 构造函数标 `#[cfg(test)]`**——把「别造第二个写者」从约定变成编译错误（判据：「一个进程全局的表被第二个实例写，症状是『我的行不见了』」）。

### 4.3 主体（谁被记账）

> ⚠️ **本节在写实施计划、逐个核签名时被推翻重写过一次。** 初稿写的是「键 = `scope::current_room_author()`」。去读代码才发现 `scope::room_author()` 第一行就是 `if !matches!(attr.scope, ScopeId::Project(_)) { return None }` ——它是**房间转录的署名器**，doc 逐字写着「一个 personal 或 org 会话只有一个人，署名是噪音」。拿它当花费主体，**每一个非房间会话的花费都会落进 `@unattributed`**，per-user 限额对绝大多数装机是个 no-op，而没有任何测试会红。这正是「一个字段回答的是另一个问题」。

**两种形状，两个调用点，同样的两个事实、同样的顺序**（判据：「每个可见性谓词都欠一个显式 actor 孪生，因为工具面取不到 task-local」）：

| 臂 | 位置 | 解析 |
|---|---|---|
| 准入 | 在 `with_request_scope` 的 task-local 巢**之外** | `meta[AUTHOR_USER_KEY]` → 否则 `meta[OWNER_META_KEY]` |
| 地板 | 在巢**之内**（`MeteringProvider`） | `scope::current_room_author()` → 否则 `scope::ambient_owner()` |

两者**可证同源**：`with_request_scope` 逐字用 `request.metadata[AUTHOR_USER_KEY]` 去 seed `CURRENT_ROOM_AUTHOR`（**不过 Project 过滤**），而 `ambient_owner()` 在 run 内等于 `current_scope().owner_user_id`，`current_scope()` 又是从 `meta[OWNER_META_KEY]` 重建的。所以两条路径读的是同一张 map 上的同两个键、同一顺序。这一点由 G13 钉住。

`AUTHOR_USER_KEY` 由 `handlers::agent::build_run_request` **无条件**从 `current_caller_user()` 盖上（不是只在房间盖），所以 personal 会话有主体；房间里它是**说话人**而不是房主。

> ⚠️ **刻意不用 `visibility::ambient_actor()`**，尽管它的前两条臂正是上表第二行。它的**第三条臂**回落到 `turn_context::current_agent_id()` —— 那是一个 **agent id 而不是 user id**。拿它当主体会静默地按 agent 开桶，而 agent 轴的键是**请求携带的字符串**（`chat.send{agent_id}`），正是 §7 列为 non-goal 的那条绕闸轴。花费主体必须是自己的函数，只取前两条臂。

`CarriedAttribution` 已经同时携带 `room_author` 与 `scope`（六个 task-local 之二），因此**后台子代理、detached run、团队成员 run 全部自带**，无需新增载体。

`None` 的处置（cron / heartbeat / webhook / 内部 run）：记在保留主体 **`@unattributed`**（`@` 前缀，`users.user_id` 铸不出这种 id），**只计入全局总额，不适用 per-user 限额**。

> ⚠️ 刻意不折成 `u-owner`。「未绑定被读作机主」在 §5.22 round-4 付过一次代价（`PairingStore::sender_user` 的 `None` 被消费者读成 legacy owner 语义，被闸住的成员因此升格成机主）。这里的 `None` 意思是「这次执行没有人类施动者」，不是「机主干的」。

### 4.4 两条臂，一个谓词

```rust
pub struct Spent {
    pub usd: f64,
    pub unpriced_calls: u64,
    pub partial_calls: u64,
    pub period_end_ms: i64,
}

pub enum Limit {
    /// 撞的是**调用者自己的**额度 —— 两个数字都是他自己的花费，照实说。
    PerUser { spent: f64, limit: f64 },
    /// 撞的是**机器级**总额 —— 刻意**不带数字**，见 §4.8。
    Total,
}

pub enum Verdict {
    Allowed(Spent),
    Denied { limit: Limit, spent: Spent },
}
```

**两条限额同时命中时报 `Total`。** 理由是本仓已有的排序判据：*这句 reason 不许误导读者「改什么能改变结果」*。总额耗尽时，给某个成员提额改变不了结果——报 per-user 就是把运维指向一个改了也没用的设置。所以不可移除的那一条排第一。

**臂 A · 准入（给话术）**
`run_loop::` 里新增一个两个引擎都调的共用 helper（先例：`ensure_session_under_request_scope` 的 doc 逐字写着「One helper rather than the same three lines at both engines' call sites … a second copy is a second answer waiting to drift」）。排在做功之前。

`Denied` ⇒ 新增 `ExecutionError::SpendExhausted { limit, spent }`，经**既有的单一源** `ExecutionError::user_receipt()` 同时到达 `RunError` 帧 / RPC 回执 / channel 回复。

**臂 B · 地板（不可绕过）**
`MeteringProvider::process` 与 `execute_streaming_dyn` 在委派给 inner 之前 check。这覆盖**准入闸结构上够不到的三类花费**：子代理（直接驱动 `AgentHarness::run`，不建 `FlowRequest`）、MoA 每 advisor、compactor —— 三者都不在父 run 的 `token_breakdown` 里。

`Denied` ⇒ `Err(...)` provider 错误，按 A2 压缩进上下文让模型自愈（收尾/汇报），不在 harness 里做恢复策略选择。

**两条臂调同一个 `spend::check(principal)`。** 源码级守卫钉住「全仓只有一个 `fn check`」。

**账本从进程全局读，不穿构造参数**：`MeteringProvider::new` 有 8 个生产构造点（`runner_impl` ×3、`subagent_spawner` ×2、`moa/provider`、`compactor`）。判据原话：「数出来是七个就别再想构造点了……配置该接在**类型**上」。先例就在同一个函数体里：`global_cache_monitor()`。

**已知且有界的超发**：check-then-act 之间并发的 N 次调用可能各自放行，超发上界 ≈ 并发数 × 单次最贵调用。写进 `check` 的 doc，不假装它不存在。这个界对预算是可接受的；把它收紧成严格串行会把每次 LLM 调用排进一把全局锁。

### 4.5 未定价 ≠ 免费，也 ≠ 拒绝

```
record(principal, &CostEstimate):
  Complete | PartialMissingPrice => usd += est.usd; (Partial 时 partial_calls += 1)
  Unknown                        => unpriced_calls += 1;   // usd 一个字节不动
```

每个 `Verdict` 都带 `unpriced_calls` / `partial_calls`；**每张回执、每行 CLI 都印它们**。绝不把未知折成 0 后闭嘴。

> **这一节顺带救活了 `pricing.rs` 那句 "never a gate"。**
> 定价**失败**永远不会拒绝任何人——`Unknown` 不计入 usd，也就永远不可能触发 `Denied`。只有定价**成功**才累加。那句裁定承重的那一半原样成立；变的只是它没打算说的那一半。
> 实施要求：把该模块 doc 改写**精确**（"a missing price never denies a call; only a measured price accumulates"），并留一条守卫钉住这个性质——而不是把那句话删掉。（判据：「一条只写在散文里的裁定，防不住下一个真诚的修复者」。）

### 4.6 配置

`src/config/types/policies/spend.rs`，挂进 `PoliciesConfig`：

```toml
[policies.spend]
per_user_usd = 20.0      # 省略 ⇒ 该轴无限额
total_usd    = 200.0     # 省略 ⇒ 该轴无限额
period       = "month"   # "month" | "day"，默认 "month"
```

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpendPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_user_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_usd: Option<f64>,
    #[serde(default)]
    pub period: SpendPeriod,   // Month | Day
}
```

`PoliciesConfig` 上的字段本身是 `#[serde(default)]` **不带** `skip_serializing_if` —— 与同级六个字段一致，因此 `[policies.spend]` 这一节**永远序列化**，不踩「空 section 被 `save_incremental` 跳过 ⇒ 删最后一个元素静默 no-op」那个坑。两个 `f64` 用 `Option` + `skip_serializing_if` 是为了区分「没设限额」与「设成 0」，而它们是**字段**不是 section，清空走的是把值置 `None`，那条路径完好。

**语义**：

- **两个限额都是 `None` ⇒ 账本整体关闭**：零 SQL、零 check、零 `install()`。单机装机逐字节不变。
- 因此**不需要给 loopback 排一条特例臂**——round-6 那条「三臂顺序承重、loopback 必须排第一」的坑在这里结构上不存在，因为默认根本没有闸。
- 设了限额就对**所有人**成立，包括机主。一条限额就是一条限额；跑飞的 loop 花的正是机主的钱。
- `period` 只在账本开启时有意义。
- **live-apply**：提额必须立刻生效，因为那是**唯一的反悔的路**（见 §4.7）。
  ⚠️ **不能把 `"policies"` 加进 `LIVE_SECTIONS`**——那张表是**顶层**粒度，而 `policies` 底下另有六个字段（`exec_tier` / `tool_permissions` / `memory` / `web_fetch` / `metrics` / `mode` / `guardian_review`）没有一个被验证过是 live 的；声明整段 live 就是 `live_apply.rs` 模块 doc 逐字在防的那个 bug（「advertise 『no restart needed』 and do nothing」）。
  正确形状是在**恰好有执行点的那条路径上**声明：新增 `reload_impact.rs::LIVE_SUBSECTIONS = &["policies.spend"]`，`classify()` 先查它；`apply_live_sections` 加一个把 `cfg.policies.spend` 存进 `spend::policy_handle()` 的臂（形状照抄 `"route"`：**句柄缺席就诚实降级**，CLI 进程 / 测试里没装句柄就不算 live）；`classify_verified` 的匹配从「只比顶层」放宽到也接受精确/前缀命中子路径。配对守卫 `every_live_subsection_has_an_apply_arm` 与既有那条同形。

### 4.7 读取面与反悔的路

**可枚举性是可撤销性的前提**，而 round-6 刚付过「五个生产者、零个读者」的学费。所以账本的写者与读者同批交付：

- **`spend.query` RPC**（admin-gated），形状对齐 round-6 的 `security.audit.query`：
  - 当期逐主体 `{ principal_id, display_name, usd, unpriced_calls, partial_calls }`
  - 机器总额 + `period_start_ms` / `period_end_ms`
  - `configured: bool` —— **未配置时答「未配置」而不是「$0」**（round-6 已付过学费：`0` 与「没测量」要分开说）
  - 空窗随响应带 `period_end_ms`，满页带 `truncated`（同 round-6 的审计读取面契约）
- **`aleph spend` CLI**（`--json` 带全部列）。CLI-only，沿用 `users.*` / `aleph audit` 的既定裁定。
- **反悔的路 = 提额，且立刻生效**（§4.6 的 `LIVE_SUBSECTIONS` 那条路径），加上周期自己会翻篇。写这条时要记住被闸住的人只有这一条出路——「把用户往宽设置上推」和「让他只能等到下个月」是同一种失败。

> **本轮不做 `spend.reset` / 补贴账本。** 那是改写历史；而「熔断了怎么恢复」这一问已经有答案了（下个周期 + 提额）。这一条是**有意的**，不是遗漏——写在这里是为了不让下一个人把它当 bug 修掉。

### 4.8 拒绝话术：靠形状而不是靠角色谓词

第一版设计想让 `user_receipt` 按角色决定印不印机器总额。**放弃了**，因为那要在渲染点读 `caller_identity`，而那个 task-local 在 spawn 出的 run 里是死的（判据：「每个可见性谓词都欠一个显式 actor 孪生，因为工具面取不到 task-local」），`user_receipt(&self, locale)` 的签名里也没有 actor。一个只在部分调用点成立的谓词，会在另一半调用点静默地印错。

**改成让形状回答**：

- `Limit::PerUser { spent, limit }` —— 两个数字都是**调用者自己的**花费，任何人看自己的花费都是对的，照实印。
- `Limit::Total` —— **不带任何数字**。话术：「这台机器本周期的共享预算已耗尽，将在 `<period_end>` 重置」。机器级数字只在 admin-gated 的 `spend.query` / `aleph spend` 上出现，那里本来就有身份闸。
- `Spent`（含 `unpriced_calls` / `partial_calls`）永远是调用者自己的，随两种 `Denied` 一起印。

于是**每个字段都有一个渲染它的调用点**（判据：「一个展示用字段在提交前必须能指出渲染它的那一行代码」），且渲染路径上没有任何角色分支可以答错。代价是 operator 撞 `Total` 时要多敲一次 `aleph spend` 拿数字——这是本轮有意付的价。

**`receipt_kind()` 必须同批设对。** `ExecutionError::receipt_kind() -> ReceiptKind` 是三个用户面共用的既有分类器，`block_goal_on_failure` 一类的上游用它区分「瞬时，值得重试」与「终态」。花费耗尽是**终态**（本周期内重试一万次都一样），不是限流——分到瞬时那一档会让 goal / cron / 重试矩阵对着一堵墙撞到周期结束。

---

## 5. 守卫 (Guards)

每条都会**手动证伪一次**（写完立刻破坏它，确认变红且点得出文件行号）。

| # | 守卫 | 钉住什么 |
|---|---|---|
| G1 | `every_run_producing_engine_consults_the_spend_gate` | 源码级 census，覆盖两个引擎。**从「凡 execute 的函数」推导，不列名字**——列举法只覆盖立法当天的世界 |
| G2 | `the_floor_and_the_gate_share_one_predicate` | 全仓只有一个 `fn check`，两处调用都解析到它。防「两个答案」 |
| G3 | `an_unpriced_call_never_becomes_zero_dollars` | `record(Unknown)` 后 `usd` 不变、`unpriced_calls` +1 |
| G4 | `a_missing_price_never_denies_a_call` | §4.5 那句被改写精确的裁定的可执行版本 |
| G5 | `a_restart_does_not_forgive_spend` | 丢弃内存缓存后重建，读回同一个数 |
| G6 | `a_room_members_spend_is_charged_to_the_member_not_the_room_owner` | §4.3 那个陷阱的红测（用 `owner_user_id` 实现会红） |
| G7 | `a_detached_subagents_spend_lands_on_the_spawning_principal` | `CarriedAttribution` 那条线（后台 spawn，不是前台子代理——前台不跨 spawn，会假绿） |
| G8 | `an_unconfigured_box_writes_no_ledger_rows` | 「逐字节不变」的证据，不是断言 |
| G9 | 非 journaling 构造函数 `#[cfg(test)]` | 第二个写者变成编译错误 |
| G10 | `both_limits_blown_reports_the_total` | §4.4 的排序判据 |
| G11 | `a_spend_denial_is_classified_terminal_not_transient` | `receipt_kind()` 的分档（§4.8）。分错会让 goal/cron 对着一堵墙重试到周期结束 |
| G12 | `the_total_limit_refusal_carries_no_machine_numbers` | §4.8 的形状——`Limit::Total` 是无字段变体，源码级即可 |
| G13 | `the_two_principal_resolvers_agree_on_the_same_run` | §4.3 的两条臂读同一张 map 的同两个键、同一顺序。**这条是本轮最贵的一条**——初稿的锚点错了，而错法是静默的 |
| G14 | `every_live_subsection_has_an_apply_arm` | §4.6 的表↔码配对，与既有的 `every_live_section_has_an_apply_arm` 同形 |
| G15 | `the_spend_principal_never_falls_back_to_an_agent_id` | 源码级：`spend` 模块里不出现 `ambient_actor` / `current_agent_id`。堵住那条被列为 non-goal 的按-agent 开桶 |

**源码级守卫的通用要求**（本仓已收过账）：分隔符不锚行首行尾（CRLF 检出下 `\n#[cfg(test)]\n` 永不匹配），扫描前先剥 `//` 注释行，且自保断言要能区分「扫到的是生产代码」而非测试模块里的字面量。

---

## 6. 验证 (Verification)

最小可信验证集（判据清单 §10）：

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo test -p aleph-cli -p aleph-tui            # 本轮碰 CLI
cargo clippy --all-targets
```

外加：`cargo test -p alephcore --lib`（G1–G10 逐条跑过并各自证伪一次）。

**真机 QA**：`qa/spend_budget/run.sh`，形状对齐 round-6 的 `qa/multiuser_audit/run.sh`（两个 URL：loopback 铸票、LAN 兑换，构造真正的第二个 principal）。断言清单：

1. 未配置装机：跑一个 run，`spend_ledger` 零行；`spend.query` 答 `configured: false`（不是 `$0`）
2. 配 `per_user_usd`：member 跑到超额后被拒，回执带 spent/limit/period_end
3. 同一时刻 operator 不受影响（**这是 round-6 修的「共享命运」在花费轴上的孪生**）
4. 撞 `total_usd`：**operator 与 member 的回执都不含机器总额数字**（§4.8 的形状，不是角色分支）；同一时刻 `aleph spend` 对 operator 报出完整数字、对 member 被 admin 闸拒
5. 提额后立刻放行（live-apply，不重启）
6. 重启后花费不归零
7. 用一个未定价模型跑一轮：`usd` 不变、`unpriced_calls` 增加、回执出声
8. 房间里 member 的花费记在 member 名下而非房主
9. 后台子代理的花费记在派发它的 principal 名下
10. cron run 记在 `@unattributed`，只进总额
11. **personal（非房间）会话的花费记在那个人名下，不是 `@unattributed`** ← §4.3 那次推翻的真机证据；用 `room_author()` 实现会在这一条上红

---

## 7. 明确不做的 (Explicit Non-Goals)

- ❌ **per-agent 那一层**。agent 轴的键是请求携带的字符串（`chat.send{agent_id}`），判据清单点过名：「一个权限层按某个轴分级，那个轴不能由调用方自己挑」。
- ❌ **滚动窗口**。日历周期已选定；两条聚合路径就是两个答案。
- ❌ **Panel 面**。沿用 `users.*` 的 CLI-only 裁定。
- ❌ **`spend.reset` / 补贴账本**。见 §4.7 的理由。
- ❌ **动 `pricing.rs` 的费率表**，或动既有 `BudgetExhaustedPartialResult` 家族。
- ❌ **observe-only 模式**。「设一个大限额」已经能表达；多一个 knob 就是多一个真源。
- ❌ **把 `RateLimitConfig` 变成可配置**。那是 round-6 留下的另一个缺口（每个构造点都是 `::default()`），与花费轴正交，不搭车。
- ❌ **审计联动**。花费拒绝不是 `AuthorityChange`；限额的**配置变更**是否该进 `security_audit_log` 留作独立议题。

## 8. 已知残留 (Known Gaps — shipped knowingly)

- ⚠️ **pin 一个未定价模型仍是一条绕闸路径**。按 D2 的选择它是**可见的**（每张回执都在报「N 次调用未计入」），但没有被堵死。堵死它要么杀掉本地模型，要么给未定价模型编一个数字，两者都比这个缺口贵。
- ⚠️ **有界超发**：并发 check-then-act 的上界 ≈ 并发数 × 单次最贵调用（§4.4）。
- ⚠️ **`@unattributed` 是一个桶**：cron / heartbeat / webhook 的花费混在一起，只进总额。按 job 拆分需要 cron 侧带上 owner，是另一轮。
- ⚠️ **round-5 欠的 teams 房间语义端到端复验**仍然欠着（跨轮遗留，非本轮引入）。

---

**Related**: `docs/superpowers/specs/2026-08-04-multi-user-org-project-design.md` · `docs/reference/SECURITY.md` · `docs/reference/FEATURE_LOCATOR.md` §5.22
