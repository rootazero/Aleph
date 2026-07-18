# Context 占用百分比 — 修复与重构设计

> 日期：2026-06-28 ｜ 范围：桌面（wide）Panel 的 Context 占用环（ContextGauge）
> 触发：Aleph 上下文占用环**恒显示 100%**；参考 opencode 的「按模型窗口 + 当前会话上下文」真实百分比。

## 1. 问题诊断（已逐跳查证）

### 1.1 现状数据链
```
FlowOutcome(累计 token)
  → build_run_summary (gateway/execution_engine/event_drain.rs:253)
  → RunSummary 上 wire
  → 面板 events.rs:371-389 读 summary.token_breakdown.input 当分子
  → context_gauge.rs 用面板内 context_window_for() 子串启发式当分母
  → 画 SVG 环
```

### 1.2 根因（真 bug：恒 100%）
分子 `token_breakdown.input` 是**整个 run 内每一次 LLM 调用的累计输入 token**
（`harness/agent.rs:107` 文档明写 "Cumulative ... across every LLM call"，
`accumulate_token_breakdown` 是 `+=` 累加）。多轮 agentic run 每一轮都把**不断增长
的上下文重新发送一遍**，输入被反复计入；跑 2~3 轮后累计输入即远超模型窗口，
`frac.clamp(0.0, 1.0)` 把它钉死在 1.0 → 永远 100%。

### 1.3 第二问题（分母近似 / 双脑割裂）
分母用面板内子串启发式 `context_gauge.rs::context_window_for()`，而 core 的
`providers/model_catalog/capabilities.rs` 每个模型都带权威 `context_window` 字段。
现状＝「分子来自 core（错的累计值）＋ 分母来自面板（近似猜测）」。

### 1.4 连带漏网 bug（reset 缺失）
`ChatState.context_usage`（`state.rs:303`）初始化为 `None`（:397），但
`clear()`（:821）/ `clear_session()`（:836）/ `restore_from()`（:882）**三处均未
reset**，而同类 ephemeral 字段 `plan` / `strip_open` 在三处都 reset。后果：切 tab /
新建 chat / 恢复会话时**上一会话的占用环残留**，直到新会话跑完一轮才覆盖。
（与已修过的 Todo 面板 `plan` 字段同源 seam bug。）

### 1.5 参考：opencode 的正解
`packages/app/src/components/session/session-context-metrics.ts:37-75`：取**最后一条**
assistant 消息的 `input+output+reasoning+cache.read+cache.write`，除以
`model.limit.context`（per-model 目录），`round(×100)`，不扣 output reserve。
关键＝用**最近一轮的真实占用快照**，而非累计。

## 2. 决策（已与用户确认）

| 决策点 | 选定 |
|--------|------|
| 架构边界 | **Core 权威 + 面板纯渲染**：分子分母都在 core 算好发上 wire，面板只画环 |
| 触发时机 | run_complete 触发（**不做**实时逐轮更新） |
| 端 | **仅桌面（wide）**；手机端不做 |
| 分子语义 | 最近一次调用的 `TokenUsage::prompt_tokens_total()` ＋ 该次 `output_tokens` |
| 分母语义 | 模型权威 `context_window`；未知模型 core 侧回退 `128_000` |
| output reserve | 不扣（与 opencode 一致） |

**为什么分子用 `prompt_tokens_total()` 而非照搬 opencode 的朴素求和**：core 已有的
`providers/adapter.rs:450 TokenUsage::prompt_tokens_total()` 是 provider-aware 的
「真实 prompt 大小」——正确处理 OpenAI 系「input 已含 cache」vs Anthropic「input 与
cache_read/cache_creation disjoint」两种约定（`cache_read > input` 判别），把 cache
折回 prompt。直接 `input+cache_read+cache_creation` 求和会在 OpenAI 系**重复计 cache**。
再加该次 `output_tokens` ＝「此刻窗口里装了多少」。

## 3. 改造后数据链
```
最近一次 LLM 调用 response.usage (TokenUsage)
  └─ harness 快照 last_turn_usage (last-writer-wins，与现有 cumulative 并存)
       └─ runner: context_tokens = last.prompt_tokens_total() + last.output_tokens
                  context_window = capabilities_for(model).context_window 或 128_000 保底
            └─ FlowOutcome { +context_tokens, +context_window }
                 └─ build_run_summary → RunSummary { +context_tokens, +context_window } [wire]
                      └─ 面板 events.rs 读两字段 → ChatState.context_usage
                           └─ ContextGauge 纯渲染 used/window（删本地启发式）
```
符合 R4（面板纯 I/O）/ R7（业务逻辑在 core）/ R10（不进 harness 认知层，仅加观测快照）。

## 4. 改动清单

