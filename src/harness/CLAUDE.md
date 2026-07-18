# src/harness/ — 薄 Harness 护栏 (R10 本地红线)

> 本文是根 `CLAUDE.md` R10 的本地强化，编辑本目录前必读。完整哲学见
> [HARNESS_PHILOSOPHY.md](../../docs/reference/HARNESS_PHILOSOPHY.md)。

## 硬边界：12 文件 + 行数棘轮

- 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
- `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`

**新增文件须在 PR 描述说明为何无法装进现有 12 个文件之一。**

**口径**：行数按"文件开头到该文件内第一个**顶层（第 0 列）** `#[cfg(test)]` 之前"计，内联测试不计入预算（超预算就把测试搬去 `src/harness/tests/`，而不是当作行数豁免的借口）。

**「顶层」二字是本条最重要的部分**——见下方警告。**口径现在由测试执行**：`src/harness/tests/budget.rs`（跑在 `cargo test -p alephcore --lib` 里），同时守 12 文件与行数；出现第 13 个文件或行数上涨即 FAIL。**改这里的数字就得改那里的 `CEILING`，反之亦然。**

**当前测量（2026-07-18，流式装饰器下沉后）：5070 行。这就是红线本身。** 由 `tests/budget.rs` 的棘轮守（`CEILING = 5070`，实测非手算，只减不增，增必答下方 3 问）。**5072 → 5070（−2）**：`stream_llm_call` 弃掉 `as_http_provider()` 降级分支、改单个多态 `execute_streaming_dyn` 调用——那个 downcast 直取内层 `HttpProvider`，每个流式回合都跳过 `ThinkLevelProvider`/`MeteringProvider`（丢 `think_level`、不发 `ProviderUsage`）；副作用下沉到 `src/providers/` 装饰器（trait 默认 + 三处覆写），认知/策略留在 harness 外（R10）。下调无需答 3 问。Batch 6 两侧同日从 5035 出发、合并实测 5072（净 +37）：上调侧 +80——ambient CallIdentity 审批关联（换掉 `newest_tool_call` 名扫描，scoped/dispatch.rs 净删 −75）+ 完成序 live 事件（`buffer_unordered` 完成驱动环，转录仍输入序），3 问作答见 `tests/budget.rs::CEILING` 的 Batch 6 注释；删除侧 −42——test-only `run_turn` 簇迁去 `tests/harness_ext.rs`（`agent.rs` −39）＋删恒零 `consecutive_errors` trace 字段（`trace.rs`/`think.rs` −3）。**旧的 ~4900 目标已退休**——它是一次手算口径事故（生产 `impl` 中间那个缩进 `#[cfg(test)]` 截断 `agent.rs`、静默漏计 846 行）的残值，从不是实测地板；循环不再背那个不存在的"143 行债"。红线是**棘轮机制本身**，不是某个具体数字。

**单文件软性气味线（非硬门）**：任何一个文件逼近 **~800 行**，先找下沉去处（见文末"下沉去处"）再往里写，别让它继续吃行数。当前最大的三个未审计面——`agent/think.rs` 1509 / `agent/act.rs` 1120 / `agent.rs` 1037——都已越过 800，这不是 `budget.rs` 强制的门，是给人看的"该拆了"信号。

