# Harness Philosophy — 薄 Harness 哲学 与 笨循环编排核心

> *"If you're not the model, you're the harness."* — Vivek Trivedy, LangChain
>
> *"Models get stronger → harness gets thinner."* — Anthropic 工程团队

本文档是 Aleph 架构的核心哲学之一，从 2026-04-24 启动的 `harness-dissolution` 重构（P0–P7，8 阶段）固化而来。它定义了 Aleph 在 Agent Harness 工程中的根本立场，与 [CLAUDE.md](../../CLAUDE.md) R3 / R8 / R10 / R11 共同构成不可逾越的红线。

---

## 1. 行业背景：Agent Harness 是什么

2026 年初，全球 AI 工程社区正式定义了 **Agent Harness**：包裹大语言模型的一整套操作系统级软件基础设施。它把一个只会输出文本、无状态、容易出错的裸 LLM，变成有目标、会用工具、能纠错、可持久运行的、靠谱的智能体。

行业共识把生产级 Harness 拆为 **12 个独立但互锁的模块**：

| # | 模块 | 中文 | 职责 |
|---|------|------|------|
| 1 | Orchestration Loop | 编排循环 | Think→Act 心跳，**笨循环**，本身不含推理 |
| 2 | Tools | 工具 | Schema 化、注册、参数校验、沙箱执行 |
| 3 | Memory | 记忆 | 短期会话 + 长期跨会话持久化 |
| 4 | Context Management | 上下文管理 | 压缩、屏蔽、JIT 检索、子代理委派 |
| 5 | Prompt Assembly | 提示词组装 | 系统 → 工具 → 记忆 → 历史 → 用户 的分层栈 |
| 6 | Tool Calling / Structured Output | 结构化工具调用 | Schema 约束输出，消除模糊解析 |
| 7 | State & Checkpointing | 状态与检查点 | 断点续跑、回溯、可调试 |
| 8 | Error Handling | 错误处理 | 瞬时 / 模型可恢复 / 用户可修复 / 意外 |
| 9 | Guardrails | 护栏 | 输入、输出、工具调用三重红线 |
| 10 | Verification & Feedback | 验证与反馈 | 规则 / 视觉 / LLM 当裁判 |
| 11 | Subagent Orchestration | 子代理编排 | Fork、交接、嵌套状态图 |
| 12 | Initialization & Environment | 初始化与生命周期 | Boot 装配 → 推理 → 工具 → 上下文更新 → 循环 |

**核心洞见**：玩具级 Demo 与生产级 Agent 的区别，从来不在模型本身，而在 Harness。LangChain 仅优化 Harness（不动模型），就把 TerminalBench 2.0 评测从 30 名外冲到第 5。

---

## 2. Aleph 的根本立场：薄 Harness 哲学 (Thin Harness Philosophy)

> **Aleph 选择 Anthropic 流派：信任模型，运行时极简。**

### 2.1 核心命题

- **运行时只做调度，不做推理** — 所有智能决策（意图理解、工具选择、安全评估、完成度判断）由 LLM 一次推理完成
- **模型越强，Harness 越薄** — Harness 是脚手架，不是认知层。模型升级，Harness 复杂度应当下降，而非上升
- **面向未来测试 (Future-Proof Test)** — 优秀的 Harness 必须满足：换更强的模型，性能自然提升，无需修改 Harness 代码
- **智慧在 Prompt 中** — 被移除的中间件智能不是丢弃，而是迁移到 system prompt 模板中

### 2.2 与冯·诺依曼架构的精确类比

| 计算机架构 | Agent Harness |
|------------|---------------|
| CPU | 裸 LLM |
| 临时内存 | 上下文窗口 |
| 硬盘 | 向量数据库 / 长期记忆 |
| 设备驱动 | 工具集成 |
| **操作系统** | **Agent Harness** |

Aleph 的 OS 哲学：内核极小，能力插件化，不替应用做推理决策。

### 2.3 三大流派的选择

| 流派 | 代表 | 哲学 | Aleph 立场 |
|------|------|------|------------|
| 薄 Harness | Anthropic Claude SDK | 信任模型，运行时极简，Git 管状态 | ✅ **采纳** |
| 代码优先 | OpenAI Agents SDK | Python 写工作流，开发者友好 | ❌ 不采纳（违反 R8） |
| 显式状态图 | LangGraph | 把 Harness 建模为可视化状态图 | ❌ 不采纳（违反 R3） |

---

## 3. 笨循环编排核心 (The Dumb Loop)

