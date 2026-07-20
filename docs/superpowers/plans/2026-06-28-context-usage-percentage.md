# Context 占用百分比 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让桌面 Panel 的上下文占用环显示**真实百分比**（按模型权威窗口 + 最近一轮真实占用），不再恒 100%。

**Architecture:** Core 拥有全部业务逻辑——harness 快照最近一轮 `TokenUsage`，runner 用 `prompt_tokens_total()+output` 算占用、用模型目录查权威 `context_window`，两个数随 `FlowOutcome → RunSummary` 发上 wire；Panel 退化为纯渲染并删除本地子串启发式。顺带修 `context_usage` 在会话切换时未 reset 的残留 bug。

**Tech Stack:** Rust (alephcore: harness / orchestrator / gateway / providers) + Leptos/WASM (interfaces/webchat)。

## Global Constraints

- **架构红线**：R4（Panel 纯 I/O，业务逻辑不进 interface）、R7（推理/判断在 core）、R10（不向 `src/harness/` 加认知层；本计划只加一个**观测快照**字段 + 只读 accessor，无决策逻辑）。
- **不引依赖**：全程零新 crate（复用既有 `TokenUsage::prompt_tokens_total`、`model_catalog::capabilities_for`）。
- **wire 加性**：`RunSummary` 已 `#[derive(Default)]` 且新字段带 `#[serde(default)]`；旧客户端忽略未知字段，新字段缺失时反序列化为 0。不破协议。
- **数值语义（spec §2 已定）**：分子 = 最近一次调用 `prompt_tokens_total() + output_tokens`；分母 = `capabilities_for(model).context_window`，未知模型回退 `CONSERVATIVE_CONTEXT_WINDOW = 128_000`；不扣 output reserve。
- **Build & Verification Policy（本仓库 cargo 节制纪律，覆盖 skill 的 per-task cargo 默认）**：**实现者不跑 cargo / just**。每个 task 先写测试代码再写实现（TDD 顺序保留在文件里），但**编译与测试执行由控制器批量验证**：core 改完后一次 `cargo check -p alephcore --lib`（必要时定向 `cargo test -p alephcore --lib <name>`）；Panel 改完后 `just wasm`。各 task 仍独立 commit。
- **commit 规范**：English，`<scope>: <description>`（如 `panel: render real context-window occupancy`）。归属已全局禁用，不加 Co-Authored-By。

---

### Task 1: `TokenUsage::context_occupancy_tokens()`（占用数学，纯函数）

把「此刻窗口里装了多少 token」收敛成 `TokenUsage` 上一个方法，复用既有 provider-aware 的 `prompt_tokens_total()`（已正确处理 OpenAI「input 含 cache」vs Anthropic「disjoint」）。这是分子的单一来源。

**Files:**
- Modify: `src/providers/adapter.rs`（在 `impl TokenUsage` 内，紧接 `prompt_tokens_total` 之后，约 :463 块尾）
- Test: `src/providers/adapter.rs`（`#[cfg(test)] mod tests`，约 :687 `prompt_tokens_total_no_cache` 之后）

**Interfaces:**
- Consumes: 既有 `TokenUsage::prompt_tokens_total(&self) -> u64`、字段 `output_tokens: u32`。
- Produces: `TokenUsage::context_occupancy_tokens(&self) -> u64`（Task 4 调用）。

- [ ] **Step 1: 写失败测试**（加到 `mod tests`）

```rust
    #[test]
    fn context_occupancy_folds_prompt_plus_output_anthropic_shape() {
        // Anthropic 形态：input 不含 cache，cache_read > input ⇒ disjoint。
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 40,
            cache_read_tokens: Some(900),
            cache_creation_tokens: Some(50),
            thinking_tokens: None,
            cost: None,
        };
        // prompt = 100 + 900 + 50 = 1050; + output 40 = 1090
        assert_eq!(u.context_occupancy_tokens(), 1090);
    }

    #[test]
    fn context_occupancy_no_double_count_openai_shape() {
        // OpenAI 形态：input 已含 cache_read（cache_read <= input）。
        let u = TokenUsage {
            input_tokens: 1000,
            output_tokens: 30,
            cache_read_tokens: Some(200),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        };
        // prompt = 1000（cache 已在内）; + output 30 = 1030
        assert_eq!(u.context_occupancy_tokens(), 1030);
    }
```