> 5593 → 5043（−550）：第四轮，本战役最大的两次搬迁，也是第一次**搬走的是依赖而不只是行数**。两项都是纯搬迁，因此可接受的行为差异只有零——两份 diff 都对着 `HEAD` 逐行审过，确认为零。
> - **−221 trace.rs（465 → 244）**：六个 `From<LoopTrace*> for aleph_protocol::AgentTrace*` 迁往 `src/gateway/trace_protocol.rs`，紧挨着它仅有的三个调用点。为一个循环根本不认识的传输层做序列化，从来就不是**循环的**脚手架。真正的战利品不是那 221 行：`rg aleph_protocol src/harness/` 现在**返回空**——Think→Act 循环不再依赖 gateway 线协议。纯切除：diff 为 0 增 221 删，搬走的函数体与原文逐字节相同。
> - **−335 agent/think.rs（1844 → 1509）**：反应式压缩救援簇（`drain_context_overflow` / `try_reactive_compact_and_retry` / `reactive_fit_and_retry` / `MAX_REACTIVE_COMPACT_ATTEMPTS` 上限，以及只有一个调用者、直接删掉的 `compact_to_fit_in_place` 包装）迁往 `src/context/compact/rescue.rs`。它是**机制不是认知**：压不压缩完全由 providers 层的 `llm_retry::classify` 给出 `CompactAndRetry` 裁决决定。故这不触犯 R10 第 5 不（harness 仍然不挑恢复策略），也不是对 A2 的倒退（模型依旧看得见错误并自愈）。
>
> 缝的方向决定了这是**下沉**而非**挪窝**：`RescueHost` 定义在 **context 层**、由 harness 实现（P4），关联类型 `Fatal: From<AlephError>` 使 `src/context/` 永不点名 `HarnessError`。`rg "crate::harness" src/context/` 返回空。
>
> **Task 8 那句挂了很久的"BLOCKED——依赖读写私有 harness 状态的 `&self` 方法，不是可参数化的 `self.deps.X` 字段"是错的。** 真正需要的只有 5 个运行态把手（LLM 调用 / 救援槽 / token 记账 / trace / 终止原因），它们装进了一个 52 行的适配器。
>
> 这个适配器加上 `RescueCx` 的构造，正是本次净 −335 而非计划预估 −367 的原因。**按真实成本记账**，一如既往。
>
> - **+6 `agent.rs`**：下沉暴露出一个纯搬迁本会原样保留的谎——`MAX_REACTIVE_COMPACT_ATTEMPTS` 是**装饰性的**，真正的上限是槽位里硬编码的 `compare_exchange(0, 1)`，把常量调大什么都不会发生。而 S2 之后常量在 **context 层**、槽位在 **harness**，一个被无视的 cap 就不只是脚下的雷，而是对刚建起来的那个 seam（policy 归 context，state 归 harness）的直接背叛。槽位现在真的去读那个 cap 了，并由 `the_rescue_slot_is_bounded_by_the_context_layers_cap_not_a_hardcoded_one` 钉住。
>
> 净 **−550**，落到 **5043**——循环有史以来离旧的 4900 目标最近的一次。**2026-07-15 该目标已退休**（见上方状态行）：它是测量事故不是实测地板，那"143 行债"欠的是个从不存在的数。`CEILING = 5043` 现在就是红线本身，棘轮纪律不变——只减不增，增必答 3 问。

> 5739 → 5593（−146）：第三轮。**Act 期墙钟离开循环**。它从来不是脚手架——"一个工具最多能跑多久"是工具自己声明的属性，harness 却拿 run 级的 `turn_timeout` 去替它判，超时还升级成 `StalledTurn` **直接杀掉整个 run**（生产受害者：人类审批慢于 120s 的那一批命令）。墙钟下沉到工具唯一收口 `src/tools/scoped/dispatch.rs::execute_inner`，且**落在所有能等人的闸门之下**；同一次超时变成模型下一轮读得到的 `ToolError::Timeout`，循环随之删掉只为跑这块表而存在的机件。
> - **−149 act.rs**：`resolve_effective_budget`、两处 `describe()` 预算探测、两处 `tokio::time::timeout` 包裹、串行 `StalledTurn` 恢复块、并行路径的 `budgets` 向量 / `Err(elapsed)` 臂 / `first_stall`，以及 `TurnPhase` / `STALLED_CALL_CAUSE` / `budget_overrun_cause` 三个 import。`ExecOutcome` 塌回 `Result<ToolOutput, ToolError>`。
> - **+2 deps.rs / +1 trait_def.rs**：这两个文件的文档都**断言** `turn_timeout` 约束 Act。它现在不约束了——文档一旦点名不变量，就必须是真的，故改写为"Act 由每工具预算约束，`turn_timeout` 只约束 Think"。
>
> 净 **−146**，全部来自真删除（+3 是被搬走的那个不变量的文档）。
>
> 5997 → 5863（−134）：棘轮第一次往回转。全部来自删除，没有靠搬家或删注释凑数——`trait_def.rs` −56（`Harness` trait 及其默认 `run()` 循环，唯一 impl 是 `AgentHarness` 且已覆写；真正的多态缝是 `SessionDriver` 与 `Arc<dyn HarnessRunner>`）、`chain_context.rs` −21（`with_max_depth` + `Display`，调用方全在 `#[cfg(test)]`）、`callback.rs`/`agent.rs`/`act.rs` −21（`on_complete` + `on_tool_call` 两条回调通道：循环里 9 个发射点，生产侧 0 个监听者）、`trace_sink.rs` −10（`on_init_seam`）、`think.rs` −21（`reactive_fit_and_retry` 两个同构分支合并 + `fire_grace_turn` 折进 `fire_boundary_grace_turn`）。逐项理由见 `tests/budget.rs::CEILING` 注释。
>
> 5863 → 5739（−124）：第二轮，也是**第一次在往循环里加生产代码的同时还净减**。三个 bug 修复 + 一个并发守卫共 **+21** 行，靠两笔下沉付清，而不是靠账面抹平：
> - **−90 文案下沉**：循环注入模型的 9 条字符串（`MAX_STEPS_HINT` / `MAX_OUTPUT_TOKENS_RESUME_NUDGE` / `INTERRUPTION_NOTE` / 两条合成 tool-error cause / deferred reason / 三个插值构造函数）迁往 `src/thinker/nudges.rs`（think −30、prompt −36、act −24）。提示词文案是认知（R9），harness 只是脚手架（R10）。**纯搬运**：渲染结果逐字节相同，`nudges.rs` 里有 golden 测试钉住。
> - **−55 护栏下沉**：输入护栏整体迁往 `GuardrailRegistry::screen_session_input`（`agent/guardrails.rs` −40、`agent.rs` −14、`think.rs` −1）。原实现只筛 tail 最新一条用户消息，而 `build_prompt` 每轮重放**整条日志** → 被脱敏的密钥从第 2 轮起又以明文上线。对**历史**消息的 `Block` 降级为 redaction：事件不可变且每轮重筛，对称 Block 会让此后每一轮都终止，**永久砖化会话**。
> - **+8 think.rs**：`max_output_tokens` 续跑循环只保留最后一段续写，长回答被持久化（并据以重建下轮 prompt）时从句子中间开始。现在各段先累积、在输出护栏**之前**拼接，护栏因此也能看到前半段。
> - **+11 prompt.rs**：`SessionEvent::SystemMessage` 落进 `_ => {}`，静默抹掉 split 子会话赖以重建的 `[Context Summary]` 头。（计划估 +6；rustfmt 把 match 臂展开成 8 行，另 3 行是点名 bug 的注释。**按真实成本记账**，不用估算值蒙混。）
> - **+2 act.rs**：并行准入用模型的**原始** args 算不相交证明，PASS 1 却执行护栏**改写后**的 args。PII 掩码会把两个不同路径塌成同一个 `[PHONE]` 占位符 → 被判定"不相交"的两个写变成对同一文件的并发截断写。现在只要有改写就串行化该批次。
>
> 上一轮的 5994 → 5997（`think.rs` 把账单 token 盖到 `AssistantMessage` 上）仍在代码里，只是被这次的删除盖过去了。**不靠删注释来凑行数**——那正是 `budget.rs` 要防的那种账目粉饰。

