# Strategic Planner — Round 2: Naked Agent-Loop Coverage — Design Spec

> Status: **design approved, ready for implementation plan**
> Date: 2026-06-19
> Builds on: [`2026-06-18-strategic-planner-design.md`](2026-06-18-strategic-planner-design.md) (round 1)
> Provenance: StraTA (Strategic Trajectory Abstraction) **application-layer** pattern — the explainer's own "应用层" advice: *"别再一上来就让 Agent 去调工具去搜索了，在 agent loop / workflow 的最顶端加一个独立的 LLM 节点……先让它输出一段 Strategy，然后把这段 Strategy 死死焊在后续所有执行节点的 prompt 里。"*
> Hardened by a 6-agent adversarial verify+critique workflow (run `wf_f96348c4-fde`) against the real Aleph codebase. §12 records what it confirmed and corrected.

---

## 0. 一句话 (TL;DR)

round 1 已经把"顶端规划器 + 焊接"落地到 `/goal`·`/loop`·`/workflow` 三条**显式长任务**入口。round 2 补齐 round 1 唯一漏掉的那条路径——**裸 agent loop**：一条没打 `/goal` 的普通复杂请求（论文开篇那个"查三款车销量→做表格→发邮件给老李"）。在会话**首条真人消息**上，先过同一个无工具规划器；产出非空 Strategy 就存到新的 `session:{key}` 槽——既有焊接层（`StrategyLayer` + `StrategyPointerLayer`）经 `active_strategy()` **零改动自动接住**。

三个已确认的产品决策：① 主轴 = 补齐裸 loop 覆盖；② 触发 = **仅会话首轮、整会话保留**；③ 默认 = **开启、阻塞首个 Think（焊接在第一步就生效）**，受 `[strategy].enabled` + 新子开关 `plan_naked_loop` 双重约束。

**复用 round 1 的全部机械**（Strategy 结构、规划器 `plan_strategy`、StrategyStore、两个焊接层、`strategy` revise/show 工具、`[strategy]` 配置、红线论证）——本 spec 只描述 round 2 新增的 **触发点 + 新 key + `active_strategy` 扩展 + ExecutionEngine 接线 + 配置子开关**，以及对抗式核验逼出的全部修正。

---

## 1. Problem & Goal

**round 1 的覆盖缺口（已核验）：** Strategy 只在 `goal(set)` / `loop(start)` / `workflow(run)` 三个工具入口铸造。一条**普通的复杂首条消息**走的是裸 agent loop（`ExecutionEngine::execute` → harness run），**根本不触发规划器**——而这正是 StraTA 开篇要治的"健忘症"病例（AI 立刻冲去咖啡桌、忘了冰箱）。用户的原话也是"在 **agent loop**/goal/workflow 的最顶端"。

**Goal:** 让一条裸 loop 的复杂首条消息也"开始前先画地图"——在第一个 Think 之前生成 Strategy 并焊进 system prompt，使执行器从**第一步**就锚定目标 + 一小撮具体 guardrail。

**Success criteria:**
- 裸 loop 首条**实质**消息 → 铸出 `session:{key}` Strategy 且被 `StrategyLayer`(P70 全文) + `StrategyPointerLayer`(P1756 guardrails) 在 turn 1 焊接到。
- 裸 loop 首条**琐碎**消息（"hi"）→ 规划器 self-gate 返回 None → 无 Strategy、prompt **字节不变**。
- 成本 = 每个**符合条件的会话** ≤1 次规划 LLM 调用（绝不每轮）。
- 内部自动 run（cron / 群聊成员 / 子代理 / 续跑 / resume）**绝不触发**。

---

## 2. Scope & Decisions

