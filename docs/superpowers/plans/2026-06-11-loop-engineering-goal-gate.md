# Loop Engineering — Objective Gate for Autonomous Goal Loop · Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把红线合规的客观闸门（`ShellStopHook` 退出码）接进自主 goal 续跑循环，让模型自报的 `complete` 在循环终止前必须通过闸门确认（修复 Ralph Wiggum loop）。

**Architecture:** 全部改动落在 **loop 层**（`src/goal/` + `src/tasks/goal_pursuit.rs` + `src/gateway/execution_engine/` + boot wiring），`src/harness/` 零改动（R10）。maker(模型 claim)/checker(闸门 confirm) 的区分编码进类型态 `GateOutcome`。复用现有全局 `config.toml [[stop_hooks]]` 作为闸门，零新配置面。

**Tech Stack:** Rust / Tokio（闸门并发已内建于 `execute_stop_hooks`）/ serde / schemars。

> **⚠️ 项目资源治理约束**：本轮**不在本地跑 `cargo check` / `cargo test`**，直接提交（见 CLAUDE.md 强制约束）。测试**代码照写照提交**（CI 与后续会跑），只是不在本地执行 RED/GREEN 步骤。下文 "Run test" 步骤标注为 *DEFERRED*——写完测试与实现后直接 commit。

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `src/goal/types.rs` | `GateOutcome` 枚举 + `Goal.gate_outcome` 字段 + `with_gate_outcome` mutator | Modify |
| `src/tasks/goal_pursuit.rs` | 纯决策函数：`awaiting_gate` / `confirm_complete` / `reopen_after_gate_failure` / `gate_failure_prompt` | Modify |
| `src/verification/stop_hooks.rs` | 新增 `execute_stop_hooks_arc`（Arc 版闸门 runner，DRY） | Modify |
| `src/verification/stop_hook_verifier.rs` | 改用 `execute_stop_hooks_arc`，删内联 `ArcHook`（熵减） | Modify |
| `src/gateway/execution_engine/engine.rs` | `ContinuationDeps` 命名结构替换 2-tuple，含 `gate` 字段 | Modify |
| `src/gateway/execution_engine/execute.rs` | 续跑钩子加 `awaiting_gate` 分支 + 抽 `spawn_continuation_run` helper（熵减） | Modify |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | boot 时从 `app_config.stop_hooks` 构造闸门并注入 `ContinuationDeps` | Modify |

---

## Task 1: `GateOutcome` 类型态 + `Goal.gate_outcome` 字段

**Files:**
- Modify: `src/goal/types.rs`（`Goal` struct ~27、mutator 区 ~80、tests ~133）

- [ ] **Step 1: 在 `GoalStatus` 枚举（types.rs:13 附近）之后新增 `GateOutcome` 枚举**

在 `PursuitMode` 定义之后、`Goal` struct 之前插入：

```rust
/// Maker/checker 分离的类型态：模型调用 `goal(complete)` 是一个 *claim*；
/// 客观闸门（config.toml `[[stop_hooks]]` 退出码）通过才是 *confirmation*。
/// 只有自主续跑（`PursuitMode::Active`）的 goal 会被闸门守护；交互/被动
/// goal 的 complete 立即终止，不经闸门。`#[serde(default)]` → 旧持久化
/// 反序列化为 `Unchecked`。
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    /// 尚无闸门确认完成（默认；也是 Active/Paused/Blocked 的静息态）。
    #[default]
    Unchecked,
    /// 客观闸门确认了模型的 `Complete` 主张 → 循环真正终止。
    Passed,
}
```

- [ ] **Step 2: 给 `Goal` struct（types.rs:27）末尾字段后新增 `gate_outcome`**

在 `pub continuations_used: u32,` 之后加：

```rust
    /// 模型自报的 `Complete` 是否已被客观闸门确认（见 [`GateOutcome`]）。
    /// `#[serde(default)]` → 旧持久化读为 `Unchecked`。
    #[serde(default)]
    pub gate_outcome: GateOutcome,
