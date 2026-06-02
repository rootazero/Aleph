# 维度 3：边界异常反馈与异步干预 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Aleph 的 Harness 在熔断时主动给用户上下文反馈，并让用户中途指令能热插拔进正在执行的 scratchpad 任务列表 —— 全部通过连线已合并子系统完成，不新增 `src/harness/` 文件。

**Architecture:** 三处外科改动。(A) 把 `think.rs` 里已有的 `fire_max_iterations_grace_turn` 泛化为 `fire_boundary_grace_turn(reason)`，复用同一套救援轮机制接到 `VerifierVeto` / `ConsecutiveFailureCap` 两个目前静默终止的点；救援轮的剩余步骤/障碍上下文**已在 event log 里**（veto 原因 `[verifier veto] …` 与 `ToolError` 事件），故 nudge 保持静态 `&str`，模型自己写"卡住"消息（R7）。(B) `steering.rs` 注入前查 `scratchpad_registry::active`，有活跃 scratchpad 时给消息加 reconcile 前导语，合并决策归模型（R7）。(C) `scratchpad/manager.rs::write` 换成既有 `utils::atomic_write::atomic_write_file`，防中断损坏。

**Tech Stack:** Rust / Tokio。复用：`crate::utils::atomic_write::atomic_write_file`、`crate::builtin_tools::scratchpad_registry`、`think.rs` 的 `GraceReason` + `fire_grace_turn`。

**红线**: R7（模型写文本/做合并决策，harness 只组装上下文）、R10（不新增 harness 文件，仅泛化一个既有函数）。

---

## 文件结构

| 类型 | 文件 | 职责 |
|---|---|---|
| 改 | `src/harness/agent/think.rs` | 加 2 个 `GraceReason` 变体 + 2 个静态 nudge 常量；`GraceReason` 改 `pub(crate)`；`fire_max_iterations_grace_turn` → `fire_boundary_grace_turn(reason)` |
| 改 | `src/harness/agent.rs` | 3 个终止点统一调 `fire_boundary_grace_turn`（veto/failure 新增救援轮，max-iter 传参不变） |
| 改 | `src/gateway/execution_engine/steering.rs` | 新纯函数 `apply_reconcile_preamble`；注入前查 registry 加前导语 |
| 改 | `src/memory/scratchpad/manager.rs` | `write` 主写换原子写 |

无新增文件。无破坏性接口变更。

---

## Task 1: 3a — 边界异常救援轮（veto / 连续失败 也产出终端反馈）

**Files:**
- Modify: `src/harness/agent/think.rs:36`（nudge 常量区）、`:88`（`GraceReason` 枚举）、`:97`（`nudge()`）、`:1155`（`fire_max_iterations_grace_turn`）
- Modify: `src/harness/agent.rs:496`、`:520`、`:537`（三个终止点）
- Test: `src/harness/agent/think.rs`（`#[cfg(test)] mod tests`，已有 nudge 测试在 `:1313`）

### 背景（实现者必读）
- `think.rs::fire_grace_turn`（私有）已封装"追加一条 nudge user 消息 → 一次 LLM 调用 → 把回复写回 session 作为终端 AssistantMessage"的完整逻辑，且 `if last_assistant_has_text(events) { return; }` 保证良好结束的轮次零成本。
- `fire_max_iterations_grace_turn`（`:1155`）是它的唯一 public 包装，目前硬编码 `GraceReason::MaxIterations`，唯一调用点在 `agent.rs:537`。
- veto 救援轮无需动态上下文：`think.rs:778` 已把 `[verifier veto] {reason}`（含剩余步骤列表）作为 `UserMessage` 写入 session log；`ConsecutiveFailureCap` 的障碍是 `ToolError` 事件。救援轮的 prompt 由全量 event log 构建，故模型已看到剩余步骤与障碍，nudge 只需"停手 + 向用户说明 + 求指示"。

- [ ] **Step 1: 写失败测试 —— 两个新 nudge 变体**

在 `src/harness/agent/think.rs` 的 `#[cfg(test)] mod tests` 内（紧邻 `:1313` 既有 nudge 测试）新增：