- [ ] **Step 2: 实现方法**（加到 `impl TokenUsage`，`prompt_tokens_total` 之后）

```rust
    /// Tokens occupying the model's context window as of this call: the full
    /// prompt actually sent ([`Self::prompt_tokens_total`]) plus the tokens
    /// generated on this call (which join the context for the next turn).
    /// Provider-aware via `prompt_tokens_total` — no cache double-count. This
    /// is the display-gauge numerator (current occupancy), distinct from the
    /// run-cumulative token counters.
    #[must_use]
    pub fn context_occupancy_tokens(&self) -> u64 {
        self.prompt_tokens_total()
            .saturating_add(u64::from(self.output_tokens))
    }
```

- [ ] **Step 3: 验证**（控制器批量）— 纳入 core `cargo check` + 定向 `cargo test -p alephcore --lib context_occupancy`，期望两测试 PASS。实现者**不在本步跑 cargo**。

- [ ] **Step 4: Commit**

```bash
git add src/providers/adapter.rs
git commit -m "providers: add TokenUsage::context_occupancy_tokens for gauge numerator"
```

---

### Task 2: `resolve_context_window()` + 保底常量（分母单一来源）

把「按模型 id 取权威窗口、未知回退」收敛成 model_catalog 的一个函数。这是分母的单一来源，取代 Panel 的子串启发式。

**Files:**
- Modify: `src/providers/model_catalog/capabilities.rs`（文件尾，`capabilities_for` 之后）
- Modify: `src/providers/model_catalog/mod.rs:27`（导出新符号）
- Test: `src/providers/model_catalog/capabilities.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: 既有 `capabilities_for(model: &str) -> Option<ModelCapabilities>`、`ModelCapabilities.context_window: u32`。
- Produces: `pub const CONSERVATIVE_CONTEXT_WINDOW: u32`、`pub fn resolve_context_window(model: &str) -> u32`（Task 5 调用）。

- [ ] **Step 1: 写失败测试**（加到 capabilities.rs 的 `mod tests`）

```rust
    #[test]
    fn resolve_context_window_uses_catalog_for_known_models() {
        // claude-opus-4-8 是目录里的精确前缀（context_window = 200_000）。
        assert_eq!(resolve_context_window("claude-opus-4-8"), 200_000);
        assert_ne!(
            resolve_context_window("claude-opus-4-8"),
            CONSERVATIVE_CONTEXT_WINDOW,
            "known model must not hit the fallback"
        );
    }

    #[test]
    fn resolve_context_window_falls_back_for_unknown_models() {
        assert_eq!(
            resolve_context_window("totally-unknown-model"),
            CONSERVATIVE_CONTEXT_WINDOW
        );
        assert_eq!(resolve_context_window(""), CONSERVATIVE_CONTEXT_WINDOW);
    }
```

- [ ] **Step 2: 实现**（capabilities.rs 文件尾，`capabilities_for` 之后）

```rust
/// Conservative context window (tokens) for models absent from the capability
/// catalogue — keeps the occupancy gauge meaningful for custom / local models
/// instead of failing. Matches the panel's prior unknown-model fallback so the
/// migration to core-authoritative windows is behaviour-preserving for them.
pub const CONSERVATIVE_CONTEXT_WINDOW: u32 = 128_000;

