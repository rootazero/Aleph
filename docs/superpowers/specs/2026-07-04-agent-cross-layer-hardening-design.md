# Aleph Agent 跨层收口 + R10 归位 — 设计 spec

> 日期：2026-07-04 ｜ 状态：已获用户批准（brainstorming 全节认可）
> 前置侦察：① hermes-agent 深读（Masterclass 全文 + 代码，Top 8 可移植模式清单）；② Aleph 五轴跨层缝隙侦察（11 条发现，3 条承重结论已人工复验）。

## 1. 背景与目标

各子系统（loop/goal/subagent/note/steering/工具并发/streaming echo）在 2026-06~07 已逐个深度硬化，且 2026-07-03 对 codex/hermes/pi 的 gap 复扫结论为「参考仓无新增机制」。本次任务转向**从未整体审过的四层接缝**（Prompt→Context→Harness→Loop 之间），以「一条消息从入站到最终答案的完整旅程」为轴，收口跨层双源、归位 R10 越界、连线断掉的不变量。

**目标**：五个工作流，覆盖「错误修复 / 功能连线 / 架构归位重构 / 细节打磨」四类工作。
**验证优先纪律**：所有「待验证」发现先写复现测试或读码确认；确认是 bug 才修，验证不成立则在 FEATURE_LOCATOR 记录「已验证安全」后关闭。

**非目标**：
- A4 生命周期契约统一（TWELVE_FACTOR_AUDIT B §P1-1）——价值确认但另立 spec。
- 任何 fat-harness 机制（hermes 的 25 枚 FailoverReason 错误分类矩阵、意图分类、工具相关性评分）——违 R7/R10，明确排除。
- 新功能开发。

## 2. 工作流 1 — token 估算单源收口（错误修复 + 熵减）

**问题**（复验确认）：全库存在 5–6 套 token 估算实现、两个不同换算常量：
- `src/thinker/prompt_budget.rs:13` `CHARS_PER_TOKEN_ESTIMATE = 4`（系统提示侧，注释自称 "matching the chars/4 heuristic already used elsewhere"——但 elsewhere 实为 3.5，注释即漂移证据）
- `src/context/budget/pressure.rs:101` `DEFAULT_PROSE_RATIO = 3.5`，`estimate_tokens_aware` CJK-aware（CJK ~1.2、代码 ~2.5、英文 3.5 混合插值）——最完善实现
- `src/context/compact/compactor.rs` 私有 `estimate_tokens`
- `src/memory/session_compactor/context_window.rs:17`（被 harness think.rs 使用）与 `summary_source.rs:145` 各一套
- `src/thinker/cache.rs` 疑似内联 `/4`（侦察报告称 :110，人工 grep 未复现——实施时先核实，不存在则从清单剔除）

**后果**（5-1）：同一段文本在 prompt 层与 context 层被量成不同 token 数。CJK 密集部署下，系统提示被 /4 低估 token、按 10% 窗口设计意图裁剪后实际占比更高，**静默挤压历史预算**。

**方案**：
1. `pressure.rs::estimate_tokens_aware` 提升为全库唯一估算源（保持现址，其余模块 import；不新建模块——它已是 pub 且测试最全）。
2. 其余各处逐个改为调用或删除私有副本。`context_window.rs`/`summary_source.rs` 的 ratio 参数化签名可保留为兼容壳、内部转发单源。
3. `prompt_budget.rs` 的**字符级裁剪机制保留**（裁剪按字符精确本来正确），仅「token 预算→字符预算」换算改用单源 ratio。
4. 边界快照测试：固定样本（纯英文/纯 CJK/混合/代码）在收口前后各层估算值对照，行为变化点显式断言。

**行为变化（已获用户知情认可）**：CJK 密集部署下系统提示预算收紧——这是对 5-1 的修正而非回归。

## 3. 工作流 2 — R10 归位：harness 减重回预算内（架构归位重构）

**问题**（复验确认）：`src/harness/` 生产行 ~5267 > ~4900 红线（+7.5%）；think.rs 2065 生产行独占 39%；七条 grace-turn nudge prompt 文案（think.rs:49-116）+ agent.rs `SOFT_FAILURE_WARNING` 硬编码在笨循环里（认知住进 harness，违 R9 方向）；agent.rs 1721 行中 ~1500 行是内联测试（首个 `#[cfg(test)]` 在 :222），与同目录测试外置到 `src/harness/tests/` 的约定漂移；HARNESS_PHILOSOPHY.md:123-136 仍写「9 文件/~1500 行」并列出已删除的 `loop_callback.rs`。

**方案**（四步，互相独立）：
1. **nudge 文案下沉**：七条 grace nudge 常量 + `SOFT_FAILURE_WARNING` 移到新增聚焦小文件 `src/thinker/nudges.rs`（prompt 文案归 prompt 层，R9 归位），harness 仅 import。约 -70 生产行。
2. **压缩三态派发下沉**：think.rs:544-665 的 `CompactAndContinue`/`CompactToFit`/`SplitSession` match 派发 + fail-soft 回退编排，收进 `src/context/` 单一入口函数（纯机械分发、零语义，R10-safe），harness 一次调用。约 -100 行。**外科手术纪律：只搬不改逻辑，diff 逐行可对照。**
3. **agent.rs 内联测试外置**：~1500 行 `#[cfg(test)]` 迁至 `src/harness/tests/`，对齐既有约定（act/think/prompt/driver 的测试都在那里）。
4. **文档同步**：HARNESS_PHILOSOPHY.md、`src/harness/CLAUDE.md`、FEATURE_LOCATOR §3.1 按重构后现实更新，并**明确定义行数口径 = 生产行（剥 `#[cfg(test)]`）**，消除 8070 vs 5267 的账目歧义。

