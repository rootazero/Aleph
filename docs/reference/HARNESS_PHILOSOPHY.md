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

**笨循环** (Anthropic 原词 *dumb loop*) — Aleph 的 `src/harness/` 仅承载一件事：**Think → Act 的轮次调度**。它不分析意图、不评估完成度、不做工具过滤、不做安全决策。所有这些都是 LLM 在一次推理调用中自然完成的副产品。

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
2. ❌ 循环里**不**做工具过滤 / 工具相关性评分
3. ❌ 循环里**不**做完成度判断（除模型显式 stop）
4. ❌ 循环里**不**做内容审查 / 安全打分
5. ❌ 循环里**不**做错误恢复策略选择（错误分类是工具层职责）

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

### 4.1 重构成果（2026-04-24 → 2026-04-25）

经 P0–P7 共 8 阶段 dissolution，`src/harness/` 从 **16 文件 / 3712 行** 瘦身到 **9 文件 / ~1500 行**。

```
src/harness/                          # 笨循环编排核心 (Thin Core)
├── mod.rs               (28)         # 导出 Harness trait + AgentHarness
├── agent.rs            (~1000)       # Think→Act 驱动（实质循环）
├── deps.rs              (62)         # HarnessDeps: DI 容器
├── trait_def.rs         (99)         # Harness trait + HarnessError + TurnState
├── callback.rs         (144)         # HarnessCallback (编排级事件)
├── loop_callback.rs     (62)         # LoopCallback (turn 级钩子)
├── trace.rs            (265)         # 编排 trace 收集
├── trace_sink.rs        (19)         # Trace 输出抽象
└── chain_context.rs    (156)         # turn 间状态接力
```

只有这些。其它一切搬走。

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

- **R3 (核心轻量化)** — 1500 行的 harness 是这条红线的活样本
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
| 7 | **Harness 厚度** | **极薄** — 1500 行 9 文件 | **本文档的核心** |

---

## 8. 参考文献

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