/// Authoritative context-window size for a model id, with a conservative
/// fallback. Display/occupancy consumers use this as the gauge denominator
/// (R7 — the window lookup is business logic and lives in core, not the panel).
#[must_use]
pub fn resolve_context_window(model: &str) -> u32 {
    capabilities_for(model)
        .map(|c| c.context_window)
        .unwrap_or(CONSERVATIVE_CONTEXT_WINDOW)
}
```

- [ ] **Step 3: 导出**（`src/providers/model_catalog/mod.rs:27`，把现有 `capabilities` re-export 行扩成）

```rust
pub use capabilities::{
    capabilities_for, resolve_context_window, ModelCapabilities, CONSERVATIVE_CONTEXT_WINDOW,
};
```

- [ ] **Step 4: 验证**（控制器批量）— core `cargo check` + 定向 `cargo test -p alephcore --lib resolve_context_window`，期望 PASS。

- [ ] **Step 5: Commit**

```bash
git add src/providers/model_catalog/capabilities.rs src/providers/model_catalog/mod.rs
git commit -m "model-catalog: add resolve_context_window with conservative fallback"
```

---

### Task 3: wire 加两字段 `context_tokens` / `context_window`（FlowOutcome + RunSummary 透传）

加上 wire 字段并打通透传，先用默认 0（真实值由 Task 5 在 runner 填）。`RunSummary` 已 `Default` 且 ancillary 字面量都用 `..Default::default()`，故只需改 `build_run_summary` 一处；`FlowOutcome` 部分字面量是全字段拼写，编译器会逐个报缺字段，按下方两行补 0。

**Files:**
- Modify: `src/orchestrator/dispatch.rs:64-104`（`FlowOutcome` 结构体加两字段）
- Modify: `src/gateway/event_emitter/types.rs:342`（`RunSummary` 加两字段）
- Modify: `src/gateway/execution_engine/event_drain.rs:322-339`（`build_run_summary` 透传）
- Modify: 所有 `FlowOutcome { ... }` 全字段字面量（编译器枚举，见下）
- Test: `src/gateway/execution_engine/event_drain.rs`（扩 `build_run_summary_carries_enriched_signals`，约 :692）

**Interfaces:**
- Consumes: 无（本 task 字段默认 0）。
- Produces: `FlowOutcome.context_tokens: u32`、`FlowOutcome.context_window: u32`；`RunSummary.context_tokens: u32`、`RunSummary.context_window: u32`（Task 5/Task 6 消费）。

- [ ] **Step 1: 给 `FlowOutcome` 加字段**（`src/orchestrator/dispatch.rs`，在 `estimated_cost` 字段之后、结构体闭合 `}` 之前）

```rust
    /// Tokens occupying the model's context window as of the run's most recent
    /// LLM call (last prompt sent + that call's output). Gauge numerator —
    /// distinct from cumulative [`total_tokens`]. `0` when no LLM call ran.
    pub context_tokens: u32,
    /// Authoritative context-window size (tokens) for this run's model, with a
    /// conservative fallback for unknown models. Gauge denominator. `0` only on
    /// default/test fixtures.
    pub context_window: u32,
```

- [ ] **Step 2: 给 `RunSummary` 加字段**（`src/gateway/event_emitter/types.rs`，在 `errors` 字段之前或之后均可，建议紧接 `token_breakdown` 之后）

```rust
    /// Current context-window occupancy (tokens) after the latest turn. Gauge
    /// numerator on the panel. `#[serde(default)]` so legacy payloads → 0.
    #[serde(default)]
    pub context_tokens: u32,
    /// Authoritative context-window size (tokens) for the run's model. Gauge
    /// denominator on the panel. `#[serde(default)]` so legacy payloads → 0.
    #[serde(default)]
    pub context_window: u32,
```

- [ ] **Step 3: `build_run_summary` 透传**（`src/gateway/execution_engine/event_drain.rs`，在末尾 `RunSummary { ... }` 字面量里，紧接 `token_breakdown,` 之后加）

```rust
        context_tokens: outcome.context_tokens,
        context_window: outcome.context_window,