### 3.1 定义

**笨循环** (Anthropic 原词 *dumb loop*) — Aleph 的 `src/harness/` 仅承载一件事：**Think → Act 的轮次调度**。它不分析意图、不评估完成度、不按消息意图做工具过滤、不做安全决策。所有这些都是 LLM 在一次推理调用中自然完成的副产品。

### 3.2 唯一的循环骨架

```
loop {
    // Think
    let prompt    = assemble(history, tools, memory, system);
    let response  = provider.infer(prompt).await?;        // LLM 一次推理
    let turn      = parse(response);                       // 仅做协议解析

    // Act
    if turn.is_terminal() { break }                        // 模型自己说停才停
    for call in turn.tool_calls() {
        let result = tools.execute(call).await?;           // 沙箱执行
        history.append(result);                            // 状态追加
    }
}
```

**注意 5 个"不"**：

1. ❌ 循环里**不**判断意图分类
2. ❌ 循环里**不**按消息意图做工具过滤 / 工具相关性评分（渐进式工具披露例外，见下方注）
3. ❌ 循环里**不**做完成度判断（除模型显式 stop）
4. ❌ 循环里**不**做内容审查 / 安全打分
5. ❌ 循环里**不**做错误恢复策略选择（错误分类是工具层职责）

> **渐进式工具披露例外**: 上面第 2 不针对的是"循环按当前消息意图**动态**筛工具"。它**不**禁止**静态**的工具呈现分区 —— `src/tools/scoped/` 早已有 allowlist / 权限 Deny / 健康三道静态 `retain`（都在工具层、不在 `src/harness/`）；同理"core 常驻 + 全量目录 + `tool_search` 按需加载 schema"也是不看消息内容的静态分区、加载 100% 由模型发起（LLM 主权），属当前 agent 设计主流方案（Anthropic thin-harness / Claude Code 同款）。它落在工具呈现层，**不进 `src/harness/`**，笨循环依旧只做 Think→Act 调度。

### 3.3 笨循环的边界

笨循环允许做的"无脑"事：

- **协议解析** — 把 provider 的 chunk/stream 解析为 `UnifiedMessage` / `ContentBlock`
- **事件追加** — 把 turn 写入 `SessionEvent` 流（事件溯源）
- **回调触发** — 触发 `HarnessCallback` / `LoopCallback` 让 UI 流式渲染
- **取消信号** — 监听 `CancellationToken`，模型不停用户也能停
- **预算询问** — 调用 `context_budget.before_turn(...)`，但**不自己**决定何时压缩
- **停止钩子** — 调用 `stop_hooks.consult(...)`，但**不自己**决定接受/拒绝

> **判定标准**：循环里的每一行都应当通过这个测试 —
> *"如果换成更强的模型，这一行还需要吗？"* 不需要 → 删掉。

---

## 4. Aleph 的物理实现 (Physical Topology)

### 4.1 重构成果（2026-04-24 P0–P7 dissolution → 2026-07-15 R10 diet 第四轮）

经 P0–P7 共 8 阶段 dissolution，`src/harness/` 从 **16 文件 / 3712 行** 瘦身到 9 文件。2026-07-04 的跨层收口任务（Tasks 5–8）在此基础上把 `agent.rs` 按 Think/Act/Guardrails/Prompt 拆成 `agent/` 子目录四文件、把 nudge 文案与压缩派发下沉出 harness、把内联测试搬去 `src/harness/tests/`，当前物理边界固化为 **12 文件**（与 [src/harness/CLAUDE.md](../../src/harness/CLAUDE.md) 的硬边界一致）：

```
src/harness/                            # 笨循环编排核心 (Thin Core)
├── mod.rs                  (20)        # 导出 AgentHarness
├── agent.rs              (1037)        # AgentHarness struct + run() 顶层循环 + 救援 CAS 槽
├── deps.rs                (228)        # HarnessDeps: DI 容器
├── trait_def.rs           (156)        # HarnessError + TurnState
├── callback.rs             (86)        # HarnessCallback (编排级事件)
├── chain_context.rs        (79)        # turn 间状态接力
├── trace.rs               (244)        # 编排 trace 收集（线协议 DTO 已迁出）
├── trace_sink.rs           (23)        # Trace 输出抽象
└── agent/                              # Task 7 拆分（原 agent.rs 单文件 1713 行）
    ├── think.rs          (1509)        # Think：LLM 调用 + 守卫 + 验证（救援簇已迁出）
    ├── act.rs            (1120)        # Act：工具执行 + 资源域并行调度
    ├── guardrails.rs       (84)        # 输入/输出/工具调用护栏挂载
    └── prompt.rs          (451)        # 逐轮消息组装
                          ─────
                   TOTAL   5037 行
```