### In scope (round 2)
- 在 `ExecutionEngine::execute` 的 first-message 路径加一个 fire-once 裸 loop 规划器触发（镜像 `goal.rs::maybe_plan_strategy`）。
- 新 composite key `session_key`；`active_strategy()` 优先级扩为 **goal > loop > session**。
- `strategy_manage::resolve_key()` 末档加 `session_key`（让 `strategy show/revise` 在裸 loop 会话可用）。
- `ExecutionEngine` 加 `planner_provider` 字段 + builder；从 `agent_init` 把现成 provider 接进来。
- 配置子开关 `[strategy] plan_naked_loop`（默认 true），**门控在 `agent_init` 处折叠**（不往 gateway 塞 Config）。

### 对 round 1 非目标的有意修订（必须显式记录）
round 1 §2 把"在三条 flow 之外自动检测复杂请求 / 任何确定性复杂度分类器"列为**非目标**（反 R10）。round 2 **有意放宽这条 scope**，但**不引入任何复杂度分类器**：

> 我们**不**在代码里判断"这条消息够不够复杂"。规划器在**每条符合条件的真人首条消息**上无条件开火，由它**自身的 self-gate**（round 1 §3：产不出具体 guardrail 就存 None）决定是否值得焊接。"复杂与否"始终是 **LLM 判断**（R7-aligned），与 round 1 三条 flow 的 self-gating 同源。代码侧只做**来源判别**（§4：是不是真人交互首条消息）——那是确定性的"管道事实"，不是内容启发式。

这条修订是本轮的核心 scope 决策，已获用户确认（"补齐裸 agent loop 覆盖"）。

### Non-goals (round 2，继续 YAGNI)
- 动态重规划（论文第一局限）、自我审查审计员（第三创新）、多样性 best-of-N（第二创新）——**本轮不碰**，留给后续轮。
- 与 harness 内部记忆检索阶段的**完全并发重叠**——会耦合 gateway↔harness，**有意不做**（见 §8 延迟诚实声明）。
- 对裸 loop Strategy 的 objective-change 自动失效——裸 loop 无 `goal_id` 交叉引用（§7）；中途转向靠 `strategy revise` 或升级 `/goal`（goal_key 优先级更高自动盖过）。
- 任何基于消息**内容**的触发短路（长度/关键词/正则判复杂度）——会违 R7/P8。来源判别只用 SessionKey 变体 + resume 标记 + 空输入（管道事实）。

---

## 3. Architecture & Data Flow

焊接侧与触发源**已解耦**：`StrategyLayer`/`StrategyPointerLayer` 只读 `ResolvedContext.{strategy, strategy_guardrails}`，唯一生产填充点是 `prompt_build.rs` 的 `active_strategy()`（每轮、所有 paradigm）。所以补齐裸 loop = 新增一个**写**触发 + 让 `active_strategy()` 多读一个 key。

```
首条真人复杂消息 → ExecutionEngine::execute (gateway/execution_engine/execute.rs)
   │
   │  GATE (全部为"来源/管道事实",零内容启发式 — §4):
   │    is_first_message  (execute.rs:188 = history_empty && !is_slash, 既有)
   │    && session_key.is_interactive()   (NEW: Main/Peer/Dm/Group; 排除 Task/Subagent/Ephemeral)
   │    && !is_resume                      (execute.rs:216 既有标记)
   │    && !request.input.trim().is_empty()
   │    && self.planner_provider.is_some() (enabled && plan_naked_loop 已在 agent_init 折叠)
   │    && matches!(store.get(&session_key_str), Ok(None))   (fire-exactly-once; 注意 matches! 不是 ==)
   ▼
plan_strategy(provider, objective = request.input(原文), &PlannerContext{[],env_summary(),[]}, goal_id=None)
   │  与既有 first-message setup (topic 生成已 spawn 异步 / session-context & memory-scope 传播 I/O) 并发,
   │  在派发 harness run (execute.rs:399 的 tokio::select! → run_agent_loop) 之前 await
   ▼
若 Some(strategy) → store.put(session_key(request.session_key.to_key_string()), &strategy)   // 否则什么都不做
   ▼
harness run 启动 → build_system_prompt (prompt_build.rs) → active_strategy() 读到 session 槽
   ▼
StrategyLayer(P70,Stable,全文) + StrategyPointerLayer(P1756,Dynamic,仅guardrails) 自动焊接 (零改动)
   │
   └─(附带,intended) 该会话 spawn 的子代理经 run_loop/inner.rs:818 也走 active_strategy → 继承 session Strategy
```

