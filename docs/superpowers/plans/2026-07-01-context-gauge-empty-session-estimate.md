# 空会话上下文百分比预估 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让从未跑过 LLM 轮次、因而没有真实占用记录的会话（全新空会话 + 修复前老会话），通过对"下一轮 prompt"的本地预算预演显示一个 `≈N%` 的上下文占用预估。

**Architecture:** Core 在 `AgentHarnessRunner`（已持有全部 deps）上新增一个 `HarnessRunner` trait 方法 `estimate_context`，复用现有 `build_system_prompt(user_query="")`（空 query 跳过 memory 召回）+ `budget::pressure` 估算器算静态开销，按 `(agent_id, model)` 缓存；叠加该会话历史消息 token；配 `resolve_context_window_with_override` 给的窗口返回 `{used, window}`。网关新增惰性 RPC `chat.context_estimate`，面板仅在 `occupancy_from_history` 为 `None`（无真实占用）时调它，并以 `≈` 标记区分预估与实测。

**Tech Stack:** Rust (`alephcore`，tokio + serde + async-trait) · Leptos/WASM Panel (`aleph-panel`) · JSON-RPC over WS。

## Global Constraints

以下逐条 verbatim 自 spec，每个任务的要求隐式包含本节：

- **R7（LLM 主权）**：预估是纯确定性 token 计数，**零 LLM 调用**、无路由/意图判断。
- **R10（薄 harness）**：**不在 `src/harness/` 新增任何文件/字段**；预估引擎落 `src/orchestrator/harness_bridge/`，只**读** `harness::agent::prompt::build_prompt`（既有 pub(crate) 投影）。
- **R3（核心轻量）/ 禁用清单**：**零新依赖**；复用 `budget::pressure` 估算器，不引入新估算栈。
- **R4（Interface 纯 I/O）**：面板只调 RPC + 渲染；预估值由 Core 计算。
- **P6（KISS）/ D5**：缓存键含 model → model 变更自然 miss；**无 eviction、无 TTL**；工具/技能/身份变更内的轻微陈旧可接受。
- **cargo 纪律（极度节制）**：实现者**不跑全量 cargo**；用定向 filter（`cargo test -p alephcore --lib context_estimate`）；panel 走 `cargo test -p aleph-panel --lib <name>` + `just wasm` 编 dist。共享 warm target，勿清理。
- **提交规范**：English commit，`<scope>: <description>`；单分支 main 直接提交；归属全局禁用（不加 Co-Authored-By）。

## 文件结构

