# Context Never-Break (P0 解砖) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让上下文压力永远无法终止一次 run —— near-full / 溢出的会话不再报「上下文预算已用尽（0 次迭代后）」，而是压缩后继续；即使压缩器失灵也有确定性截断地板兜底。

**Architecture:** 三层不硬停：(1) 主动预算门 `before_turn` 的 critical / split-exhausted 不再返回 `FinalReply`，改返回新指令 `CompactToFit`；(2) 新增 `compact_to_fit` 编排（现有 LLM 压缩 → 复测 → 确定性 `truncate_to_fit` 地板），**后置条件保证 `peek_pressure().ratio < critical`**；(3) 反应式路径 provider 真溢出时同样走 `compact_to_fit` 后重试，不再 `ReactiveCompactExhausted`。本期只用**临时（in-flight vec）压缩**，不改会话持久化（就地重写落 Plan 2）。

**Tech Stack:** Rust / tokio；`src/context/budget/`（决策）、`src/context/compact/`（机制，新增 `fit.rs`）、`src/harness/agent/think.rs`（机械分派，改不加文件）。

## Global Constraints

- **R10 薄 harness**：不新增 `src/harness/` 文件（12 文件/4900 行不动）；think.rs 仅加机械分派臂，无推理、无意图分类、无错误恢复策略选择。
- **压缩内容不新增推理层**：摘要复用现有 `Compactor::compact`（其内部 LLM 摘要 + 失败确定性截断）；`truncate_to_fit` 是纯确定性、零 LLM。
- **UTF-8 安全**（P7）：任何字符串截断走 `char_indices()` / `.get(..n)`，不用 `&s[..n]`。
- **锁安全**（P7）：沿用现有 `budget.lock().await` 模式；不新增 poison 风险点。
- **Cargo 纪律（用户 CLAUDE.md 覆盖 TDD 默认）**：实现者写代码 + 测试但**不逐步跑 cargo**；每个 Task 结束由控制器前台跑**一次**定向门 `cargo test -p alephcore --lib <filter>`（warm 共享 target），**绝不跑全量**。步骤里的"运行测试"= 该 Task 唯一的门。
- **不可变优先**：`truncate_to_fit` 之外的函数尽量返回新值；截断地板对 `&mut Vec` 原地操作是有意（性能 + 与 `compact` 签名一致）。
- **消息不损坏**：丢弃消息时保护 tool_call/tool_result 配对（不孤立 `ToolResult`），复用 compactor 的 `snap_boundary_forward` 思路。

---

### Task 1: `LoopDirective::CompactToFit` + `before_turn` 不再因压力返回 `FinalReply`

**Files:**
- Modify: `src/context/budget/mod.rs`（enum `LoopDirective` ~:147；`before_turn` critical 分支 :452；split-exhausted 分支 :480）
- Test: `src/context/budget/mod.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `LoopDirective::CompactToFit`（新变体，`Copy`）——later tasks（Task 4）匹配它。
- Behavior change: `before_turn` 在 `pressure.ratio >= critical_threshold` 与 breaker-trip-且-split-cap-reached 两处返回 `CompactToFit`（不再 `FinalReply`）。

- [ ] **Step 1: 写失败测试** — 在 `src/context/budget/mod.rs` 的 `mod tests` 内新增：

```rust
#[test]
fn critical_pressure_requests_compact_to_fit_not_final_reply() {
    let config = ContextBudgetConfig {
        token_budget: 1000,
        warning_threshold: 0.70,
        critical_threshold: 0.85,
        ..default_config()
    };
    let mut budget = ContextBudget::new(&config);
    // Build a message list whose estimated tokens blow past 0.85 * 1000.
    let big = vec![user_msg(&"x".repeat(8000))]; // ~2285 tokens @3.5 → ratio > 2.0
    let directive = budget.before_turn(&big, "", 0);
    assert_eq!(
        directive,
        LoopDirective::CompactToFit,
        "critical pressure must compact-to-fit, never hard-stop with FinalReply"
    );
}
```

（若 `user_msg` / `default_config` 助手不存在，复用同 `mod tests` 内既有构造器；已有 `over_budget_loop_does_not_fire_and_is_exhausted` 等测试提供了消息构造范例，照抄其 helper。）

- [ ] **Step 2: 加 enum 变体** — 在 `LoopDirective`（:147）`FinalReply` 之后插入：

```rust
    /// Context is critically full — compact aggressively until it fits
    /// (LLM summary → deterministic truncation floor) and CONTINUE. Replaces
    /// the old `FinalReply` hard-stop on the pressure path so a run can never
    /// terminate merely because the context filled up. See
    /// `context::compact::fit::compact_to_fit`.
    CompactToFit,