```

- [ ] **Step 4: 补全所有 `FlowOutcome` 全字段字面量**

`FlowOutcome` 没有全局 `..Default::default()` 兜底，编译器会对每个**全字段拼写**的字面量报 `E0063 missing fields context_tokens, context_window`。在每个被报的字面量里（紧接 `estimated_cost: ...,` 之后）插入：

```rust
            context_tokens: 0,
            context_window: 0,
```

已知需要补的全字段站点（带 `..Default::default()` 的字面量**无需改**）：
- `src/orchestrator/summary_format.rs:353`（`outcome_with_tools` fixture）
- `src/gateway/execution_engine/event_drain.rs:653` / `:695` / `:773` / `:806` / `:845`
- `src/orchestrator/harness_bridge/tests.rs:84`（若为全字段；若已 `..Default::default()` 跳过）
- `src/orchestrator/tests/dispatch.rs:88` / `:374`（若为全字段；`:262` 已用 spread，跳过）
- `src/orchestrator/dispatch.rs:929`（已用 `..Default::default()`，跳过）

> 实操：以控制器 `cargo check` 的 `E0063` 列表为准 worklist，逐个补上面两行。`runner_impl.rs:547` **本 task 也先补 `: 0`**，真实值在 Task 5 替换。

- [ ] **Step 5: 扩 `build_run_summary` 测试**（`event_drain.rs` 的 `build_run_summary_carries_enriched_signals`：在该测试构造的 `FlowOutcome { ... }` 里把两新字段设为非 0，并在断言段加两行）

构造处（fixture 内，`estimated_cost` 之后）：
```rust
            context_tokens: 1234,
            context_window: 200_000,
```
断言处（`let summary = super::build_run_summary(&outcome);` 之后）：
```rust
        assert_eq!(summary.context_tokens, 1234);
        assert_eq!(summary.context_window, 200_000);
```

- [ ] **Step 6: 验证**（控制器批量）— core `cargo check`（确认全部 `E0063` 清零）+ 定向 `cargo test -p alephcore --lib build_run_summary_carries_enriched_signals`，期望 PASS。

- [ ] **Step 7: Commit**

```bash
git add src/orchestrator/dispatch.rs src/gateway/event_emitter/types.rs \
        src/gateway/execution_engine/event_drain.rs src/orchestrator/summary_format.rs \
        src/orchestrator/harness_bridge/tests.rs src/orchestrator/tests/dispatch.rs
git commit -m "orchestrator: carry context_tokens/context_window through FlowOutcome and RunSummary"
```

---

### Task 4: harness 快照最近一轮用量 + 只读 accessor

给 `AgentHarness` 加一个 last-writer-wins 的 `last_turn_usage` 快照（折进既有 `accumulate_token_breakdown`，3 个调用点不动），并暴露 `last_turn_context_tokens()`。R10-safe：纯观测，无决策。

**Files:**
- Modify: `src/harness/agent.rs`（字段 :126 后、构造 :185 后、`accumulate_token_breakdown` :353-369、accessor 紧接 `token_breakdown()` :284 后）
- Test: `src/harness/agent.rs`（`#[cfg(test)] mod tests`，若无则新建）

**Interfaces:**
- Consumes: `TokenUsage::context_occupancy_tokens`（Task 1）。
- Produces: `AgentHarness::last_turn_context_tokens(&self) -> u32`（Task 5 调用）。

- [ ] **Step 1: 加字段**（`src/harness/agent.rs`，紧接 `token_breakdown: Mutex<TokenBreakdown>,`（:126）之后）

```rust
    /// Most recent LLM call's raw usage (last-writer-wins snapshot), refreshed
    /// in [`AgentHarness::accumulate_token_breakdown`] alongside the cumulative
    /// [`token_breakdown`]. Unlike the cumulative counter this is *replaced*
    /// each call, so it reflects current context-window occupancy (last prompt
    /// sent + that call's output) rather than the run total — the cumulative
    /// input would blow past the window and peg the gauge at 100%. Read after
    /// the run via [`AgentHarness::last_turn_context_tokens`].
    last_turn_usage: Mutex<Option<crate::providers::adapter::TokenUsage>>,
```