**架构定位（红线）：** 触发点在 `src/gateway/execution_engine/execute.rs`——goal 续跑（`should_continue@763`）、stop-hook gating、token 预算、welded-strategy 生命周期清理（`execute.rs:711-719` 删 goal_key）**已经都待在这个文件**。它是**编排缝**，既非 `src/harness/` 笨循环（守 R10），也非 WS handler / channel 纯 I/O 边界（守 R4——R4 治的是 `server::handler`+`handlers::connect`，不是这个已在跑自治续跑引擎的文件）。新触发器与既有 strategy 生命周期代码同处一文件，形态与既有续跑钩子一致（guard → global store 检查 → `matches!(get, Ok(None))` → fire）。

---

## 4. The Trigger Gate（本轮最关键的正确性元素）

`ExecutionEngine::execute()` 是**共享入口**——cron 首次触发、群聊 team 成员首轮、subagent_announce 回报、unattended 续跑全都流经它，且都 `history_empty && !is_slash` → `is_first_message=true`。**朴素的 `is_first_message` 门控会让规划器错误地在 cron prompt / 团队成员首轮上开火，并把 Strategy 泄漏进团队子代理。** 这是对抗式核验揪出的头号缺陷（红队 REFUTED）。

**修正 = fail-closed 白名单，按 SessionKey 变体判别真人交互来源**（已核验 `src/routing/session_key.rs`）：

| 来源 | SessionKey 变体 | 触发? |
|---|---|---|
| Panel chat / 真人 channel（DM/群/线程） | `Main` / `Peer` / `Dm` / `Group` | ✅ 触发 |
| cron 任务 | `Task{task_type:"cron"}` | ❌ 排除 |
| 群聊成员 run | `Task{task_type:"team_chat"}` | ❌ 排除 |
| 子代理 spawn | `Subagent` / `Ephemeral` | ❌ 排除 |
| subagent_announce 回报 | 复用父 `session_key`（已有 history） | ❌ 天然不触发（非首条）+ 白名单兜底 |
| unattended 续跑 | 复用真人 key 但 history 非空 | ❌ 天然不触发（非首条） |

实现：在 `SessionKey` 加小 helper `pub fn is_interactive(&self) -> bool`（`Main`/`Peer`/`Dm`/`Group` 为 true，`Task`/`Subagent`/`Ephemeral` 为 false）+ 单测。门控用 `request.session_key.is_interactive()`。**fail-closed**：未来新增的内部 run 类型只要不是上述四个交互变体，自动不触发——比元数据黑名单（`unattended`/`cron_job_id`/`subagent_announce`/`team_id`）稳健得多。

> R7 合规：白名单判别只看 **run 来源**（SessionKey 变体、resume 标记、空输入）——确定性的管道事实，**不是**对消息内容"看起来复杂吗"的启发式。复杂度判断 100% 留给规划器 self-gate。

---

## 5. Change Sites（已核验，含编译陷阱）