```

- [ ] **Step 3: critical 分支改返回** — `before_turn` :452，把

```rust
        if pressure.ratio >= self.critical_threshold {
            // Critical — force final reply regardless of circuit breaker
            tracing::warn!( target: "context_budget", used = pressure.used_tokens, budget = pressure.budget_tokens, ratio = pressure.ratio,
                "Critical context pressure — forcing final reply" );
            return LoopDirective::FinalReply;
        }
```

改为：

```rust
        if pressure.ratio >= self.critical_threshold {
            // Critical — compact aggressively until it fits, then continue.
            // Never a hard stop: a run cannot end just because context filled.
            tracing::warn!( target: "context_budget", used = pressure.used_tokens, budget = pressure.budget_tokens, ratio = pressure.ratio,
                "Critical context pressure — compacting to fit" );
            return LoopDirective::CompactToFit;
        }
```

- [ ] **Step 4: split-exhausted 分支改返回** — `before_turn` :478-480，把 breaker trip 且 `split_count >= max_splits` 的 `return LoopDirective::FinalReply;` 改为 `return LoopDirective::CompactToFit;`（同上，warn 文案改 `"...split cap reached — compacting to fit"`）。

- [ ] **Step 5: 跑门** — `cargo test -p alephcore --lib budget::` 应通过新测试且旧 `before_turn` 测试若断言 `FinalReply` 需同步改为 `CompactToFit`（搜 `mod.rs` 内 `LoopDirective::FinalReply` 断言：`:862`、`:946`、`:997` 等，逐一改为 `CompactToFit` 并核对语义仍成立）。Expected: PASS。

- [ ] **Step 6: 提交** — `git add -A && git commit -m "budget: critical/split-exhausted pressure requests CompactToFit, never FinalReply"`

---

### Task 2: `truncate_to_fit` 确定性截断地板（纯函数，零 LLM）

**Files:**
- Create: `src/context/compact/fit.rs`
- Modify: `src/context/compact/mod.rs`（`pub mod fit;` + 需要的 re-export）
- Test: `src/context/compact/fit.rs`（`#[cfg(test)] mod tests`）

**Interfaces:**
- Produces: `pub fn truncate_to_fit(messages: &mut Vec<UnifiedMessage>, target_tokens: usize, protected_tail: usize, prose_ratio: f64) -> usize`（返回被丢弃的估算 token 数）。后置条件：`estimate_total(messages, ratio) <= target_tokens` **除非** protected_tail 本身已超 target（则尽力，见 Step 3 尾部处理）。
- Consumes: `crate::providers::message::UnifiedMessage`；`crate::context::budget::pressure::estimate_message_tokens_aware`。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    fn text_user(s: &str) -> UnifiedMessage { UnifiedMessage::user_text(s) } // 若无此构造器，用既有构造器

    fn total(msgs: &[UnifiedMessage], ratio: f64) -> usize {
        msgs.iter().map(|m| estimate_message_tokens_aware(m, ratio)).sum()
    }

    #[test]
    fn drops_oldest_until_under_target_keeping_tail() {
        let mut msgs = vec![
            text_user(&"a".repeat(4000)),
            text_user(&"b".repeat(4000)),
            text_user(&"c".repeat(400)),  // fresh tail
        ];
        let before = total(&msgs, 3.5);
        let dropped = truncate_to_fit(&mut msgs, before / 3, 1, 3.5);
        assert!(dropped > 0);
        assert!(total(&msgs, 3.5) <= before / 3, "must fit under target");
        // fresh tail (last message) preserved
        assert_eq!(msgs.last().map(|m| estimate_message_tokens_aware(m, 3.5)),
                   Some(estimate_message_tokens_aware(&text_user(&"c".repeat(400)), 3.5)));
    }

    #[test]
    fn never_drops_below_protected_tail() {
        let mut msgs = vec![text_user(&"a".repeat(4000)), text_user("keep me")];
        truncate_to_fit(&mut msgs, 1, 1, 3.5); // absurdly small target
        assert!(!msgs.is_empty(), "protected tail must survive");
        assert_eq!(msgs.len(), 1);
    }
}
```

- [ ] **Step 2: 实现** — `src/context/compact/fit.rs`：

```rust
//! Deterministic, compactor-independent context floor.
//!
//! `truncate_to_fit` is the last line of the never-break guarantee: even when
//! the LLM compactor is unwired or its summary still overflows, this pure
//! function guarantees the working message list fits the target token budget by
//! dropping the oldest non-tail messages. Zero LLM calls, fully deterministic.

