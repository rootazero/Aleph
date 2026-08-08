# 中断—恢复能力对标设计 (Interruption / Resume Parity)

- 日期: 2026-08-08
- 分支: `worktree-worktree-resume-parity`
- 范围: G1（子代理跨重启可寻址）+ G2（崩溃边界语义）+ G3（按需恢复入口）
- 参考实现: Claude Code Workflow journal · `codex-rs`（`rollout_reconstruction` / `multi_agents::resume_agent` / `agent::control::residency`）· `pi`（JSONL session）

---

## 1. 触发这次工作的观察

一次 Claude Code 会话被 `/clear` 清零后经 `/resume` 恢复，正在跑的 workflow **没有被中断**，且丢失的完成通知可以从 `journal.jsonl` 里把每个 agent 的真实返回值捞回来。拆开看是三件独立的事：

1. **解耦** — 工作不属于会话上下文，清空对话杀不掉它。
2. **持久台账** — 每个工作单元把 (输入键 → 返回值) 写进进程外的 append-only 日志。
3. **前缀重放** — 恢复时按输入键命中缓存，只重跑变化处。

## 2. 扫描结论

Aleph 在**主 run** 这条轴上已经领先两个参考实现：`session_events`（SQLite SSOT）+ `ResumeCoordinator` 开机扫描 + 崩溃边界修复 + `orphan_notice` 用户回执 + scope 重盖。pi 是单进程 CLI，没有服务端恢复概念；codex 有 rollout 重放但没有开机自动扫描。

缺口集中在三处：

| 缺口 | 现状 | 参考实现的对位能力 |
|---|---|---|
| **G1 子代理跨重启** | `BackgroundAgentTracker` 是 `RwLock<HashMap>`，纯内存 | codex `resume_agent` + `residency` LRU |
| **G2 崩溃边界语义** | 悬空调用被合成 `ToolError("interrupted by server restart")` | Claude Code journal 只记完成态，天然无此问题 |
| **G3 恢复触发面** | 只有 boot 一个触发面，无按需入口 | `resumeFromRunId` / `resume_agent` 工具 |

### G1 的根因：断线在 id 上，不在基础设施上

`subagent_spawner` **已经**把 `SubagentSpawned { child_id }` / `SubagentReturned { child_id, summary }` 写进父会话的持久事件日志。但 `BackgroundAgentTracker` 按 `request_id`（`subagent_tool/spawn.rs` 现造的 UUID）索引，而 `child_id` 由 `ephemeral_for()` 另造一个 UUID。两个 UUID 从不互指，所以重启后模型手里的 id 查不到任何东西，而完整结果就在数据库里，没有寻址路径。

## 3. 设计

### G1 — 让 `request_id` 成为持久子会话 id

**写侧**：`request_id` 顺 `AgentRuntimeConfig.request_id` → `SpawnRequest.request_id` → `ephemeral_for(agent_id, request_id)`，子会话键成为 `Ephemeral { agent_id, ephemeral_id: "sub-bg-<request_id>" }`。零 schema 变更——`SessionKey` 的字符串形态本来就往返。前缀常量 `SUBAGENT_BG_CHILD_PREFIX` 由铸造侧与恢复侧共用。

只有**后台** spawn 传 `Some`。前台 / batch / MoA aggregator 在同一次工具调用里返回结果，没有需要事后寻址的 id，保持裸 nonce `sub-<uuid>`。

**两个前缀必须保持可区分**（自查中发现的真缺陷）：前台 / batch 子代理**走同一个 spawner、写同一批持久事件**。如果它们也带后台前缀，`recovery::enumerate` 会把每个随机 nonce 读成一个"不可恢复的 request_id"，`subagent list` 于是塞满该会话跑过的每一个前台子代理并逐条标成可恢复——正是本模块要修的那种"目录撒谎"，方向相反。守卫 `anonymous_foreground_children_are_not_enumerated`。

**为什么不用位置关联**：一个 turn 可以并发 spawn 多个后台子代理，它们的 `SubagentSpawned` 共享 `turn_id`，按顺序或按 turn 关联必然串台——正是 `tools::scoped::dispatch` 用 ambient call identity 替换掉的那个 parallel-batch 歧义。`recovery.rs::concurrent_siblings_in_one_turn_do_not_cross_talk` 钉住这一点。

**读侧**（`src/agents/subagent_tool/recovery.rs`）：懒解析，只在 tracker 报告 unknown id 时才读一次父会话日志，一次读服务本次调用的全部 unknown id。

| 日志里有什么 | 返回 |
|---|---|
| `SubagentReturned` | `status: completed_recovered` + **真实 summary** |
| 只有 `SubagentSpawned` | `status: interrupted` + `child_session` 指针 |
| 都没有 | 维持现有 `unknown` |

接入点：`check_status` / `wait`（单 id 与 wait_any）/ `cancel` / `wait_cancelled` / `list`。`list` 额外带 `from_durable_log` 数组——它自称是"恢复你不再持有的 request_id"的目录，重启后报告空会话就是目录本身在撒谎。

**`list` 的行必须预览，不能上全文**（自查中发现的第二个真缺陷）：条目一旦被 tracker 的 TTL 剪掉就**永久**落在这条路上（只增不减），所以整份 summary 上行会让 `list` 随会话年龄无界膨胀。恢复行走 `to_list_row`（`LIST_RESULT_PREVIEW_CHARS` 预览 + `result_chars` 说明真实大小 + `MAX_LISTED_COMPLETED` 条数上限 + 明说截掉了多少），全文仍由 `check_status` 提供。守卫 `list_rows_preview_the_summary_while_to_json_carries_it_whole`。