| 文件 | 职责 | 动作 |
|------|------|------|
| `src/orchestrator/harness_bridge/context_estimate.rs` | 纯估算数学 + 静态开销缓存 + `ContextEstimate` 类型 | **Create** |
| `src/orchestrator/harness_bridge/mod.rs` | 声明新模块 + `AgentHarnessRunner` 加缓存字段 | Modify |
| `src/orchestrator/dispatch.rs` | `HarnessRunner` trait 加 `estimate_context`（默认 `None`） | Modify |
| `src/orchestrator/harness_bridge/runner_impl.rs` | `AgentHarnessRunner` 实现 `estimate_context` | Modify |
| `src/bin/aleph-server/commands/start/orchestrator_init.rs:238` | 构造 `AgentHarnessRunner` 时初始化缓存字段 | Modify |
| `src/gateway/handlers/chat.rs` | `handle_context_estimate` handler + `EstimateParams` | Modify |
| `src/bin/aleph-server/commands/start/mod.rs:~1198` | 注册 `chat.context_estimate`（捕获 harness 句柄） | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/state.rs:165` | `ContextUsage` 加 `is_estimate: bool` | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/events.rs:248` | 真实路径设 `is_estimate: false` | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs` | `≈` 标记 + 预估 tooltip | Modify |
| `interfaces/webchat/src/api/chat.rs` | `ChatApi::context_estimate` + DTO | Modify |
| `interfaces/webchat/src/components/chat_sidebar.rs:102,222` | `is_estimate: false` + hydrate None→预估接线 | Modify |

---

## Task C1: 纯估算模块（types + 缓存 + compose，可单测）

**Files:**
- Create: `src/orchestrator/harness_bridge/context_estimate.rs`
- Modify: `src/orchestrator/harness_bridge/mod.rs`（加 `pub mod context_estimate;`）
- Test: 同文件 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `pub struct ContextEstimate { pub used_tokens: u32, pub window_tokens: u32 }`（`Debug, Clone, Copy, PartialEq, Eq`）
  - `pub struct OverheadCache`（`Default`），方法 `get(&self, agent_id: &str, model: &str) -> Option<usize>` / `insert(&self, agent_id: &str, model: &str, overhead: usize)`
  - `pub fn tool_schema_tokens(tools: &[crate::tool_metadata::ToolDefinition], ratio: f64) -> usize`
  - `pub fn compose_estimate(overhead_tokens: usize, history: &[crate::providers::message::UnifiedMessage], window: u32, ratio: f64) -> ContextEstimate`
  - `pub const ESTIMATE_RATIO: f64`

- [ ] **Step 1: 写失败测试**

在新文件 `src/orchestrator/harness_bridge/context_estimate.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    #[test]
    fn compose_empty_history_is_overhead_only() {
        let est = compose_estimate(10_000, &[], 200_000, ESTIMATE_RATIO);
        assert_eq!(est.used_tokens, 10_000);
        assert_eq!(est.window_tokens, 200_000);
    }

    #[test]
    fn compose_adds_history_message_tokens() {
        let history = vec![UnifiedMessage::user("hello there, this is a user turn")];
        let est = compose_estimate(10_000, &history, 200_000, ESTIMATE_RATIO);
        assert!(est.used_tokens > 10_000, "history tokens must add on top of overhead");
    }

    #[test]
    fn tool_schema_tokens_empty_is_zero() {
        assert_eq!(tool_schema_tokens(&[], ESTIMATE_RATIO), 0);
    }

    #[test]
    fn cache_round_trips_and_model_change_misses() {
        let cache = OverheadCache::default();
        assert_eq!(cache.get("agentA", "kimi"), None);
        cache.insert("agentA", "kimi", 12_345);
        assert_eq!(cache.get("agentA", "kimi"), Some(12_345));
        // Model change = different key = natural miss (D5).
        assert_eq!(cache.get("agentA", "claude"), None);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败（未实现）**

Run: `cargo test -p alephcore --lib context_estimate -- --list`
Expected: 编译失败（`context_estimate` 模块/符号不存在）。

- [ ] **Step 3: 写实现**

在 `src/orchestrator/harness_bridge/context_estimate.rs` 顶部（测试模块之前）：

```rust
//! Context-occupancy estimation for sessions that never ran an LLM turn.
//!
//! Pure token arithmetic + a per-(agent, model) static-overhead cache, so a
//! freshly-opened conversation can show a `≈N%` gauge before its first reply.
//! No LLM call, no decision — scaffolding only (R7/R10). Reuses the
//! `budget::pressure` estimators so the whole estimate is self-consistent.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::context::budget::pressure::{
    estimate_message_tokens_aware, estimate_tokens_aware, DEFAULT_PROSE_RATIO,
};
use crate::providers::message::UnifiedMessage;

/// Prose anchor for the estimate. CJK/code density overrides still apply inside
/// `estimate_tokens_aware`, so this only sets the natural-language baseline.
pub const ESTIMATE_RATIO: f64 = DEFAULT_PROSE_RATIO;

/// Estimated context occupancy for a session's *next* prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextEstimate {
    pub used_tokens: u32,
    pub window_tokens: u32,
}

/// Per-(agent_id, model_id) cache of the static prompt overhead
/// (system prompt + tool schemas) in tokens. Keyed so a model change is a
/// natural miss; no eviction (overhead drifts only on tool/skill/identity
/// edits, where a slightly stale `≈` estimate is acceptable — spec D5).
#[derive(Default)]
pub struct OverheadCache {
    inner: Mutex<HashMap<(String, String), usize>>,
}

impl OverheadCache {
    #[must_use]
    pub fn get(&self, agent_id: &str, model: &str) -> Option<usize> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(agent_id.to_string(), model.to_string()))
            .copied()
    }

    pub fn insert(&self, agent_id: &str, model: &str, overhead: usize) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((agent_id.to_string(), model.to_string()), overhead);
    }
}

/// Token cost of the tool schemas as sent on the wire (name + description +
/// JSON params), content-aware. Mirrors the budget sensor's per-tool charge but
/// reuses the `budget::pressure` estimator (kept here, not imported from
/// `harness`, to avoid widening the harness boundary — R10).
#[must_use]
pub fn tool_schema_tokens(tools: &[crate::tool_metadata::ToolDefinition], ratio: f64) -> usize {
    tools
        .iter()
        .map(|t| {
            estimate_tokens_aware(&t.name, ratio)
                + estimate_tokens_aware(&t.description, ratio)
                + estimate_tokens_aware(&t.parameters.to_string(), ratio)
        })
        .sum()
}

/// Compose the final estimate: static overhead + this session's history message
/// tokens, against the resolved window. Pure → unit-testable without a runner.
#[must_use]
pub fn compose_estimate(
    overhead_tokens: usize,
    history: &[UnifiedMessage],
    window: u32,
    ratio: f64,
) -> ContextEstimate {
    let msg_tokens: usize = history
        .iter()
        .map(|m| estimate_message_tokens_aware(m, ratio))
        .sum();
    let used = overhead_tokens.saturating_add(msg_tokens);
    ContextEstimate {
        used_tokens: u32::try_from(used).unwrap_or(u32::MAX),
        window_tokens: window,
    }
}
```

在 `src/orchestrator/harness_bridge/mod.rs` 的模块声明区（与其它 `mod` 同处）加：

```rust
pub mod context_estimate;
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib context_estimate`
Expected: 4 passed（`compose_empty_history_is_overhead_only` / `compose_adds_history_message_tokens` / `tool_schema_tokens_empty_is_zero` / `cache_round_trips_and_model_change_misses`）。

- [ ] **Step 5: 提交**

```bash
git add src/orchestrator/harness_bridge/context_estimate.rs src/orchestrator/harness_bridge/mod.rs
git commit -m "orchestrator: pure context-estimate math + per-agent overhead cache"
```

---

## Task C2: `estimate_context` trait 方法 + `AgentHarnessRunner` 实现 + 缓存字段

**Files:**
- Modify: `src/orchestrator/dispatch.rs`（trait 加默认方法）
- Modify: `src/orchestrator/harness_bridge/mod.rs`（`AgentHarnessRunner` 加缓存字段）
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`（实现方法）
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs:238`（初始化字段）

**Interfaces:**
- Consumes: `context_estimate::{ContextEstimate, OverheadCache, tool_schema_tokens, compose_estimate, ESTIMATE_RATIO}`（Task C1）
- Produces: `HarnessRunner::estimate_context(&self, session_key: &str) -> Option<ContextEstimate>`（trait 方法，默认 `None`；`AgentHarnessRunner` 给真实实现）

> **测试说明（verbatim 自 testing 规则的"说明为何"条款）**：`estimate_context` 的真实实现依赖完整 boot deps（provider / session_service / skill_system / build_system_prompt），无法在 `--lib` 单测里廉价构造 `AgentHarnessRunner`。其数学已由 C1 纯函数单测覆盖；本任务的验证门是 `cargo check -p alephcore --lib`（编译通过 = trait/类型/借用正确），端到端行为留运行时 QA。

- [ ] **Step 1: trait 加默认方法**

`src/orchestrator/dispatch.rs`，在 `pub trait HarnessRunner` 内（紧跟 `routing_store` 默认方法之后，约 `:513`）加：

```rust
    /// Estimate the context-window occupancy of this session's *next* prompt,
    /// for sessions that never ran an LLM turn (no persisted real occupancy).
    /// Deterministic token counting only — no LLM call (R7). Default `None`
    /// keeps test mocks / the simple engine gauge-less.
    async fn estimate_context(
        &self,
        _session_key: &str,
    ) -> Option<crate::orchestrator::harness_bridge::context_estimate::ContextEstimate> {
        None
    }
```

- [ ] **Step 2: `AgentHarnessRunner` 加缓存字段**

`src/orchestrator/harness_bridge/mod.rs`，在 `pub struct AgentHarnessRunner { ... }` 末尾字段处加：

```rust
    /// Per-(agent_id, model) static-overhead cache for the context-occupancy
    /// estimate. Populated on demand by `estimate_context`; never evicted
    /// (spec D5). Shared `Arc` so the gauge estimate is cheap on repeated
    /// history switches.
    pub estimate_overhead_cache:
        std::sync::Arc<crate::orchestrator::harness_bridge::context_estimate::OverheadCache>,
```

- [ ] **Step 3: 构造点初始化字段**

`src/bin/aleph-server/commands/start/orchestrator_init.rs:238`，在 `AgentHarnessRunner { ... }` 字面量里加一行（与其它字段并列）：

```rust
        estimate_overhead_cache: std::sync::Arc::new(
            crate::orchestrator::harness_bridge::context_estimate::OverheadCache::default(),
        ),
```

> 若 `grep -n "AgentHarnessRunner {" src/ -r` 显示其它构造点（如测试），同样补这一行。已知仅此一处生产构造点。

- [ ] **Step 4: 实现 `estimate_context`**

`src/orchestrator/harness_bridge/runner_impl.rs`，在 `impl HarnessRunner for AgentHarnessRunner` 块内（`run` 方法之后）加：

```rust
    async fn estimate_context(
        &self,
        session_key: &str,
    ) -> Option<crate::orchestrator::harness_bridge::context_estimate::ContextEstimate> {
        use crate::orchestrator::harness_bridge::context_estimate as est;

        // 1. Resolve agent_id + session id straight from the key (no store hit).
        let session_id = crate::routing::session_key::SessionKey::from_key_string(session_key)?;
        let agent_id = session_id.agent_id().to_string();

        // 2. Resolve the model the next turn would use: session pin → agent
        //    hint → none. Same precedence as `run` (Step 3); empty string falls
        //    back through `resolve_context_window_with_override` to the configured
        //    override or the conservative default.
        let model: String =
            crate::providers::session_model_handle::get_session_model(&session_id.to_key_string())
                .map(|p| p.model)
                .or_else(|| {
                    self.agent_registry
                        .get(&agent_id)
                        .and_then(|d| d.model_hint)
                })
                .unwrap_or_default();

        // 3. Window = exactly what `run` resolves (runner_impl.rs:611): the
        //    configured per-provider override first, else the model catalog.
        let window = crate::providers::model_catalog::resolve_context_window_with_override(
            self.primary_context_window,
            &model,
        );

        let ratio = est::ESTIMATE_RATIO;

        // 4. Static overhead (system prompt + tool schemas), cached per (agent, model).
        let overhead = if let Some(o) = self.estimate_overhead_cache.get(&agent_id, &model) {
            o
        } else {
            // user_query="" skips the expensive memory recall (prompt_build.rs:181)
            // while still assembling skills / identity / tool-description layers.
            let provider = self.default_provider.current();
            let sandbox: std::sync::Arc<dyn crate::sandbox::Sandbox> =
                std::sync::Arc::new(crate::sandbox::NoopSandbox);
            let system_prompt = self
                .build_system_prompt(
                    &agent_id,
                    &session_id,
                    "",
                    provider.as_ref(),
                    self.default_max_iterations,
                    None,
                    sandbox.as_ref(),
                    None,
                    None,
                )
                .await
                .map(|(s, _parts)| s)
                .unwrap_or_default();
            let sp_tokens =
                crate::context::budget::pressure::estimate_tokens_aware(&system_prompt, ratio);
            let tools = self.tool_service.metadata_schema();
            let tool_tokens = est::tool_schema_tokens(&tools, ratio);
            let o = sp_tokens + tool_tokens;
            self.estimate_overhead_cache.insert(&agent_id, &model, o);
            o
        };

        // 5. History messages this session already carries → the same
        //    UnifiedMessage projection the harness uses (think.rs:463).
        let events = self
            .session_service
            .get_events(&session_id, None, None)
            .await
            .unwrap_or_default();
        let history = crate::harness::agent::prompt::build_prompt(&events, 0);

        // 6. used = overhead + history tokens; against the resolved window.
        Some(est::compose_estimate(overhead, &history, window, ratio))
    }
```

> 若 `crate::harness::agent::prompt::build_prompt` 从 orchestrator 不可见（visibility 报错），在 `src/harness/agent.rs` 把 `pub(crate) mod prompt;` 确认为 `pub(crate)`（已是），并确认 `build_prompt` 为 `pub(crate)`（Explore 已证实在 `prompt.rs:43`）。**不要**新增 harness 文件/字段（R10）。

- [ ] **Step 5: 编译门**

Run: `cargo check -p alephcore --lib`
Expected: `Finished`，0 error。

- [ ] **Step 6: 提交**

```bash
git add src/orchestrator/dispatch.rs src/orchestrator/harness_bridge/mod.rs src/orchestrator/harness_bridge/runner_impl.rs src/bin/aleph-server/commands/start/orchestrator_init.rs
git commit -m "orchestrator: estimate_context on AgentHarnessRunner (dry-run overhead + history)"
```

---

## Task C3: 网关 RPC `chat.context_estimate` + boot 注册

**Files:**
- Modify: `src/gateway/handlers/chat.rs`（handler + params）
- Modify: `src/bin/aleph-server/commands/start/mod.rs:~1198`（注册）
- Test: `src/gateway/handlers/chat.rs` 同文件 `#[cfg(test)] mod tests`（params 解析）

**Interfaces:**
- Consumes: `crate::orchestrator::dispatch::HarnessRunner::estimate_context`（Task C2）
- Produces: `pub async fn handle_context_estimate(request: JsonRpcRequest, harness: Arc<dyn crate::orchestrator::dispatch::HarnessRunner>) -> JsonRpcResponse`；RPC `chat.context_estimate { session_key } -> { used_tokens, window_tokens } | null`

- [ ] **Step 1: 写失败测试（params 解析）**

`src/gateway/handlers/chat.rs` 的 `#[cfg(test)] mod tests` 内加：

```rust
    #[test]
    fn estimate_params_parses_session_key() {
        let v = serde_json::json!({ "session_key": "main:agentA" });
        let p: super::EstimateParams = serde_json::from_value(v).unwrap();
        assert_eq!(p.session_key, "main:agentA");
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p alephcore --lib estimate_params_parses_session_key`
Expected: 编译失败（`EstimateParams` 不存在）。

- [ ] **Step 3: 写 handler + params**

`src/gateway/handlers/chat.rs`，在文件靠近其它 `*Params` 处加：

```rust
/// Params for chat.context_estimate.
#[derive(Debug, serde::Deserialize)]
pub struct EstimateParams {
    pub session_key: String,
}
```

在文件末尾（其它 `handle_*` 之后）加：

```rust
/// Handle chat.context_estimate RPC.
///
/// Returns an estimated next-prompt occupancy for sessions that never ran an
/// LLM turn (so the gauge can show `≈N%`). `null` when core can't resolve the
/// session/model — the panel then keeps the gauge hidden (graceful, P7).
pub async fn handle_context_estimate(
    request: JsonRpcRequest,
    harness: Arc<dyn crate::orchestrator::dispatch::HarnessRunner>,
) -> JsonRpcResponse {
    let params: EstimateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match harness.estimate_context(&params.session_key).await {
        Some(est) => JsonRpcResponse::success(
            request.id,
            json!({
                "used_tokens": est.used_tokens,
                "window_tokens": est.window_tokens,
            }),
        ),
        None => JsonRpcResponse::success(request.id, serde_json::Value::Null),
    }
}
```

> `parse_params` / `JsonRpcResponse` / `json!` / `Arc` 已在 `chat.rs` 顶部 import（`handle_history` 用同一套）。`serde::Deserialize` derive 直接全路径写，无需新 import。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p alephcore --lib estimate_params_parses_session_key`
Expected: 1 passed。

- [ ] **Step 5: boot 注册**

`src/bin/aleph-server/commands/start/mod.rs`，在 orchestrator 装配完、`server.orchestrator = Some(orch);`（约 `:1203`）**之前**加：

```rust
    // Register chat.context_estimate now that the orchestrator (and its harness)
    // exist. Captures the harness handle so the gauge can show an estimated
    // occupancy for sessions that never ran an LLM turn. Registered here (not in
    // register_common_handlers) because that seam has no orchestrator handle.
    {
        let harness = orch.harness.clone();
        server.handlers_mut().register("chat.context_estimate", move |req| {
            let harness = harness.clone();
            async move {
                crate::gateway::handlers::chat::handle_context_estimate(req, harness).await
            }
        });
    }
```

> 确认此处 `orch` 是 `Arc<crate::orchestrator::Orchestrator>`、`server` 仍可变（设 `server.orchestrator` 前）。`register` 是 `HandlerRegistry::register`（Explore: `handlers/mod.rs:870`），闭包形如 `Fn(JsonRpcRequest) -> impl Future<Output=JsonRpcResponse>`，与 `common_handlers.rs` 现有注册同形。

- [ ] **Step 6: 编译门**

Run: `cargo check -p alephcore --lib && cargo check --bin aleph-server`
Expected: 两个 `Finished`，0 error。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/handlers/chat.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "gateway: chat.context_estimate RPC for empty-session gauge estimate"
```

---

## Task P1: 面板 `ContextUsage.is_estimate` + 仪表 `≈` 渲染

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs:165`（struct + 2 处测试字面量 `:1356,:1387`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs:248`（真实路径）
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:102`（occupancy_from_history）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs`（渲染）
- Test: `chat_sidebar.rs` 的 `gauge_tests`

**Interfaces:**
- Produces: `ContextUsage { used_tokens, window_tokens, total_tokens, is_estimate: bool }`（新增末字段）

- [ ] **Step 1: struct 加字段**

`state.rs:165`，`pub struct ContextUsage` 内末尾加：

```rust
    /// True when these figures are a pre-run estimate (no real LLM turn yet),
    /// so the gauge renders `≈N%` instead of `N%`.
    pub is_estimate: bool,
```

- [ ] **Step 2: 更新所有现存 `ContextUsage` 字面量为 `is_estimate: false`**

四处真实/持久/测试字面量补 `is_estimate: false,`：
- `state.rs:1356`（测试 `seed`）
- `state.rs:1387`（测试断言 `Some(ContextUsage { ... })`）
- `events.rs:248`（`apply_context_gauge` — 真实 run_complete 路径）
- `chat_sidebar.rs:102`（`occupancy_from_history` — 历史持久占用路径）

每处在 `total_tokens: ...,` 之后加一行 `is_estimate: false,`。

- [ ] **Step 3: 写失败测试（真实路径 is_estimate=false）**

`chat_sidebar.rs` 的 `#[cfg(test)] mod gauge_tests` 内加：

```rust
    #[test]
    fn real_occupancy_is_not_marked_estimate() {
        let h = vec![crate::api::chat::ChatMessage {
            role: "assistant".into(),
            content: "hi".into(),
            run_id: Some("r1".into()),
            timestamp: None,
            metadata: None,
            context_tokens: Some(10_000),
            context_window: Some(200_000),
            total_tokens: Some(12_000),
        }];
        let u = super::occupancy_from_history(&h).expect("real occupancy present");
        assert!(!u.is_estimate, "history-persisted occupancy is real, not an estimate");
    }
```

> 若 `gauge_tests` 已 import `ChatMessage`/字段路径不同，复用该模块现有构造法（mod 头已有 `use super::occupancy_from_history;`）。

- [ ] **Step 4: 渲染 `≈` + 预估 tooltip**

`context_gauge.rs`，把 `let title = format!(...)` 与 `<span>{format!("{pct}%")}</span>` 改为分支版（替换现有 title 计算与 span）：

```rust
                let (label, title) = if usage.is_estimate {
                    (
                        format!("≈{pct}%"),
                        format!(
                            "预估上下文占用 {pct}% · {} / {} tokens（首次对话后转为实测）",
                            usage.used_tokens, usage.window_tokens,
                        ),
                    )
                } else {
                    (
                        format!("{pct}%"),
                        format!(
                            "上下文占用 {pct}% · {} / {} tokens（本轮累计 {}）",
                            usage.used_tokens, usage.window_tokens, usage.total_tokens,
                        ),
                    )
                };
```

并把 span 改为：

```rust
                        <span class="text-[10px] tabular-nums">{label}</span>
```

（`title=title` 那行不变，复用新 `title`。）

- [ ] **Step 5: 运行 panel 单测，确认通过**

Run: `cargo test -p aleph-panel --lib gauge_tests`
Expected: 全 pass（含新 `real_occupancy_is_not_marked_estimate` + 既有 `picks_latest_assistant_occupancy` 等仍绿）。
另跑 `cargo test -p aleph-panel --lib context_usage_clears_on_reset_but_survives_tab_swap` 确认 tab-swap 测试随新字段仍绿。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state.rs interfaces/webchat/src/platform/wide/views/chat/events.rs interfaces/webchat/src/platform/wide/views/chat/context_gauge.rs interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: ContextUsage.is_estimate drives ≈ gauge label + estimate tooltip"
```

---

## Task P2: 面板 api `context_estimate` + hydrate None→预估接线

**Files:**
- Modify: `interfaces/webchat/src/api/chat.rs`（DTO + `ChatApi::context_estimate`）
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:222`（hydrate 末尾接线）
- Test: `api/chat.rs` 同文件 `#[cfg(test)] mod tests`（DTO serde）

**Interfaces:**
- Consumes: RPC `chat.context_estimate`（Task C3）；`ContextUsage.is_estimate`（Task P1）
- Produces: `ChatApi::context_estimate(state, session_key) -> Result<Option<ContextEstimateResponse>, String>`

- [ ] **Step 1: 写失败测试（DTO serde 往返）**

`interfaces/webchat/src/api/chat.rs` 末尾加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_response_round_trips() {
        let v = serde_json::json!({ "used_tokens": 12_000, "window_tokens": 200_000 });
        let r: ContextEstimateResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.used_tokens, 12_000);
        assert_eq!(r.window_tokens, 200_000);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p aleph-panel --lib estimate_response_round_trips`
Expected: 编译失败（`ContextEstimateResponse` 不存在）。

- [ ] **Step 3: 写 DTO + api 方法**

`api/chat.rs`，在 `ChatSendResponse` 附近加：

```rust
/// Response from chat.context_estimate — a pre-run occupancy estimate for a
/// session that never ran an LLM turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEstimateResponse {
    pub used_tokens: u32,
    pub window_tokens: u32,
}
```

在 `impl ChatApi` 内加：

```rust
    /// Estimate a session's next-prompt occupancy (sessions with no real
    /// occupancy recorded). `Ok(None)` when core returns null (unresolvable
    /// session/model) → caller keeps the gauge hidden.
    pub async fn context_estimate(
        state: &DashboardState,
        session_key: &str,
    ) -> Result<Option<ContextEstimateResponse>, String> {
        let params = serde_json::json!({ "session_key": session_key });
        let result = state.rpc_call("chat.context_estimate", params).await?;
        if result.is_null() {
            return Ok(None);
        }
        serde_json::from_value(result).map(Some).map_err(|e| e.to_string())
    }
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p aleph-panel --lib estimate_response_round_trips`
Expected: 1 passed。

- [ ] **Step 5: hydrate 接线**

`chat_sidebar.rs:222`，把：

```rust
            chat.context_usage.set(occupancy_from_history(&history));