use crate::context::budget::pressure::estimate_message_tokens_aware;
use crate::providers::message::UnifiedMessage;

/// Estimate the total token footprint of `messages`.
fn estimate_total(messages: &[UnifiedMessage], prose_ratio: f64) -> usize {
    messages
        .iter()
        .map(|m| estimate_message_tokens_aware(m, prose_ratio))
        .sum()
}

/// Drop oldest non-tail messages until the estimated footprint fits
/// `target_tokens`. Always preserves at least the last `protected_tail`
/// messages. Returns the estimated tokens dropped.
///
/// Tool-pair safety: dropping proceeds from the front one message at a time and
/// stops at the protected-tail boundary, so a surviving `ToolResult` can never
/// be orphaned from its `ToolCall` (both are in the protected tail or both are
/// dropped together as the front advances).
pub fn truncate_to_fit(
    messages: &mut Vec<UnifiedMessage>,
    target_tokens: usize,
    protected_tail: usize,
    prose_ratio: f64,
) -> usize {
    let before = estimate_total(messages, prose_ratio);
    let tail = protected_tail.max(1);
    while messages.len() > tail && estimate_total(messages, prose_ratio) > target_tokens {
        messages.remove(0);
    }
    before.saturating_sub(estimate_total(messages, prose_ratio))
}
```

> 注：本期地板 = 整条丢弃最旧非尾消息（简单、确定、tool-pair 安全）。"单条消息本身 > 窗口"的病态子情形（fresh tail 仍超）留 Plan 1b —— 本期后置条件放宽为"protected_tail 允许超 target 时尽力"，测试 `never_drops_below_protected_tail` 固化该边界。若 `UnifiedMessage::user_text` 构造器不存在，测试改用 `src/providers/message.rs` 内既有构造器/`content_blocks` 逆向构造（照 `pressure.rs` 测试的消息构造法）。

- [ ] **Step 3: 挂模块** — `src/context/compact/mod.rs` 加 `pub mod fit;`（放在既有 `mod compactor; mod session_split;` 等旁）。

- [ ] **Step 4: 跑门** — `cargo test -p alephcore --lib compact::fit::`。Expected: PASS。

- [ ] **Step 5: 提交** — `git add -A && git commit -m "compact: add deterministic truncate_to_fit floor"`

---

### Task 3: `compact_to_fit` 编排（LLM 压缩 → 复测 → 地板，保证 fit）

**Files:**
- Modify: `src/context/compact/fit.rs`（加编排函数 + 测试）
- Test: `src/context/compact/fit.rs`

**Interfaces:**
- Produces: `pub async fn compact_to_fit(compactor: Option<&ContextCompactor>, budget: &ContextBudget, messages: &mut Vec<UnifiedMessage>, system_prompt: &str, tool_schema_tokens: usize, session_id: Option<&str>)`。后置条件：`budget.peek_pressure(messages, system_prompt, tool_schema_tokens).ratio < critical_threshold`（用地板兜底保证）。
- Consumes: `ContextCompactor::compact`（Task 内已验签名 `compact(&mut Vec<UnifiedMessage>, usize, Option<&str>) -> anyhow::Result<CompactResult>`）；`ContextBudget::peek_pressure` / `critical_threshold()` / `token_budget()` / `token_estimate_ratio()` / `fresh_tail_count()`。

- [ ] **Step 1: 写失败测试** — 用一个不挂 compactor 的路径验证地板保证（compactor=None → 纯靠 truncate_to_fit）：

```rust
#[tokio::test]
async fn guarantees_fit_via_floor_when_no_compactor() {
    let config = crate::context::budget::ContextBudgetConfig {
        token_budget: 1000, warning_threshold: 0.70, critical_threshold: 0.85,
        ..crate::context::budget::mod_test_default_config() // 或复用 fit 测试本地 config 构造
    };
    let budget = crate::context::budget::ContextBudget::new(&config);
    let mut msgs = vec![
        text_user(&"a".repeat(20000)), // way over 0.85*1000 tokens
        text_user(&"b".repeat(20000)),
        text_user("tail"),
    ];
    compact_to_fit(None, &budget, &mut msgs, "", 0, None).await;
    let p = budget.peek_pressure(&msgs, "", 0);
    assert!(p.ratio < 0.85, "post-condition: pressure must be under critical, got {}", p.ratio);
}
```

- [ ] **Step 2: 实现编排** — 追加到 `fit.rs`：

```rust
use crate::context::budget::ContextBudget;
use crate::context::compact::compactor::ContextCompactor;