```rust
    #[test]
    fn verifier_veto_nudge_is_distinct_and_set() {
        assert_eq!(GraceReason::VerifierVeto.nudge(), GRACE_NUDGE_VERIFIER_VETO);
        assert_ne!(GRACE_NUDGE_VERIFIER_VETO, GRACE_NUDGE_MAX_ITERATIONS);
        assert!(GRACE_NUDGE_VERIFIER_VETO.contains("user"));
    }

    #[test]
    fn consecutive_failure_nudge_is_distinct_and_set() {
        assert_eq!(
            GraceReason::ConsecutiveFailureCap.nudge(),
            GRACE_NUDGE_FAILURE_CAP
        );
        assert_ne!(GRACE_NUDGE_FAILURE_CAP, GRACE_NUDGE_VERIFIER_VETO);
        assert!(GRACE_NUDGE_FAILURE_CAP.contains("user"));
    }
```

- [ ] **Step 2: 运行测试，确认编译失败**

资源门控后运行（详见文末"校验命令"）：
`cargo test -p alephcore --lib harness::agent::think::tests::verifier_veto_nudge_is_distinct_and_set -- --nocapture`
Expected: 编译失败 —— `no variant named VerifierVeto`、`cannot find value GRACE_NUDGE_VERIFIER_VETO`。

- [ ] **Step 3: 加两个静态 nudge 常量**

在 `src/harness/agent/think.rs` 紧跟 `GRACE_NUDGE_MAX_ITERATIONS` 定义（`:39` 之后）插入：

```rust

/// Ephemeral nudge for the grace turn fired when the verifier-veto safety
/// cap trips — the model kept trying to finish with required steps still
/// incomplete. The remaining steps are already in context (the
/// `[verifier veto] …` messages list them), so this only tells the model to
/// stop and hand control back to the user. The model writes the actual
/// message (R7 — no hardcoded user-facing template).
const GRACE_NUDGE_VERIFIER_VETO: &str =
    "You have repeatedly tried to finish while required steps from your \
     execution list remain incomplete, and the safety cap has now stopped \
     the loop. Do NOT call any more tools. Respond now with a clear message \
     for the user: which steps remain unfinished, what is blocking you from \
     completing them, and what decision or input you need from the user to \
     proceed.";

/// Ephemeral nudge for the grace turn fired when the consecutive-failure
/// safety cap trips. The recurring error is already in context (the
/// `ToolError` events), so this only tells the model to stop and surface the
/// blocker to the user.
const GRACE_NUDGE_FAILURE_CAP: &str =
    "Your recent turns have failed repeatedly and the safety cap has now \
     stopped the loop. Do NOT call any more tools. Respond now with a clear \
     message for the user: what you were attempting, the specific error or \
     obstacle that keeps recurring, and what decision or input you need from \
     the user to proceed.";
```

- [ ] **Step 4: 加枚举变体 + 改 `pub(crate)` + 扩 `nudge()`**

把 `:88` 的 `GraceReason` 枚举改为 `pub(crate)`（让兄弟模块 `agent.rs` 能命名变体）并加两个变体：

```rust
/// Why a grace turn is being fired. Selects the nudge text; otherwise
/// the call path is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GraceReason {
    /// `LoopDirective::FinalReply` — context-budget critical.
    Budget,
    /// `LoopDirective::StopDiminishing` — diminishing-returns detector trip.
    Diminishing,
    /// `max_iterations` cap reached in the outer loop.
    MaxIterations,
    /// `MAX_VERIFIER_VETOS` cap reached — model kept finishing with steps left.
    VerifierVeto,
    /// `consecutive_failure_cap` reached — repeated total-failure turns.
    ConsecutiveFailureCap,
}
```

扩 `nudge()`（`:97`）match：

```rust
impl GraceReason {
    fn nudge(self) -> &'static str {
        match self {
            Self::Budget => GRACE_NUDGE_BUDGET,
            Self::Diminishing => GRACE_NUDGE_DIMINISHING,
            Self::MaxIterations => GRACE_NUDGE_MAX_ITERATIONS,
            Self::VerifierVeto => GRACE_NUDGE_VERIFIER_VETO,
            Self::ConsecutiveFailureCap => GRACE_NUDGE_FAILURE_CAP,
        }
    }
}
```

- [ ] **Step 5: 运行测试，确认通过**

`cargo test -p alephcore --lib harness::agent::think::tests::verifier_veto_nudge_is_distinct_and_set harness::agent::think::tests::consecutive_failure_nudge_is_distinct_and_set`
Expected: 2 passed。

- [ ] **Step 6: 泛化 `fire_max_iterations_grace_turn` → `fire_boundary_grace_turn(reason)`**

把 `:1155` 的方法整体替换为（仅签名加 `reason` 参数、改名、把硬编码的 `GraceReason::MaxIterations` 换成 `reason`；其余逐字不变）：

