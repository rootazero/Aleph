# 自动建计划 + 阶段性进度叙述 — 设计 (Design Spec)

> Date: 2026-06-28
> Topic: 让 LLM 自主决定是否启动任务列表，并在多步执行中做「预告+里程碑」叙述
> Status: Approved design, ready for writing-plans

## 1. 问题 (Problem)

两个用户报告的体验缺口，均来自同一类根因——**系统 prompt 没有教模型如何"主持"一个多步任务**：

1. **每次都要手敲触发词**：要启动任务列表面板，用户必须输入类似「用 scratchpad 计划工具把任务……」的提示词。模型不会自己判断"这是个多步任务、该建列表"。
2. **56 轮全程沉默**（更重要）：上次一道 8 步任务，agent 从头到尾无任何阶段性回复，主窗口一直显示"思考中"。即使完成一步进入下一步，也不吐一句过程描述。只干活不回话，用户毫无"任务在推进"的感知。

## 2. 根因 (Root Cause) — 已查证

两个问题都是 **prompt 缺指引**，不是代码缺能力：

- **想法 1**：`scratchpad` 工具本身有描述（`src/builtin_tools/scratchpad.rs:202`），但**系统 prompt 无任何一句教模型"何时该自主启动它"**。`ExecutionPlanLayer`（`src/thinker/layers/execution_plan.rs:69-77`）只在计划**已存在**时才提醒推进，无法触发"从无到有"地建计划。当前完全依赖用户手敲触发词。
- **想法 2**：harness **已 100% 实时流式推送所有中间助手文本**到面板（`think.rs:735` → `orchestrator/harness_bridge/callback.rs:41` → `FlowStreamEvent::Delta`），**无任何抑制**。56 轮沉默的真因是**模型那些轮根本没产可见文本**——它只发了工具调用。次要可能：叙述被埋进 thinking 块而非可见 text（实现期运行时 QA 验证）。

**结论**：流式管线已就绪，工具已就绪。唯一缺的是 prompt 指引。

## 3. 架构契合 (Constitutional Fit)

- **R7 / R9（LLM 主权 / 智慧在 Prompt）**：把"判断要不要建列表""何时开口"的智慧放进 system prompt，由 LLM 一次推理自然完成——而非在循环里写 `if 步数>N then 建列表` 这类确定性判断。
- **R10（薄 Harness / 笨循环）**：改动落在 `src/thinker/layers/`，**不碰 `src/harness/`，不占其 12 文件 / ~4900 行预算**。
- **A1 / R3（自有 context / 核心轻量）**：叙述指引明确要求简洁，控制 token 膨胀。

## 4. 方案 (Approach)

新增**一个稳定 prompt 层**，承载两段相关指引：(1) 何时自主建计划；(2) 交互式下如何做进度叙述。零 harness 改动、零确定性判断逻辑、零新依赖。

### 4.1 新层契约 (Layer Contract)

镜像现有 `execution_plan.rs` / `protocol_tokens.rs` 的写法（`impl PromptLayer`）。

| 项 | 取值 | 依据 |
|---|---|---|
| 文件 | `src/thinker/layers/multi_step_conduct.rs`（名称可在 plan 微调）+ `mod.rs` 注册一行 | 新建，单一职责 |
| `name()` | `"multi_step_conduct"` | 与模块名一致 |
| `priority()` | **805**（紧邻 `operational_guidelines`=800，其唯一空位；810=provider_guidance、820=session_budget 已占） | 已查证既有：tool_usage_grammar=550, protocol_tokens=700, heartbeat=710, operational_guidelines=800, provider_guidance=810, session_budget=820, citation_standards=900；动态带 1745+ |
| `stability()` | **`Stable`**（用 trait 默认，不显式 override，对标 `operational_guidelines` / `protocol_tokens`） | 内容恒定不随轮次变 → 进缓存稳定前缀，不引入 token 抖动（区别于 `execution_plan` 的 `Dynamic`） |
| `supports_mode()` | 仅 `Full`（Minimal 不带） | 对标 `protocol_tokens.rs:16` / `operational_guidelines.rs:16` |
| `paths()` | 与 `execution_plan` / `protocol_tokens` 同：Basic / Hydration / Soul / Context / Cached | 复用现有装配路径 |
| **门控 (gating)** | `inject()` 内：无 `ResolvedContext` → 空；**`active_capabilities` 含 `Capability::SilentReply`（Background/cron）→ 整层渲染空**；否则（交互式）注入第①段 + 第②段 | 与 `protocol_tokens.rs:38-44` **正好互补**：SilentReply 那条路得到静默 token + **完全不得到本层** → Background 提示词字节不变 |

**门控取舍（plan 自检定稿，最保守方案）**：**整层仅在交互式（`!SilentReply`）注入**，第①段与第②段共用同一道门 → Background/cron 渲染空、提示词**字节不变**、`ALEPH_SILENT_COMPLETE` 路径一字不动。