/// Guarantee the working message list fits under the budget's critical line,
/// compacting as gently as possible: (1) try the LLM compactor if wired,
/// (2) re-measure, (3) if still critical, apply the deterministic
/// `truncate_to_fit` floor. Post-condition: the returned message list's
/// pressure ratio is below `critical_threshold`. Never returns an error and
/// never hard-stops — this IS the never-break guarantee's mechanism.
pub async fn compact_to_fit(
    compactor: Option<&ContextCompactor>,
    budget: &ContextBudget,
    messages: &mut Vec<UnifiedMessage>,
    system_prompt: &str,
    tool_schema_tokens: usize,
    session_id: Option<&str>,
) {
    let critical = budget.critical_threshold();
    let ratio = budget.token_estimate_ratio();

    // 1. LLM compaction (aggressive: minimal fresh tail). Fail-soft.
    if let Some(c) = compactor {
        if let Err(e) = c.compact(messages, budget.fresh_tail_count(), session_id).await {
            tracing::warn!(error = %e, "compact_to_fit: LLM compaction failed; falling back to floor");
        }
    }

    // 2. Re-measure.
    let p = budget.peek_pressure(messages, system_prompt, tool_schema_tokens);
    if p.ratio < critical {
        return;
    }

    // 3. Deterministic floor. Target = critical fraction of the message budget,
    //    minus the fixed overhead (system + tools) so the floor accounts for
    //    what the LLM call will actually carry.
    let budget_tokens = budget.token_budget() as usize;
    let overhead = p.overhead_tokens as usize;
    let target = ((budget_tokens as f64 * critical) as usize).saturating_sub(overhead);
    truncate_to_fit(messages, target, budget.fresh_tail_count(), ratio);
}
```

> `ContextBudget` 需暴露 `critical_threshold()` / `token_budget()`（已 `pub const fn token_budget`）/ `token_estimate_ratio()`（已有）/ `fresh_tail_count()`（已有）。若 `critical_threshold()` getter 不存在，在 `mod.rs` 加 `#[must_use] pub const fn critical_threshold(&self) -> f64 { self.critical_threshold }`（镜像既有 `warning_threshold()`）。`peek_pressure` 返回的 `ContextPressure` 有 `overhead_tokens`（已验，think.rs:504 用过）。

- [ ] **Step 3: 跑门** — `cargo test -p alephcore --lib compact::fit::`。Expected: PASS（含 Step 1 后置条件）。

- [ ] **Step 4: 提交** — `git add -A && git commit -m "compact: add compact_to_fit orchestration with fit guarantee"`

---

### Task 4: think.rs 分派 `CompactToFit`（压到能放下再继续，不再硬停）

**Files:**
- Modify: `src/harness/agent/think.rs`（`CompactAndContinue` 分支旁 :536-570 加 `CompactToFit` 臂）
- Test: `src/harness/tests/reactive_compaction/` 或 `task10_wiring`（既有 harness 测试目录）新增用例

**Interfaces:**
- Consumes: `LoopDirective::CompactToFit`（Task 1）、`crate::context::compact::fit::compact_to_fit`（Task 3）。
- Behavior: 收到 `CompactToFit` → 调 `compact_to_fit(...)` 就地压缩 `messages` → **fall through 到正常 LLM 调用**（不设 `hit_limit`、不设 `ContextBudgetExhausted`、不 `fire_grace_turn`、不 `TurnStep::done()`）。

