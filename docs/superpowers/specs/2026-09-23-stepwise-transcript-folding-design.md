# 按迭代折叠的转录 — 设计规格 (Stepwise Transcript Folding — Design Spec)

**Date:** 2026-09-23
**Branch:** main（单分支开发；三期各自在 worktree 里做、各自合并）
**Scope:** `src/gateway/execution_engine/agent_trace_emit_sink.rs` · `src/orchestrator/dispatch.rs` · `src/gateway/handlers/trace_replay.rs` · `shared/ui_logic/src/transcript/` · `interfaces/tui/` · `interfaces/webchat/`
**Status:** 已审阅；Phase S 实施中（计划 `docs/superpowers/plans/2026-09-23-stepwise-fold-phase-s-server-and-shared-core.md`）。Phase S 的裁定已回写进各节，每处带 `〔amended 2026-09-23 (Phase S)〕` 标记；逐条台账（各修订段引用的裁定 id 都在这里）在 [`docs/superpowers/plans/2026-09-23-stepwise-fold-phase-s-rulings.md`](../plans/2026-09-23-stepwise-fold-phase-s-rulings.md)
**Supersedes (partially):** [`2026-09-06-cc-style-transcript-rendering-design.md`](2026-09-06-cc-style-transcript-rendering-design.md) — 见 §10
**Reference:** [`docs/reference/TRANSCRIPT_RENDERING.md`](../../reference/TRANSCRIPT_RENDERING.md) · FEATURE_LOCATOR §6.13 · `docs/reference/SESSION_KNOBS.md`

