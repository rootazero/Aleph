# §4.1 Goal 命令（Standing Goal）第 10 轮 — 设计

- 日期：2026-08-05
- 分支：`worktree-goal-round10`
- 对标：codex `codex-rs/ext/goal`（含 2026 新增的 `goals_migrations` / `thread_goal_continuation_deferrals`）、kimi-cli `max_ralph_iterations`
- 关联：FEATURE_LOCATOR §4.1（第 9 轮止于 2026-07-30）、§4.2 loop round-9（本轮平移其核心教训）

---

## 1. Gap Analysis：参考项目 ↔ Aleph

| 维度 | codex 现状 | Aleph 现状 | 判定 |
|---|---|---|---|
| 状态机 | 6 态，含 `usage_limited` / `budget_limited` | 4 态，`Blocked` 吞掉全部停因（靠 note 区分） | ⚠️ 行为面差距 → 由 B2 以 park 覆盖；不引入新枚举值 |
| 用量记账 | 进程内 `Arc<RolloutBudget>` + `tokens_used` / `time_used_seconds` 落库 | 从 `SessionStore::get_total_tokens` **派生** + 树预算求和（A3 单一持久源） | ✅ Aleph 更优，**不移植** |
| token 口径 | `input − cached_input + output`（排除缓存读） | 全量 total | ◻️ 可议，本轮不动（要动 `get_total_tokens` 全体消费者） |
| 续跑触发 | `continue_if_idle` 轮询 idle 线程 | 原子 claim + `confirm_fire` CAS + busy 重排 | ✅ Aleph 更优 |
| fork 抑制 | `thread_goal_continuation_deferrals`（fork 出的 thread 不自动续跑） | 无 fork 语义；`terminate_session_continuations` 是对位物 | ✅ 已覆盖，但有缺口 → B3 |
| 完成闸门 | 无（纯 prompt 契约） | `gate_command` + fail-closed 仲裁 + CAS 写回 | ✅ Aleph 更优 |
| wait barrier | 无 | 时限障 + 任务障 + 事件唤醒 + boot rearm | ✅ Aleph 独有，但界限失效 → B1 |
| blocked 判据 | prompt 要求「同一阻塞连续 3 轮」 | `AUDIT_CONTRACT` 的弱版本 | ◻️ R9 剪枝，不加 prompt 字节 |
| objective 转义 | `escape_xml_text`（`&` `<` `>`） | `objective_block` 只中和 `</objective` | ◻️ 低危，本轮顺带收敛到单一源 |

**结论：参考项目无一项架构值得整体移植。** 本轮价值全部在 Aleph 自身的断线与界限失效。

---

## 2. 待修缺陷

### B1（HIGH）「在认领时求值的上限不是上限」——§4.2 loop round-9 的教训从未平移到 goal

`GoalStore::try_claim_continuation` 的 Timer 臂用 `should_continue(goal, tokens, now_ms)` 判定，随后 `Fire { delay_ms = wake_ms − now_ms }`。`confirm_fire` 只查 `is_active()` 与 marker 匹配，**不查 deadline / 迭代 / 预算**。

后果三条，逐条独立：

1. `goal(update, wait_minutes=180)` 配 `timeout_minutes=30` ⇒ 在用户所设上限之后 **150 分钟**跑完一整轮 LLM 并推送到原频道。
2. `GoalWakeService::rearm_parked_goals` 按设计**绕过 claim**、拿存储 marker 直接 `spawn_wake_run`（round-7 为修「pending 门吞唤醒」而引入）——那条路上**一个界都没有**。重启落在 wake 之后时，一个 deadline 早已过期的 goal 照样醒来跑完整轮。
3. `GoalWakeService::claim_and_spawn` 传 `tokens_total = None`，`try_claim_continuation` 于是按 0 计算 ⇒ **每一次唤醒都免预算**。而任务障 goal 的唯一驱动就是它。

### B2（HIGH）限流 = 判决——三块现成零件从没连线

`block_goal_on_failure` 对**任何**非取消非 busy 的 `ExecutionError` 一律：`Blocked` + `clear_goal_welded_strategy`（删掉焊入的计划）+ 推送「⚠️ 自主追求已中止」。

而仓库里三块零件齐备：

- `ExecutionError::receipt_kind()` —— 已把 `RateLimited` / `Unreachable` 分类为「值得稍后重试」，且已是三个用户面的单一源；
- `llm_retry::extract_retry_after_str` —— 已能解析退避时长（全格式）；
- wait-barrier —— 已能 park 并自唤醒。

codex 为此立了可自动恢复的 `usage_limited` 态。Aleph 不需要新状态：park 就是行为面的等价物。