只有这些（+ `src/harness/tests/*` 的内联测试外置，不计入预算）。其它一切搬走。

**口径**：每个文件的行数取"文件开头到该文件内第一个**顶层（第 0 列）** `#[cfg(test)]` 之前"——测试代码不计入 12 文件预算，这也是 Task 7 把 `agent.rs` 内联测试搬到 `src/harness/tests/agent.rs` 的动机。

> ⚠️ **"顶层"二字是口径的全部**。本表上一版写着 `agent.rs (212)` / `TOTAL 5077`，那是**错的**：`agent.rs` 在生产 `impl` 中间挂着一个**缩进的** `#[cfg(test)]`（4 行测试专用取值器），朴素的"第一个 `#[cfg(test)]`"读法在那里截断，静默丢掉 846 行生产代码。当年"超红线 177 行"的结论因此是粉饰过的。**这些数字现在一律由 `src/harness/tests/budget.rs` 实测产出，不再手算**——那个测试就是本表的唯一来源。

**已不存在 `loop_callback.rs`**——本节此前记为第 9 个文件（`LoopCallback` turn 级钩子），该类型已在更早的重构中删除/合并进 `callback.rs`，此前的表格是未跟进的过时残留，2026-07-04 一并订正。

**honest 现值（2026-07-15）**：TOTAL **5043 行**——由 `src/harness/tests/budget.rs` 的棘轮实测（`CEILING = 5043`）。**旧的 ~4900 红线已退休**：它是一次手算口径事故（上方警告所述缩进 `#[cfg(test)]` 截断 `agent.rs`、静默漏计 846 行）的残值，从不是实测地板，循环不再背那个不存在的"137 行债"。红线现在就是**棘轮机制本身**——只减不增，增必答 3 问。

真实 baseline 从来不是 5077（见上方警告），而是 ~5997。四轮棘轮：5997 → 5863 → 5739 → 5593 → 5037 →（+6 `agent.rs`，见 `budget.rs` 批四注）**5043**。第四轮的两次搬迁（−556）搬走的是**依赖**而非仅仅行数：

- **−221 `trace.rs`**：六个 `From<LoopTrace*> for aleph_protocol::AgentTrace*` DTO 转换迁往 `src/gateway/trace_protocol.rs`。战利品不是行数——`rg aleph_protocol src/harness/` 现在返回空，**循环不再依赖 gateway 线协议**。
- **−335 `agent/think.rs`**：反应式压缩救援簇迁往 `src/context/compact/rescue.rs`，缝是 context 层定义、harness 实现的 `RescueHost` / `RescueCx`（P4 依赖倒置；`rg "crate::harness" src/context/` 返回空）。

**Task 8 当年判定此簇 BLOCKED（"依赖读写私有 harness 状态的 `&self` 方法，不是可参数化的 `self.deps.X` 字段"）——这个判断是错的。** 真正需要的把手只有 5 个（LLM 调用 / 救援 CAS 槽 / token 记账 / trace / 终止原因），装进一个 52 行适配器即可；CAS 槽本身留在 `agent.rs` 未动。**教训：「依赖私有状态」不等于不可下沉。先数把手，再宣布 BLOCKED。**

若要进一步瘦身，单文件气味线（~800 行）以上的候选（按价值排序，均未验证；注意这已是**可选优化**，不再是"欠 4900 的债"）：
1. `agent.rs`（1037 行）——四轮以来从未被审计过，是现在最大的未探区域。
2. `agent/act.rs`（1120 行）——第三轮已取走墙钟，剩余并行调度机件是否全属脚手架待查。
3. `agent/think.rs` 的 `fire_boundary_grace_turn` 及残余 nudge 管线——预期收益较小。

