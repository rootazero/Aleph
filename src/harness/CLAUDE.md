# src/harness/ — 薄 Harness 护栏 (R10 本地红线)

> 本文是根 `CLAUDE.md` R10 的本地强化，编辑本目录前必读。完整哲学见
> [HARNESS_PHILOSOPHY.md](../../docs/reference/HARNESS_PHILOSOPHY.md)。

## 硬边界：12 文件 / ~4900 行

- 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
- `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`

**新增文件须在 PR 描述说明为何无法装进现有 12 个文件之一。**

**口径**：行数按"文件开头到该文件内第一个**顶层（第 0 列）** `#[cfg(test)]` 之前"计，内联测试不计入预算（超预算就把测试搬去 `src/harness/tests/`，而不是当作行数豁免的借口）。

**「顶层」二字是本条最重要的部分**——见下方警告。**口径现在由测试执行**：`src/harness/tests/budget.rs`（跑在 `cargo test -p alephcore --lib` 里），同时守 12 文件与行数；出现第 13 个文件或行数上涨即 FAIL。**改这里的数字就得改那里的 `CEILING`，反之亦然。**

**当前测量（2026-07-14）：5739 行 — 超 ~4900 红线 839 行。**（由 `tests/budget.rs` 实测，非手算；`CEILING = 5739`）

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

Task 8 的下一步（把 `agent/think.rs` 的 `drain_context_overflow` + `try_reactive_compact_and_retry` + `reactive_fit_and_retry` 反应式压缩救援簇下沉）仍 BLOCKED——其依赖是读写私有 harness 状态的 `&self` 方法，不是可参数化的 `self.deps.X` 字段。缺口与候选下沉项详见 [HARNESS_PHILOSOPHY.md §4.1](../../docs/reference/HARNESS_PHILOSOPHY.md) 与 `.superpowers/sdd/task-8-report.md`。

**下沉去处（新增代码先看这里，而不是塞回 harness）**：
- Nudge / 护栏文案 → `src/thinker/nudges.rs`
- 压缩指令派发（`LoopDirective` → 具体动作）→ `src/context/compact/directive.rs`

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