> English summary: Today every agent TUI streams the whole process — thinking, narration, tool
> output — in full, which suits expert users and drowns everyone else. This design makes the
> *Think→Act iteration* the unit of folding: each iteration renders as one line (a headline derived
> from the provider's summarized thinking, a tool tally, a duration) that expands on demand; the
> run's final answer stays fully visible; a per-device `brief`/`full` detail level picks the
> default. The folding is one pure reducer in `shared-ui-logic::transcript`, fed by live frames
> and by `trace.by_runs` replay through the same code path. The server changes twice and the wire
> changes not at all: the agent-trace sink stops spawning its own drain task (so `turn_started`
> can no longer arrive after the text it precedes), and the replay RPC learns to return the
> thinking that the session log already stores per iteration. Three phases: S server + shared
> core, T TUI, P Panel.
>
> 〔amended 2026-09-23 (Phase S)〕 The last server change above rested on a false premise (§1: the
> session log's `turn_id` is the user turn, not the iteration). Ruling R9 replaced it: the harness
> emits a new trace variant, `ReasoningEmitted { iteration, text }`, which `task_traces` persists
> and `trace.by_runs` replays with no new code. So the `StreamEvent` frame set is unchanged, and the
> `AgentTraceEvent` event set gains one variant (§5).

---

## 1. 背景与扫描结论 (Verified facts, measured 2026-09-23 at `97d656a20`)

| 事实 | 锚点 | 对设计的意义 |
|---|---|---|
| Provider 侧的 thinking 流**已经是摘要**：OpenAI Responses 请求 `summary: "auto"`，`reasoning_summary_text.delta` 与原始 `reasoning_text.delta`（gpt-oss）**都**映射为 `ProviderDelta::ThinkingDelta`；Anthropic `ThinkingBlock.display` 从未被设置 ⇒ API 默认 `summarized` | `src/providers/responses/shared.rs:220` · `src/providers/protocols/openai_responses/tests.rs:616,650` · `src/providers/anthropic/types.rs:92-100` | 折叠正文 = provider 摘要，**wire 上分不出摘要与原始 CoT**；一行标题仍要确定性截首句（§4） |
| 每次 Think 迭代把 `{text, tool_use blocks, thinking, thinking_signature, usage}` 写进 session 事件日志（SSOT）——**但** `turn_id` 是整个用户轮的 uuid，不是迭代号；一次没有工具调用的迭代（最终回答、grace turn）在日志里**没有** join 键能对回某一步 | `src/harness/agent/think.rs:903-915` · `src/harness/agent.rs:988-998`（`current_turn_id`） | session 日志**不能**当按迭代重放的来源（2026-09-23 修正；原稿写"一次迭代一条事件，不用猜"，前提为假） |
| `trace.by_runs` 回放的是 `task_traces` 表（每行一个 `AgentTraceEvent`），文字行 `TextEmitted{iteration}` 与工具行**已经带迭代号**；session 日志只被读来补 diff presentation。缺的**只有 reasoning**：`LoopTraceEvent` 没有 thinking 变体 | `src/gateway/handlers/trace_replay.rs:129-242,324-378` · `src/harness/trace.rs:24-177` | 重放腿要补的是一个 trace 变体，不是一条读腿（§5.2） |
| 投影**每次迭代写一行** assistant 行 | `session_projector.rs:733` 每条 `AssistantMessage` 事件一行 | 冷加载时 history 与 replay 会双份，须按 run 去重（§8） |
| 实时帧 `Reasoning{run_id,seq,content,is_complete}` / `ResponseChunk{…, full_text, chunk_index, is_final}` **不带迭代号** | `src/gateway/event_emitter/types.rs:96-101,140-149` | 归属只能靠 `TurnStarted` 的顺序 |
| `TurnStarted{iteration}` 走 `AgentTraceEmitSink`：`try_send` 进 mpsc(256) + `tokio::spawn` 排水，`seq` 在排到时才分配；文字 / 思考 / 工具帧走 `harness_bridge/callback.rs` 的**同步** `broadcast::Sender<FlowStreamEvent>`（容量 `CHANNEL_BUFFER_SIZE = 256`） | `src/gateway/execution_engine/agent_trace_emit_sink.rs:92-118` · `src/orchestrator/harness_bridge/callback.rs:151,155` · `src/orchestrator/dispatch.rs:589,1048` | 竞态是**结构性**的（两条管道）；Panel `begin_step` 的注释就是在描述它 |
| 竞态修法的两个入口都在 orchestrator / gateway 侧（`callback.rs:21-28` 只定义 `on_delta/on_reasoning`，实现在 `orchestrator/harness_bridge/callback.rs`）；但 thinking **从未进过 trace 流**，让重放腿拿到按迭代的 reasoning 必须在 harness 加一个变体 + 两处 emit | `src/harness/trace.rs` · `src/harness/agent/think.rs:896,1335`（`TextEmitted` 的两个生产点） | R10 棘轮：按实测抬 `CEILING`，commit 里答三问（裁定 R9） |
| `RunSummary.loops: u32` 已在 wire 上，= `outcome.iterations` | `shared/protocol/src/events.rs:804` · `src/gateway/execution_engine/event_drain.rs:437` | 终局闸的现成事实（§5） |
| TUI：thinking 非 `/verbose` 时**整段不显示**（不是折叠）；`TranscriptEntry::Reasoning{collapsed}` 的 `collapsed` 字段**零读者**；`/verbose` 与 `/tools off\|new\|all\|verbose` 两根旋钮互不相干；`TurnStarted/TurnCompleted/VerifierVeto/ReactiveCompaction/CacheHealth/MoA*/ToolSummary` 全部经 `append_reasoning_entry` 变成"reasoning"条目 | `interfaces/tui/src/tui/widgets/chat_area.rs:789-793` · `app/trace.rs:56-82,303-320` · `app/mod.rs:953,1183` · `app/events.rs:421-424` · `commands.rs:348` | §7 的删除清单与 notes 去向 |
| Panel：`ReasoningPanel` 是挂在消息列表尾部的**单 run** 信号（`reasoning_text`），刷新即丢；`begin_step(iteration)` 已把上一气泡改标 `intermediate-{run}-{n}`，中间步骤已是无气泡密排行；`append_reasoning` 有约十个调用点承接旁白 | `interfaces/webchat/src/platform/wide/views/chat/reasoning.rs` · `state/mod.rs:1478-1530` · `events.rs:566` | Panel 已有"中间 vs 最终"边界，缺的是容器与持久 |
| Panel 右侧：inspector **已删除**，`WorkspaceBody` 只剩 `Artifacts \| Deliverables \| Tasks \| Canvas`；`WorkspaceState.tool_payloads` **活着**（`get_tool_payload` 三个生产读者：`tool_card.rs:507` · `messages.rs:1561` · `events.rs:817`，都在 chat 侧）；`expanded_events` 注释仍提"workspace-timeline cards" | `interfaces/webchat/src/state/layout.rs:60-70,229,236,392` | 用户裁定"右侧不承载对话"**已成立**；剩搬家与改注释 |
| `shared/ui_logic::transcript` 已有 `fold / summarize / group / view_model / diff_view / md_enhance / affordance / context / turn_summary`，Phase B 起 TUI 是它的客户端 | `docs/reference/TRANSCRIPT_RENDERING.md §4` | 本轮在其上加三块，不另起炉灶 |

---

## 2. 用户裁定（2026-09-23，**不再重问**）

| # | 问题 | 裁定 |
|---|---|---|
| R1 | 折叠行"简述"从哪来 | **B**：正文 = provider 摘要思考；标题 = 其首句 → 退回过渡文字首句 → 退回工具计数。**不**调小模型生成，**不**让主模型在 prompt 里写标题 |
| R2 | 流式期间折叠行显示什么 | **A**：思考块折叠流（动词 + 计时 + 尾 2 行）；助手文字**先开后折**——照常流入/打字机，下一个 `TurnStarted` 到达时回收成一行 |
| R3 | 右侧工作区 | **不承载任何对话内容**（附件 / 浏览器 / 画布等真功能保留）。折叠块只在 chat 窗口里展开 |
| R4 | 刷新 / 重连 / 换设备后 | **A**：原样重建——服务端补重放读腿，两腿一处派生 |
| R5 | "最终的总结性内容" | = **这一轮最后一段助手回复**，不额外调模型。最后一段太薄是 prompt 的事（R9），渲染层不补 |
| R6 | 折叠粒度 | **A**：按 Think→Act **迭代**折叠，一次迭代一行；展开后里面才是思考 / 过渡文字 / 工具行（工具行沿用 09-06 裁定的 cc-tui 规则） |
| R7 | `brief` / `full` 档位住哪 | **A**：每台设备一根（Panel localStorage 外观轴、TUI `<aleph_home>/tui-detail`），零服务端改动；TUI 的 `/verbose` 与 `/tools …` 合并为 `/detail`。R8 会话式入口**不预建**，只记落点（§13） |
| R8 | 实现走法 | **方案 1**：共享视图模型按迭代成型 · 单管道有序帧 · 一条重放读腿（§3） |
| R9 | 重放腿的 reasoning 从哪来（2026-09-23 计划期发现 §1 前提为假后追加） | **(b)**：harness trace 流加 `ReasoningEmitted { iteration, text }`，在 `think.rs` 紧挨 `TextEmitted{Final}` 的两个生产点发；迭代号由 harness 计数器一处派生，无人值守走写时脱敏，重放零新代码。**不**从 session 日志按 run 标记范围数第 N 条（grace turn 会错位，且是第二份推导）。代价：动 `src/harness/`（抬棘轮答三问）+ `AgentTraceEvent` 事件集 +1；只覆盖此后录的 run |

---

## 3. 架构 (Architecture)

```
harness ──on_reasoning/on_delta──▶ harness_bridge/callback ─┐
        ──on_trace(TurnStarted…)──▶ AgentTraceEmitSink ──────┤  同一条 broadcast<FlowStreamEvent>
                                                             ▼
                                              event_drain（原地 next_seq → StreamEvent::*）
                                                             │ wire（字节不变）
                     ┌───────────────────────────────────────┴──────────────────────────┐
              实时腿 apply_live(&StreamEvent)                              重放腿 apply_replay(&ReplayRow)
                     └────────────────► shared-ui-logic::transcript::Transcript ◄────────┘
                                                (纯 reducer，一处派生)
                                                             │ Vec<TranscriptEntry>
                                          ┌──────────────────┴─────────────────┐
                                        TUI 绘制                              Panel 绘制
```

**R9 修正（2026-09-23）**：reasoning 走 harness trace 流的新变体 `ReasoningEmitted` → `task_traces` 与 wire 的 `agent_trace` 帧；实时腿的 `StreamEvent::AgentTrace { event }` 与重放行是**同一个** `AgentTraceEvent`，所以 reducer 的两条腿收敛到同一个 `apply_trace(&AgentTraceEvent)`（§6）。

**为什么不是方案 2（帧上盖迭代号）**：迭代号只有 harness 知道——要么改 `HarnessCallback` trait（R10 棘轮，答 3 问），要么让 drain 再数一遍（判据 §1 两份推导）；frame_census 三个变体加字段；两腿的迭代号来源不同；竞态只是绕开没解决。
**为什么不是方案 3（服务端发 Step 信封帧）**：新帧家族过 census + 三份客户端解析 + 重放再造一份信封；标题要等 Think 结束才有 ⇒ 服务端做呈现决策，与"共享核 data→data、两端各自画"（TRANSCRIPT_RENDERING §4）分层相反；且它同样得先解决方案 1 的竞态。

---

## 4. 数据模型 · `shared/ui_logic::transcript`

```rust
TranscriptEntry::Step(StepEntry)          // 新：中间过程的唯一容器
TranscriptEntry::AssistantText { .. }     // 只剩一种用途：最终回答（不在 Step 里）
TranscriptEntry::Reasoning { .. }         // 删除：思考只能住在 Step 里

StepEntry {
    id: EntryId,
    iteration: Option<u32>,               // None = 服务端没告诉我是第几步（fail-closed，不猜）
    thinking: Option<ThinkingBlock { text: String, streaming: bool, unavailable: Option<Unavailable> }>,
    text: Option<String>,                 // 过渡文字（markdown）
    tools: Vec<StepTool>,                 // Tool(ToolRow) | Group(ToolGroup)，沿用 Phase B 的分组
    notes: Vec<Note { kind: NoteKind, text: String }>,   // step 作用域旁白，展开时才显示
    status: StepStatus,                   // Live | Settled | Pending（重放恢复、无结束事件，永不转圈）
    started_ms: Option<u64>, ended_ms: Option<u64>,
}
NoteKind = ToolSummary | VerifierVeto | ReactiveCompaction | MoaAdvisor | MoaAggregating | MoaSpend
```

〔amended 2026-09-23 (Phase S)〕 落地形状（`shared/ui_logic/src/transcript/step.rs`）与上面的草图有五处不同，各有裁定：

- `id: String`——仓里没有 `EntryId` 类型；`effective_open` 因此是 `(level, &HashSet<String>, &str)`（D5）。
- `ThinkingBlock { text, streaming }`，**没有** `unavailable` 字段：thinking 在写时被**原地**脱敏（`mask_trace_event`），wire 零改动，reducer 拿不到「被脱敏过」这个信号，所以不借 `Unavailable::Redacted`、不画原因芯片——折叠正文显示的就是打过码的文字（D1/D2；§5.3 G3 与 §9 同改）。
- `tools: Vec<ToolRow>`——分组是**绘制时**的投影（`group::group_tool_rows` → `StepTool`），不是存储形状（D3）。
- `NoteKind::MoaAdvisorSpend`，与协议变体同名（D4）。
- `TranscriptEntry::Reasoning` 在 Phase S **保留**：它的 TUI 渲染器要活到 Step 能被画出来为止，删除归 Phase T（D16；§12 的「本轮」指 S+T+P 这一整轮）。

**标题派生**（纯函数，一处实现，R1 的三级退化）：

```
step_headline(&StepEntry, max_cols) -> Headline { text: String, source: Thinking | Text | Tally }
  ① thinking.text 首句   ② 否则 text 首句   ③ 否则 tally 文本（"Read 2 files, Ran 1 command"）
first_sentence(s, max_cols): 第一个 。！？.!? 或换行之前；超宽按 char_indices 截断加 …；空白/空串 → None
tally(&StepEntry) -> Option<TurnSummaryEntry>: 复用 turn_summary::summarize_turn（改成也能吃一个 step）
```

〔amended 2026-09-23 (Phase S)〕 落地的规则：

- **句末**：`first_sentence` **保留**终止符；ASCII `.` `!` `?` 只在后面是空白（`char::is_whitespace`）或文本结尾时才算句末——`src/parse.rs`、`a != b`、`?q=` 都不切；全角 `。！？` 无条件；换行与单独的 `\r` 也是句末（D6 · T78-M1：一个太长的标题比一个切错的标题便宜）。
- **宽度**：候选前缀**整体**的字符串宽度（`UnicodeWidthStr::width`），不是逐字符求和——多码点序列（emoji + `U+FE0F`）的字符串宽度大于各字符之和，而 ratatui 按字符串宽度排版；截断扫描上限 `4 × max_cols` 个字符，超出即故意早截（T78-M2 · T78-R2b）。
- **tally** = `step_tally`，经 `turn_summary::summarize_rows`，没有最少工具数门槛；run 级的 `summarize_turn` 保留原样（D5）。
- **`step_headline` 可以是 `None`**：没有 thinking/text 首句、也没有终态工具行时（只有 notes，或只有 running/pending 的工具）。Phase S **没有第四级退化**——一个这样的活 step 怎么起标题是 Phase T 的绘制决定（§9「什么都没产出」那一行；T78-H1）。

**最终回答的判定**：`RunComplete` 时，当前打开的 step 的 `text` **提升**为顶层 `AssistantText`；该 step 若还剩 thinking / notes 就留下作一行折叠（`💭 Thought for 4s · 首句`），什么都不剩就删除。纯聊天一轮（思考 + 回答、无工具）因此渲染为一行折叠 + 完整回答，与 Claude Code 一致。

〔amended 2026-09-23 (Phase S)〕 **记录是权威的，未被记录覆盖的流式文字是临时的**。`TextEmitted{Final}` 替换它所覆盖的那一段流式文字，正如 `ReasoningEmitted` 替换流式 thinking（§5.2；T910-F1）——实时腿因此在 grace 轮与救援路径上也收敛到重放腿，这种情形**永不**报 `NeedsResync`。没有任何 `TextEmitted{Final}` 覆盖的流式 delta 在 run 结束时被丢弃：打开的 step 的文字截回到它的记录所覆盖的长度，没有记录的 step 以无文字结束（T910-N1）。**最终文字的填补是 run 作用域的**（T910-N1b）：只有当这个 run **没有任何 step** 持有文字记录时，打开的 step 才取 run 的最终文字（实时腿 `RunSummary.final_response`，重放腿 `SessionCompleted.final_text`）；只要有一步记录过文字，服务端的最终文字就是在复述已经显示过的东西，不再填。重放腿的填补还**按结局设闸**：只对「实时那一侧的 run 结束**确定带着同一份最终文字**」的那些结局填（T910-NEW1）——一个失败的 run 实时往往也以 `RunComplete` 结束，却不在此列，因为那一帧带的文字不确定（§7 末尾）——逐个结局的判定与它的服务端依据写在 `shared/ui_logic/src/transcript/reducer/mod.rs::apply_trace` 的 `SessionCompleted` 臂的注释里，那里是这份清单的唯一真源。`RunError` 不带最终文字，只有被记录覆盖的文字留下。理由是 R4：重放永远看不到未覆盖的 delta，实时腿就不能把它们当作回答呈现。代价：一次被叫停的流里没被确认的半截文字，在 run 结束时消失。

**run 作用域 vs step 作用域**：`CacheHealthDegraded` / `SessionCompleted` 等不属于某一步的旁白走顶层 `SystemNotice`（既有变体），不进 step。

〔amended 2026-09-23 (Phase S)〕 落地（`reducer/mod.rs::apply_trace`）：`CacheHealthDegraded` → 顶层 `SystemNotice`，它可能来自子代理的 metering（§5.4），只影响归属；`ProviderUsage` 是账不是 step，不产条目；`SessionCompleted { final_text }` 只在重放腿上到达（实时 sink 不发它，`is_step_event`），是**最终回答的兜底**——按上一段的 run 作用域与结局闸填进打开的 step，随后照常提升；它的 `duration_ms` 只供 run 结束的摘要行（D7 · T910-N1b · T910-NEW1）。

**档位与展开**：

```rust
DetailLevel { Brief, Full }                       // 默认 Brief
effective_open(level, overrides: &HashSet<EntryId>, id) -> bool   // = level.default_open() ^ overrides.contains(id)
```

两级两键：**翻 step**（Enter / 单击 step 行）与 **翻工具正文**（Ctrl+O，09-06 裁定不变），不混用。

〔amended 2026-09-23 (Phase S)〕 `DetailLevel::parse` 对读不懂的值返回 `None`，由**各端表面**静默退到 `Brief`（T78-M4）。覆盖集合是相对档位默认值的 XOR，所以一个记住的展开 / 折叠在切换档位后会**反转**——这符合上面的定义；切档时要不要清空覆盖集，列在 §7 末尾的交接清单里。

**归属规则（实时腿）**：每个内联帧归到"最近一次 `TurnStarted` 的迭代"。§5 修完后这一定成立；对着旧服务端的 TUI 遇到没开 step 就来了帧，开一个 `iteration: None` 的 step，标题只用计数，**永不**被后来的 `TurnStarted` 追认成某一步。

〔amended 2026-09-23 (Phase S)〕 reducer 里只有 `ReasoningEmitted` 核对迭代号：它与打开的、带编号的 step 不符即 `NeedsResync`，什么都不移动（`reducer/step_ops.rs::record_thinking`）。其余帧按到达顺序落进打开的 step——上面那句「归到最近一次 `TurnStarted`」是**意图**，它与实际有没有分歧由 Phase T 的真机阶段去量（§7 末尾）。

---

## 5. 服务端 · 三处改动；`StreamEvent` 帧集不变，`AgentTraceEvent` 事件集 +1

### 5.1 一条管道（`src/gateway/execution_engine/agent_trace_emit_sink.rs`）

`AgentTraceEmitSink::on_trace` 不再 `try_send` 进自己的 mpsc + `tokio::spawn` 排水，而是把 step 事件 `send` 进 `FlowStreamEvent` 那条 broadcast 管道（新变体 `FlowStreamEvent::Trace(aleph_protocol::AgentTraceEvent)`，在 sink 里先过 `is_step_event` 再 `.into()` 转成协议类型），由 `event_drain` 原地 `next_seq()` 后发 `StreamEvent::AgentTrace`。harness 调 `on_trace` 与调 `on_delta` 是同一线程同一程序序，所以 `seq` 从此就是逻辑序。`is_step_event` 过滤不变、帧内容不变、`frame_census` 不动、`UnattendedRedactingSink` 仍包在最外层先看到一切。

- **计划期已核（2026-09-23）**：sink 在 `run_loop/inner.rs:1014-1019` 构造并随 `FlowRequest.trace_sink` 进 dispatch，而 broadcast 发送端在 `orchestrator/dispatch.rs:1048` 之后才创建——两处**拿不到**同一个 `Sender`。裁定的落点：`FlowRequest` 加 `event_tx: Option<broadcast::Sender<FlowStreamEvent>>`，由 inner.rs 用 `flow_event_channel()` 创建后同时交给 sink 与请求；`dispatch` 有则 `subscribe()`、无则自建。脱敏 sink 仍包在 emit sink 外层（`redacting.rs:453` 那条读源码的测试钉着这个顺序）。**不准**开第二条管道。
  - 〔amended 2026-09-23 (Phase S)〕 **sink 不持有这条通道**：`AgentTraceEmitSink` 只存 `broadcast::WeakSender<FlowStreamEvent>`，每个事件 `upgrade` 一次，upgrade 失败即丢弃（run 已结束）。`inner.rs` 创建通道，**当场 drop 初始 receiver**，把强 sender **按值**移进 `FlowRequest.event_tx`（sink 先取它的弱半边）。理由：gateway 的 drain 在 harness 没发 `Complete` 就返回的那条臂上无界等待，只在 `Closed` 时退出，而后台子代理**有意**活过 run、经父链握着这个 sink——一个强 sender 会让 drain 等到每个后台孩子结束（T2-WEAK；否掉的两条是「每条提前返回补一个 `Complete`」与「给 await 加上限」）。守卫：`no_sender_outlives_dispatch` · `a_live_sink_does_not_keep_the_channel_open` · G1a/G1b（§5.3）。
  - 〔amended 2026-09-23 (Phase S)〕 一条链一个构造点：run 的 trace 链与子代理的链都在 `src/gateway/execution_engine/run_trace_sinks.rs::RunTraceSinks::build` 里建（§5.4）。
  - 〔amended 2026-09-23 (Phase S)〕 上面「计划期已核」那条里的三个行号是计划期的，已漂移，按符号读：通道在 `run_loop/inner.rs` 调 `RunTraceSinks::build` 之前创建；`dispatch` 在 `req.event_tx` 那个分支里 `subscribe`。`redacting.rs` 的那条测试（`the_agent_trace_exemption_is_backed_by_an_exhaustive_upstream_match`）钉的是 `mask_trace_event` 的**穷尽性**，不是包裹顺序；包裹顺序由 `RunTraceSinks::build` 一处构造决定，run 链上的端到端打码由 `a_reasoning_row_replays_masked_on_the_by_runs_leg` 经真链驱动。
- `FlowStreamEvent` 加变体会让每个穷尽 `match` 编译不过——那是穷尽匹配替我们数消费者，逐处答，不加 `_ =>`。
- **承重后的丢帧**：broadcast 接收端 `Lagged(n)` 丢掉的事件**没分配过 seq**，客户端的 `MissedSeqs` 看不见这种洞。step 事件每迭代约 2 条，缓冲按文字 delta 算本来就够；真正的闸在终局——`RunSummary.loops` 必须等于客户端数出的 step 数，不等就 `NeedsResync`（§9）。drain 遇到 `Lagged(n)` 时 `warn!` 带 run_id + 丢弃数。
  - 〔amended 2026-09-23 (Phase S)〕 **名字订正**：仓里的 `MissedSeqs` 是服务端 session 投影器对**事件日志** seq 的追踪（`src/gateway/session_projector/projector_sub/missed_seqs.rs`），与线上帧的 `seq` 无关；今天的聊天流客户端本来就**没有**线上帧的缺号检测。结论不变：`Lagged` 丢的帧没有 `seq`，任何按 `seq` 连续性查洞的检测都看不见它们。
  - 〔amended 2026-09-23 (Phase S)〕 **闸的方向**：落地的是 `summary.loops > 0 && steps_seen < summary.loops`，不是 `≠`（`reducer/run_end.rs::complete_run`；PF-D8）。grace 轮上 `loops` 与 `TurnStarted` 个数的关系没测过，一个会误报的闸比一个少报的更贵；Phase T 在第一次真 run 上量，量得精确就收紧成 `≠`。「不修补」指不铸 step、不重新编号——终局提升与摘要行照常追加。
- 不经 harness 的路径（`execution_engine/simple.rs` / `slash_command.rs` / `openai_api/completions/agent.rs`）本来就没有 `TurnStarted`，它们的 reasoning 落进 `iteration: None` step，不改。

### 5.2 重放腿 = 一个 trace 变体（R9；`src/harness/trace.rs` · `src/harness/agent/think.rs` · `src/gateway/trace_protocol.rs` · `shared/protocol/src/events.rs`）

`LoopTraceEvent::ReasoningEmitted { iteration, text }` 与协议侧 `AgentTraceEvent::ReasoningEmitted { iteration, text }`（`kind = "reasoning_emitted"`）。harness 在 `TextEmitted{Final}` 的**两个**生产点旁各发一次（正常 Think 轮 + grace 轮），只在 `response.thinking` 非空时发。它经既有链路落地：`GatewayTraceSink` → `task_traces`（`trace.by_runs` 原样回放，**零新代码**）；`is_step_event` 放行 → 实时 `agent_trace` 帧（reducer 用它把流式 thinking 替换成权威全文，与 `TextEmitted` 对文字的关系相同）；无人值守时 `mask_trace_event` 的新臂在写前脱敏（那个 match 是穷尽的，少一臂编译不过——机制而非纪律）。原稿"从 session 日志派生"的整段作废（§1）。

〔amended 2026-09-23 (Phase S)〕 **契约**（落地在协议变体 `AgentTraceEvent::ReasoningEmitted` 的 doc 上，`shared/protocol/src/events.rs`；harness 侧经一个私有 `emit_reasoning`，`src/harness/agent/think.rs`）：

- 只记**非空白**的 thinking（纯空白不记）。
- 通常**一次 Think 迭代一条**：有文字时发在 `TextEmitted{Final}` 之后，纯工具轮单独发。**verifier 叫停之后的 salvage 轮可以在同一个迭代号上再加一条**——消费者**追加**（空行连接），从不替换（T456-M2：给 salvage 轮单独编号是 R10 棘轮下的 harness 语义改动；丢掉任一条都是丢掉真实的推理）。
- `max_output_tokens` 续写时只记**最后一段**的 thinking，与 `AssistantMessage.thinking` 相同。
- **`TextEmitted{Final}` 对文字同样是权威的**（§4 的修订段；T910-F1）。
- 脱敏规则与 `TextEmitted` 相同：无人值守 run 写前打码；**有人值守 run 的 reasoning 行不打码落盘**（同一个 run 的属主）。受护栏的 run 上，reasoning 现在不经输出护栏就到达实时 `agent_trace` 与 `task_traces`——是否要这道闸是产品决定，列在 §7 末尾。

**穷尽 match 的连带**：`AgentTraceEvent` 不是 `non_exhaustive`，新变体让每个穷尽 match 编译不过——这正是它替我们数消费者。已知的：`trace_protocol.rs` 的 `From`、`unattended_redacting_sink.rs::mask_trace_event`、`events.rs::kind()`、`shared/protocol/src/trace_presentation.rs`、`interfaces/tui/src/tui/app/trace.rs`。TUI 的那一臂在 Phase S 里只求编译（追加为 thinking），Phase T 再接进 step。**没有一处允许 `_ =>`**。

〔amended 2026-09-23 (Phase S)〕 TUI 那一臂落地为 `=> {}`（`interfaces/tui/src/tui/app/trace.rs::append_trace_debug_entry`），**不是**「追加为 thinking」：TUI 已经在渲染实时的 `StreamEvent::Reasoning`，再追加一次就画两遍（PF-D14）。后果：重放与非流式轮次在 Phase T 之前**不显示** thinking。Panel 侧是字符串 `kind` 分派而不是穷尽 match：聊天转录的 `chat/events.rs::apply_trace_event` 落进 `_ => {}`；agent-trace 检查视图（`agent_trace_model.rs::map_node_type`）把它映成 Thinking 节点。

### 5.3 守卫（各答"什么时候红"）

- **G1 顺序**：单线程 runtime 下连发 `on_reasoning → on_trace(TurnStarted 2) → on_delta` 不 `await`，断言记录到的 seq 顺序 = 调用顺序。旧代码的 spawn 任务在测试让出前跑不了，`TurnStarted` 排到最后 ⇒ **旧代码确定性红**。
- **G2 两腿同构**：同一条录好的会话，实时帧折出的 `Vec<TranscriptEntry>` 与 `trace.by_runs` 折出的**逐字段相等**（判据 §9：共用判据也要共用推导）。住在 alephcore 集成测试里，用 `shared-ui-logic`（它是 alephcore 的 dev-dependency）。
- **G3 脱敏**：thinking 里塞一段 PEM，重放腿返回 `Unavailable::Redacted`。

〔amended 2026-09-23 (Phase S)〕 三个守卫的落地：

- **G1 拆成两条**：G1a `on_trace_publishes_onto_the_flow_channel_before_returning`（`agent_trace_emit_sink.rs`，一次**不带 runtime** 的 `try_recv` 必须已经看到帧——确定性的那半）；G1b `trace_frames_keep_their_place_in_the_run_seq_order`（`src/orchestrator/harness_bridge/tests.rs`，经真 `BroadcastCallback` 与真 sink 发、经真 drain 取号，断言 `seq` 序——驱动生产者而不是通道，PF-D13）。G1b 单独存在时能不能抓住「推迟到别的任务再发」，取决于测试里的让出点，所以 G1a 不可省。
- **G2 位置**：Phase S 的 G2 是共享核里的单元测试，两条腿都是手工构造的 fixture（`shared/ui_logic/src/transcript/reducer/tests/g2.rs::the_live_leg_and_the_replay_leg_fold_to_the_same_entries`，去掉时钟后比整份条目，覆盖两种轮内顺序、salvage 的两条记录、grace 轮）。**集成层的 G2**（真 drain 帧 vs 真 `trace.by_runs` 行）推迟到 Phase T 的 `qa/transcript_fold` 真机阶段，两条腿都来自活的服务端（PF-D12）。
- **G3 = 打过码的文字，没有芯片**（§4 的修订段）：`reasoning_emitted_is_masked_before_persistence_on_unattended_runs`（`unattended_redacting_sink.rs`）与 `a_reasoning_row_replays_masked_on_the_by_runs_leg`（`trace_replay.rs`，PEM 经真 `UnattendedRedactingSink` → 持久化链写入，`trace.by_runs` 返回的文字不含 PEM、带打码占位）。

### 5.4 子代理边界〔amended 2026-09-23 (Phase S)：原稿没有这一节〕

计划期发现（PF-C11）：子代理的 harness trace 事件原样流进父 run 的 trace 链——同步孩子在 spawn 时直接拿父 sink，后台孩子经 `ForwardingTraceSink` 转发——于是孩子的 `TurnStarted` 以父 run 的 `run_id` 成为**父**的 `agent_trace` 帧、落在父任务的 `task_traces` 下，`AgentTraceEvent` 上没有字段分得出来。一个孩子的迭代会被折成父的一步。

**裁定（PF-C11 + T2B-SPLIT，按生产者拆，不按 kind 过滤）**：

- 孩子的 **harness** 事件（`TurnStarted` / `TextEmitted` / `ReasoningEmitted` / `ToolSummary` / `SessionCompleted` / worktree 与 MCP-scope 生命周期）**不再**到达父 run 的 `agent_trace` 帧与 `task_traces` 行。孩子的 harness 链是 `[UnattendedRedactingSink if unattended] → [ScratchpadProgressSink if armed] → NoopTraceSink`；后台孩子的进度仍经 `ForwardingTraceSink` 到达 tracker 侧信道。
- 孩子的 **`MeteringProvider`** 仍握父 run 的链：`ProviderUsage` / `CacheHealthDegraded` 照旧落在父任务下，但**只在父 run 的强 sender 还活着时上线**。机制：`AgentTraceEmitSink` 只存通道的 `WeakSender`、每个事件 upgrade 一次（§5.1 修订段，T2-WEAK）；run 结束、强 sender 全部释放后 upgrade 失败，于是**后台**孩子在那之后的这两种帧**只落库、不上线**（持久化叶子仍活着）。本分支之前 mpsc drain 握着 emitter，会照发——这是 T2-WEAK 的代价，接受为文档化行为，要不要让它们实时可见见 §7 清单（终审 H2）。落库的那份：`teams.usage`、Panel Usage、`team_usage` 工具、MoA advisor 账与 doctor 的缓存检查都按 agent 聚合它们，而 `task_traces.task_id` 对 `agent_tasks` 有外键，孩子无法自有行。整条切断会静默把子代理花费归零。
- 两条链在 `src/gateway/execution_engine/run_trace_sinks.rs::RunTraceSinks::build` **一处**构造，经字段私有的 `ChildTraceSinks` 交给 `SubagentTool`。
- **对 §4 / §6 / §9 的后果**：两条腿都把 `ProviderUsage` / `CacheHealthDegraded` 当非 step 的 notice；父通道上的一个 `CacheHealthDegraded` 可能来自孩子（只影响归属）。孩子的生命周期事件**哪里都不落**（泄漏信号是 `tracing::error!`，见 `docs/reference/MULTI_AGENT_SYSTEM.md`）。孩子**自己的** run 级 `agent_trace` 流与持久化（嵌套 step）是 Phase T/P 的决定（§7 末尾）。
- **守卫**：`a_childs_turn_started_is_not_persisted_under_the_parents_task` 与它的对照 `a_childs_provider_usage_is_still_persisted_under_the_parents_task`（`src/agents/subagent_tool/tests.rs`）· `a_childs_compactor_usage_is_still_persisted_under_the_parents_task`（`src/agents/subagent_spawner/tests.rs`）· `a_childs_text_reaches_the_scratchpad_sharer_masked_on_unattended_runs`（`run_trace_sinks.rs`）。

---

## 6. 共享核 · `shared/ui_logic::transcript` 新增三块

| 模块 | 内容 |
|---|---|
| `step.rs` | §4 的类型 + `step_headline` / `first_sentence` / `tally` |
| `detail.rs` | `DetailLevel` + `effective_open`（纯函数；持久化在各客户端） |
| `reducer.rs` | `Transcript`：`apply_live(&StreamEvent, now_ms) -> Vec<Change>` · `apply_replay(&AgentTraceEvent) -> Vec<Change>` · `finish_replay() -> Vec<Change>`；`apply_live` 遇到 `StreamEvent::AgentTrace { event }` 与 `apply_replay` 都进同一个 `apply_trace(&AgentTraceEvent)`；`Change = Updated(id) \| Inserted(id) \| Removed(id) \| NeedsResync` |

reducer **替换**今天的两份客户端私有折叠器：TUI `app/trace.rs`（`push_reasoning` / `append_assistant_content` / `start_tool_execution`）与 Panel `ChatState` 的 `begin_step` / `append_chunk` / `set_step_text` / `update_tool` 一族。`settle_resumed` 的"恢复后永不转圈"、`ToolRow` 状态机、两张脸（`ToolStart` vs `AgentTrace::ToolCallStarted`）按 `tool_id` 的去重都搬进来，不复制。`RunComplete` 时它是 `TurnSummary`（`✻ Worked for 32s · 3 steps`）的**唯一生产者**——B6 发现的"有渲染器没生产者"就此闭合。

〔amended 2026-09-23 (Phase S)〕 落地：

- **文件**：`reducer.rs` 已按职责拆成 `reducer/{mod,step_ops,tool_rows,run_end}.rs`（测试在 `reducer/tests.rs` 与 `reducer/tests/g2.rs`）——生产代码过了 P2 的文件上限，在 Phase T 建在它上面之前拆开（T910-SPLIT）。
- **搬，不复制**：`trace_result_to_wire` 从 TUI `app/trace.rs` 搬进 `reducer/tool_rows.rs`，TUI 改为 import 共享的那一份（PF-C3/D15）。reducer/折叠器的其余部分仍是 Phase T 的替换工作。
- **`NeedsResync` 的客户端动作**（T910-N2）：可达的成因有两个——`RunSummary.loops` 多于数到的 `TurnStarted`，或一条 thinking 记录指向打开的 step 以外的迭代；其余生产者是今天没有输入能到达的 fail-closed 分支。动作：丢弃整份转录，从 session 的重放重建——用户消息来自 session 日志，每个 run 的 `trace.by_runs` 行逐条 `apply_replay`、再 `finish_replay`。run 进行中收到它时保留当前显示、标为陈旧，等 run 结束再重建。reducer 没有「替换某个 run」的接口，Phase S 也不加。
- **摘要行的输入**（T910-SUM）：两条腿都是**这个 run 的全部工具行**（行就是记录）；实时腿漏掉的行仍先经 `RunSummary.tool_summaries` 对账补建。摘要行**按 run** 计（`RunState.first_entry` 圈出这个 run 的条目），不是按整份转录。`TurnSummaryEntry` 没有 id 字段，它的 `Change::Inserted` 用 `next_id` 铸的 `summary-N`——一个按键重绘的消费者要么按位置作键、要么届时加字段（PF-C9，归 Phase T/P）。
- **`RunError` / 中止**：`fail_run` 先把未被记录覆盖的流式文字丢掉（错误不带最终文字，§4 修订段），再把打开的 step 剩下的文字提升为普通的顶层 `AssistantText`，另推一条 `SystemNotice("Error: …")`——`AssistantText` 没有 error 字段（D10；§9 同改）。
- **下标 fail-closed**：打开的 step 经 `get_mut` 取得，取不到即 `NeedsResync`，不用会 panic 的下标；越界的线上迭代号读作「未知」，不截断（PF-C5/C10 · PF-C6）。

**约束**：`default-features = false` 下必须编译（TUI 构建），不许碰 leptos / web-sys；输入是 `aleph_protocol` 的**类型化**帧。`md_enhance` / `fold` / `diff_view` / `group` 原样在 step 内部复用。在 reducer 自己的状态上就地更新是这个 crate 的既有习惯（`ToolRow::start/finish`），本轮沿用，不为不可变性另造一层拷贝。

**守卫**：
- `first_sentence` proptest：任意 UTF-8 输入不 panic、不切半个字、宽度不超限；CJK 与英文标点各有用例。
- 没开 step 就来了 `Reasoning` → `iteration: None`，断言它**永远不会**被后来的 `TurnStarted` 追认。
- `RunComplete` 提升：{有 thinking, 无 thinking} × {有 text, 无 text} 四种组合各一个用例（无 text 无 thinking 无 tools 无 notes ⇒ 消失）。
- `summary.loops ≠ 数出的 step 数` → 返回 `NeedsResync`，且**不**自行修补。〔amended 2026-09-23 (Phase S)：落地为 `loops > 0 && steps_seen < loops`，见 §5.1 的修订段；守卫断言 step 条目在 `complete_run` 前后逐字段相同，非 step 的提升与摘要行照常追加（PF-C7）。〕
- 一次迭代内并行工具调用（多个 `ToolStart` 先于任何 `ToolEnd`）落在同一个 step。

---

## 7. TUI · `interfaces/tui/`

**折叠行（brief 默认）**：

```
⏺ 失败在 parse_date 的时区处理      · Read 1 file, Ran 1 command · 8s
⠹ Pondering… 4s                                       ← 活着的 step：动词 + 计时
  ⎿ 最后两行 thinking 尾巴滚动
```

文字流入时按 R2：step 打开、文字直出（TUI 没有打字机）；下一个 `TurnStarted` 到达即回收成一行。回收时若视口正跟底（`scroll_offset == 0`）就继续跟底，否则保持锚点——B5 的 offset 模型已覆盖。

**展开的 step**：thinking 块（cc 规则折 2 行 + `… +N lines`）→ 过渡文字（完整 markdown）→ 工具行 / 分组（Phase B 原样，Ctrl+O 翻正文）→ notes（暗色）。

**键与鼠标**：`Enter` / 单击 step 行 = 翻这个 step（`RegionKind::StepToggle`，沿用 B5 的"头行 + 收尾行都是靶"）；`Ctrl+O` = 翻工具正文，不变。提示行加一段 `↵ step`，按既有优先级丢段。

**`/detail brief|full`**：设档位并持久化到 `<aleph_home>/tui-detail`，与 `/theme` 同一套机制（`aleph_protocol::paths::aleph_home`、`ALEPH_TUI_DETAIL` 单次覆盖、`cfg(test)` 下不读文件）。

**冷加载 / 重连**：`trace.by_runs` → `apply_replay`；`NeedsResync` → 只重拉那个 run 并**整段替换**它的条目。〔amended 2026-09-23 (Phase S)：reducer 没有「替换一个 run」的接口；`NeedsResync` 的动作是丢弃整份转录、从 session 重放重建（§6 的修订段，T910-N2）。下面守卫里的「整段替换」按此读。〕

**删除（CUT，不注释）**：`app/trace.rs` 整个；`chat_area.rs` 的 `render_reasoning`、`MessageKind::Reasoning`、`reasoning_shown_only_in_verbose` 测试；`AppState.verbose` + `/verbose`；`ToolProgressMode` + `/tools off|new|all|verbose`（brief 下工具行本在折叠里，`off` 失去意义；进度行已是运行中工具行的正文）；`current_run_uses_agent_trace`（去重进 reducer）。

**守卫**：
- brief：一个 settled step 恰好占一行；full：thinking + text + tools 都在——同一份 fixture 两个档位断言**行数不同且符合预期**。
- 回收：文字流入后 `TurnStarted(k+1)` 到达，step k 的物理行数**下降**——断言后果不断言格子（B5 教训：`TestBackend` 看不见宽度溢出）。
- 旧服务端：无 `TurnStarted` 的 `Reasoning` 渲染成没有步号的行，且**不含**"Step N"字样。
- `summary.loops` 不等 → 条目被重放结果整段替换（断言替换后的内容来自 replay fixture，不是断言"发了请求"）。
- `cargo build -p aleph-tui` 与 `cargo test` 都跑（B7：测试读遍每个字段，看不见死字段）。

### Phase S 交给本期（与 Phase P）的清单〔amended 2026-09-23 (Phase S)〕

每行一件事。括号里是来源：裁定 id 可在 [`2026-09-23-stepwise-fold-phase-s-rulings.md`](../plans/2026-09-23-stepwise-fold-phase-s-rulings.md) 里查到；`910-F7` 这类复核条目号只是标签，每行本身已写明那件事：

- **消费 `Change::NeedsResync`**：实现 §6 修订段里的整份重建（910-F7）。
- **`apply_replay` 撞上一个打开的实时 run**：今天它折进那个 run；fail-closed 的选项是报 `NeedsResync`（910-F9）。
- **只有 `ReasoningEmitted` 核对迭代号**：真机阶段量「归到最近一次 `TurnStarted`」与实际有没有分歧，含 salvage 形状（没有新的 `TurnStarted`，被叫停那一轮的迭代上来 text + reasoning）（910 concern 2/4）。
- **集成层 G2**：`qa/transcript_fold`，真 drain 帧 vs 真 `trace.by_runs` 行（§5.3 修订段；Task 10）。
- **`<` 收紧成 `≠`**：在第一次真 run 上量 grace 轮的 `loops` 与 `TurnStarted` 个数（§5.1 修订段；PF-D8）。
- **活 step 的空标题**：只有 running/pending 工具或只有 notes 的 step，`step_headline` 是 `None`，怎么画由本期定（§4 修订段；T78-H1）。
- **两条记录的 step**：标题取第一条记录（910-F13）；要不要给 salvage 那条加标签（T456-M2）。
- **切档与覆盖集**：XOR 覆盖在切档后反转，要不要切档即清（§4 修订段；T78-L5）。
- **摘要行措辞**：零 loop 路径上的 `· 1 steps` / `· 0 steps`（910-F12）；`TurnSummaryEntry` 的键（PF-C9）。
- **续写轮的 thinking**：一个 step 的 thinking 会从第 1 段（流式）翻成第 N 段（记录）（456-L3）。
- **reasoning 第二份持久副本没有字节上限**：`task_traces` 里的 thinking 不设上限，`trace.by_runs` 一次最多服务 `MAX_RUNS` 个 run、不截断（456-L8）。
- **受护栏的 run**：reasoning 不经输出护栏到达实时 `agent_trace` 与 `task_traces`；有人值守 run 的 reasoning 不打码落盘（与 `TextEmitted` 同规则、同属主）——产品决定（456-L2）。
- **服务端的最终文字扫描是整个会话的**：`src/harness/agent.rs` 的 `final_text` 与 `think.rs` 的 `last_assistant_has_text` 扫描整份 session 日志，而 `runner_impl.rs` 把 `final_response` 限在本 run——一个没记下任何 `AssistantMessage` 的 run，重放时会把上一个 run 的回答当成自己的，grace 轮也可能因上一个 run 的文字被跳过。修法是把两处扫描限在本 run：这是 `src/harness/` 的改动（R10 棘轮，答三问），不在 Phase S 的服务端范围内（910-rereview2 NEW-1 (ii)）。
- **失败 / 取消的 run，两条腿的回答不一致**：实时腿从 `RunComplete.final_response` 填回答（runner 在两条臂上都发完成帧，drain 在 `RunError` 之前就发出 `RunComplete`），重放腿按结局闸不填（§4 修订段）。实时那份文字有三种形态：本 run 的文字 / 被重试取代后什么都没有 / drain 错过完成帧时的错误回执。关闭方式是服务端对「本 run 的最终文字」只有一个推导，与上一条的 run 作用域扫描一起做，而不是在 reducer 里加启发式（T910-NEW1b）。
- **取消还有第二条没追过的竞态**：`src/gateway/execution_engine/execute.rs` 外层 `select!` 的取消 / 截止分支，与上一条追过的 drain 内部竞态是两回事；与上一条同一个桶（910-rereview3 LOW）。
- **thinking 兜底的回答会重复 thinking 块**：一个 run 没有文字记录、而服务端的最终文字取自最后一条消息的 thinking（只有 thinking 的 provider 兜底）时，两条腿上的回答都会重复该 step 的 thinking 块；本期确认它看起来如何（910-rereview2 LOW-3）。
- **截断切开 ZWJ 序列**：纯外观（78-L4）。
- **只认单一位置的 TUI 扫描**：Step 有了生产者之后，这几处看不见 step 里的行——`interfaces/tui/src/tui/app/mod.rs` 的 `find_tool_mut` 与按工具名的 `find_map`、`app/tests.rs` 泄漏测试里的 `TranscriptEntry::Step(_) => ""` 臂、`widgets/chat_area.rs` 的 `content_fingerprint(&s.id)` 臂（78-L6，替换清单）。
- **`TranscriptEntry::Reasoning` 的删除**（§4 修订段；PF-D16）。
- **子代理自己的 run 级流**：孩子自己的 `agent_trace` 与它名下的 `trace.by_runs`（需要每个孩子一行 `agent_tasks`，因为 `task_traces.task_id` 有外键）——嵌套 step 是 T/P 的决定（§5.4；2b 遗留）。
- **后台孩子的迟到账目帧不上线**：父 run 结束后，后台孩子的 `ProviderUsage` / `CacheHealthDegraded` 只落库、不上线（§5.4）；任何实时消费者在那之后都看不到它们，`teams.usage` 与 doctor 读的是落库行。要不要让它们实时可见由本期决定（终审 H2，T2-WEAK 的代价）。
- **中途接管一个进行中的 run**：`apply_live` 只折由 `RunAccepted` 打开的 run；`finish_replay` 把 `run` 置空，于是重放之后仍在跑的那个 run 的实时帧也会被丢弃，reducer 没有 adopt 入口。TUI 的 `adopt_active_run` 与 Panel 第二个标签页加入都是这个形状，「先重放、再接实时」需要一个 reducer 入口；设计问题是半成品行与后续帧怎么接（终审 M3）。

---

## 8. Panel · `interfaces/webchat/`

**输入侧**：`subscribe_run_events` 顶部做**一次**类型化解码，转录相关帧（`reasoning` / `response_chunk` / `tool_*` / `agent_trace` / `run_complete` / `run_error`）以 `StreamEvent` 喂共享 reducer；其余帧（phase、retry 通知、成本、plan、rooms……）暂留字符串分派。这是"一条流两张脸"，计划期数清剩几条臂、记入 §13 遗留，本轮不全量改写。

**状态侧**：`ChatState.messages: Vec<ChatMessage>` 让位给 `transcript: RwSignal<Transcript>`；`timeline.rs` 的 `build_rows` 改吃 `TranscriptEntry`，行型变成 `DaySeparator | User | Step | Final | Notice`。`ChatMessage` 上的 `is_intermediate` / `iteration` / `text_finalized` / `is_final` 随旧模型消失；`transcript_markdown` 导出改吃条目（语义不变：用户 + 最终回答）。

**渲染**：`StepRow` 组件——折叠 = 一行，点击翻；活着 = 脉冲点 + 动词 + 计时 + thinking 尾 2 行（今天 `ReasoningPanel` 的手感**搬进 step 行**）；文字流入时 step 打开、`TypewriterRenderer` 照常揭示（游标 keyed on step id）；下一个 `TurnStarted` 到达即回收，`max-height` 过渡软化跳动，`prefers-reduced-motion` 时不动画。展开 = thinking（`line-clamp: 2` + 共享 `fold` 算 `+N`）→ 文字 → `ToolCard`（09-06 Phase C 既定：消费共享 `ToolRow`）→ notes。最终回答仍是顶层 `MessageBubble`，打字机不变。

**档位**：`appearance.rs` 加第七根轴 `detail`，localStorage；chat 头部一个「简洁 / 详细」切换。每 step 覆盖集合复用 `expanded_events`——键从 tool_id 泛化为 `EntryId`（step 与 tool 同一集合、按会话、LRU ≤ 500）。`Ctrl+O` 仍是工具正文。

**冷加载**：`chat.history` 只取用户行；run 内一切来自 `trace.by_runs` → `apply_replay`。**规则**：`trace.by_runs` 里有的 run，其 `chat.history` 助手行一律丢弃（投影每次迭代写一行，不丢就双份）；没有 trace 的老会话才用 history 助手行兜底。手机端：同一 reducer、同一 `StepRow`、只窄不改。

**删除 / 搬家**：`reasoning.rs`（`ReasoningPanel`）；`ChatState.reasoning_text` / `append_reasoning` / `apply_reasoning_block` 及 events.rs 里全部 `append_reasoning(...)` 调用点（改投 notes / `SystemNotice`）；`begin_step` / `set_step_text` / `text_finalized` 整套竞态机器；`WorkspaceState.expanded_events` 与 `tool_payloads` **搬家**到 chat 状态（活着，只是住错地方）；那条"workspace-timeline cards"注释。右侧面板除此不动（R3）。

**守卫**：
- 同一 run 只渲染一次：history + replay 双源 fixture，按 run_id 计数 = 1。
- 回收后 step k 的打字机**停止**订阅 tick（`has_reveal == false`），不泄漏动画帧。
- `detail` 轴 localStorage 往返；`summary.loops` 不等 → 整段替换（同 TUI）。〔amended 2026-09-23 (Phase S)：「整段」= 整份转录从 session 重放重建，见 §6 修订段（T910-N2）。〕
- `cargo test -p aleph-panel --lib` + `just wasm`（唯一编译出厂形态的命令）。

---

## 9. 失败语义（fail-closed）

| 情形 | 行为 |
|---|---|
| 帧前面没有 `TurnStarted` | `iteration: None` step，标题只用计数，**永不**被追认步号 |
| `summary.loops ≠ 数出的 step 数` | `NeedsResync` → 重拉该 run 整段替换；重拉也失败 → 保留实时条目 + 顶层 `SystemNotice`「过程可能不完整」，**不**装作完整。〔amended 2026-09-23 (Phase S)：条件落地为 `loops > 0 && steps_seen < loops`（§5.1）；重建的是整份转录，run 进行中则先标陈旧、run 结束再建（§6，T910-N2）〕 |
| thinking 记录的迭代号与打开的 step 不符〔amended 2026-09-23 (Phase S)：新增行〕 | `NeedsResync`，什么都不移动；客户端动作与上一行相同（§6 修订段，T910-N2） |
| run 结束时 step 里有未被记录覆盖的流式文字〔amended 2026-09-23 (Phase S)：新增行〕 | 丢弃；最终文字只在整个 run 没有文字记录时填补，重放腿另按结局设闸（§4 修订段，T910-N1 · N1b · NEW1） |
| 重放行无结束事件 | `Pending`，不转圈（沿用 `settle_resumed`） |
| 一次迭代什么都没产出（thinking/text/tools/notes 全空） | 不渲染——"Step 3：（无）"是在断言什么都没发生 |
| thinking 被脱敏 | 正文显示 `Unavailable::Redacted` 原因芯片，标题退到 text → 计数。〔amended 2026-09-23 (Phase S)：没有芯片——thinking 在写时原地打码，reducer 不知道它被打过码；正文与标题都是打过码的文字（§4 修订段，D1/D2）。标题里出现打码占位符还是退到 text，由 Phase T 定〕 |
| provider 给的是原始 CoT | wire 分不出来，正文照显、首句可能弱；写进文档，不加启发式猜 |
| `RunError` / 中止时 step 还开着 | 已流出的 text **提升**为顶层回答并带既有的 error 标记——用户已经看见了它，藏回折叠里才是撒谎。〔amended 2026-09-23 (Phase S)：`AssistantText` 没有 error 字段，落地为普通 `AssistantText` + 一条 `SystemNotice("Error: …")`（D10）〕 |
| `tui-detail` 读不了 / localStorage 不可用 | 退 `Brief`，静默 |
| drain 遇到 broadcast `Lagged(n)` | `warn!` 带 run_id + 丢弃数；终局由 `loops` 闸兜底 |
| 新 TUI 对旧 server | wire 没变 ⇒ 只是没有 `TurnStarted`，落第一行 |
| 无人值守 run | 实时腿 `redacting.rs` 已脱敏 reasoning；重放腿无条件脱敏——不变。〔amended 2026-09-23 (Phase S)：`ReasoningEmitted` 行在**写时**由 `mask_trace_event` 打码；重放腿对 trace 行没有自己的 masker，返回的就是写下的那一份（「无条件脱敏」指的是 `presentation`，见 `TRANSCRIPT_RENDERING.md` §2.3）〕 |

---

## 10. 与 09-06 spec 的关系 · 文档落点

本 spec **取代** [`2026-09-06-cc-style-transcript-rendering-design.md`](2026-09-06-cc-style-transcript-rendering-design.md) 的这几处：

| 旧 spec 位置 | 取代内容 |
|---|---|
| §5 `view_model` 行 | 条目集：去 `Reasoning`，加 `Step` |
| §6 Phase B「转录模型：按时间交错」 | 工具行仍按时间交错，但住在 step 容器里 |
| §7 Phase C `expanded_events` 键 | tool_id 泛化为 `EntryId` |
| §7 Phase C「刻意不做：reasoning 改逐消息」 | 现在经 step 做了 |
| §7 Phase C「每轮摘要行」 | 生产者是共享 reducer |

其余**照旧有效**：cc-tui 折叠规则、diff、mermaid、TUI 身份、主题、`/context` 覆盖层。做法：旧 spec 顶部加一条横幅指向本文，**不**逐行改它（一个事实一个家）。

**文档落点**（完成后独立 commit）：`TRANSCRIPT_RENDERING.md` 新增 §7（Step · reducer · 重放腿 · 单管道），§5.1「没有渲染器」那笔债关闭；`FEATURE_LOCATOR §6.13` 更新，〔amended 2026-09-23 (Phase S)：Phase S 不关闭 §5.1 那笔债——它的形状在 TRANSCRIPT_RENDERING §7.3 以新的日期复现，Phase T 关闭它；FEATURE_LOCATOR 的 `### 6.13` 由 `e164ce568`（2026-09-07）写下；同日的合并 `6eaa64b94` 手工解了两处真的附录编号冲突（解得对），但改写的范围超出了 git 标出的冲突块，把本节的标题与正文连同 git 能干净合并的其他内容一起丢了，十一处引用却全部留下；这一轮是**重建**它（按合并前的原文取回仍然成立的部分，见 FEATURE_LOCATOR §6.13 与附录 D.0.198）〕新的判据实例进附录 D/E；`SESSION_KNOBS.md` 加一行"显示密度**刻意不是**会话旋钮，住在客户端"；`CLAUDE.md` 路由表那一行只在事实变了时改（`trace.tool_output` 仍零客户端，本轮不动）。

---

## 11. 分期与验证

**三期，沿用 09-06 的 A→B→C 顺序**：

| 期 | 内容 | 合并条件 |
|---|---|---|
| **S** 服务端 + 共享核 | §5 两处 + §6 三块 + G1–G3 + reducer 守卫 | 这一期结束时**没有任何东西在渲染它**——被裁定许可的零消费者区间，文档里带日期写明；T 在同一轮内开工收口 |
| **T** TUI | §7 | T 合并后 `shared/ui_logic::transcript::reducer` 有第一个客户端 |
| **P** Panel | §8 | P 合并后 `ReasoningPanel` / `begin_step` 消失 |

每期各自一份 writing-plans 计划、各自 worktree、各自合并。

**最小可信验证集**：CLAUDE.md 六条 + `cargo build -p shared-ui-logic --no-default-features` + `cargo build -p aleph-tui` + `just wasm`。`alephcore --lib` **按名字**对 Windows 基线 diff，不看条数（附录 C）。

**真机**：新增 `qa/transcript_fold/run.sh {order,replay}`——`order` 用已有的 WS 帧 tap 对真 run 断言每个迭代 `TurnStarted.seq <` 该迭代第一条 `Reasoning.seq`；`replay` 把实时抓到的 step 与 `trace.by_runs` 折出的逐字段比对。`resync` 没法诚实注入，**不写**假阶段。Windows Terminal 上 `⏺ ⠹ ✻` 列宽仍 UNMEASURED，继续挂在清单上。

**回滚**：`StreamEvent` 帧集未变 ⇒ 客户端不受服务端回滚影响；服务端回滚 = revert §5 的三组改动（管道 / harness 变体与协议变体 / 穷尽 match 各臂）——三组必须一起 revert，单独 revert 变体会让各臂编译不过。

---

## 12. 熵减清单（本轮内完成，CUT 不是注释掉）

- TUI：`app/trace.rs` · `render_reasoning` · `MessageKind::Reasoning` · `AppState.verbose` + `/verbose` · `ToolProgressMode` + `/tools` · `current_run_uses_agent_trace`
- Panel：`reasoning.rs` · `reasoning_text` / `append_reasoning` / `apply_reasoning_block` · `begin_step` / `set_step_text` / `text_finalized` · `ChatMessage.{is_intermediate, iteration, is_final}` · "workspace-timeline cards" 注释
- 共享核：`TranscriptEntry::Reasoning`（含零读者的 `collapsed` 字段）〔amended 2026-09-23 (Phase S)：归 Phase T；「本轮」= S+T+P 整轮（PF-D16）〕
- 服务端：`AgentTraceEmitSink` 的 mpsc + spawn〔amended 2026-09-23 (Phase S)：已 CUT，连同只服务它的 `emit_agent_trace`〕

---

## 13. 刻意不做 · 遗留

- **不**给思考做 LLM 二次摘要、**不**让主模型写标题（R1）。
- **不**做服务端每用户档位（R7）。哪天要 R8 会话式入口：值放 `SessionMetadata.identity_meta.custom["detail_level"]`，读者是客户端启动时的一次 `sessions.get`，仍以设备本地覆盖为准——先记落点，不预建。
- **不**改其他通道（Telegram / Slack 等只收最终回答，本来如此）。
- **不**做 phone 专属版式；**不**做并排 diff；**不**接 `trace.tool_output`（仍零客户端）。
- **遗留**：R9 之前录的 run 在 `task_traces` 里没有 reasoning——冷加载时那些 step 的标题退到 text 首句 / 工具计数，thinking 位显示为"未记录"，**不**回 session 日志去猜〔amended 2026-09-23 (Phase S)：Phase S 不给 reducer 这个信号——`ThinkingBlock` 没有 `unavailable` 字段，reducer 分不出「老 run」与「这一步本来就没有 thinking」；旧 run 的 thinking 位怎么画由 Phase T 定（§4 修订段）〕；Panel `events.rs` 非转录帧仍走字符串分派（§8）；`RunSummary` 网关孪生与协议孪生的整体统一（09-06 §4.4a 已记）；`ToolGroup::headline` 无条件数 `rows.len()`（TRANSCRIPT_RENDERING §5.6）。