- [ ] **Step 2: 构造初始化**（紧接 `token_breakdown: Mutex::new(TokenBreakdown::default()),`（:185）之后）

```rust
            last_turn_usage: Mutex::new(None),
```

- [ ] **Step 3: 折进 `accumulate_token_breakdown`**（在既有 `.accumulate(...)` 调用之后、`if let Some(u)` 块内追加快照写入）

```rust
            // Snapshot the latest call's usage (last-writer-wins) so the
            // orchestrator can report *current* context-window occupancy, not
            // the run-cumulative input. Kept in lockstep with the cumulative
            // fold above — both update from the same `usage` in one place.
            *self
                .last_turn_usage
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(u.clone());
```

- [ ] **Step 4: 加 accessor**（紧接 `token_breakdown()` 方法（约 :284-286）之后）

```rust
    /// Tokens occupying the model's context window as of the most recent LLM
    /// call: the last prompt actually sent plus that call's output. `0` when no
    /// LLM round-trip happened this run. Gauge numerator — distinct from the
    /// run-cumulative [`AgentHarness::total_tokens`].
    pub fn last_turn_context_tokens(&self) -> u32 {
        let occupancy = self
            .last_turn_usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map_or(0, crate::providers::adapter::TokenUsage::context_occupancy_tokens);
        u32::try_from(occupancy).unwrap_or(u32::MAX)
    }
```

- [ ] **Step 5: 写测试**（`src/harness/agent.rs` 的 `#[cfg(test)] mod tests`；若文件无 tests 模块则在文件尾新建 `#[cfg(test)] mod tests { use super::*; ... }`）

> 说明：直接构造 `AgentHarness` 需要 `HarnessDeps`，过重。本测试只验证 `accumulate_token_breakdown` 的快照语义——若仓库已有构造 harness 的测试 helper（搜 `AgentHarness::new` 的测试用例并复用），按下方断言；若无轻量构造路径，则**跳过 harness 实例测试**，依赖 Task 1 的 `context_occupancy_tokens` 单测 + 控制器 `cargo check` 覆盖编译正确性，并在 commit message 注明「accessor 由 Task 1 数学单测 + 编译保证，无独立 harness 实例测试」。

若有 helper（示意）：
```rust
    #[test]
    fn last_turn_context_tokens_reflects_latest_call_not_cumulative() {
        let harness = /* 复用现有测试 helper 构造最小 AgentHarness */;
        let first = Some(crate::providers::adapter::TokenUsage {
            input_tokens: 100, output_tokens: 10,
            cache_read_tokens: None, cache_creation_tokens: None,
            thinking_tokens: None, cost: None,
        });
        let second = Some(crate::providers::adapter::TokenUsage {
            input_tokens: 300, output_tokens: 20,
            cache_read_tokens: None, cache_creation_tokens: None,
            thinking_tokens: None, cost: None,
        });
        harness.accumulate_token_breakdown(&first);
        harness.accumulate_token_breakdown(&second);
        // 最近一轮 = 300 prompt + 20 output = 320（不是累计 430）。
        assert_eq!(harness.last_turn_context_tokens(), 320);
        // 初始无调用时为 0。
    }
```

- [ ] **Step 6: 验证**（控制器批量）— core `cargo check`；若写了 harness 实例测试则定向 `cargo test`，否则仅 check。

- [ ] **Step 7: Commit**

```bash
git add src/harness/agent.rs
git commit -m "harness: snapshot last-turn usage for current context-window occupancy"
```

---

### Task 5: runner 填真实 `context_tokens` / `context_window`

在 `FlowOutcome` 构造处用 Task 4 的 accessor 取占用、用 Task 2 的查窗取分母（复用已解析的 model id），替换 Task 3 暂填的 `0`。