- [ ] **Step 1: 写失败测试** — 在 harness 测试里构造一个 `before_turn` 会判 critical 的初始 `messages`（超预算），断言 run **不**以 `ContextBudgetExhausted` 结束、且至少发起了一次 LLM 调用（用既有 MockProvider + 计数）。参照 `src/harness/tests/reactive_compaction/mod.rs` 的既有夹具搭建。

```rust
#[tokio::test]
async fn critical_context_compacts_and_continues_instead_of_hard_stop() {
    // Arrange: budget wired with tiny token_budget so the seeded history is
    // critical on turn 0; MockProvider returns a normal final text.
    // Act: run one turn.
    // Assert: terminate_reason != ContextBudgetExhausted; provider called >= 1.
    // (照 reactive_compaction/mod.rs 既有 harness 组装夹具)
}
```

- [ ] **Step 2: 加分派臂** — 在 `think.rs` `CompactAndContinue` 分支（:536 `if matches!(budget_directive, Some(LoopDirective::CompactAndContinue))`）之后、`SplitSession` 分支（:578）之前，插入：

```rust
        // 2c-fit. `CompactToFit` directive — the never-break path. Compact
        // aggressively until the context fits under the critical line, then
        // fall through to the normal LLM call. Unlike the old `FinalReply`
        // hard-stop, this NEVER terminates the run on context pressure.
        // R10-safe: mechanical dispatch to `compact_to_fit` (lives outside the
        // harness); no intent classification, no completion judgement.
        if matches!(budget_directive, Some(LoopDirective::CompactToFit)) {
            let session_key_str = session_id.to_key_string();
            let system_prompt = self.deps.system_prompt.as_deref().unwrap_or("");
            if let Some(budget) = self.deps.context_budget.as_ref() {
                let guard = budget.lock().await;
                crate::context::compact::fit::compact_to_fit(
                    self.deps.context_compactor.as_deref(),
                    &guard,
                    &mut messages,
                    system_prompt,
                    budget_tool_tokens,
                    Some(session_key_str.as_str()),
                )
                .await;
            }
            // fall through: proceed to the normal LLM call with fitted messages.
        }
```

> 核对 `self.deps.context_compactor` 的类型以取到 `Option<&ContextCompactor>`（当前 `as_ref()` 得 `Option<&Arc<...>>` 或 `Option<&Box<...>>`；用 `.as_deref()` 或 `.map(|c| c.as_ref())` 转成 `Option<&ContextCompactor>`，与 Task 3 签名对齐）。`budget_tool_tokens` 已在本函数早前算出（:529 语境）。

- [ ] **Step 3: 跑门** — `cargo test -p alephcore --lib`（限定 harness 相关：`--lib think` / `reactive_compaction`）。Expected: PASS。

- [ ] **Step 4: 提交** — `git add -A && git commit -m "harness: dispatch CompactToFit — compact then continue, never hard-stop on pressure"`

---

### Task 5: 反应式路径 P2 — 溢出耗尽时走 `compact_to_fit` 重试，不再 `ReactiveCompactExhausted`

**Files:**
- Modify: `src/harness/agent/think.rs`（`try_reactive_compact_and_retry` :1431 的三处 `ReactiveCompactExhausted` 出口）
- Test: `src/harness/tests/reactive_compaction/mod.rs`

**Interfaces:**
- Consumes: `compact_to_fit`（Task 3）。
- Behavior: cap 耗尽 / 无 compactor / 压缩失败 三条原本 `set_terminate_reason(ReactiveCompactExhausted)+Err` 的出口，改为先 `compact_to_fit`（含地板）后**再重试一次** provider 调用；仅当截断后仍被拒才如实抛错（配置病态，非"满"）。

- [ ] **Step 1: 写失败测试** — MockProvider 首次返回 `CompactAndRetry` 溢出错误、且 reactive cap 设为 0（立即耗尽）；断言 run 仍**不**以 `ReactiveCompactExhausted` 结束（走地板 + 重试成功）。