### B3（MEDIUM-HIGH）退休会话上的 Paused goal 成为「看得见、停不掉」

`terminate_session_continuations` → `block_session_goal` → `block_if_active`，**只认 Active**。一个 `Paused` goal 在 epoch bump / `sessions.delete` 之后：

- `goal(action='list')` 从任何频道都列得出；
- `status='active'` 恢复它**必须在它自己会话跑**（`reject_remote` 拒绝远程 arming），而那个会话已不可达；
- 焊入的 strategy 行永久泄漏（只在 `Ok(true)` 分支删）。

loop 侧**恰好修过这一条**，理由写在 `terminate_session_continuations` 自己的注释里：「Paused loops are retired too: the epoch bump makes the old session unreachable... leaving it `Paused` would only make `loop(action='list')` advertise a loop nobody can ever restart.」goal 侧没跟。

### B4（MEDIUM）每轮注入的 `<standing_goal>` 完全不提 wait barrier

`render_goal_summary` 渲染 `objective (status=active, budget=N, deadline set, autonomous 3/10)` —— parked 的 goal 对模型读作「正在跑」。用户问「目标进展如何」，模型据此答错；模型自己也看不出已经 park 过，可能重复 park 或重做已委派的工作。

三个 render 里 `GoalTool::render`（`waiting: parked until task 'x' settles`）与 `render_list_line`（`| parked (waiting)`）都说了，**唯独每轮都发的那个漏了**。

### B5（MEDIUM）`goal(update, objective=…)` 静默丢弃

`GoalAction::Update` 分支从不读 `args.objective`，且末尾回显 `Self::render(&goal)`——即**旧** objective。模型得到 `success: true` 与「Updated. Standing goal: 旧目标」。

跨会话路径为**完全相同的理由**显式拒绝夹带字段（`remote_pause` 的 `extras` 检查，注释：「a caller that passed `pursuit_max_iterations` alongside the pause would otherwise be told "Paused" while its cap change vanished」），本地路径没有。

### B6（MEDIUM）`validate_wait_args` 对**更新前**的 goal 校验，且不看 status

两个方向都错：

- **漏放**：`goal(update, wait_minutes=60)` 打在 `Blocked` / `Complete` goal 上通过校验并提交 —— `wait_parked` 要求 `status == Active`，所以障**永远不会醒**，`GoalWakeService` 也只扫 `is_active()`。而 `render` 照说「waiting: parked for ~60m more」。它 doc 里写的正是「must learn that here, not never」。
- **误拒**：`goal(update, pursuit_max_iterations=5, wait_minutes=10)` 在 Passive goal 上被拒——尽管同一次调用正把它变成自主 goal。

### B7（LOW）Timer 臂存在不可达分支

`wait_parked` 的前置条件已含 `status == Active` 且 `pursuit == Active`，而 `exhausted_while_active` 的条件是同两条 + `!should_continue`。故在 Timer 臂内 `should_continue == false ⟹ exhausted_while_active == true`，其后的「neither runnable nor exhausted (e.g. status not Active)」兜底注释与路径均不可达。

---

## 3. 设计

### 3.1 B1 — 界限在执行时刻成立

单一源置于 `src/goal/pursuit.rs`，形状与 `looping::pursuit::fires_out_of_bounds` 逐字对称（两个 pursuit 必须读起来像姊妹）：

```rust
/// Would a continuation claimed now — waking at `wake_ms` — still be inside
/// the goal's wall-clock bound when it actually EXECUTES?
///
/// `should_continue` answers "may this goal be claimed", which is a different
/// question: a timer barrier is claimed at one post_run and the wake executes
/// hours later, and nothing in between re-reads the clock.
///
/// `now_ms == 0` (clock unavailable) fails open, matching `should_continue`.
pub fn fires_out_of_bounds(goal: &Goal, wake_ms: u64, now_ms: u64) -> bool {
    goal.deadline_ms.is_some_and(|d| now_ms != 0 && wake_ms > d)
}
```

三个消费者：

1. **claim 端**（`store.rs` Timer 臂）：arm 之前判 `fires_out_of_bounds(&goal, wake_ms, now_ms)`，越界 ⇒ 走已有的耗尽仲裁（`Blocked` + `stop_reason_note` + `without_wait` + 清 marker），返回 `Exhausted`。代价：最多**早**一个 park 长度停，而不是任意晚——与 loop 同一笔刻意交易，安全上限是上限不是近似。
2. **fire 端**（`confirm_fire`）：今天回 `bool`，`false` 的语义是「被取代，静默跳过」。越界若也回 `false`，就把「跑晚了」换成「永久静默卡死」。故改签名：

   ```rust
   pub enum FireDecision { Proceed, Superseded, OutOfBounds { note: String } }
   pub fn confirm_fire(&self, session_id: &str, wake_ms: u64, now_ms: u64) -> Result<FireDecision>
   ```

   `OutOfBounds` 臂在同一把锁里完成 `Blocked` + note + 清 marker（与 claim 端同一仲裁），`execute.rs` 的 Goal fire 点把它路由到 Exhausted 臂**同一个** `notify_origin("⏹ {note}")`。`Superseded` 行为逐字不变。