```

- [ ] **Step 3: 在 `Goal::new`（types.rs:46 的 `Self { ... }`）初始化里加字段**

在 `continuations_used: 0,` 之后加：

```rust
            gate_outcome: GateOutcome::Unchecked,
```

- [ ] **Step 4: 新增 `with_gate_outcome` mutator（紧跟 `with_status` 之后）**

```rust
    /// Lifecycle transition（闸门确认/复位）——bump `updated_at_ms`，
    /// 与 `with_status`/`with_note` 同型。返回新 `Goal`（§不可变性）。
    #[must_use]
    pub fn with_gate_outcome(mut self, outcome: GateOutcome, now_ms: u64) -> Self {
        self.gate_outcome = outcome;
        self.updated_at_ms = now_ms;
        self
    }
```

- [ ] **Step 5: 写测试（types.rs `#[cfg(test)] mod tests`）**

在 `mod tests` 内追加：

```rust
    #[test]
    fn new_goal_gate_outcome_is_unchecked() {
        assert_eq!(sample().gate_outcome, GateOutcome::Unchecked);
    }

    #[test]
    fn with_gate_outcome_returns_new_goal_and_bumps_updated_at() {
        let g = sample();
        let after = g.clone().with_gate_outcome(GateOutcome::Passed, 9_000);
        assert_eq!(after.gate_outcome, GateOutcome::Passed);
        assert_eq!(after.updated_at_ms, 9_000);
        assert_eq!(g.gate_outcome, GateOutcome::Unchecked, "original unchanged");
        // 其它字段不受影响
        assert_eq!(after.status, g.status);
        assert_eq!(after.objective, g.objective);
    }

    #[test]
    fn old_payload_without_gate_outcome_deserializes_unchecked() {
        // 模拟本字段引入前持久化的 JSON（无 gate_outcome 键）。
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(g.gate_outcome, GateOutcome::Unchecked);
    }
```

- [ ] **Step 6: Run test — *DEFERRED*（不本地跑；测试已写入待 CI）**

- [ ] **Step 7: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-loop-gate
git add src/goal/types.rs
git commit -m "feat(goal): add GateOutcome type-state for maker/checker completion split"
```

---

## Task 2: `goal_pursuit.rs` 纯决策函数

**Files:**
- Modify: `src/tasks/goal_pursuit.rs`（imports 行 14、helper 区 ~86-106、tests ~110）

- [ ] **Step 1: 扩展 import（goal_pursuit.rs:14）**

把：

```rust
use crate::goal::{Goal, GoalStatus, PursuitMode};
```

改为：

```rust
use crate::goal::{GateOutcome, Goal, GoalStatus, PursuitMode};
```

> 若 `GateOutcome` 未在 `crate::goal` 顶层 re-export，先在 `src/goal/mod.rs` 的
> `pub use types::{...}` 行把 `GateOutcome` 加进去：
> `pub use types::{GateOutcome, Goal, GoalStatus, PursuitMode};`

- [ ] **Step 2: 在 `cap_reached_note`（goal_pursuit.rs:97）之后追加四个纯函数**

```rust
/// 模型在 `Active` 续跑下自报 `Complete`，但客观闸门尚未确认。
/// 调用方据此在续跑钩子里跑闸门。被动/交互 goal（非 Active 续跑）永远
/// 返回 false——它们的 complete 立即终止，不经闸门。
#[must_use]
pub fn awaiting_gate(goal: &Goal, gate_configured: bool) -> bool {
    gate_configured
        && matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Complete
        && goal.gate_outcome == GateOutcome::Unchecked
}

/// 闸门通过：完成被确认（`gate_outcome = Passed`），循环终止。
#[must_use]
pub fn confirm_complete(goal: &Goal, now_ms: u64) -> Goal {
    goal.clone().with_gate_outcome(GateOutcome::Passed, now_ms)
}

