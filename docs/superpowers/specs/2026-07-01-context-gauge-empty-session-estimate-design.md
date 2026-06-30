# 空会话上下文百分比预估（预算预演兜底）

- **Date**: 2026-07-01
- **Scope**: Core (`alephcore`) 预估引擎 + 缓存 + RPC，外加 `interfaces/webchat` 面板 `≈` 呈现。仅现有仪表所在面（**wide / iPad 继承 wide**）；phone 若无此仪表则不在本期。
- **Status**: Design approved — pending implementation plan。
- **承接**: [[project-context-gauge-history-replay]]（档②已让"有真实占用记录"的会话切历史也显示）。本 spec 补最后一块：**从未跑过任何 LLM 轮次、因而没有真实占用记录的会话**，按"假设下一轮 prompt"做一次预算预演来预估百分比。
- **红线对照**: 纯确定性 token 计数、零 LLM 调用、不进 `src/harness/`、零新依赖、复用已有缝 → R3（核心轻量）、R4（Interface 纯 I/O）、R7（LLM 主权）、R10（薄 harness）、P6（KISS）、P7（优雅降级）全过。

## 1. 背景与问题

上下文占用仪表（`chat.context_usage: Option<ContextUsage>`）的值历来**只由一次 live `run_complete` 事件产生**——分子是 provider 上报的 `prompt_tokens_total`，必须真的发过一次 LLM 请求才有。

档②（[[project-context-gauge-history-replay]]）已把每轮真实占用持久化到 assistant 消息 metadata，并在面板 hydrate 末尾回投，使得**有真实记录**的历史会话切回来也能显示。

**仍未覆盖的缺口**：一个从未跑过任何轮次的会话（全新空会话；或修复前遗留、有历史但无占用 metadata 的老会话）`occupancy_from_history()` 返回 `None` → 仪表自隐 → 用户看到的还是"空会话不显示百分比"。

**目标**：让这类"无真实占用"的会话也显示一个**预估**百分比——通过对"下一轮将要发送的 prompt"做一次**本地预算预演**（system_prompt + 工具 schema + 已有历史的 token 估算），配上该会话模型的上下文窗口算出占比。

## 2. 关键技术地基（已查证，复用而非新造）

| 砖块 | 位置 | 作用 |
|------|------|------|
| `ContextPressure::compute(messages, system_prompt, tool_schema_tokens, budget, ratio)` | `src/context/budget/mod.rs:74` | 现成的"发请求前预估 prompt 占用"传感器，内容感知（CJK/code/prose 比率）。返回 `used_tokens`。**预估直接复用它，不另写估算器。** |
| `estimate_message_tokens_aware` / `estimate_tokens_aware` | `src/context/budget/pressure.rs:202/261` | 内容感知 token 估算（含图片计费）。 |
| `estimate_tool_schema_tokens(tools, ratio)` | `src/harness/agent/think.rs:259` | 工具 schema 的 token 成本。 |
| `build_system_prompt(agent_id, session_id, user_query, …)` | `src/orchestrator/harness_bridge/prompt_build.rs:140` | 组装系统提示。**当 `user_query` 为空时跳过最贵的 memory 召回**（`prompt_build.rs:181 if user_query.is_empty() { None }`）→ 预演只剩 skill 快照 + 身份文件读盘 + 层组装，**不触发向量检索**。 |
| `resolve_context_window_with_override(primary_context_window, model)` | `src/providers/model_catalog` | 不需要跑 run 就能拿到窗口（仪表分母）。 |

**核心洞察**：静态开销（system_prompt + 工具 schema 的 token）**只与 agent 有关、与具体会话无关** → 可按 agent 缓存复用；每会话只需再叠加该会话历史消息的本地估算（廉价）。

## 3. 已敲定的设计决策

| # | 决策 | 取值 |
|---|------|------|
| D1 | 触发范围 | **任何无真实占用的会话**：`occupancy_from_history()` 返回 `None` 即回退预估。统一覆盖全新空会话 + 修复前老会话；预估时把该会话已有历史也算进"下一轮 prompt"。 |
| D2 | 预估机制 | **按需预演 + 按 agent 缓存**：面板遇无占用会话时调 Core RPC；Core 用 `user_query=""` 一次性预演静态开销，按 agent 缓存复用；叠加该会话历史 token。 |
| D3 | 诚实度 | **加 `≈` 标记 + tooltip**：预估状态显示 `≈N%`、tooltip 注明"预估值，首次对话后转为实测"。与真实值视觉区分（首轮后数字会从预估跳到实测，可能 ±10~20%）。 |
| D4 | 传输 | **专用惰性 RPC** `chat.context_estimate`，而非折进 `chat.history`：保持 history 取数纯 I/O；预估是独立能力（也可服务未来"首条消息前预览开销"等调用方）；惰性=只在 `occupancy_from_history` 为 None 时调，有真实记录的会话零浪费。 |
| D5 | 缓存失效 | **刻意从简（KISS）**：model 变更必失效；工具/技能/身份变更若有现成 hook 就挂，否则接受进程生命周期内缓存（预估本是 `≈`，轻微陈旧无害）。**不引入 TTL**（避免时钟依赖）。"真实 run 后用实测开销刷新缓存"列为可选增强，本期 YAGNI 不做。 |

## 4. 架构与数据流

预估是一条与现有"真实占用"路径并存、互不干扰的**兜底路径**：