3. **boot rearm**（`goal_wait.rs::rearm_parked_goals`）：保留「用存储 marker 当 confirm 键」（那部分是对的，round-7 的理由仍成立），但在 `spawn_wake_run` **之前**评估 `fires_out_of_bounds` / `exhausted_while_active`；越界即在 boot 当场仲裁并推送，不再 spawn。

**外加唤醒路径的预算连线**：`GoalWakeService::new(deps, coord_store, session_store)` 新收一个 `Option<Arc<dyn SessionStore>>`，`claim_and_spawn` 用它算 `goal_budget::tree_tokens(...)` 传给 claim，`None`（未注入 / 读失败）保持今天的 fail-open 语义。

> 选型说明：`SessionStore` 放在 `GoalWakeService::new` 而不是 `ContinuationDeps`。后者被 clone 进 `continuation_cell` 供多方消费，而这个 store 的消费者只有一个（R10 零现有消费者的抽象不预留）。

**B1 与 B2 的组合关系**：修好执行时刻的墙钟界之后，它才真的能给 §3.2 的自动重试循环封顶。

### 3.2 B2 — 瞬时失败 park，不判决

`block_goal_on_failure` 改为分类驱动，判据取**已有的** `ExecutionError::receipt_kind()`：

| `ReceiptKind` | 处置 |
|---|---|
| `RateLimited` / `Unreachable` | **park 在既有时限障上**：不改状态（留 `Active`）、**不删焊入的计划**、`waiting_until_ms = now + delay`、`waiting_reason` = 已本地化且不泄漏内部链的 receipt 文案 |
| `Timeout`（run 超了自己的墙钟）/ `Auth` / `Failed` 及其余 | 与今天完全一致：`Blocked` + 清 weld + `⚠️` 推送 |

（`AgentBusy` / `Cancelled` 不经此函数——`execute.rs` 早在调用前就把它们分流到 `rearm_goal_after_busy` 与静默跳过，本轮不动。）

- `delay` = `llm_retry::extract_retry_after_str(raw)`，缺省 `TRANSIENT_PARK_FALLBACK`（5 分钟）；
- 推送动词与中止分开：`⏸ …（将在 ~Nm 后自动重试）` vs 今天的 `⚠️ …已中止`；
- 新增 `GoalStore::park_if_active(session, until_ms, reason, now) -> Result<bool>` —— `block_if_active` / `pause_if_active` 的第三个孪生：同锁、同「绝不碰终态」契约、清 pending marker（那次失败的 run 的 marker 必须让位给新 park 的 wake）。

**上限不新增字段**：失败的那次 run 在 claim 时已花掉一次迭代，唤醒再花一次 ⇒ 自动恢复**天然被既有迭代上限封顶**；墙钟 deadline（B1 修好后才真的有效）封第二道。`transient_parks` 计数器是第三个不需要的真源，撤回（R10 三问第 3 问：零真实消费者）。

### 3.3 B3 — 退休缝补上 Paused

新增 `GoalStore::block_if_not_terminal`（Active **或** Paused ⇒ Blocked；Complete / 已 Blocked 不碰），**只**给 `continuation_lifecycle::block_session_goal` 用。

`block_goal_on_failure` 与 agent-miss 路径**保持** `block_if_active`——在那两处「只 Active」是对的：一个被用户刻意 pause 的 goal 不该因为某个无关 run 失败而被降级为 Blocked。

同分支照常删焊入的 strategy（现在会覆盖 Paused 那一类）。

### 3.4 B4 — `<standing_goal>` 说出 parked

- **parked 事实**（parked 期间稳定）进 `render_goal_summary`：`, parked (waiting on task 'x')` / `, parked (waiting)`；
- **parked 倒计时**（时钟派生）进 `live_deadline_status`（transient tail），与既有的 deadline 倒计时并列。

沿用本模块已有的「稳定事实进 Dynamic 层 / 时钟派生进消息尾」切分，不新造通道；`standing_goal` 层的 stability 不变。

### 3.5 B5 — `update(objective=…)` 显式拒绝

返回 `Err`，文案指向 `set`。