/// 闸门否决（Ralph Wiggum 营救）：把误报完成的 goal 退回 `Active` 并把
/// 闸门失败原因写入 `note`，让下一次续跑能据此行动。若迭代上限已耗尽，
/// 退回 Active 会立刻再次 exhaust——直接转 `Blocked`（复用 `cap_reached_note`，
/// 不复制 Blocked 逻辑）。无论哪条路径都把 `gate_outcome` 复位 `Unchecked`，
/// 保证下一次 complete 主张会被重新 gate。
#[must_use]
pub fn reopen_after_gate_failure(goal: &Goal, reason: &str, now_ms: u64) -> Goal {
    let cap_spent = match goal.pursuit {
        PursuitMode::Active { max_iterations } => goal.continuations_used >= max_iterations,
        PursuitMode::Passive => true,
    };
    if cap_spent {
        let note = cap_reached_note(goal);
        goal.clone()
            .with_status(GoalStatus::Blocked, now_ms)
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
    } else {
        let trimmed: String = reason.chars().take(300).collect();
        let note = format!("Objective gate vetoed completion: {trimmed}");
        goal.clone()
            .with_status(GoalStatus::Active, now_ms)
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
    }
}

/// 闸门失败后的续跑 prompt——把客观失败信号注入下一轮（R9 智慧在 prompt）。
#[must_use]
pub fn gate_failure_prompt(goal: &Goal, reason: &str) -> String {
    let trimmed: String = reason.chars().take(600).collect();
    format!(
        "[Your standing goal is NOT done — the objective gate rejected your \
         completion claim]\nGoal: {}\n\nThe automated gate (tests / build / \
         lint) failed with:\n{trimmed}\n\nThis is an objective signal, not an \
         opinion. Fix what the gate flagged, then call goal(action='update', \
         status='complete') again only when the work truly passes. If you \
         cannot resolve it, call goal(action='update', status='blocked') with \
         a note describing what remains.",
        goal.objective,
    )
}
```

- [ ] **Step 3: 写测试（goal_pursuit.rs `mod tests`，复用现有 `active_goal` helper）**

> 现有 `active_goal(max_iter: u32)` 返回 Active-pursuit Active-status goal。
> 新测试需要构造 Complete-status 变体。

```rust
    #[test]
    fn awaiting_gate_true_only_for_active_pursuit_complete_unchecked() {
        let mut g = active_goal(5);
        g = g.with_status(GoalStatus::Complete, 1);
        // Active pursuit + Complete + Unchecked + gate → true
        assert!(awaiting_gate(&g, true));
        // 无闸门 → false（回归保护：无 stop_hooks 用户行为不变）
        assert!(!awaiting_gate(&g, false));
        // 已 Passed → false（不重复 gate）
        let passed = g.clone().with_gate_outcome(GateOutcome::Passed, 2);
        assert!(!awaiting_gate(&passed, true));
        // 仍 Active 状态（未自报 complete）→ false
        assert!(!awaiting_gate(&active_goal(5), true));
        // 被动 goal → false（交互 complete 不经闸门）
        let mut passive = Goal::new("s", "o", 0, 0);
        passive = passive.with_status(GoalStatus::Complete, 1);
        assert!(!awaiting_gate(&passive, true));
    }

    #[test]
    fn confirm_complete_sets_passed() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 1);
        let c = confirm_complete(&g, 9);
        assert_eq!(c.gate_outcome, GateOutcome::Passed);
        assert_eq!(c.updated_at_ms, 9);
    }

    #[test]
    fn reopen_after_gate_failure_reopens_active_when_cap_remaining() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 1);
        let r = reopen_after_gate_failure(&g, "tests failed: 3 errors", 9);
        assert_eq!(r.status, GoalStatus::Active);
        assert_eq!(r.gate_outcome, GateOutcome::Unchecked);
        assert!(r.note.unwrap().contains("tests failed"));
    }

    #[test]
    fn reopen_after_gate_failure_blocks_when_cap_spent() {
        let mut g = active_goal(3).with_status(GoalStatus::Complete, 1);
        g.continuations_used = 3; // cap 已满
        let r = reopen_after_gate_failure(&g, "still red", 9);
        assert_eq!(r.status, GoalStatus::Blocked);
        assert!(r.note.unwrap().contains("Blocked"));
    }

    #[test]
    fn gate_failure_prompt_restates_goal_and_reason() {
        let g = active_goal(5);
        let p = gate_failure_prompt(&g, "lint: 2 warnings");
        assert!(p.contains(&g.objective));
        assert!(p.contains("lint: 2 warnings"));
        assert!(p.contains("objective gate"));
    }