```rust
    pub(crate) async fn fire_boundary_grace_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        reason: GraceReason,
        parent_cancel: &CancellationToken,
    ) {
        let events = match self.deps.session.get_events(session_id, None, None).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(?session_id, ?e, "boundary grace turn: get_events failed");
                return;
            }
        };
        if last_assistant_has_text(&events) {
            return; // user already has terminal text; skip.
        }
        let tail_start = super::tail_start_index(&events);
        let messages = super::prompt::build_prompt(&events, tail_start);
        self.fire_grace_turn(
            session_id,
            &events,
            &messages,
            callback,
            iterations,
            reason,
            parent_cancel,
        )
        .await;
    }
```

- [ ] **Step 7: 接 3 个终止点（`agent.rs`）**

在 `src/harness/agent.rs` 把 max-iter 调用点（`:537`）从 `fire_max_iterations_grace_turn(&current_session, callback, iterations, cancel)` 改为：

```rust
                            self.fire_boundary_grace_turn(
                                &current_session,
                                callback,
                                iterations,
                                crate::harness::agent::think::GraceReason::MaxIterations,
                                cancel,
                            )
                            .await;
```

在 veto 终止点，把 `:520` 的 `callback.on_complete();` **之前**插入救援轮（即 `:516-521` 块改为）：

```rust
                            self.hit_limit.store(true, Ordering::Relaxed);
                            self.set_terminate_reason(TerminateReason::VerifierVeto {
                                vetos: verifier_veto_count.try_into().unwrap_or(u32::MAX),
                            });
                            // 3a: surface a context-rich terminal message instead
                            // of a silent HitLimit break. The remaining steps are
                            // already in the prompt (the `[verifier veto]` events).
                            self.fire_boundary_grace_turn(
                                &current_session,
                                callback,
                                iterations,
                                crate::harness::agent::think::GraceReason::VerifierVeto,
                                cancel,
                            )
                            .await;
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
```

在连续失败终止点，把 `:496` 的 `callback.on_complete();` **之前**插入救援轮（即 `:488-499` 块的 `callback.on_complete()` 前加）：

```rust
                                    self.hit_limit.store(true, Ordering::Relaxed);
                                    self.set_terminate_reason(
                                        TerminateReason::ConsecutiveFailureCap {
                                            consecutive: consecutive_failure_turns
                                                .try_into()
                                                .unwrap_or(u32::MAX),
                                        },
                                    );
                                    // 3a: surface the recurring blocker to the user
                                    // instead of a silent HitLimit break.
                                    self.fire_boundary_grace_turn(
                                        &current_session,
                                        callback,
                                        iterations,
                                        crate::harness::agent::think::GraceReason::ConsecutiveFailureCap,
                                        cancel,
                                    )
                                    .await;
                                    callback.on_complete();
                                    break Ok(
                                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                                    );
```

> 注：三处的 `&current_session`、`callback`、`iterations`、`cancel` 均与 max-iter 点同处一个 match 臂，已在作用域内。

- [ ] **Step 8: 全量编译 + 既有 wiring 测试不回归**

```
cargo check -p alephcore --bin aleph-server
cargo test -p alephcore --lib harness::agent::think::tests
cargo test -p alephcore --lib harness::tests::task10_wiring
```
Expected: check exit 0；think nudge 测试全绿；既有 task10_wiring（含 veto-cap 用例 `mod.rs:741`）保持绿（泛化是行为相容的重构，veto/failure 多了一次救援 LLM 调用，但 mock LLM 在 `last_assistant_has_text` 为真时直接 return，不影响断言）。

- [ ] **Step 9: 提交**

```bash
git add src/harness/agent/think.rs src/harness/agent.rs
git commit -m "harness: surface terminal feedback on veto/failure caps via boundary grace turn"
```

---

## Task 2: 3b — Scratchpad 感知的 steering 信封

**Files:**
- Modify: `src/gateway/execution_engine/steering.rs`（新增纯函数 + 注入路径 1 处）
- Test: 同文件 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败测试 —— `apply_reconcile_preamble` 纯函数**

在 `src/gateway/execution_engine/steering.rs` 的 `mod tests` 内新增：

```rust
    #[test]
    fn preamble_added_only_when_scratchpad_active() {
        let with = apply_reconcile_preamble("do X".to_string(), true);
        assert!(with.contains("do X"));
        assert!(with.starts_with(RECONCILE_PREAMBLE));

        let without = apply_reconcile_preamble("do X".to_string(), false);
        assert_eq!(without, "do X");
    }
```