```

替换为：

```rust
            match occupancy_from_history(&history) {
                Some(real) => chat.context_usage.set(Some(real)),
                None => {
                    // No real occupancy recorded → ask core for a next-prompt
                    // estimate so a freshly-opened conversation still shows a
                    // `≈N%` gauge. Null/err ⇒ leave it hidden.
                    let est = ChatApi::context_estimate(&dash, &key).await.ok().flatten();
                    chat.context_usage.set(est.map(|e| ContextUsage {
                        used_tokens: e.used_tokens,
                        window_tokens: e.window_tokens,
                        total_tokens: u64::from(e.used_tokens),
                        is_estimate: true,
                    }));
                }
            }
```

> `ChatApi`、`ContextUsage`、`dash`、`key` 均已在 `hydrate_session_history` 作用域内（`ChatApi::history` 同处已用；`ContextUsage` 由 P1 文件顶 import）。

- [ ] **Step 6: 编 wasm + dist 门**

Run: `just wasm`
Expected: `✓ panel dist OK: all N wasm references resolve`。

- [ ] **Step 7: 提交（源 + dist 分开）**

```bash
git add interfaces/webchat/src/api/chat.rs interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: fetch context estimate on history load when no real occupancy"
git add interfaces/webchat/dist
git commit -m "panel: rebuild dist for context-estimate gauge"
```

---

## Self-Review

**1. Spec coverage:**
- D1 触发范围（任何无真实占用）→ P2 Step 5（`occupancy_from_history` None 分支调预估）✓
- D2 按需预演 + 按 agent 缓存 → C1（OverheadCache）+ C2 Step 4（cache miss 走 build_system_prompt）✓
- D3 `≈` 标记 + tooltip → P1 Step 4 ✓
- D4 专用惰性 RPC `chat.context_estimate` → C3 ✓
- D5 缓存失效从简（键含 model、无 TTL）→ C1（key=(agent,model)，无 eviction）✓
- §5 引擎步骤（agent/model/window/overhead/history/used）→ C2 Step 4 全覆盖 ✓
- §8 边界（空会话/老会话/有真实占用不调/真实 run 覆盖/解析不出返 null）→ C2（unwrap_or_default 历史）/ P2（None 才调）/ C3（None→null）/ events.rs 真实路径覆盖 ✓
- §10 R10 三问 → 不新增 harness 文件、复用既有缝 ✓

**2. Placeholder scan:** 无 TBD/TODO；C2 的"无法单测"已 verbatim 说明理由并指明替代验证门（`cargo check`），非占位符。

**3. Type consistency:**
- `ContextEstimate{used_tokens:u32, window_tokens:u32}`（C1）↔ C2 返回 ↔ C3 `json!{used_tokens,window_tokens}` ↔ P2 `ContextEstimateResponse{used_tokens:u32,window_tokens:u32}` ↔ P1 `ContextUsage{used_tokens:u32,window_tokens:u32}` 字段名/类型一致 ✓
- `OverheadCache::get/insert(&str,&str[,usize])`（C1）↔ C2 `estimate_overhead_cache.get/insert` ✓
- `estimate_context(&self, &str) -> Option<ContextEstimate>`（C2 trait）↔ C3 `harness.estimate_context(&params.session_key)` ✓
- `is_estimate: bool`（P1）↔ P2 `is_estimate: true` / events.rs `false` ✓