**下沉目的地（新增代码先看这里，而不是塞回 harness）**：
- Nudge / 护栏文案 → [`src/thinker/nudges.rs`](../../src/thinker/nudges.rs)（6 个 `GRACE_NUDGE_*` + `SOFT_FAILURE_WARNING` + `MUTATION_EVIDENCE_NUDGE`）——R9「智慧在 prompt 中」的具体落地：面向模型的文案是 prompt 内容，不是调度逻辑。
- 压缩指令派发 → [`src/context/compact/directive.rs`](../../src/context/compact/directive.rs)（`DirectiveOutcome` + `apply_budget_directive` + `compact_to_fit_and_note`）——`LoopDirective` 到具体压缩/session-split 动作的分发落在 Context 层，harness 只消费返回的 `DirectiveOutcome`。
- 反应式压缩救援 → [`src/context/compact/rescue.rs`](../../src/context/compact/rescue.rs)（`RescueHost` / `RescueCx` + `drain_context_overflow` + `try_reactive_compact_and_retry` + `reactive_fit_and_retry`）——**机制不是认知**：压不压缩完全由 `llm_retry::classify` 的 `CompactAndRetry` 裁决决定，harness 仍不挑恢复策略（R10 第 5 不），模型仍看得见错误并自愈（A2）。
- Trace → 线协议 DTO 转换 → [`src/gateway/trace_protocol.rs`](../../src/gateway/trace_protocol.rs)——为传输层序列化不是**循环的**脚手架，它属于 transport 自己。

### 4.2 12 模块归位映射

| # | 模块 | Aleph 中的家 | 备注 |
|---|------|--------------|------|
| 1 | Orchestration Loop | `src/harness/` | **薄核心**（本文档主题） |
| 2 | Tools | `src/tools/` + `src/builtin_tools/` | 吸收了原 harness 的 exec_context |
| 3 | Memory | `src/memory/` | 不动（最干净的一块） |
| 4 | Context Management | `src/context/{budget,compact}/` | 5 处合并到 1 处 |
| 5 | Prompt Assembly | `src/thinker/` | 历史名保留，不强行改名 |
| 6 | Tool Calling / Structured Output | `src/tools/calling/` + `src/providers/bridge.rs` | |
| 7 | State & Checkpointing | `src/session/` | `SessionEventStore` + `replay()` 已是事件溯源框架 |
| 8 | Error Handling | `HarnessError` + 各域 typed error | 跨模块，无单一容身处 |
| 9 | Guardrails | `src/{security,sandbox,approval,pii}/` | 4 域分立，不强搞 facade |
| 10 | Verification & Feedback | `src/verification/` | 新建，吸收 stop_hooks |
| 11 | Subagent Orchestration | `src/{agents,teams,orchestrator,group_chat}/` | 4 域职责正交，不合并 |
| 12 | Initialization & Environment | `src/init_unified/` + `src/bin/aleph-server/commands/start/` | 装配顺序见 [BOOT_ASSEMBLY.md](BOOT_ASSEMBLY.md) |

### 4.3 关键的"不归位" — YAGNI 撤回 (Retraction Pattern)

dissolution 过程中识别并删除了 **~5,200 行无消费者的死代码**，并撤回了多项原计划：

- ❌ `src/prompt/` (747 行) — 删除（消费链全死）
- ❌ `src/payload/` (~1,700 行) — 删除（test-only 闭环）
- ❌ `src/capability/` (~2,500 行) — 删除（test-only 闭环）
- ❌ `src/prompt_assembly/` (289 行) — 删除（P0 临时占位，无消费者）
- ❌ `src/permission/` — 删除（孤儿代码，零消费者）
- ❌ `VerifyStopHook` (194 行) — 删除（孤儿代码，零生产实例化）
- ❌ `src/compressor/` — 删除（已证实死代码）
- ❌ `PromptAssembler` / `Section` / `PromptLayer` trait — 撤回（与现有 PromptLayer/PromptPipeline 重复）
- ❌ `SubagentOrchestrator` trait + Fork/Handoff/Graph 模式 — 撤回（零消费者，违反 R3）
- ❌ `src/session/checkpoint/` Git 风格 trait — 撤回（事件溯源已覆盖）
- ❌ `src/runtime/boot.rs` — 撤回（实际 boot 在 `aleph-server/commands/start/` 6,194 行，无重写动机）

**通用模式**：每发现一个"零现有消费者"的抽象，就删除/撤回，绝不"为未来留口"。

---

## 5. 编写决策时的判定流程

向 `src/harness/` 添加任何代码前，回答这三个问题：

### Q1. 这是脚手架，还是认知？
- ✅ 脚手架（IO、调度、回调、追踪、错误传递）→ **可以**进入 harness
- ❌ 认知（分类、判断、过滤、评估）→ **必须**搬到 prompt 模板，让 LLM 做

### Q2. 模型升级一档，这段代码还需要吗？
- ✅ 仍然需要（如 provider 协议解析、事件追加）→ 留下
- ❌ 不再需要（如规则化意图识别、工具过滤评分）→ 删掉，迁移到 prompt