- *备选（未采纳）*：把第①段（何时建计划）也注入 Background，让后台多步任务也享受自动规划。未采纳因为：(a) 会改变 Background 提示词字节，与"Background 字节不变"（§6 #4）冲突；(b) 用户诉求集中在交互式面板体验，后台自动规划属 YAGNI。如确需后台规划，另立增量。

> 注：本层为交互式每轮常驻的稳定文本（约 100–140 token，可接受）。它**不能**像 `ExecutionPlanLayer` 那样按"计划已存在"门控——触发建计划正是其全部目的，必须在计划尚不存在时就出现。

### 4.2 注入内容 (Prompt Copy)

英文写入 prompt（与既有层一致）。语义要点如下，确切措辞在 plan 中定稿：

**第①段 — 何时建计划（交互式，Full mode；与第②段共用 `!SilentReply` 门）**
- 当任务确需**多个有序步骤** / **多个阶段** / **用户一次提了多件事** → 主动用 `scratchpad`：`initialize` 设 objective 与 plan，随后逐步 `start_item` / `complete_item` 推进。
- **反例必须明确写死（防过度触发刷屏）**：简单的一步问答、只需 1–2 个工具调用即可完成的任务，**不要**建计划——直接做即可。
- 灵活：中途发现任务比预想简单可弃用列表；发现更复杂可中途补建（对标 Codex "Use a plan when…" + Kimi "be flexible"）。

**第②段 — 如何叙述（仅交互式 / `!SilentReply`）**
- 在**可见回复**里（**不是** thinking 块）做叙述。
- **预告**：动作（或一批相关动作）前，一句极短预告，约 8–12 字（例：「接着写 API 层」）。对标 Codex preamble。
- **里程碑**：每完成一个计划步骤，一句话 recap（例：「✓ 数据模型完成」）。这是用户要求的最低线。
- 保持简洁，勿长篇，避免上下文膨胀。

## 5. 数据流 (Data Flow) — 不变

新层只往 system prompt 注入文本。运行期数据流**完全复用现有管线**，无新增 wiring：

```
模型产可见文本(被新指引引导) → CallbackSink.on_delta (think.rs:35)
  → BroadcastCallback.on_delta (callback.rs:41) → FlowStreamEvent::Delta → 面板气泡
模型自主调用 scratchpad → 现有 tool 执行 → 现有 events.rs 投影 → TodoPanel 面板
```

## 6. 成功标准 (Success Criteria)

1. 多步请求（**不带任何触发词**）→ 模型自动 `scratchpad initialize` → 任务面板出现。
2. 简单单步请求 → **不**建列表、**无**面板（不过度触发）。
3. 多步执行中 → 对话气泡出现**预告 + 里程碑**，不再 56 轮纯"思考中"。
4. Background / cron 行为**字节不变**；`ALEPH_SILENT_COMPLETE` 仍照常工作。
5. **零 harness 代码改动；零确定性判断逻辑；零新依赖。**

## 7. 测试 (Testing)

**单元测试**（镜像 `execution_plan.rs` / `protocol_tokens.rs` 的测试风格）：
- 交互式 paradigm（如 WebRich）→ 渲染出第①段 + 第②段（含 `scratchpad`、`## Narrate Your Progress`）。
- SilentReply paradigm（Background）→ **渲染空**（整层门控，Background 字节不变）。
- 无 `ResolvedContext` → 渲染空。
- Minimal mode → 渲染空（`supports_mode` 拒绝）。
- `priority()` / `name()` / `stability()` 断言（对标 `execution_plan` 的 `priority_is_1755` 等）。
- pipeline 集成：`test_default_layers_count` 43→44、`test_default_layers_sorted`（vec 升序，805 插在 800 与 810 之间）。

**权威门 = 运行时截图 QA**（实现期，controller 执行）：
- 重跑那道 8 步题（不带触发词）：肉眼确认①自动建面板、②对话气泡里逐步出现预告+里程碑。
- 另发一个单步问题：确认**不**建面板（验证不过度触发）。
- 抽查 Background/cron 一路未受影响（可选，或靠单元测试覆盖门控）。

## 8. 改动规模 (Footprint)

- 1 个新文件 `src/thinker/layers/multi_step_conduct.rs`（~100 行含测试）。
- `src/thinker/layers/mod.rs` 注册一行。
- 无其它文件改动。极小、极安全、可回滚。

## 9. 非目标 (Non-Goals)

- 不改 harness、不改流式管线、不改 `scratchpad` 工具、不改面板渲染（那些都已就绪）。
- 不引入任何确定性的"任务复杂度评分 / 步数阈值 / 完成度判断"代码（违 R7/R10）。
- 不改 Background/cron 的静默语义。
- 不在本次扩展到 OpenAI 兼容 API 等其它 surface（如需另立 spec）。