- [ ] **Step 2: 运行测试，确认编译失败**

`cargo test -p alephcore --lib gateway::execution_engine::steering::tests::preamble_added_only_when_scratchpad_active`
Expected: 编译失败 —— `cannot find function apply_reconcile_preamble` / `cannot find value RECONCILE_PREAMBLE`。

- [ ] **Step 3: 加常量 + 纯函数**

在 `src/gateway/execution_engine/steering.rs` 的 `render_user_session_text` 之后（`:76` 后）插入：

```rust
/// Prepended to a steering message when the target session has an active
/// scratchpad execution list, so the model reconciles its task list before
/// continuing. The model decides append / insert / reprioritize (R7 — the
/// harness never splices `scratchpad.md` for the user).
const RECONCILE_PREAMBLE: &str =
    "[user added mid-task] The user sent new input while you are executing a \
     task list. Reconcile your scratchpad first — call the scratchpad tool to \
     append, insert, or reprioritize steps as you judge appropriate — then \
     continue.\n\nNew input: ";

/// Prepend [`RECONCILE_PREAMBLE`] to `text` iff the session has an active
/// scratchpad. Pure so the policy is unit-tested without a registry global,
/// mirroring [`find_steering_target`].
pub(super) fn apply_reconcile_preamble(text: String, has_active_scratchpad: bool) -> String {
    if !has_active_scratchpad {
        return text;
    }
    format!("{RECONCILE_PREAMBLE}{text}")
}
```

- [ ] **Step 4: 运行测试，确认通过**

`cargo test -p alephcore --lib gateway::execution_engine::steering::tests::preamble_added_only_when_scratchpad_active`
Expected: 1 passed。

- [ ] **Step 5: 注入路径接线**

在 `try_inject_steering` 内，把构建 `event` 的 `text: render_user_session_text(request),`（`:124`）替换为先算 text。即把 `:121-133` 的 `let event = …` 块改为：

```rust
    // 3b: if this session is driving a scratchpad execution list, tell the
    // model to reconcile it before continuing. Mechanical lookup, no I/O.
    let has_active_scratchpad =
        crate::builtin_tools::scratchpad_registry::active(&request.session_key.to_key_string())
            .is_some();
    let text = apply_reconcile_preamble(render_user_session_text(request), has_active_scratchpad);
    let event = SessionEvent::UserMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text,
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        at: now_ms(),
        // `false` → the prompt builder (G2) wraps this in `<system-reminder>`
        // as a real user interjection, exactly the designed steering path.
        synthetic: false,
    };
```

- [ ] **Step 6: 编译 + steering 套件不回归**

```
cargo check -p alephcore --bin aleph-server
cargo test -p alephcore --lib gateway::execution_engine::steering::tests
```
Expected: check exit 0；steering 全部测试（4 个既有 + 1 个新）绿。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/execution_engine/steering.rs
git commit -m "gateway: prepend scratchpad-reconcile preamble to mid-loop steering when a task list is active"
```

---

## Task 3: 跨切 — scratchpad 写入原子化

**Files:**
- Modify: `src/memory/scratchpad/manager.rs:272`（`write`）
- Test: 同文件 `#[cfg(test)] mod tests`（已用 `tempfile::tempdir`，见 `:499`）

> **TDD 说明（诚实标注）**：原子性（中断时不留半写文件）无法在单元测试里确定性触发（需真崩在 rename 之前）。本任务是**重构 + 回归保护**：测试锁定"`write` 后内容正确、且不残留 `.aleph_atomic_*` 暂存文件"，对裸写与原子写都应通过 —— 它保证切换实现后语义不退化，并把"暂存文件命名前缀"钉成回归不变量。真正的安全收益由实现切换本身（temp+rename）提供。

- [ ] **Step 1: 写回归测试 —— 写入内容正确且无残留 temp 文件**

在 `src/memory/scratchpad/manager.rs` 的 `#[cfg(test)] mod tests` 内新增（构造器与文件内既有测试一致：`ScratchpadManager::with_dir`）：

```rust
    #[tokio::test]
    async fn write_roundtrips_and_leaves_no_temp_files() {
        let temp = tempfile::tempdir().unwrap();
        let manager = ScratchpadManager::with_dir(temp.path().to_path_buf(), "sess-atomic");
        manager.write("# Objective\nhello\n").await.unwrap();
        assert_eq!(manager.read().await.unwrap(), "# Objective\nhello\n");
        // No `.aleph_atomic_*` staging files survive a successful write.
        let leftovers: Vec<_> = std::fs::read_dir(manager.scratchpad_path().parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".aleph_atomic_"))
            .collect();
        assert!(leftovers.is_empty(), "no atomic temp files should remain");
    }
```