**为什么是拒绝而不是在 update 里实现它**：objective 是被 `owns_reference` 边治理的参照，`set` 分支带那道 ACL（`governing_owner_or_refuse`）。在 update 里实现改 objective 需要复制第二份 ACL —— 而「同一个闸的第二份实现」正是本仓反复被咬的形状。指路是正解。

### 3.6 B6 — 对更新后的 goal 校验

调整 `Update` 分支的顺序：先应用 status / pursuit / budget / deadline / gate / note / lesson，**再** `validate_wait_args(&args, &goal)`（此时 `goal` 已携带本次更新后的 pursuit 与 status），**最后**应用 wait 字段。`validate_wait_args` 增加一条 status 检查：非 `Active` 拒绝并说明「a parked barrier on a non-active goal would never wake」。

### 3.7 B7 + 熵减

- 删 Timer 臂不可达的兜底分支与其注释；补单测钉住不变量 `wait_parked(g) == Some(Timer) ⟹ (!should_continue ⟹ exhausted_while_active)`。
- `objective_block` 收敛到 `crate::thinker::xml_util::escape_xml`（`StandingGoalLayer` 已用同一份；今天两处对同一段文本用两套转义）。prompt 字节会变（`&` `<` `>` 被转义），这是有意的。

---

## 4. 触及文件

| 文件 | 改动 |
|---|---|
| `src/goal/pursuit.rs` | `fires_out_of_bounds`、`transient_park_note`、`objective_block` 收敛、测试 |
| `src/goal/store.rs` | Timer 臂界限、`confirm_fire → FireDecision`、`park_if_active`、`block_if_not_terminal`、删死分支、测试 |
| `src/goal/mod.rs` | re-export `FireDecision` |
| `src/gateway/execution_engine/goal_continuation.rs` | `block_goal_on_failure` 分类驱动 |
| `src/gateway/execution_engine/goal_wait.rs` | boot 界限、`session_store` 注入、唤醒预算 |
| `src/gateway/execution_engine/execute.rs` | `FireDecision` 在 Goal fire 点的路由 |
| `src/gateway/continuation_lifecycle.rs` | 退休缝放宽到非终态 |
| `src/builtin_tools/goal.rs` | B5、B6、`render` 顺序 |
| `src/orchestrator/harness_bridge/context_blocks.rs` | B4 两半 |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | 注入 session store |
| `docs/reference/FEATURE_LOCATOR.md` | §4.1 第 10 轮 |
| `CLAUDE.md` | 「界限在执行时刻」扩展到 goal；新增「分类器已存在却没接」一条 |

零新子系统、零新持久字段、零新枚举值。

## 5. 测试策略

- **单元**（`src/goal/`）：`fires_out_of_bounds` 真值表；Timer 臂越界 ⇒ `Exhausted` 而非 `Fire`；`confirm_fire` 三态；`park_if_active` 的终态不变量；`block_if_not_terminal` 覆盖 Paused、不碰 Complete；B7 不变量。
- **工具层**（`src/builtin_tools/goal.rs`）：`update(objective=)` 报错；wait 校验的两个方向（Blocked 拒绝 / 同调用变自主放行）。
- **渲染**：parked 事实进 `render_goal_summary`、倒计时进 `live_deadline_status`。
- **分类**：`ReceiptKind::RateLimited` ⇒ park 且 weld 保留；`Auth` ⇒ Block 且 weld 删除。
- **回归护栏**：`confirm_fire` 的 `Superseded` 路径逐字节不变；无 deadline 的 goal 全程行为不变。

## 6. 明确不做（deferred 账本）

1. **codex 的 `usage_limited` 独立状态** —— `Goal` 是 serde 持久化类型，新增枚举值使旧二进制读不了新行（无向后兼容通道，`GoalStatus` 无 `#[serde(other)]`）；且 B2 的 park 已覆盖行为面。
2. **codex 的 token 口径 `input − cached + output`** —— 要动 `SessionStore::get_total_tokens` 的全体消费者（context gauge / 计费 / 树预算），跨子系统，独立评估。
3. **每轮 live 剩余 token 预算** —— 需在 prompt 组装路径上加一次异步 store 读；剩余量已在续跑 prompt（`render_quota`）与 `goal(get)` 两处可得。
4. **codex 的「blocked 需连续 3 轮」prompt 契约** —— R9 剪枝：`AUDIT_CONTRACT` 已有弱版本，不加 prompt 字节。
5. **codex `time_used_seconds`（活跃耗时，区别于绝对 deadline）** —— 需要新持久字段与逐 turn 记账，收益不抵 A3 代价。
6. **goal 的 Panel / RPC 面** —— 今天只有工具面（R8 已满足）；Panel 面是独立一轮。