> ⚠️ **旧状态行「2026-07-04：TOTAL 5077 行 — 超 177 行」是错的，真实值约为 5923（超 ~1000）。** 错因不是笔误，是口径本身有歧义：`agent.rs` 在**生产 `impl` 中间**（第 215 行 / 全文 1060 行）有一个挂在 4 行测试专用取值器上的**缩进** `#[cfg(test)]`。按"第一个 `#[cfg(test)]`"的朴素读法，整个文件在第 214 行被截断，**846 行生产代码被静默排除在预算之外**——这正是 5077 与真实值之间的全部差额。旧 baseline（5267 → 5077）出自同一套读法，一并作废。
>
> 教训：**红线的状态行如果靠人手算、且规则有歧义，它迟早会说谎，而且是往好听的方向说谎。** 这就是 `budget.rs` 存在的理由。

> 唯一的自动检查曾是 `scripts/graph-audit.mjs` 的 `redline-r10` —— 它只数**文件数**（自红线写下之日起恒为 12，是唯一不会动的量），从不数行数，且未接入任何门（还需要一个生成的知识图谱产物才能跑）。

Task 8 曾把反应式压缩救援簇判为 BLOCKED（"依赖读写私有 harness 状态的 `&self` 方法"）。**2026-07-15 已下沉，那个判断是错的**——见上文第四轮。教训：**"依赖私有状态"不等于不可下沉。** 先数一数到底需要几个把手（这里是 5 个），再宣布 BLOCKED；把手能塞进一个小 trait，算法就能走。

**下沉去处（新增代码先看这里，而不是塞回 harness）**：
- Nudge / 护栏文案 → `src/thinker/nudges.rs`
- 压缩指令派发（`LoopDirective` → 具体动作）→ `src/context/compact/directive.rs`
- 反应式压缩救援（含 `RescueHost` / `RescueCx` 缝）→ `src/context/compact/rescue.rs`
- Trace → 线协议 DTO 转换 → `src/gateway/trace_protocol.rs`

## 加代码前必答 3 问

1. 这是脚手架还是认知？认知必须搬到 prompt。
2. 模型升级一档还需要它吗？不需要就删。
3. 现在有几个真实消费者？零个就撤回。

## 循环里的 5 个"不"

1. ❌ 不判断意图分类
2. ❌ 不做工具过滤 / 相关性评分
3. ❌ 不做完成度判断（除模型显式 stop）
4. ❌ 不做内容审查 / 安全打分
5. ❌ 不做错误恢复策略选择

任何"零现有消费者"的抽象立即撤回，绝不"为未来留口"。