- [ ] **Step 2: 运行测试（基线，应通过）**

`cargo test -p alephcore --lib memory::scratchpad::manager::tests::write_roundtrips_and_leaves_no_temp_files`
Expected: PASS（基线：裸写也满足"内容正确 + 无 temp 残留"）。这是切换前的回归基线。

- [ ] **Step 3: 主写换原子写**

把 `:283` 的主写：

```rust
        fs::write(self.scratchpad_path(), content)
            .await
            .map_err(|e| AlephError::other(format!("Failed to write scratchpad: {}", e)))
```

替换为：

```rust
        crate::utils::atomic_write::atomic_write_file(&self.scratchpad_path(), content).await
```

> 保留上方 `backup_on_write` 备份块不动（它是独立的 config-gated 功能，删除会改变公开配置语义 → 违反非破坏性红线）。本任务只把**主写**变原子，消除中断半写损坏风险。详见下方"对 spec 的偏离"。

- [ ] **Step 4: 运行测试，确认切换后仍通过**

`cargo test -p alephcore --lib memory::scratchpad::manager::tests::write_roundtrips_and_leaves_no_temp_files`
Expected: PASS（切换到 temp+rename 后内容正确、无 `.aleph_atomic_*` 残留 —— 回归不变量保持）。

- [ ] **Step 5: scratchpad 全套件不回归**

`cargo test -p alephcore --lib memory::scratchpad`
Expected: 全绿（含 snapshot 解析、set_plan/complete_item 等既有测试 —— 它们都经 `write`）。

- [ ] **Step 6: 提交**

```bash
git add src/memory/scratchpad/manager.rs
git commit -m "memory: make scratchpad write atomic (temp+rename) to prevent mid-write corruption"
```

---

## 最终校验

- [ ] **三道关全绿**（资源门控后运行）：

```
cargo check -p alephcore --bin aleph-server     # exit 0
cargo check -p alephcore --tests                # exit 0（cfg(test) 代码）
cargo test -p alephcore --lib harness::agent::think::tests \
    harness::tests::task10_wiring \
    gateway::execution_engine::steering \
    memory::scratchpad
```

---

## 校验命令（资源门控，强制）

每条 `cargo` 前先自检本地负载（移植自 goal.md 的 `check_and_run_cargo`，zsh 下用 `${=cmd}` 避免不分词）：

```bash
check_and_run_cargo() {
    local cmd="$1"
    while true; do
        local count=$(pgrep -x cargo | wc -l | tr -d ' ')
        if [ "$count" -lt 3 ]; then
            cargo ${=cmd}
            break
        else
            echo "检测到 $count 个 cargo 实例，等待 10s..."; sleep 10
        fi
    done
}
# 例：check_and_run_cargo "check -p alephcore --bin aleph-server"
```

> macOS BSD `pgrep` 无 `-c`，用 `pgrep -x cargo | wc -l`。

---

## 对 spec 的偏离（已在计划内消化，供 review）

1. **3a 不传动态上下文参数。** spec §3.1 设想 `fire_boundary_grace_turn(reason, scratchpad_ctx, last_blocker)`。研究发现剩余步骤（`[verifier veto] …`）与障碍（`ToolError`）**已在 event log → prompt** 中，救援轮 prompt 由全量日志构建，模型已见。故 nudge 保持静态 `&str`，签名只多一个 `reason: GraceReason`。更 DRY、更贴合既有 `fire_grace_turn` 机制，零额外 I/O。

2. **保留 `backup_on_write` 备份块。** spec §5 写"删非原子备份拷贝"。`backup_on_write` 是 config-gated 的独立功能，删除会改变公开配置语义 → 触红线"非破坏性/向后兼容"。本计划只把**主写**变原子（真正消除半写损坏），备份块原样保留。如确需删除该功能，应作为单独的 deprecation 改动另行评估。

3. **3a 行为变更以编译 + 既有 wiring 测试保护，而非新增重型集成测试。** 救援轮接线是机械重构，端到端 mock harness 测试成本高且既有 `task10_wiring` 已覆盖 veto-cap 路径；TDD 锚点放在确定性纯/单元面（nudge 常量、`apply_reconcile_preamble`、`write` 原子性）。