| # | 文件 | 改动 | 核验要点 |
|---|------|------|---------|
| 1 | `src/routing/session_key.rs` | 加 `pub fn is_interactive(&self)->bool`（Main/Peer/Dm/Group=true）+ 单测 | 内部 run 全用 `Task`/`Subagent`/`Ephemeral` |
| 2 | `src/strategy/mod.rs` | 加 `pub fn session_key(key:&str)->format!("session:{key}")`，镜像 goal_key/loop_key + 单测 | 入参必须是 `to_key_string()` 形式（见 #4 风险） |
| 3 | `src/orchestrator/harness_bridge/context_blocks.rs` | `active_strategy()` 末尾追加 `store.get(&session_key(session_key)).ok().flatten()` 作为第三档（goal > loop > session）+ 单测 | 既有唯一测试只测 store==None 分支，不受影响 |
| 4 | `src/builtin_tools/strategy_manage.rs` | `resolve_key()` 末档加 session_key（show/revise 可用）；**保留** `revise` 的 `unwrap_or_else(\|\| goal_key(session))`（line 141）不动 | 无 row 时 revise 落 goal_key 是**无害**的——active_strategy goal>loop>session，goal_key 仍最先被读到、一致 |
| 5 | `src/gateway/execution_engine/engine.rs` | `ExecutionEngine` 加字段 `planner_provider: Option<Arc<dyn crate::providers::AiProvider>>`（全限定，engine.rs 无该 import）+ `new()` 初始化为 `None`（漏则 E0063）+ `with_planner_provider()` builder（镜像 `with_state_database@259`） | `Arc` 用 engine.rs 既有 `crate::sync_primitives::Arc` 别名 |
| 6 | `src/gateway/execution_engine/execute.rs` | first-message 路径加 §4 门控 + fire-once 裸 loop 规划器（镜像 `goal.rs::maybe_plan_strategy`）；并发于既有 setup，**派发前 await** | objective 用 `request.input`（原文），**非** `render_user_session_text`（带 steering banner 会污染）；guard 用 `matches!(store.get(&k),Ok(None))` **非 `==`**（anyhow::Result 无 Eq，`==Ok(None)` 编译失败） |
| 7 | `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | `planner_provider` 在 :449 被 move 进 tool_config 之前 **clone**；engine 的那份按 `enabled && plan_naked_loop` 门控；构造点（mod.rs:704）`.with_planner_provider(naked_loop_planner)` | 不 clone → E0382 use-after-move；折叠门控使 engine 在任一开关关时收到 `None`，无需往 gateway 塞 Config |
| 8 | `src/config/types/phase6_wiring.rs` | `StrategyToml` 加 `#[serde(default="strategy_plan_naked_loop_default")] pub plan_naked_loop: bool` + `fn ..._default()->bool{true}` + **更新手写 `Default` impl 字面量（:240-243 的 `Self{enabled,planner_model}`）** + 测试锁默认值 | 手写 Default 是 `Self{...}` 不是 `StrategyToml{...}`，grep `StrategyToml {` 会漏 → E0063；唯一生产字面量 `deps_builder.rs:1804` 已用 `..Default::default()` 安全 |

**附带行为（intended，需文档+测试）：** #3 让 `active_strategy()` 多认 session_key，于是裸 loop 会话 spawn 的子代理（`run_loop/inner.rs:818` 也调 `active_strategy`）会继承 session Strategy。这是与 goal/loop 焊接继承的**对等行为**，记一笔并加测。

---

## 6. Config

```toml
[strategy]
enabled = true            # round 1 既有总闸（关 = 全停，含 goal/loop/workflow）
planner_model = "..."     # round 1 既有；建议裸 loop 配便宜模型压低首轮延迟（§8）
plan_naked_loop = true    # round 2 新增子开关:裸 loop 是否规划(默认开),受 enabled 约束
```

**门控折叠（关键 — 避免往 gateway 塞 Config）：** `ExecutionEngine` 不持有 Config/StrategyToml。所以在 `agent_init/mod.rs` 处把 `plan_naked_loop` 折进 provider 决策：

```rust
// 既有:planner_provider 受 enabled 门控,喂给 goal/loop/workflow 工具(tool_config)
let planner_provider = if enabled { Some(build...unwrap_or default) } else { None };
// 新增:engine 那份额外受 plan_naked_loop 门控(clone 在 move 进 tool_config 之前)
let naked_loop_planner = if enabled && plan_naked_loop { planner_provider.clone() } else { None };
...
tool_config = BuiltinToolConfig { planner_provider, .. };          // move,enabled-gated
engine = engine.with_planner_provider(naked_loop_planner);         // plan_naked_loop-gated clone
```