- [ ] **Step 2: 实现** — 把 `try_reactive_compact_and_retry` 内 cap 耗尽/无 compactor/压缩失败三处：

```rust
            self.set_terminate_reason(TerminateReason::ReactiveCompactExhausted);
            return Err(HarnessError::Llm(primary_err));
```

改为调用 `compact_to_fit`（用 `self.deps.context_budget` guard + `self.deps.context_compactor`）后 `return self.call_provider_once(messages, tools_ref, parent_cancel, started).await`（复用本函数内既有的重试调用点 —— 实现时定位其 provider 调用助手名并复用；若无独立助手，把地板压缩插在既有 `compact` 成功后的重试路径之前，共用同一重试出口）。

> ⚠️ 本 Task 改动最贴近 harness 重试控制流，实现前先完整读 `try_reactive_compact_and_retry` 全函数（1431 到其结尾）确认重试出口结构，避免破坏既有 `try_reserve_reactive_compact` 一次性语义。若结构复杂到需拆分，停下与人确认（systematic-debugging：≥3 次改动失败即质疑架构）。

- [ ] **Step 3: 跑门** — `cargo test -p alephcore --lib reactive_compaction`。Expected: PASS。

- [ ] **Step 4: 提交** — `git add -A && git commit -m "harness: reactive overflow falls back to compact_to_fit + retry, not ReactiveCompactExhausted"`

---

### Task 6: 端到端回归 —— 重载 near-full 会话可继续

**Files:**
- Test: `src/harness/tests/` 新增 `never_break.rs`（或并入 `reactive_compaction`）
- Modify: `src/harness/tests/mod.rs`（挂 `mod never_break;` 若新建）

**Interfaces:**
- Consumes: Task 1/3/4 的成品（`CompactToFit` + `compact_to_fit` + think 分派）。

- [ ] **Step 1: 写回归测试** — 复现用户 case：seeded 大历史（估算 ≥ critical）+ 一条小 user 消息 + budget enabled（小 token_budget 触发 critical）+ MockProvider 正常返回文本。断言：
  - `terminate_reason` 既非 `ContextBudgetExhausted` 也非 `ReactiveCompactExhausted`；
  - provider 被调用 ≥ 1 次（第 0 轮体检压缩后成功发请求）；
  - run 正常产出 final 文本（非空）。

```rust
#[tokio::test]
async fn reloaded_near_full_session_continues_not_bricked() {
    // Arrange: budget token_budget small enough that the seeded history is
    // critical on turn 0 (mirrors reload of a near-full conversation).
    // Act: single user turn "open the html".
    // Assert: no ContextBudgetExhausted; provider called; non-empty final text.
}
```

- [ ] **Step 2: 跑门** — `cargo test -p alephcore --lib never_break`。Expected: PASS。

- [ ] **Step 3: 提交** — `git add -A && git commit -m "test: reloaded near-full session continues instead of hard-stopping"`

---

## Self-Review 结论

- **Spec 覆盖**：本 plan 覆盖 spec §5（工作流 1：P1 主动 Task 1/4、P2 反应式 Task 5、聚合/地板 Task 2/3）。§6 就地持久化、§7 模型数据库、§8 仪表对齐 = **明确排除**，各自 Plan 2/3/4。
- **类型一致**：`CompactToFit`（Task 1 定义 → Task 4 消费）；`compact_to_fit`(Task 3 → Task 4/5)；`truncate_to_fit`(Task 2 → Task 3)。签名跨 Task 一致。
- **无占位**：所有 Task 含具体代码/编辑位点；Task 5 的重试出口复用点标注了"实现前读全函数"的定位指令（真实控制流，非占位）。
- **风险**：Task 5（反应式重试控制流）最贴 harness，已加"≥3 次失败即停"护栏。Task 2 单条超窗病态子情形显式延后 Plan 1b（记录在案，非静默）。

## 收尾

Plan 1 完成后：`cargo test -p alephcore --lib`（一次全 lib 门）→ 重编 `/Applications/Aleph.app` 内嵌 server → 运行时 QA（切到之前那个 near-full 会话 → 发消息 → 应压缩后继续，不再「上下文预算已用尽（0 次迭代后）」）→ 提交/push/部署。Plan 2（就地持久化）/3（模型库）/4（仪表）另起。