**Files:**
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs:526-558`（`FlowOutcome` 构造前后）

**Interfaces:**
- Consumes: `harness.last_turn_context_tokens()`（Task 4）、`crate::providers::model_catalog::resolve_context_window`（Task 2）、`FlowOutcome.context_tokens/context_window`（Task 3）。
- Produces: 真实占用/窗口进 `FlowOutcome`。

- [ ] **Step 1: 解析窗口**（在 `let outcome = FlowOutcome {`（:547）**之前**，复用 `estimated_cost` 那段已解析的 model；若 `token_breakdown == default` 分支里的 `model` 作用域不可见，则在外层独立解析一次）

```rust
        // Gauge denominator: authoritative per-model context window (R7 — the
        // lookup is core's, not the panel's). Reuse the same model id the cost
        // estimate resolved; fall back to the provider name when the brain
        // carries no explicit model (mirrors the pricing path above).
        let gauge_model: &str = match &spec.brain {
            crate::orchestrator::flow_spec::BrainRef::Strict { model: Some(m), .. } => m.as_str(),
            _ => provider_name.as_str(),
        };
        let context_window = crate::providers::model_catalog::resolve_context_window(gauge_model);
        let context_tokens = harness.last_turn_context_tokens();
```

- [ ] **Step 2: 把真实值放进 `FlowOutcome`**（在 `FlowOutcome { ... }` 里，把 Task 3 暂填的 `context_tokens: 0, context_window: 0,` 替换为）

```rust
            context_tokens,
            context_window,
```

- [ ] **Step 3: 验证**（控制器批量）— core `cargo check`，确认 runner 编译通过、无未用变量告警。（runner 全链路行为由后续运行期 QA 验证，无独立单测。）

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator/harness_bridge/runner_impl.rs
git commit -m "orchestrator: stamp real context occupancy and window into FlowOutcome"
```

---

### Task 6: Panel events 读新字段（分子分母都来自 core）

Panel 投影改为读 `summary.context_tokens` / `summary.context_window`，不再用累计 `token_breakdown.input`、不再调本地启发式。

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs:366-390`
- Test: `interfaces/webchat/src/platform/wide/views/chat/events.rs`（`mod projection_tests`，与既有 `scratchpad_string_encoded_output_projects_plan_to_panel` 同模块）

**Interfaces:**
- Consumes: wire `summary.context_tokens` / `summary.context_window`（Task 3）。
- Produces: `ChatState.context_usage`（`ContextUsage { used_tokens, window_tokens, total_tokens }`）。

- [ ] **Step 1: 写失败测试**（`projection_tests` 模块内）

```rust
    #[test]
    fn run_complete_projects_core_context_occupancy_to_gauge() {
        let chat = ChatState::new(/* 复用本模块既有测试的构造方式 */);
        // 构造一个带 context_tokens/context_window 的 run_complete summary，
        // 走与既有投影测试相同的事件分发入口。
        // 期望：used=42_000、window=200_000、total=与 total_tokens 一致。
        // （具体事件构造镜像 scratchpad_string_encoded_output_projects_plan_to_panel）
        let usage = chat.context_usage.get_untracked().expect("gauge published");
        assert_eq!(usage.used_tokens, 42_000);
        assert_eq!(usage.window_tokens, 200_000);
    }
```

> 实操：照抄同模块既有投影测试的事件构造样板，把 `summary` 设为
> `{"context_tokens": 42000, "context_window": 200000, "total_tokens": 55000}`，
> 分发 `run_complete` 后断言 `context_usage`。

- [ ] **Step 2: 改投影逻辑**（`events.rs`，把现有 `if let Some(summary) = data.get("summary") { ... }` 块整体替换为）

```rust
                // Context gauge: core now ships the authoritative current
                // occupancy (`context_tokens`) and the per-model window
                // (`context_window`) — the panel is a pure renderer (R4). The
                // run-cumulative `total_tokens` stays as a complementary
                // tooltip figure (tokens billed this run), NOT the gauge ratio.
                if let Some(summary) = data.get("summary") {
                    let used = summary
                        .get("context_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as u32;
                    let window = summary
                        .get("context_window")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as u32;
                    let total = summary
                        .get("total_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    if used > 0 && window > 0 {
                        chat.context_usage.set(Some(ContextUsage {
                            used_tokens: used,
                            window_tokens: window,
                            total_tokens: total,
                        }));
                    }
                }
```

> 注意：发布条件改为 `used > 0 && window > 0`（窗口恒由 core 给，0 只会在无 LLM 调用/旧 payload 时出现——此时自隐，符合 spec 验收 #5）。`model_for_run` / `context_window_for` 调用在本块**删除**。

- [ ] **Step 3: 验证**（控制器批量）— `just wasm`（含本投影测试编译）。若 events 测试是普通 `cargo test` 目标，定向跑 `run_complete_projects_core_context_occupancy_to_gauge`。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/events.rs
git commit -m "panel: read core-authoritative context occupancy and window for gauge"
```

---

### Task 7: Panel 删除子串启发式 + 更新 doc

分母已由 core 给，删掉 `context_window_for()` 及其家族表和两个相关测试；`gauge_color` 与环渲染保留。

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs`（删 `context_window_for` :22-51 + 测试 `known_families_resolve_expected_windows`、`unknown_model_falls_back_conservatively`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs:139-155`（更新 `ContextUsage` doc 注释）

**Interfaces:**
- Consumes: 无（纯删除 + 注释）。
- Produces: 无。

- [ ] **Step 1: 删 `context_window_for` 函数**（context_gauge.rs:16-51 整段 `#[must_use] pub fn context_window_for(...) { ... }` 连同其文档注释删除）。`gauge_color`、`ContextGauge` 组件、SVG 数学**不动**。

- [ ] **Step 2: 删对应测试**（context_gauge.rs 的 `mod tests` 内删除 `known_families_resolve_expected_windows` 与 `unknown_model_falls_back_conservatively` 两个 `#[test]`；保留 `gauge_color_tracks_thresholds`）。

- [ ] **Step 3: 更新 `ContextUsage` doc**（state.rs，把「window 由面板 `context_window_for` 解析」那段注释改为）

```rust
/// Context-window occupancy snapshot for the composer gauge. All three figures
/// are computed by core and shipped on the `run_complete` summary — the panel
/// is a pure renderer (R4): `used_tokens` = current occupancy
/// (`prompt_tokens_total` + last output), `window_tokens` = the model's
/// authoritative context window, `total_tokens` = the run's cumulative total.
```

- [ ] **Step 4: 验证**（控制器批量）— `just wasm`，确认无 `context_window_for` 未解析引用残留（events.rs 已在 Task 6 去掉调用）。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs \
        interfaces/webchat/src/platform/wide/views/chat/state.rs
git commit -m "panel: drop client-side context-window heuristic, core owns the window now"
```

---

### Task 8: 修 `context_usage` 会话切换残留 bug（连线漏网）

`context_usage` 在 `clear` / `clear_session` / `restore_from` 未 reset（同类 `plan` / `strip_open` 已 reset），导致切 tab/新建/恢复会话残留上一会话的占用环。镜像 `plan.set(None)` 补齐。

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs`（`clear()` :832、`clear_session()` :848、`restore_from()` :900 三处，紧接各自 `self.plan.set(None);` 之后）
- Test: `interfaces/webchat/src/platform/wide/views/chat/state.rs`（tests 模块；若已有 plan/strip_open 的 reset 回归测试，扩之）

**Interfaces:**
- Consumes: 无。
- Produces: 无（行为修复）。

- [ ] **Step 1: 写失败测试**（state.rs tests 模块；镜像现有 reset 测试风格）

```rust
    #[test]
    fn context_usage_is_cleared_on_session_transitions() {
        let st = ChatState::new(/* 复用既有测试构造 */);
        let seed = || st.context_usage.set(Some(ContextUsage {
            used_tokens: 10_000, window_tokens: 200_000, total_tokens: 12_000,
        }));

        seed(); st.clear();
        assert!(st.context_usage.get_untracked().is_none(), "clear() must reset gauge");

        seed(); st.clear_session();
        assert!(st.context_usage.get_untracked().is_none(), "clear_session() must reset gauge");

        // restore_from 也应清空（恢复别的会话不应继承旧占用环）。
        seed();
        st.restore_from(/* 任一 SessionSnapshot fixture */);
        assert!(st.context_usage.get_untracked().is_none(), "restore_from() must reset gauge");
    }
```

- [ ] **Step 2: 三处补 reset**（state.rs，在 `clear()`、`clear_session()`、`restore_from()` 各自的 `self.plan.set(None);` 之后各加一行）

```rust
        self.context_usage.set(None);
```

- [ ] **Step 3: 验证**（控制器批量）— `just wasm`（含本回归测试编译）；定向跑 `context_usage_is_cleared_on_session_transitions`，期望 PASS。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state.rs
git commit -m "panel: reset context_usage on clear/clear_session/restore_from"
```

---

## 控制器收尾验证（全部 task 完成后一次性）

> 实现者全程不跑 cargo / just；以下由控制器在合并前批量执行（对齐本仓库 cargo 节制纪律）。

- [ ] **C1: core 编译 + 定向测试** — `cargo check -p alephcore --lib`（期望 0 error）；`cargo test -p alephcore --lib context_occupancy resolve_context_window build_run_summary_carries_enriched_signals last_turn_context_tokens`（期望全 PASS）。
- [ ] **C2: Panel 构建** — `just wasm`（期望成功；含 events 投影测试 + state reset 测试编译通过）。
- [ ] **C3: 运行期 QA**（对照 spec §7 验收标准 1-5）— 重编本地 core 服新 dist（见 [[feedback-ios-panel-test-via-full-macos-app]] 流程），多轮 agentic run 后确认占用环 **< 100% 真实爬升**；切模型看分母随权威窗口变；切 tab/新建 chat 确认旧环立即消失。

---

## Self-Review（计划 vs spec 覆盖）

- spec §1.2 根因（累计→爆窗）→ Task 4（快照最近一轮）+ Task 6（Panel 改读 context_tokens）。✅
- spec §1.3 分母双脑割裂 → Task 2（core 查窗）+ Task 5（runner 填）+ Task 6/7（Panel 删启发式）。✅
- spec §1.4 reset 漏网 bug → Task 8。✅
- spec §2 数值语义（prompt_tokens_total+output / 128k 保底 / 不扣 reserve）→ Task 1 + Task 2。✅
- spec §3 数据链（core 算好发 wire，Panel 纯渲染）→ Task 3 wire + Task 5 stamp + Task 6 render。✅
- spec §5 测试（core 双形态 + 窗口回退 + Panel 投影 + reset）→ Task 1/2/3/6/8 各带测试；Task 4 视 helper 可得性。✅
- spec §6 out-of-scope（实时/手机/provider 覆盖）→ 计划未触及，符合。✅
- spec §7 验收 → 控制器 C3 运行期 QA 对照。✅
- 类型一致性：`context_tokens`/`context_window`（u32）在 FlowOutcome/RunSummary 同名同型；`resolve_context_window`/`CONSERVATIVE_CONTEXT_WINDOW`/`context_occupancy_tokens`/`last_turn_context_tokens` 跨 task 命名一致。✅
- 占位符扫描：无 TBD/TODO；唯一带条件的 Task 4 harness 实例测试与 Task 6/8 的「复用既有测试构造」均给了明确 fallback 与样板出处，非占位。✅