于是 `execute.rs` 触发器只需检查 `self.planner_provider.is_some()`（与 `goal.rs:156` 同形）——任一开关关 → engine 收到 `None` → 触发器静默休眠（P7）。`enabled=false` 时 `naked_loop_planner` 也必为 `None`（因 `&&`），不会出现"总闸关了裸 loop 还在跑"。

**`[strategy]` 整段缺省语义：** 当配置完全没有 `[strategy]` 段时，`enabled` 与 `plan_naked_loop` **均按 true 解析**（镜像 round 1 在 `agent_init:371` 对 `enabled` 用的 `map_or(true, |s| s.enabled)`）。即 `plan_naked_loop = app_config.strategy.as_ref().map_or(true, |s| s.plan_naked_loop)`。因此**默认配置的全新安装即开启裸 loop 规划**——这是用户选定的"默认开"，其首轮延迟代价由 §8 的缓解（来源白名单 + 便宜 `planner_model` 建议 + 空输入跳过 + self-gate）承接。

---

## 7. Lifecycle & Edge Cases

- **生命周期 / 清理：** session_key Strategy **持久、整会话保留、无失效钩子**（`goal_id=None`，故 goal 的 objective-change 自动失效不触及它）。这与 round 1 **loop 的"persist-for-resume 不删"行为一致**；且 `session_id` 唯一不复用、session 本身即生命周期，故常驻**无害**（≤1 行/会话，读取 fail-soft）。**不新增清理钩子**（YAGNI）。
- **中途转向：** 裸 loop 无 objective 追踪。用户中途改需求 → 靠 `strategy revise`（LLM 自主）或升级 `/goal`（写 goal_key，active_strategy 优先级更高自动盖过陈旧 session Strategy）。陈旧 session Strategy 在转向后仍焊接是已知小代价，符合"仅首轮、整会话保留"的选定语义。
- **空 / 纯附件首条消息：** 门控含 `!request.input.trim().is_empty()`（管道事实，R7-safe）——空白/纯附件首条直接跳过，不浪费 LLM 调用。
- **超长首条消息作 objective：** `build_planner_prompt` 原样嵌入 `request.input` 无截断（goal 路径的 objective 已被 goal 工具有界，裸 loop 是原始 channel 输入可任意大）。**加一个 UTF-8 安全的 char 上限**（`char_indices`，P7）切片喂给 `plan_strategy` 的 objective，防超长 paste 撑爆规划器 prompt。
- **双开火竞态：** 两条近同时的首条消息都见 `history_empty` 且 `matches!(get,Ok(None))` → 都可能 put。与 `goal.rs` 今天同构、容忍（last-write-wins，等价 Strategy 幂等覆盖）。记一笔，不额外加锁。
- **simple.rs：** `SimpleExecutionEngine`（simple.rs:114 自有 is_first_message）是 **provider-less 测试用回退**（输出占位串、不建 system prompt、不调 harness、`total_tokens=0`）。它本就没有 provider 也没有焊接目标——**正确地不触发**，非漏点。

---

## 8. Latency（诚实声明）

用户选定"默认开 + 阻塞首 Think + 并发藏延迟"。诚实地讲：

- **plan-first 要求 turn-1 焊接** → 规划器结果必须在 harness 派发（`execute.rs:399`）前 `store.put` 完成 → 规划器 LLM 往返被**串行进 turn-1 的 time-to-first-token**。这对一句"hi"是真实的首回复延迟。
- **"并发藏延迟"的真实范围：** 规划器与 gateway **自己的** first-message setup（已异步 spawn 的 topic 生成 + session-context/memory-scope 传播 I/O）重叠；它**不**与 harness 内部记忆检索重叠（那要耦合 gateway↔harness，§2 有意不做）。所以被藏掉的主要是那点 setup I/O，**不是** LLM 往返本身。不夸大。
- **缓解（全部 R7-safe）：** ① §4 来源白名单——只有真人交互首条付这个延迟，cron/团队/子代理/续跑全不付；② `!input.trim().is_empty()` 跳过空/附件首条；③ self-gate 对琐碎输入返回 None（仍付一次调用，但建议配便宜 `planner_model` 压低，文档注明）；④ `plan_naked_loop=false` 一键关闭裸 loop 规划而保留三条 flow 规划。