**验收**：重构后生产行 ≤ ~4900；`cargo check -p alephcore` 通过；既有 harness 回归测试全绿。

## 4. 工作流 3 — 事件流不变量连线（功能连线）

**问题**（部分复验）：
- seq 单调性：`impls.rs` 三个 emitter 结构各持独立 `seq_counter`（从 0 起）；`origin_fanout` 正确委托 `inner.next_seq()`（✓），但两个 fan-out 装饰器内存在多处 `seq: 0` 字面量的合成事件（origin_fanout.rs:144,178；team_fanout.rs:212,255,297,337）；`team_fanout` 在 inner=None 时用本地计数器。一个 run 的事件流经装饰器链后，消费者看到的 seq 是否全局单调**归属不清**。
- 降级不一致：`event_emitter/mod.rs` 9 处 + `execute.rs:254,1131` 的事件发射 `let _ =` 静默吞，而 §4.5 硬化已把 teams 层同类改为 `warn!`/`debug!`。骨架事件（RunAccepted/RunComplete）发射失败完全不可观测。

**方案**：
1. **验证→修复 seq**：先写测试/读码确认消费侧（Panel/WS forwarder/持久层）是否依赖 seq 排序、`seq: 0` 合成事件是否泄漏到这些消费者。确认破坏单调性则修：合成事件从 inner 取号；team_fanout fallback 计数器语义要么消除要么文档化其隔离边界。
2. **降级一致化**：骨架生命周期事件（RunAccepted/RunComplete 等）发射失败 → `warn!`；装饰性/UI 尽力事件 → `debug!`。对齐 teams 层既定策略，全部 11 处收口。

## 5. 工作流 4 — recall 瞬态与压缩边界（验证优先）

**问题**（待验证，5-2）：`deps.rs::recall_context` 瞬态尾部消息（「never persisted」契约）在 think.rs:471 被 push 进 `messages` 后才做 `peek_pressure`/`before_turn`/`compact`。正常路径 fresh_tail 保护尾部；但临界压力下 `CompactToFit`（think.rs:587）把 fresh_tail 压到极小时，recall 可能被卷进压缩窗口；`compactor.rs` 的 `store_cache` 可能把含瞬态内容的窗口摘要写进跨轮缓存——违背「永不持久」契约。

**方案**：写复现测试（临界压力 + recall 尾部 → 检查压缩窗口内容与 store_cache 落盘物）。确认则修：compact 前摘除 recall、压缩后重挂，或给 transient 消息打压缩免疫标记（两法实施时择简）。不成立则在 FEATURE_LOCATOR 记录「已验证安全」。

## 6. 工作流 5 — hermes 三项 delta 验证（有缺才补）

hermes Top 8 中 Aleph 已覆盖：迭代预算+grace（goal/loop 封顶 + grace turn×7）、错误回注自纠（ToolError 事件）、压缩（compactor 三策略+preventive band）、steering 不破坏 role 交替（Pi parity checkpoint）、窗口缩放工具输出预算（result store + budget.rs）、subagent 摘要回传（capture registry）。剩三项逐一验证：

| 项 | 验证点 | 若缺，补法（保持机械/nudge 形态） |
|---|---|---|
| tool-call 先持久化后执行 | act.rs 执行破坏性副作用前，assistant tool-call 块是否已落事件日志（对照 hermes conversation_loop.py:4506 flush-before-execute） | 调整持久化时序，零语义改动 |
| 压缩防抖动 | compactor 对连续低收益压缩（两次节省 <10%）是否会无限重压（对照 hermes should_compress 防抖） | 加节省率检查，两次低收益跳过后续压缩 |
| verify-on-stop 软门 | 现有 verifier 链（Halt/Veto）+ `verification/stop_hooks.rs` 是否覆盖「本轮改了代码文件却无验证证据 → 推一轮验证」；触发信号必须纯机械（文件变更集非空），且**必须是 nudge 非 gate**（模型可再次决定已验证够了） | synthetic nudge 不持久化，机械信号触发 |

**纪律**：已覆盖则记录等价实现锚点后关闭；补的话严守机械形态，任何需要语义判断的形态直接放弃（违 R7/R10）。

## 7. 验证与测试纪律

- 修 bug 类（工作流 1/3/4）：先写失败测试（RED→GREEN）。
- 重构类（工作流 2）：靠既有回归测试守护，只搬不改。
- cargo 节制（用户约定）：实施期用 `cargo check -p alephcore` 定点验证，不跑全量套件；执行阶段按 plan 的 checkpoint 跑定向测试。

## 8. 风险与回滚

- 工作流 1 是横切改动，影响 prompt 预算数值 → 换算点单一替换 + 边界快照测试控制爆炸半径。
- 工作流 2.2 动 think.rs 核心路径 → 外科手术、只搬不改、diff 可逐行对照。
- 五个工作流互相独立，可乱序实施、单独回滚。

## 9. 发现清单归档（供 plan 引用）

跨层侦察 11 条发现的完整清单与锚点：token 多源（中）、prompt/context 两把尺子（中）、harness 破预算（中）、nudge 文案在循环（中）、seq 归属不清（中）、recall 压缩边界（低-中）、生命周期事件静默吞（低-中）、agent.rs 内联测试（低）、压缩派发可下沉（低）、strategy 首写 best-effort（低，有意设计不动）、active_runs 内存态恢复语义不一（低，= A4 backlog 不在本 spec）。