### Q3. 现在有几个真实消费者？
- ≥ 1 个生产消费者 → 可以保留
- 0 个，"为未来准备" → **撤回**，按 P1/P3/P4/P5/P6/P7 dissolution 同样的死代码处理原则删除

---

## 6. 与 CLAUDE.md 红线的对应

本哲学是 [CLAUDE.md](../../CLAUDE.md) 多条红线的具体落地：

- **R3 (核心轻量化)** — 12 文件 / 5077 行的 harness（§4.1 详述 honest 缺口）是这条红线的活样本，也是"红线不代表零偏差、偏差要留痕"的活样本
- **R8 (LLM 主权)** — 笨循环不替 LLM 做推理判断
- **R10 (智慧在 Prompt 中)** — 移除中间件后智能搬迁到 system prompt
- **R11 (薄 Harness, 笨循环)** — 本文档对应的最高级红线

---

## 7. 七大架构抉择 — Aleph 的答卷

行业总结的 7 大 Harness 抉择，Aleph 的立场：

| # | 抉择 | Aleph 选择 | 依据 |
|---|------|------------|------|
| 1 | 单代理 vs 多代理 | **单代理优先**，工具重叠 >10 或任务域分离时再拆 | R3 + 现实演进 |
| 2 | ReAct vs Plan-Execute | **ReAct (Think→Act)**，由 LLM 自适应 | R8（不替模型规划） |
| 3 | 上下文管理 | 时间清理 + 摘要 + 观察掩码 + 子代理委派 | `src/context/{budget,compact}/` |
| 4 | 验证循环 | 计算式 + LLM 当裁判，但**裁判逻辑写在 prompt** | `src/verification/` + R10 |
| 5 | 权限与安全 | 分层（security/sandbox/approval/pii），按部署调档 | R3 + R9 |
| 6 | 工具范围 | 暴露当前步骤所需最小工具集 | R8（避免污染推理） |
| 7 | **Harness 厚度** | **极薄** — 12 文件 / 5043 行（`budget.rs` 棘轮实测；旧"5077 / 超 177"是手算口径事故的残值，已作废，见 §4.1） | **本文档的核心** |

---

## 8. 提示词也是 Harness：模型越强越该精简 (Prune the Prompt)

> *"模型越聪明，需要越少方向、越少约束、越少例子。"* — Anthropic / Thariq

"薄 Harness"不止指 `src/harness/` 的 12 文件行数预算——**系统提示词本身也是 Harness**。同一条"模型越强，Harness 越薄"的规律，作用到 prompt 层就是 **prune-the-prompt**。

**反直觉的事实**：Anthropic 把 Claude Code 的 system prompt 砍掉了 **80%**。团队成员 Thariq 的解释是——模型越聪明，需要**越少方向、越少约束、越少例子**。那些示例正在**限制**它：模型会觉得你想要跟示例一样的东西；去掉示例它反而更自由。这意味着**每次新模型发布，你该做的第一件事是修剪上下文**——最新的模型需要更多空间去跑。一个直观对比：臃肿的 CLAUDE.md 从 12.8MB 塞满 95% 上下文窗口，瘦身到 2.1MB 只占 18% 后，模型专注度反而更好、结果更优。

**这不是 prompt 技巧迭代，是认知架构的范式转移**：

- 早期模型像新手，需要大量 few-shot 示例和详细约束来对齐。
- 当模型推理能力大幅跃升后，那些示例**从脚手架变成了枷锁**——模型倾向于模仿例子而非真正理解问题空间；再聪明的模型塞满低密度冗余信息也会**注意力稀释**。瘦身后模型把更多算力用于真正重要的 token，效果反而上升。这和人类信息过载导致决策退化高度同构。

**Agent 构建的根本转向**：

| | 旧范式 | 新范式 |
|---|---|---|
| 构成 | 重型 system prompt + 大量例子 + 复杂工作流定义 | 最小核心原则 + 清晰目标 + 强验证/自我纠错循环 + 模块化技能 |
| 模型角色 | 被遥控的**执行者** | 被赋能的**探索者** |

长时运行（社区有人做到连续运行十几个小时）的核心从来不是写更长的 prompt，而是**把验证和自我纠错能力建到架构里，而不是建到 prompt 里**。给 AI 一本厚厚的操作手册加几十个案例，它按图索骥、安全但平庸；给它一个清晰的**北极星** + 一枚**指南针** + **自我纠错雷达**，它可以自由探索、但必须能自己发现偏航并修正。手册越厚，AI 越容易学得像老手却没有老手的直觉；去掉手册，它才有机会发展出真正的问题空间直觉。