### 4.1 Core 后端
- **`src/harness/agent.rs`**
  - 新增 `last_turn_usage: Mutex<Option<TokenUsage>>` 字段 + 构造初始化 `None`。
  - 把快照逻辑**折进现有** `accumulate_token_breakdown(&self, usage)`：`Some(u)` 时
    在累加之外同时 `*last_turn_usage.lock() = Some(u.clone())`。3 个调用点
    （think.rs:393 / 976 / 1731）**不改**，两计数器天然 lockstep。
  - 新增只读 accessor `last_turn_context_tokens(&self) -> u32`：
    `last.map(|u| u.prompt_tokens_total().saturating_add(u.output_tokens.into()))`
    饱和转 `u32`，无快照时 `0`。
- **`src/providers/model_catalog/`**（capabilities 旁）
  - 新增常量 `pub const CONSERVATIVE_CONTEXT_WINDOW: u32 = 128_000;` 并导出。
- **`src/orchestrator/harness_bridge/runner_impl.rs`**
  - 复用已有 model 解析（:535 那段 `let model: &str = match &spec.brain {...}`）查窗：
    `capabilities_for(model).map(|c| c.context_window).unwrap_or(CONSERVATIVE_CONTEXT_WINDOW)`。
  - stamp 进 `FlowOutcome`：`context_tokens = harness.last_turn_context_tokens()`，
    `context_window = <上面解析的窗口>`。

### 4.2 Wire（加性字段，不破协议）
- **`src/orchestrator/dispatch.rs` `FlowOutcome`**：`+ context_tokens: u32`、
  `+ context_window: u32`（`token_breakdown`/`total_tokens` 保留不动）。
- **`src/gateway/event_emitter/types.rs` `RunSummary`**：同增两字段（serde 加性，
  旧客户端忽略未知字段）。
- **`src/gateway/execution_engine/event_drain.rs` `build_run_summary`**：透传两字段。

### 4.3 Panel — 纯渲染 + 删启发式
- **`interfaces/webchat/src/platform/wide/views/chat/events.rs:371-389`**：
  分子改读 `summary.context_tokens`（弃 `token_breakdown.input`）；
  分母改读 `summary.context_window`（弃 `context_window_for(model)` 调用）。
  保留 `total_tokens`（本轮累计，tooltip 仍显示）。
  发布条件 `context_tokens > 0 || total_tokens > 0` 保持（无 LLM 调用则自隐）。
- **`interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs`**：
  **删除** `context_window_for()` 函数、其家族子串表、相关 `#[test]`
  （`known_families_resolve_expected_windows` / `unknown_model_falls_back_conservatively`）。
  环的 SVG / `gauge_color` / 百分比数学**不动**。
- **`interfaces/webchat/src/platform/wide/views/chat/state.rs`**：
  `ContextUsage` 三字段语义不变；更新 doc 注释（窗口来自 core，非面板启发式）。

### 4.4 连带 Bug 修复（reset 漏网）
- **`state.rs`**：`clear()` / `clear_session()` / `restore_from()` 三处各补
  `self.context_usage.set(None);`，紧邻现有 `self.plan.set(None);` 镜像放置。

## 5. 测试

### 5.1 Core
- `accumulate_token_breakdown` 在累加同时刷新 `last_turn_usage`（last-writer-wins）；
  `last_turn_context_tokens()` 对 Anthropic 形态（`cache_read > input`）与 OpenAI 形态
  （`input` 已含 cache）分别得到正确 prompt 大小 + output（复用既有
  `prompt_tokens_total` 双形态测试的输入）。
- runner 窗口解析：known 模型取 catalog `context_window`；unknown 模型回退 128_000。

### 5.2 Panel
- `events` 投影：给定带 `context_tokens`/`context_window` 的 `run_complete.summary`，
  `ChatState.context_usage` 得到对应 used/window/total。
- `state` reset：`clear` / `clear_session` / `restore_from` 后 `context_usage == None`
  （镜像 plan 字段的回归测试）。

## 6. 明确不做（Out of Scope）
- 实时逐轮更新（保持 run_complete 触发）。
- 手机端（phone）ContextGauge。
- provider 自配窗口覆盖（v1 用 catalog；自定义/本地模型走 128k 保底；后续可加
  provider-config `context_window` 优先级链）。

## 7. 验收标准
1. 多轮 agentic run 跑完后，桌面占用环显示 **< 100% 的真实百分比**（随上下文增长爬升），
   不再恒 100%。
2. 切换不同模型，分母随模型权威 `context_window` 变化（如 Claude 200k vs Gemini 1M）。
3. 切 tab / 新建 chat / 恢复会话后，旧占用环**立即消失**（不残留上一会话数值）。
4. tooltip 仍显示「上下文占用 X% · used / window tokens（本轮累计 Y）」，X 合理、
   used ≤ window。
5. 未知/自定义模型仍显示环（走 128k 保底），不空白、不 panic。
6. `cargo check -p alephcore --lib` 通过；`just wasm` 通过。