```

- [ ] **Step 4: Run test — *DEFERRED***

- [ ] **Step 5: Commit**

```bash
git add src/tasks/goal_pursuit.rs src/goal/mod.rs
git commit -m "feat(goal-pursuit): pure decision fns for objective-gate completion"
```

---

## Task 3: `execute_stop_hooks_arc`（Arc 版闸门 runner，DRY）

**Files:**
- Modify: `src/verification/stop_hooks.rs`（`execute_stop_hooks` 后，~178+ 之后）
- Modify: `src/verification/stop_hook_verifier.rs`（删内联 `ArcHook`，改调用新 helper）

- [ ] **Step 1: 在 `stop_hooks.rs` 的 `execute_stop_hooks` 函数之后追加 Arc 版本**

```rust
/// `execute_stop_hooks` 的 `Arc` 入参版本——用于 harness 之外、以 `Arc`
/// 持有闸门的消费者（goal-loop 闸门、`StopHookVerifier`）。把每个 `Arc`
/// 包成 forwarding box，复用上面的并发 runner，不克隆 hook 实现。
pub async fn execute_stop_hooks_arc(
    hooks: &[Arc<dyn StopHookHandler>],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult {
    struct ArcHook(Arc<dyn StopHookHandler>);
    #[async_trait::async_trait]
    impl StopHookHandler for ArcHook {
        fn name(&self) -> &str {
            self.0.name()
        }
        async fn evaluate(
            &self,
            ctx: &StopHookContext,
            cancel: &CancellationToken,
        ) -> StopHookVerdict {
            self.0.evaluate(ctx, cancel).await
        }
    }
    let boxed: Vec<Box<dyn StopHookHandler>> = hooks
        .iter()
        .map(|h| Box::new(ArcHook(h.clone())) as Box<dyn StopHookHandler>)
        .collect();
    execute_stop_hooks(&boxed, context, cancel).await
}
```

- [ ] **Step 2: 重构 `stop_hook_verifier.rs` 改用新 helper（熵减：删内联 `ArcHook`）**

把 `verify` 方法里从 `struct ArcHook(...)` 到 `let result = execute_stop_hooks(&boxed, &hctx, cancel).await;` 的整段（含内联 `ArcHook` impl、`boxed` 构造）替换为：

```rust
        let hctx = StopHookContext {
            final_text: ctx.final_text.map(|s| s.to_string()),
            iterations: ctx.iterations,
            tool_calls_made: ctx.tool_calls_made,
            stop_reason: stop_reason.to_string(),
        };
        let result =
            crate::verification::stop_hooks::execute_stop_hooks_arc(&self.hooks, &hctx, cancel)
                .await;
```

并更新该文件顶部 import：把
`use crate::verification::stop_hooks::{execute_stop_hooks, StopHookContext, StopHookHandler, StopHookVerdict};`
改为
`use crate::verification::stop_hooks::{execute_stop_hooks_arc, StopHookContext, StopHookHandler};`
（`execute_stop_hooks`、`StopHookVerdict` 若变为未使用则一并移除——消除 orphan import）。

- [ ] **Step 3: Run test — *DEFERRED***

- [ ] **Step 4: Commit**

```bash
git add src/verification/stop_hooks.rs src/verification/stop_hook_verifier.rs
git commit -m "refactor(verification): extract execute_stop_hooks_arc, dedup ArcHook"
```

---

## Task 4: `ContinuationDeps` 命名结构（替换 2-tuple，加 `gate` 字段）

**Files:**
- Modify: `src/gateway/execution_engine/engine.rs`（field 76、ctor 122、`continuation_cell` 154-163）
- Modify: `src/gateway/execution_engine/execute.rs`（destructure 623）
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`（set 1387）

- [ ] **Step 1: 在 `engine.rs` 顶部模块区新增 `ContinuationDeps` 结构**

在 `ExecutionEngine` struct 定义之前（或文件末尾的类型区）加：

```rust
/// Deferred-injected deps for the post-run autonomous-continuation hook.
/// 命名结构替代旧 2-tuple，并携带 goal-loop 的客观闸门 handler。
/// `gate` 为 `None` 时（无 `config.toml [[stop_hooks]]`）loop 行为不变。
#[derive(Clone)]
pub struct ContinuationDeps {
    pub registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    pub adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    /// 客观闸门 handler（与 `StopHookVerifier` 共享同一份），守护自主
    /// 续跑的完成决策。`None` → 无闸门，complete 主张立即终止。
    pub gate: Option<Arc<Vec<Arc<dyn crate::verification::stop_hooks::StopHookHandler>>>>,
}
```

- [ ] **Step 2: 把 `continuation_deps` 字段类型（engine.rs:76）改为 `OnceLock<ContinuationDeps>`**

```rust
    pub(super) continuation_deps: Arc<std::sync::OnceLock<ContinuationDeps>>,
```

（ctor engine.rs:122 的 `Arc::new(std::sync::OnceLock::new())` 不变。）

- [ ] **Step 3: 把 `continuation_cell`（engine.rs:154-163）返回类型改为新结构**

```rust
    #[must_use]
    pub fn continuation_cell(&self) -> Arc<std::sync::OnceLock<ContinuationDeps>> {
        self.continuation_deps.clone()
    }
```

- [ ] **Step 4: 更新 `execute.rs:623` 的 destructure**

把：

```rust
                if let Some(cont_deps) = self.continuation_deps.get() {
```

之后所有用到 `cont_deps.0` / `cont_deps.1` 的地方改为字段访问 `cont_deps.registry` / `cont_deps.adapter`。（具体在 Task 5 整体重写该块时一并处理；本步只需保证编译——把现有 `cont_deps.0`→`cont_deps.registry`、`cont_deps.1`→`cont_deps.adapter`。）

- [ ] **Step 5: 更新 boot set（agent_init/mod.rs:1387）**

把：

```rust
        let _ = continuation_cell.set((agent_registry.clone(), engine_arc.clone()));
```

改为（`gate: None` 占位，Task 6 填真闸门）：

```rust
        let _ = continuation_cell.set(alephcore::gateway::execution_engine::ContinuationDeps {
            registry: agent_registry.clone(),
            adapter: engine_arc.clone(),
            gate: None,
        });
```

> 确认 `ContinuationDeps` 在 `execution_engine` 模块 re-export：在
> `src/gateway/execution_engine/mod.rs` 的 `pub use engine::{...}` 行加上
> `ContinuationDeps`。boot 处用到的路径以该 re-export 为准（若 crate 名/
> 路径不同，按 `mod.rs:182` 处 `RunRequest` 的同款引用方式对齐）。

- [ ] **Step 6: Run test — *DEFERRED***

- [ ] **Step 7: Commit**

```bash
git add src/gateway/execution_engine/engine.rs src/gateway/execution_engine/execute.rs \
        src/gateway/execution_engine/mod.rs \
        src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "refactor(execution): ContinuationDeps struct with optional gate field"
```

---

## Task 5: 续跑钩子加 `awaiting_gate` 分支 + 抽 `spawn_continuation_run`

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs`（imports 1-9；续跑块 618-725）

- [ ] **Step 1: 扩展 execute.rs imports（顶部 use 区）**

在现有 use 之后追加：

```rust
use crate::verification::stop_hooks::{execute_stop_hooks_arc, StopHookContext};
```

（`CollectingEventEmitter`、`SessionKey` 已可经现有路径引用：前者
`crate::gateway::event_emitter::CollectingEventEmitter`，后者经 `mod.rs` 的
`SessionKey` 即 `crate::routing::session_key::SessionKey`。）

- [ ] **Step 2: 在 `execute.rs` 模块内（impl 块外，文件底部）新增续跑 spawn helper（熵减：抽出重复的 RunRequest 构造+spawn）**

```rust
/// 入队一次自主续跑 run（同一 session、同一 agent，给定 prompt）。
/// 被 should_continue 续跑分支与 gate-failure 续跑分支共用——消除重复的
/// `RunRequest` 构造与 `tokio::spawn` 样板。
fn spawn_continuation_run(
    registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    adapter: Arc<dyn crate::gateway::execution_adapter::ExecutionAdapter>,
    session_key: crate::routing::session_key::SessionKey,
    session_key_str: String,
    prompt: String,
) {
    let cont_request = super::RunRequest {
        run_id: uuid::Uuid::new_v4().to_string(),
        input: prompt,
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata: std::collections::HashMap::new(),
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };
    let cont_agent_id = session_key.agent_id().to_string();
    tokio::spawn(async move {
        let Some(cont_agent) = registry.get(&cont_agent_id).await else {
            warn!(
                agent_id = %cont_agent_id,
                session = %session_key_str,
                "goal pursuit: agent not found, skipping continuation"
            );
            return;
        };
        let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(
            crate::gateway::event_emitter::CollectingEventEmitter::new(),
        );
        if let Err(e) = adapter.execute(cont_request, cont_agent, emitter).await {
            warn!(
                error = %e,
                session = %session_key_str,
                "goal pursuit: continuation run failed"
            );
        }
    });
}
```

- [ ] **Step 3: 重写续跑块（execute.rs 618-725）—— gate 分支在前，复用 helper**

把 `if let Some(cont_deps) = self.continuation_deps.get() { ... }` 内、
`match store.get(&session_key_str)` 的 `Ok(Some(goal)) => { ... }` 臂整体替换为：

```rust
                            Ok(Some(goal)) => {
                                let now_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(0, |d| d.as_millis() as u64);

                                // 闸门分支：模型在 Active 续跑下自报 Complete，
                                // 但客观闸门（stop_hooks 退出码）尚未确认。
                                // 在接受 complete 为终止前先跑闸门（Ralph
                                // Wiggum 营救）。结构化退出码，零 LLM 调用（R7）。
                                if crate::tasks::goal_pursuit::awaiting_gate(
                                    &goal,
                                    cont_deps.gate.is_some(),
                                ) {
                                    let gate = cont_deps.gate.clone().expect("is_some checked");
                                    let hctx = StopHookContext {
                                        final_text: Some(goal.objective.clone()),
                                        iterations: goal.continuations_used as usize,
                                        tool_calls_made: 0,
                                        stop_reason: "goal_complete_claim".to_string(),
                                    };
                                    let result = execute_stop_hooks_arc(
                                        &gate,
                                        &hctx,
                                        &CancellationToken::new(),
                                    )
                                    .await;
                                    let vetoed = result
                                        .halt_reason()
                                        .or_else(|| result.blocking_reason());
                                    match vetoed {
                                        None => {
                                            // 闸门通过 → 确认完成，循环终止。
                                            let confirmed =
                                                crate::tasks::goal_pursuit::confirm_complete(
                                                    &goal, now_ms,
                                                );
                                            if let Err(e) = store.put(&confirmed) {
                                                warn!(error = %e, session = %session_key_str,
                                                    "goal pursuit: failed to persist gate confirmation");
                                            } else {
                                                info!(session = %session_key_str,
                                                    "goal pursuit: objective gate passed, goal verified complete");
                                            }
                                        }
                                        Some(reason) => {
                                            // 闸门否决 → 退回 Active(或 Blocked)。
                                            let reopened =
                                                crate::tasks::goal_pursuit::reopen_after_gate_failure(
                                                    &goal, reason, now_ms,
                                                );
                                            let reopened_active = reopened.is_active();
                                            if let Err(e) = store.put(&reopened) {
                                                warn!(error = %e, session = %session_key_str,
                                                    "goal pursuit: failed to persist gate veto");
                                            } else if reopened_active {
                                                let bumped =
                                                    reopened.clone().spent_continuation(now_ms);
                                                if let Err(e) = store.put(&bumped) {
                                                    warn!(error = %e, session = %session_key_str,
                                                        "goal pursuit: failed to persist continuation counter after veto");
                                                } else {
                                                    let prompt =
                                                        crate::tasks::goal_pursuit::gate_failure_prompt(
                                                            &goal, reason,
                                                        );
                                                    info!(session = %session_key_str,
                                                        "goal pursuit: objective gate vetoed completion, re-running with feedback");
                                                    spawn_continuation_run(
                                                        cont_deps.registry.clone(),
                                                        cont_deps.adapter.clone(),
                                                        request.session_key.clone(),
                                                        session_key_str.clone(),
                                                        prompt,
                                                    );
                                                }
                                            } else {
                                                info!(session = %session_key_str,
                                                    "goal pursuit: objective gate vetoed at iteration cap, goal blocked");
                                            }
                                        }
                                    }
                                } else if crate::tasks::goal_pursuit::should_continue(&goal, 0) {
                                    let bumped = goal.clone().spent_continuation(now_ms);
                                    if let Err(e) = store.put(&bumped) {
                                        warn!(error = %e, session = %session_key_str,
                                            "goal pursuit: failed to persist continuation counter; skipping");
                                    } else {
                                        let prompt =
                                            crate::tasks::goal_pursuit::continuation_prompt(&goal);
                                        spawn_continuation_run(
                                            cont_deps.registry.clone(),
                                            cont_deps.adapter.clone(),
                                            request.session_key.clone(),
                                            session_key_str.clone(),
                                            prompt,
                                        );
                                        info!(session = %session_key_str,
                                            continuations_used = bumped.continuations_used,
                                            "goal pursuit: enqueued autonomous continuation");
                                    }
                                } else if crate::tasks::goal_pursuit::exhausted_while_active(
                                    &goal, 0,
                                ) {
                                    let note = crate::tasks::goal_pursuit::cap_reached_note(&goal);
                                    let blocked = goal
                                        .clone()
                                        .with_status(crate::goal::GoalStatus::Blocked, now_ms)
                                        .with_note(Some(note), now_ms);
                                    if let Err(e) = store.put(&blocked) {
                                        warn!(error = %e, session = %session_key_str,
                                            "goal pursuit: failed to persist cap-reached block");
                                    } else {
                                        info!(session = %session_key_str,
                                            continuations_used = goal.continuations_used,
                                            "goal pursuit: iteration cap reached, goal blocked for user guidance");
                                    }
                                }
                            }
```

> 注：原内联的 RunRequest 构造 + `tokio::spawn`（旧 633-695）已被
> `spawn_continuation_run` 取代——**删除旧内联块**（熵减）。`emitter`、
> `CollectingEventEmitter`、`cont_registry`/`cont_adapter`/`cont_session`
> 等旧局部变量随之移除。

- [ ] **Step 4: Run test — *DEFERRED***

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/execute.rs
git commit -m "feat(goal-loop): gate model self-reported completion through objective stop-hooks"
```

---

## Task 6: boot 注入真实闸门 handler

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`（set 1387 附近；`app_config` 参数已在 121 行作用域内）

- [ ] **Step 1: 在 set 调用前构造闸门 handler**

把 Task 4 Step 5 留下的 `gate: None` 替换为从 `app_config.stop_hooks` 构造
（与 `StopHookVerifier` 同一份配置；`build_from_config` 廉价，仅构造
`ShellStopHook` 结构）：

```rust
        // Goal-loop 客观闸门：复用全局 config.toml [[stop_hooks]]（与
        // within-turn StopHookVerifier 同源）。None → 无闸门，complete 主张
        // 立即终止（行为与本特性引入前一致）。
        let goal_gate = alephcore::verification::stop_hooks::build_from_config(
            &app_config.stop_hooks,
        );
        let _ = continuation_cell.set(alephcore::gateway::execution_engine::ContinuationDeps {
            registry: agent_registry.clone(),
            adapter: engine_arc.clone(),
            gate: goal_gate,
        });
```

> `build_from_config` 返回 `Option<Arc<Vec<Arc<dyn StopHookHandler>>>>`，正好
> 匹配 `ContinuationDeps.gate` 类型——直接赋值，无空集需特判（空配置返回 None）。

- [ ] **Step 2: Run test — *DEFERRED***

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "feat(boot): wire global stop_hooks as goal-loop objective gate"
```

---

## Task 7: 熵减扫描 + spec/plan 收尾

**Files:**
- 只读扫描 + 按需删除

- [ ] **Step 1: 扫描遗留 goal-verification 死桩 / orphan import**

```bash
cd /Volumes/TBU4/Workspace/Aleph-loop-gate
grep -rn "ArcHook" src/verification/        # 应仅剩 stop_hooks.rs 一处
grep -rn "cont_deps.0\|cont_deps.1\|\.continuation_deps.get" src/gateway/  # 应无元组下标残留
grep -rn "execute_stop_hooks\b" src/verification/stop_hook_verifier.rs     # 应已改为 _arc
```

预期：`ArcHook` 仅存于 `stop_hooks.rs`；无 `cont_deps.0/.1` 元组下标；
`stop_hook_verifier.rs` 不再直接调用非-arc 版（除非仍有其它合法用途）。

- [ ] **Step 2: 确认无 `gate_outcome` 相关 orphan**

```bash
grep -rn "gate_outcome\|GateOutcome\|awaiting_gate\|confirm_complete\|reopen_after_gate_failure" src/ | wc -l
```

确保每个新符号都有消费者（types 定义 / goal_pursuit 决策 / execute 消费 /
boot 注入），无只定义不使用的死代码。

- [ ] **Step 3: Run full test — *DEFERRED***（CLAUDE.md 约束：不本地 cargo check）

- [ ] **Step 4: Commit（若 Step 1-2 有删改）**

```bash
git add -A
git commit -m "chore(goal-loop): entropy sweep — dedup ArcHook, drop tuple orphans"
```

---

## Self-Review

**Spec coverage**：
- §3 改动 1（类型态）→ Task 1 ✅
- §3 改动 2（纯决策函数）→ Task 2 ✅
- §3 改动 3（连线 engine/execute/orchestrator-init）→ Task 4+5+6 ✅（boot 落在
  `agent_init/mod.rs` 而非 `orchestrator_init.rs`——实际 set 点在 agent_init，
  spec §3 写的 orchestrator_init 是闸门**构建**处；二者都用同一 `build_from_config`，
  本计划在 agent_init 重建一份，语义等价且更近 set 点）。
- §4 熵减 → Task 3（ArcHook dedup）+ Task 5（spawn helper dedup）+ Task 7 ✅
- §5 测试 → Task 1 Step 5 + Task 2 Step 3 ✅
- §6 红线（harness 零改动）→ 所有改动文件均不在 `src/harness/` ✅

**Placeholder scan**：无 TBD/TODO；所有代码步骤含完整代码。

**Type consistency**：`GateOutcome::{Unchecked,Passed}`、`with_gate_outcome`、
`awaiting_gate(&Goal, bool)`、`confirm_complete(&Goal, u64)`、
`reopen_after_gate_failure(&Goal, &str, u64)`、`gate_failure_prompt(&Goal, &str)`、
`ContinuationDeps{registry,adapter,gate}`、`execute_stop_hooks_arc(&[Arc<...>], &StopHookContext, &CancellationToken)`
—— 各 Task 间签名一致。

**已知待执行期确认点**（非阻塞）：
1. `GateOutcome` re-export（goal/mod.rs）— Task 2 Step 1 已含。
2. `ContinuationDeps` re-export（execution_engine/mod.rs）— Task 4 Step 5 已含。
3. `request.session_key` 即 `crate::routing::session_key::SessionKey`（mod.rs:182 确认）。
4. boot 处 crate 路径前缀（`alephcore::` vs `crate::`）以 agent_init 现有引用风格为准。