> 若未来认为首轮延迟不可接受，备选是"分离开火、从 turn-2 焊接"——但那牺牲对**第一步**的 plan-first 保护（论文最看重的恰是第一步），与本轮目标冲突。故**维持派发前 await**。

---

## 9. Redline Analysis

| Redline | Verdict | Note |
|---|---|---|
| **R4 Interface 纯 I/O** | ✅ | 触发在 `execution_engine/execute.rs`——已在跑自治续跑/gating/token 预算/strategy 生命周期清理的**编排缝**,非 WS handler/connect I/O 边界。形态镜像既有续跑钩子。 |
| **R7 LLM 主权** | ✅ | "复杂与否"由规划器 self-gate（LLM）决定;代码侧门控只看 run 来源（SessionKey 变体/resume/空输入——管道事实,非内容启发式）。零正则/关键词判复杂度。 |
| **R9 智慧在 prompt / 无中间件税** | ✅ (scoped) | ≤1 次规划调用/符合条件会话,在循环**之上**;per-turn 仅多 `active_strategy()` 的第三次 SQLite indexed get(热路径,极小)。全程 fail-soft。 |
| **R10 薄 Harness / 笨循环** | ✅ | `src/harness/` **零改动**;触发在 gateway 编排层;焊接由既有层被动拾取。新增是脚手架(key/字段/builder/config),非认知。"5 个不"未触。`enabled`/`plan_naked_loop` 双开关 = Future-Proof 撤回阀。 |
| **P6 KISS/YAGNI** | ✅ | 复用 round 1 全部机械;新增面收敛到触发+key+接线+子开关;明确不做动态重规划/审计员/best-of-N。 |
| **P7 防御** | ✅ | provider None → 休眠;`matches!` guard 防双写;3-guard 空路径字节不变;UTF-8 安全 objective 截断;put 失败不致命。 |

---

## 10. Testing

**单元（host，无 LLM）:**
- `SessionKey::is_interactive()`: Main/Peer/Dm/Group=true；Task/Subagent/Ephemeral=false。
- `session_key()` 前缀 + 不与 goal/loop/workflow key 相撞。
- `active_strategy()` 三档优先级：仅 goal 命中 / 仅 loop 命中 / 仅 session 命中 / 三者皆无→None / goal 盖过 session。
- `strategy_manage::resolve_key()` 命中 session 档；既有 goal/loop 测试不破。
- `StrategyToml` 默认 `plan_naked_loop==true`（toml 反序列化 + `..default()`）；锁默认值断言。

**集成（gateway 层）:**
- 首条**实质**消息（interactive key）→ `session:{key}` Strategy 被铸出且 `parts[0]` 含 `<strategy>` 全文、`parts[1]` 含 guardrail echo（一并抓 missing-`Cached` vanish 与 stability 错放）。
- 首条**琐碎**消息 → self-gate → 无 Strategy、prompt 字节不变。
- **来源排除（关键回归）：** `SessionKey::task(_,"cron",_)` / `task(_,"team_chat",_)` / `Subagent` / `Ephemeral` 的首条 run → **不触发**规划器。
- `is_resume=true` 首条 / 空白输入首条 → 不触发。
- `plan_naked_loop=false` → 裸 loop 不规划但 `/goal` 仍规划；`enabled=false` → 全不规划。
- fire-once：同会话第二次（非首条）→ 不重铸。
- 子代理继承：裸 loop 会话 spawn 的子代理 inline prompt 含 session Strategy。
- 后续 `/goal` 在同会话铸 goal_key → active_strategy 返回 goal Strategy（盖过 session），且 None-goal_id 的 session Strategy 不被 goal 自动失效误删。