**行动信号**（对正在构建 agent 系统的人）：

1. 立刻做一次**上下文审计**——把主要 agent 的 system prompt 拿出来，标记哪些是遗留示例和过度约束，大胆砍掉一半以上观察效果。
2. 把高频重复任务抽成**独立技能文件按需加载**，而不是一直背着整本操作手册。
3. **强化验证层**——不要让 agent 只输出计划，让它先自我 critique 计划，再让你或 meta-agent review。
4. 每次**新模型发布，系统性修剪上下文**并重新测试核心工作流。
5. 把 **"room to run" 变成设计原则**——在编排里显式设计"探索→验证→修正"的闭环，而非指令执行的单向流。

模型越自由，方差越高：好处是创造性与问题解决能力提升，坏处是可能偏离你真正想要的方向。这意味着你的 **taste 和判断力不是可以弱化，而是要升级为更核心的 meta-layer**——模型越自由，你越需要成为高质量的**守门人**。继续用去年的 heavy-prompting 思维去堆今年的模型，只会越来越吃力。**模型越聪明，你就越应该少教它怎么做，多给它空间去发现怎么做。**

### 8.1 Aleph 的落地 (Application)

prune-the-prompt 是 R7（LLM 主权）/ R9（智慧在 Prompt）/ R10（薄 Harness）/ P6（KISS）在**提示词层**的自然延伸。2026-07-20 的**精简轮**把它作用到 [FEATURE_LOCATOR.md §1.1](FEATURE_LOCATOR.md) 的 40 层提示词流水线：

- 测得 Aleph 曾向 Claude **无差别注入 ~4,500 token 手写手册**（如何思考/跑循环/验证/收尾/说话/自我配速），对齐到 hermes 的 lean-Claude 基线（~1,100 token）。
- 删 5 份子代理 playbook + **VERDICT 输出模板笼子**、loop-mechanics 规则、持久化教条、few-shot 措辞；**保留**全部运行时事实、协议 token、安全铁律，以及用户明确要求的 D4 确认契约。
- 把"持久化/回退梯/2-strike"从 prompt prose **迁成运行时 error-hint 信号**（`fallback_registry` / `attempt_summary`）——正是"把自我纠错建进**架构**而非 prompt"的具体落地。
- 熵减 **−533 行**、层数 **40→39**、alephcore lib **13905 测试绿**。

**判定尺（加 prompt 字节前必答）**：这是模型**做不到的运行时事实**（时间/cwd/工具 schema/活跃目标/身份文件/安全上下文），还是我在**教强模型怎么思考**？前者=脚手架，保留；后者=枷锁，删或迁进架构。

---

## 9. 参考文献

- 行业原文：`/Volumes/TBU4/Workspace/Agent-Harness.md`（12 模块框架，中文）
- Anthropic Managed Agents 工程博客：https://www.anthropic.com/engineering/managed-agents
- Beren Millidge, *AI 的脚手架* (2023) — 把 Agent Harness 类比为冯·诺依曼架构
- Aleph 重构原始 spec：[2026-04-24-harness-dissolution-roadmap.md](../superpowers/specs/2026-04-24-harness-dissolution-roadmap.md)
- Think→Act 驱动设计：[2026-04-19-harness-think-act-design.md](../superpowers/specs/2026-04-19-harness-think-act-design.md)
- 相关红线：[CLAUDE.md](../../CLAUDE.md) §R3, R8, R10, R11
- 关联文档：
  - [AGENT_DESIGN_PHILOSOPHY.md](AGENT_DESIGN_PHILOSOPHY.md) — LLM 主权 + 双系统 + 记忆增强
  - [AGENT_LOOP_CONTEXT_BUDGET.md](AGENT_LOOP_CONTEXT_BUDGET.md) — 模块 4 的实现
  - [AGENT_LOOP_TOOL_EXECUTION.md](AGENT_LOOP_TOOL_EXECUTION.md) — 模块 2 + 6 的实现
  - [AGENT_LOOP_RECOVERY.md](AGENT_LOOP_RECOVERY.md) — 模块 8 的实现
  - [BOOT_ASSEMBLY.md](BOOT_ASSEMBLY.md) — 模块 12 的装配文档
  - [STATE_LAYER.md](STATE_LAYER.md) — 模块 7 的状态层文档