```
面板切到某会话 → chat.history(现状) → hydrate_session_history
  ├─ occupancy_from_history(history) 是 Some → 用真实值(is_estimate=false)   ← 档②现状，一行不改
  └─ 是 None → 调 chat.context_estimate{session_key}
                 └─ Core 预演 → {used, window} | null
                       ├─ Some → 仪表 ContextUsage{is_estimate=true} → 显示 ≈N%
                       └─ null → 仪表保持隐藏（model 解析不出，优雅降级）
真实 run 完成 → apply_context_gauge(现状) → 覆盖为真实值(is_estimate=false)
```

## 5. Core 预估引擎

新增 RPC：`chat.context_estimate { session_key: String } -> { used_tokens: u32, window_tokens: u32 } | null`，实现落在 gateway agent instance（持有 provider / skill_system / memory provider deps 的那层，即 `build_system_prompt` 的宿主）。

计算步骤：

1. 解析 session 的 **agent_id + model**（解析不出 → 返回 `null`）。
2. **静态开销** `(system_prompt_tokens, tool_schema_tokens)`：
   - 先查**按 agent 缓存**（§6）。
   - 未命中 → 一次性预演：`build_system_prompt(agent_id, session_id, user_query="", …)` 拿 system_prompt（**空 query 跳过 memory 召回**）；序列化该 agent 的工具集 → `estimate_tool_schema_tokens(tools, ratio)`。预演所需的 `provider / sandbox / workspace / channel_manifest / routing_text` 用 agent 已配置 provider（**不发任何请求**，仅供层的 provider_type 提示）+ 默认/None 兜底——这是本 spec 的中心实现缝，plan 阶段细化。
   - 写入缓存。
3. **历史消息 token**：对该会话已加载历史（空会话=空）逐条 `estimate_message_tokens_aware`。
4. `used = ContextPressure::compute(history, system_prompt, tool_schema_tokens, budget, ratio).used_tokens`（**复用现成传感器**；冷会话无校准 → 用默认内容感知比率，正是 `≈` 的来源）。
5. `window = resolve_context_window_with_override(primary_context_window, model)`。
6. 返回 `{ used, window }`。

**红线核对**：纯 token 计数、零 LLM 调用、不新增 `src/harness/` 文件、零新依赖。R7/R10/R3 过。

## 6. 按 agent 缓存

进程内 `Mutex<HashMap<CacheKey, StaticOverhead>>`：

```
CacheKey      = (agent_id, model_id)
StaticOverhead = { system_prompt_tokens: usize, tool_schema_tokens: usize, window: u32, ratio: f64 }
```

- 一个活跃 agent 在多会话间切换 → 首次算、之后全命中（近 100% 命中率）。
- 失效见 D5：model 变更必失效；其余从简。

## 7. 面板呈现（`≈` 标记）

- `ContextUsage`（`platform/wide/views/chat/state.rs:165`）加 `is_estimate: bool`。
- 真实路径 `apply_context_gauge`（`events.rs`）设 `is_estimate=false`。
- hydrate 预估路径：`hydrate_session_history` 末尾，若 `occupancy_from_history(&history)` 为 `None` → 调 `chat.context_estimate{session_key}`；`Some(resp)` → `chat.context_usage.set(ContextUsage{ used, window, total_tokens: used as u64, is_estimate: true })`；`null` → 不设（仪表隐藏）。
- `ContextGauge`（`context_gauge.rs`）渲染：`is_estimate` 时标签 `≈{pct}%`、tooltip 改为「预估上下文占用 {pct}% · {used} / {window} tokens（首次对话后转为实测）」；环色与几何逻辑不变。
- 新增 api DTO（`api/chat.rs`）：estimate 请求/响应类型。

## 8. 边界与降级

| 场景 | 行为 |
|------|------|
| 全新空会话（agent 从未跑过，冷启动/全新安装） | 按需预演 → 显示 `≈` 开销%（历史=0，纯静态开销）。 |
| 修复前老会话（有历史无占用 metadata） | 预演静态开销 + 历史 token → `≈`。 |
| 有真实占用记录的会话 | 面板根本不调 estimate RPC（`occupancy_from_history` 为 Some）→ 走真实值，零浪费。 |
| 预估后用户发消息 → 真实 run | `apply_context_gauge` 用实测覆盖（is_estimate=false），数字由 `≈` 跳到真实。 |
| model / provider 解析不出 | RPC 返 `null` → 仪表隐藏（与今日一致，不崩）。 |

## 9. 测试

- **Core**：
  - 空历史 = 纯静态开销；有历史 = 开销 + 历史 token。
  - 缓存命中（第二次同 agent 不重新预演）；model 变更失效。
  - model 解析不出 → 返回 `null`。
- **面板**：
  - `ContextUsage.is_estimate=true` 正确驱动 `≈` 前缀与 estimate tooltip；`false` 维持现状。
  - hydrate 中 `occupancy_from_history` 为 None → 触发 estimate 设值；为 Some → 不触发。
  - 真实 run 完成覆盖预估（is_estimate 翻 false）。

## 10. 加代码前必答 3 问（R10）

1. **脚手架还是认知？** 确定性 token 计数 = 脚手架。✓
2. **模型升级一档还需要它吗？** 需要——这是上下文窗口占用展示，模型无关。✓
3. **现在有几个真实消费者？** 1 个（用户明确要求空会话显示百分比）。✓

不新增 `src/harness/` 文件；预估引擎落 gateway/orchestrator 既有层，复用 `build_system_prompt` + `ContextPressure::compute`。