**E2E（用户跑）：** "买酱油遇打折薯片"式 canned 裸 loop 任务（带 distractor），对比 Strategy on/off 的跑偏行为。

---

## 11. Build Order（给实现计划）

1. `SessionKey::is_interactive()` + 单测（独立、零依赖）。
2. `strategy::session_key()` + 单测。
3. `active_strategy()` 第三档 + 单测；`strategy_manage::resolve_key()` 末档 + 单测。
4. `[strategy] plan_naked_loop` config（serde default + 手写 Default impl 更新）+ 单测。
5. `ExecutionEngine.planner_provider` 字段 + `new()` None + `with_planner_provider()` builder。
6. `agent_init`：clone-before-move + `enabled && plan_naked_loop` 折叠门控 + `.with_planner_provider()`。
7. `execute.rs` first-message 触发：§4 门控 + fire-once 规划器（`matches!` guard、`request.input` objective、UTF-8 截断、派发前 await）。
8. 集成测试（含来源排除回归）；空路径测试先行；用户跑 E2E。

---

## 12. Verification Summary（run `wf_f96348c4-fde`，6 代理对抗式核验）

**Confirmed（无需改）：** `active_strategy()` 是两个焊接层的唯一生产读路径，扩它即覆盖两层 + 子代理 weld；现行优先级恰为 goal→loop，追加 session 纯增量、不破既有测试；`plan_strategy` 签名 / `PlannerContext` 字段 / `env_summary()` 公开 / self-gate / fire-once guard 形态；`goal_id=None` 不与 goal 自动失效交互、后续 `/goal` 经优先级自动盖过；`resolve_key` 加 session 档安全；`StrategyToml` 唯一生产字面量已 `..default()`；R4/R7/R10 框定成立；`simple.rs` 正确排除。

**Corrected（已并入本 spec）：**
1. **共享入口误触**（红队 REFUTED）→ §4 fail-closed `is_interactive()` 白名单，排除 cron/team/subagent/resume。
2. `store.get(k)==Ok(None)` **编译失败**（anyhow::Result 无 Eq）→ 必须 `matches!(store.get(&k),Ok(None))`。
3. key 必须用 `request.session_key.to_key_string()`（与焊接层/子代理 weld 同串），否则读到不同 `session:{...}` 行、Strategy 静默不可见。
4. `planner_provider` 在 agent_init:449 被 **move**，engine 复用前必须 `.clone()`（否则 E0382）。
5. engine 配置门控：ExecutionEngine 不持 Config → `plan_naked_loop` 折叠进 agent_init provider 决策，不往 gateway 塞 Config。
6. `StrategyToml` 手写 `Default` impl（`Self{...}` 字面量,grep `StrategyToml {` 会漏）必须同步加字段,否则 E0063。
7. objective 用 `request.input` 原文,非 steering-decorated 文本。
8. 加 UTF-8 安全 objective 长度上限;空/纯附件首条跳过。
9. 延迟"并发藏"范围诚实收窄(§8):藏 setup I/O 不藏 LLM 往返。

---

## 13. Deferred to Future Rounds
- 动态重规划（论文第一局限：感知环境巨变 → 自动邀请 `strategy revise`）。
- 自我审查"无情的审计员"（论文第三创新：阶段收尾审计轨迹 vs 战略 → 回灌 lessons）。
- 多样性 best-of-N 规划（论文第二创新：FPS 选发散候选 → LLM 评审）。
- 裸 loop Strategy 的话题转向自动失效（需 LLM 侦测转向，复杂度上升）。
- 与 harness 记忆阶段的完全并发重叠（彻底消除 turn-1 延迟，但耦合 gateway↔harness）。