两个都是 `ToolResult::Success`，包括 `interrupted`：重启不是对模型**此刻这次调用**的判决。

`BackgroundAgentTracker` 保持纯内存（P2），durable 查询落在工具层——那里已经持有 `session: Arc<dyn SessionService>` 和 `parent_session_id`，零新增依赖注入。

**恢复语义 = 报告，不自动续跑**（用户裁定，R7）：要不要重跑、从哪重跑，100% 归模型判断。

### G2 — 崩溃边界改说"结果未知"

`ToolCallRequested` 在 `harness::agent::act` 里是**派发前 `await` 落盘**的，而派发后仍能拦住调用的两件事（guardrail `Block`、审批拒绝）**各自都写自己的应答事件**。所以"有 Requested 无应答"的真实含义是：调用已到达或越过派发线，副作用可能已经落地。

原文本 `"interrupted by server restart"` 读作判决 → 模型重做。改为陈述认知状态，并带上事件里本就有的工具 `name`。事件形状仍是 `ToolError`（没有结果可交，合成 `ToolResult` 会让编造的载荷与工具真实输出无法区分）。

**刻意不加安全等级分类器**：`ToolSafetyLevel` 存在，但接到 `ResumeCoordinator` 要加第 7 个构造参数，且"这个能不能安全重做"正是 R7 留给模型的推理。

### G3 — 按需恢复入口

- `ResumeCoordinator::resume_from_markers` — boot 扫描与按需恢复**共用的单一推导**（recency 过滤 / crash-loop 上限 / 边界修复 / 并发许可）。
- `ResumeCoordinator::resume_session(session_id)` — 按需面。**刻意不看 `config.enabled`**：该开关治理的是自动扫描，操作者点名一个会话时已经做完了那个开关替他推迟的决定。
- `agent.resume` JSON-RPC — `method_visibility` 登记 `KeyChecked`，真调 `visibility::session_visible`，不可见与不存在返回逐字节相同的 `not_found`。`lane.rs::override_for` → `Execute`（它启动 run，该与 `agent.run` 共用运行并发预算）。
- `POST /v1/admin/resume` + `aleph-server resume <session-key>` — CLI 面，**IPC-only 无本地回退**（恢复意味着重进 harness，只有服务端做得到）。
- 两个 surface 共用 `handlers::resume::resume_named_session`，闸与状态词表决定一次。

**不新建 builtin tool**（用户裁定 (a)）：模型无法恢复自己正在跑的 run；真实消费者是操作者。目录条目的字节每个请求都付。

**全局句柄的注册条件**：`set_global_resume_coordinator` 必须在 `[resume] enabled` 分支**之外**调用。装在里面就是"解析句柄挂在比消费者更窄的条件上"（gateway/CLAUDE.md 地雷 H），症状是关掉自动恢复的部署里手动恢复静默不可用。

**每会话恢复互斥**（自查中发现的第三个真缺陷，且是**这轮自己新引入的暴露面**）：`repair_boundary` 是 read-then-append，两次并发恢复同一会话会各追加一次同样的修复集，同一 `call_id` 于是有两条 `ToolError` ⇒ 一个 tool_use 配两个 tool_result，该会话此后每一轮都被 provider 拒。boot 扫描顺序遍历所以从未暴露；按需面暴露了，且**包括与 boot 扫描本身相撞**（它是 spawn 出来的、还要等 30s channel 快照，期间 gateway 已在收请求）。`in_flight`（`Mutex<HashSet>` + RAII slot）在读日志之前认领，撞上则 `busy += 1`。`status_of` 先判 `busy`，否则会渲染成 `no_runs`。测试断言的是**日志里只有一条 `ToolError`**（真不变量），两种交错都成立，无 sleep 无顺序假设。

## 4. 这个设计让什么变难

1. **`ephemeral_id` 从此有语义。** 原本是纯 nonce，现在 `sub-bg-<request_id>` 是契约。缓解：判据写在 `ephemeral_for` 的 doc 里 + `child_key_roundtrips_through_the_request_id` 源码级 pin。
2. **懒解析把成本挪到失败路径。** 正常路径零成本；`list` 每次调用一次读。缓解：一次读服务全部 unknown id，且 `list` 是按需目录不是热路径。
3. **`agent.resume` 是新的执行入口。** 它复用 `execution_adapter.execute`，天然经过 `src/tools/scoped/` 那唯一强制点——但这条要靠测试断言，不能靠"看起来对"。
4. **admin 路由不经 `process_request`**，所以 `visible_owner_filter()` 恒 `None`、可见性闸在那条路上恒真。这与信任模型一致（bearer token = operator），但它是**两条路径两种强度**，写在 `resume_named_session` 的 doc 里而不是分散推导。

## 5. 明确不做

- **loop 持久化** — `looping/types.rs` 首行明写 "NEVER persisted"，是有意设计。
- **A4 统一 facade** — 零新增能力的抽象 = R10 的 YAGNI 撤回模式。
- **自动续跑子代理** — 用户选"报告 + 模型决定"。子代理的 cwd / 工具权限 / 父 run 是否还活着都要重建，且会在无人看管时烧钱。
- **codex 的 `residency` LRU 驻留** — Aleph 的 tracker 没有容量压力（按会话作用域 + TTL 修剪），移植它是给不存在的问题加机制。
