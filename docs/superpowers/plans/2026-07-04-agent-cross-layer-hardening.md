# Agent 跨层收口 + R10 归位 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 收口跨层 token 估算双源、把 harness 归位回 R10 预算、修复事件流不变量、验证并按需补齐 recall 压缩边界与 hermes 三项 delta。

**Architecture:** 五个互相独立的工作流（可乱序、单独回滚）：token 单源收口（改 4 个散源文件指向 `pressure.rs`）、harness 减重（nudge 文案与压缩派发下沉到 thinker/context 层）、事件发射降级一致化与 seq 单调性、recall 瞬态压缩边界、hermes delta 验证（persist-before-execute / 防抖 / verify-on-stop）。验证优先：所有「待验证」项先确认再动手，验证不成立就记录关闭。

**Tech Stack:** Rust (tokio + serde)，测试 `#[cfg(test)]` + `src/harness/tests/` 集成测试。

**Spec:** `docs/superpowers/specs/2026-07-04-agent-cross-layer-hardening-design.md`

## Global Constraints

- **R10 薄 harness**：`src/harness/` 限 12 文件 / ~4900 生产行；循环内不做意图分类/工具过滤/完成度判断/内容审查/错误恢复策略选择。新增文件必须在 commit message 说明为何无法装进现有 12 文件（本计划所有新文件都在 harness **外**）。
- **R7/R9**：语义判断交 LLM；prompt 文案属认知，不住在 harness。
- **cargo 节制（用户约定）**：每个任务用 `cargo check -p alephcore` 定点验证；跑测试用 `cargo test -p alephcore --lib <精确过滤器>`，不跑全量套件。
- **提交规范**：English commit messages，格式 `<scope>: <description>`，直接提交 main 单分支。
- **外科手术**：diff 最小化；「搬移」类任务只搬不改逻辑；不顺手改无关代码。
- Rust 工具链若 PATH 上没有 cargo：直接用 `~/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin/cargo`。

---

## Workflow 1 — token 估算单源收口

> 单源 = `src/context/budget/pressure.rs` 的 `estimate_tokens_smart(content)` / `estimate_tokens_aware(content, prose_ratio)` / `DEFAULT_PROSE_RATIO`（内容感知：CJK 1.5 / 代码 2.5 / 英文散文 3.5 按比例混合）。
> 已确认 `src/context/compact/compactor.rs:781` **已是** `estimate_tokens_smart` 的薄别名，无需改动。真正的散源是下面四处。

### Task 1: `summary_source.rs` 私有估算器收口

**Files:**
- Modify: `src/memory/session_compactor/summary_source.rs:144-147`（私有 `fn estimate_tokens` = 平坦 `chars/3.5`，对 CJK 低估 token）

**Interfaces:**
- Consumes: `crate::context::budget::pressure::estimate_tokens_smart(content: &str) -> usize`
- Produces: 无新接口（内部收口）

- [ ] **Step 1: 写行为对照测试（先失败）**

在 `summary_source.rs` 的 `#[cfg(test)] mod tests` 中追加：

```rust
#[test]
fn estimate_tokens_is_cjk_aware() {
    // 纯 CJK：内容感知估算按 ~1.5 chars/token 计，token 数应显著高于
    // 平坦 3.5 的旧估算。50 个汉字 → 智能估算 ≈ 34，旧平坦估算 ≈ 15。
    let cjk = "记".repeat(50);
    let tokens = estimate_tokens(&cjk);
    assert!(
        tokens >= 30,
        "CJK text must be charged at the dense ratio, got {tokens}"
    );
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib session_compactor::summary_source::tests::estimate_tokens_is_cjk_aware`
Expected: FAIL（旧平坦 3.5 算出 ~15 < 30）

- [ ] **Step 3: 收口实现**

把 `summary_source.rs:144-147` 的：

```rust
/// Estimate token count using the 3.5 chars/token heuristic.
fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() as f64 / 3.5).ceil() as usize
}
```

替换为（镜像 `compactor.rs:781` 已有的薄别名模式）：

```rust
/// Estimate token count using content-aware ratio detection.
///
/// Thin alias for [`crate::context::budget::pressure::estimate_tokens_smart`]
/// — the single source of truth for the prose-anchored, CJK/code-aware
/// char→token estimate. Kept as a local name so call sites read clearly.
fn estimate_tokens(text: &str) -> usize {
    crate::context::budget::pressure::estimate_tokens_smart(text)
}
```

- [ ] **Step 4: 运行测试确认通过 + 既有测试回归**

Run: `cargo test -p alephcore --lib session_compactor::summary_source`
Expected: 全 PASS。若本文件既有测试对旧平坦估算值有硬断言，按内容感知的新值更新断言（这是本任务的**预期行为变化**，逐个核对新值合理后更新）。

- [ ] **Step 5: Commit**

```bash
git add src/memory/session_compactor/summary_source.rs
git commit -m "memory: route summary_source token estimate to the content-aware single source"
```

### Task 2: `context_window.rs` ratio 参数版收口

**Files:**
- Modify: `src/memory/session_compactor/context_window.rs:17-31`（`pub fn estimate_tokens(content, ratio)` 平坦 `chars/ratio`）

**Interfaces:**
- Consumes: `pressure::estimate_tokens_aware(content: &str, prose_ratio: f64) -> usize`
- Produces: `estimate_tokens(content: &str, ratio: f64) -> usize` 签名不变（兼容壳），内部转发单源

- [ ] **Step 1: 写行为对照测试（先失败）**

在 `context_window.rs` 的 `#[cfg(test)] mod tests` 中追加：

```rust
#[test]
fn estimate_tokens_charges_cjk_at_dense_ratio() {
    // ratio 参数仍是散文锚点；CJK 内容由单源以 ~1.5 chars/token 计。
    let cjk = "忆".repeat(50);
    let tokens = estimate_tokens(&cjk, 3.5);
    assert!(tokens >= 30, "expected dense CJK charge, got {tokens}");
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore --lib session_compactor::context_window::tests::estimate_tokens_charges_cjk_at_dense_ratio`
Expected: FAIL（平坦 chars/3.5 ≈ 15）

- [ ] **Step 3: 收口实现**

`context_window.rs:17` 附近改为：

```rust
/// Estimate token count for `content`, treating `ratio` as the prose
/// (chars-per-token) anchor. Thin forwarding shell over
/// [`crate::context::budget::pressure::estimate_tokens_aware`] — the single
/// source of truth — so CJK / code content is charged at its denser ratio
/// while the caller-supplied anchor still governs ordinary prose.
pub fn estimate_tokens(content: &str, ratio: f64) -> usize {
    crate::context::budget::pressure::estimate_tokens_aware(content, ratio)
}
```

注意：`estimate_tokens_aware` 对 `ratio <= 0.0` 返回 0，与本文件既有 `test_estimate_tokens_zero_ratio_returns_zero` 契约一致，勿改该测试。

- [ ] **Step 4: 回归本文件测试并更新硬断言**

Run: `cargo test -p alephcore --lib session_compactor::context_window`
Expected: 全 PASS。`test_estimate_tokens_english`（纯英文，ratio 3.5）应不变；`test_estimate_tokens_counts_chars_not_bytes` 用 CJK + `ratio 1.0` 断言 5——单源下纯 CJK 收敛到 CJK_RATIO=1.5 而非 1.0，**该断言会变**：把期望改为 `(5.0/1.5).ceil() = 4`，并把测试名/注释改为说明「CJK 按密集 ratio 计，且按字符不按字节」。同步核对 `estimate_total_tokens` 的两个测试。

- [ ] **Step 5: 查找并核对本函数所有调用者**

Run: `grep -rn "context_window::estimate\|estimate_total_tokens" src/ --include="*.rs" | grep -v "#\[cfg(test)\]"`
对每个调用点确认：传入的 ratio 语义是「散文锚点」（是——签名语义未变），无需改调用方。把调用点清单记录在 commit message body。

- [ ] **Step 6: Commit**

```bash
git add src/memory/session_compactor/context_window.rs
git commit -m "memory: forward context_window token estimate to the content-aware single source"
```

### Task 3: `thinker/cache.rs` 内联 `/4` 收口

**Files:**
- Modify: `src/thinker/cache.rs:110`（`let estimated_tokens = (system_prompt.chars().count() / 4) as u64;`）

- [ ] **Step 1: 读上下文确认用途**

Run: `sed -n '95,125p' src/thinker/cache.rs`
确认 `estimated_tokens` 的消费者（缓存统计/阈值判断），记录其是否影响缓存命中决策。

- [ ] **Step 2: 收口实现**

```rust
let estimated_tokens =
    crate::context::budget::pressure::estimate_tokens_smart(system_prompt) as u64;
```

- [ ] **Step 3: 定点回归**

Run: `cargo test -p alephcore --lib thinker::cache`
Expected: PASS（若有对 `/4` 值的硬断言测试，按新值更新并在断言旁注释单源出处）。

- [ ] **Step 4: Commit**

```bash
git add src/thinker/cache.rs
git commit -m "thinker: route prompt cache token estimate to the content-aware single source"
```

### Task 4: `prompt_budget.rs` 窗口换算 ratio 单源

**Files:**
- Modify: `src/thinker/prompt_budget.rs:8-20`（`CHARS_PER_TOKEN_ESTIMATE`、`estimate_tokens`）、`:49-56`（`window_char_budget`）、`:341+`（快照测试）

**Interfaces:**
- Consumes: `pressure::DEFAULT_PROSE_RATIO: f64`（= 3.5）、`pressure::estimate_tokens_smart`
- Produces: `window_char_budget(window_tokens, fraction, floor, ceil) -> usize` 签名不变，换算常数改单源

**行为变化（spec 已批准）**：tokens→chars widening 从 ×4 改为 ×3.5。200k 窗口:`200_000×0.10×3.5 = 70_000`，被 floor `DEFAULT_PROMPT_CHARS=80_000` 钳住——**不变**；1M 窗口:`400_000 → 350_000` 字符——收紧 12.5%。

- [ ] **Step 1: 更新快照测试（先失败）**

`prompt_budget.rs` 测试模块中，找到 `from_context_window(1_000_000)` 断言（约 :366，现值 `400_000`），改为：

```rust
assert_eq!(
    TokenBudget::from_context_window(1_000_000).max_total_chars,
    350_000 // 100k tokens × 3.5 chars/token (single-source prose ratio)
);
```

`200_000`/`8_000`/`0` 三个断言不变（都被 floor 钳住，本身就是回归护栏）。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore --lib thinker::prompt_budget`
Expected: 1M 断言 FAIL（当前实现给 400_000）

- [ ] **Step 3: 实现**

`window_char_budget`（:49-56）改为：

```rust
#[must_use]
pub fn window_char_budget(window_tokens: u64, fraction: f64, floor: usize, ceil: usize) -> usize {
    // Lossy cast is intentional: a window beyond usize::MAX is implausible and
    // the subsequent clamp keeps the result bounded regardless.
    // tokens → chars via the crate-wide prose ratio (single source in
    // `context::budget::pressure`), replacing the drifted local `/4` constant.
    let scaled_chars = window_tokens as f64
        * fraction
        * crate::context::budget::pressure::DEFAULT_PROSE_RATIO;
    (scaled_chars as usize).clamp(floor, ceil)
}
```

顶部 `CHARS_PER_TOKEN_ESTIMATE`（:13）与 `estimate_tokens`（:18）的处理：

Run: `grep -rn "CHARS_PER_TOKEN_ESTIMATE\|prompt_budget::estimate_tokens" src/ --include="*.rs" | grep -v "cfg(test)"`

按路由规则处理每个调用点：手头**有文本**的 → 改调 `pressure::estimate_tokens_smart(text)`；手头**只有字符数**的（无文本可感知）→ 保留调用但把 `CHARS_PER_TOKEN_ESTIMATE` 的 doc 注释改为「**遗留报告用近似**，新代码一律用 `pressure::estimate_tokens_smart`；此常数不再参与预算换算」。若清理后两者零调用者则直接删除（YAGNI）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore --lib thinker::prompt_budget`
Expected: 全 PASS

- [ ] **Step 5: 编译面回归**

Run: `cargo check -p alephcore`
Expected: 零 error（`window_char_budget` 的其他调用者——identity/extra-file caps——签名未变自动继承新 ratio，这是有意的同源统一）

- [ ] **Step 6: Commit**

```bash
git add src/thinker/prompt_budget.rs
git commit -m "thinker: derive prompt window char budget from the single-source prose ratio"
```

**已知残留（如实记录，不在本任务修）**：静态换算无法内容感知——CJK 系统提示仍可能超 10% 窗口设计意图；真正的溢出防线是 context 层 `before_turn` 的内容感知总量度量（含 system_prompt）。此残留写入 Task 16 的 FEATURE_LOCATOR §1.2 更新。

---

## Workflow 2 — R10 归位：harness 减重

### Task 5: grace nudge 文案下沉 `src/thinker/nudges.rs`

**Files:**
- Create: `src/thinker/nudges.rs`
- Modify: `src/thinker/mod.rs`（挂新模块）、`src/harness/agent/think.rs:49-112`（删除 6 条 `GRACE_NUDGE_*` const）+ `:181-186`（`GraceReason::nudge()` 改引用新路径）、`src/harness/agent.rs:56-60`（`SOFT_FAILURE_WARNING` 同法下沉）

**Interfaces:**
- Produces: `crate::thinker::nudges::{GRACE_NUDGE_DIMINISHING, GRACE_NUDGE_MAX_ITERATIONS, GRACE_NUDGE_VERIFIER_VETO, GRACE_NUDGE_FAILURE_CAP, GRACE_NUDGE_TOOL_LOOP_HALT, GRACE_NUDGE_TIMEOUT, SOFT_FAILURE_WARNING}`（全部 `pub const &str`）
- 注意：`MAX_STEPS_HINT`（think.rs:129）**不动**——spec 范围只含 6 条 grace nudge + SOFT_FAILURE_WARNING（外科手术）。

- [ ] **Step 1: 创建 `src/thinker/nudges.rs`**

新文件内容 = 从 think.rs:49-112 **原样剪切**的 6 条 const（连同各自的 doc 注释，一字不改）+ agent.rs:56 的 `SOFT_FAILURE_WARNING`（原样），所有 const 前加 `pub`，文件头加：

```rust
//! Harness rescue-turn nudge copy (R9: intelligence lives in the prompt).
//!
//! These are model-facing prompt strings consumed by the dumb loop's grace /
//! salvage paths (`src/harness/agent/think.rs`, `src/harness/agent.rs`). They
//! live in the thinker layer — NOT the harness — because prompt copy is
//! cognition, and the harness is scaffolding only (R10). Editing the wording
//! here changes model behaviour on rescue turns; it never changes loop
//! control flow.
```

- [ ] **Step 2: 挂模块 + 改引用**

`src/thinker/mod.rs` 加 `pub mod nudges;`（按现有 mod 声明的排序位置插入）。
think.rs 删除 :49-112 的 6 条 const，`GraceReason::nudge()`（:181-186）改为：

```rust
Self::Diminishing => crate::thinker::nudges::GRACE_NUDGE_DIMINISHING,
Self::MaxIterations => crate::thinker::nudges::GRACE_NUDGE_MAX_ITERATIONS,
Self::VerifierVeto => crate::thinker::nudges::GRACE_NUDGE_VERIFIER_VETO,
Self::ConsecutiveFailureCap => crate::thinker::nudges::GRACE_NUDGE_FAILURE_CAP,
Self::ToolLoopHalt => crate::thinker::nudges::GRACE_NUDGE_TOOL_LOOP_HALT,
Self::Timeout => crate::thinker::nudges::GRACE_NUDGE_TIMEOUT,
```

（或文件头 `use crate::thinker::nudges as nudge_copy;` 后用短名——取与 think.rs 现有 import 风格一致者。）
agent.rs 同法：删 const，引用点（:687）改 `crate::thinker::nudges::SOFT_FAILURE_WARNING`。
think.rs 测试模块（:2187-2203）中对 `GRACE_NUDGE_*` 的引用同步改路径，断言内容不变。

- [ ] **Step 3: 编译 + 定点回归**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib harness::agent::think`
Expected: 零 error，grace 相关测试全 PASS（纯搬移，字节级同文案）

- [ ] **Step 4: Commit**

```bash
git add src/thinker/nudges.rs src/thinker/mod.rs src/harness/agent/think.rs src/harness/agent.rs
git commit -m "harness: move rescue-nudge prompt copy to thinker layer (R9 realignment)"
```

### Task 6: 压缩三态派发下沉 `src/context/compact/directive.rs`

**Files:**
- Create: `src/context/compact/directive.rs`
- Modify: `src/context/compact/mod.rs`（挂模块）、`src/harness/agent/think.rs:545-669`（三个 directive 分支收口为一次调用）

**Interfaces:**
- Consumes（类型全部从 `src/harness/deps.rs` 字段照抄）: `Arc<Mutex<ContextBudget>>`、`Arc<ContextCompactor>`、`session_epoch_registrar`（deps.rs:136 的确切类型）、`Arc<dyn SessionService>`、`crate::context::compact::fit::compact_to_fit`、`session_split::perform_session_split`
- Produces:

```rust
/// Outcome of applying a compaction directive to the in-flight prompt.
pub enum DirectiveOutcome {
    /// Pressure handled (or nothing to do) — caller falls through to the LLM call.
    FellThrough,
    /// Session split succeeded — caller must return Continue with this child id.
    SplitTo(SessionId),
}

pub async fn apply_budget_directive(...) -> DirectiveOutcome
```

- [ ] **Step 1: 读原块，确定参数集**

Run: `sed -n '545,669p' src/harness/agent/think.rs`
原块用到：`budget_directive`、`self.deps.context_compactor`、`self.deps.context_budget`、`self.deps.session_epoch_registrar`、`self.deps.session`、`session_id`、`&mut messages`、`&events`、`tail_start`、`system_prompt`、`budget_tool_tokens`、`self.compact_to_fit_in_place(...)`（think.rs:1796，本体已是 `context::compact::fit::compact_to_fit` 的薄委托 + `note_compaction_effect` 刷新）。

- [ ] **Step 2: 创建 `directive.rs`（搬移 + 参数化）**

新文件包含：
1. `DirectiveOutcome` enum（如上）。
2. `pub async fn compact_to_fit_and_note(budget, compactor, messages, system_prompt, tool_tokens, session_key, use_llm_compactor)` —— 把 think.rs:1796-1835 `compact_to_fit_in_place` 的**函数体**原样搬入（`self.deps.X` 改为参数；含 `note_compaction_effect` 刷新和它的解释注释）。
3. `pub async fn apply_budget_directive(directive: &LoopDirective, /* 上述参数集 */) -> DirectiveOutcome` —— think.rs:546-669 三个 `if matches!` 分支原样搬入，改写点仅限：`self.deps.X → 参数`、`self.compact_to_fit_in_place(...) → compact_to_fit_and_note(...)`、`return Ok(TurnStep{...}) → return DirectiveOutcome::SplitTo(child)`，其余逻辑（含 `note_compaction_effect` 条件、fail-soft 回退链、全部 tracing 与注释）**一字不改**。`LoopDirective` 为 `None` 或 `CompactAndContinue/CompactToFit/SplitSession` 之外时直接 `FellThrough`。

- [ ] **Step 3: think.rs 收口调用**

think.rs:546-669 整块替换为：

```rust
// 2c. Apply the compaction directive (CompactAndContinue / CompactToFit /
// SplitSession) via the context layer's single dispatch entry. Mechanical
// delegation (R10): all compaction policy lives in `context::compact`.
if let Some(directive) = budget_directive.as_ref() {
    match crate::context::compact::directive::apply_budget_directive(
        directive,
        self.deps.context_compactor.as_deref(),
        self.deps.context_budget.as_ref(),
        self.deps.session_epoch_registrar.as_ref(),
        self.deps.session.as_ref(),
        session_id,
        &mut messages,
        &events,
        tail_start,
        system_prompt,
        budget_tool_tokens,
    )
    .await
    {
        crate::context::compact::directive::DirectiveOutcome::SplitTo(child) => {
            return Ok(TurnStep {
                state: TurnState::Continue,
                executed: 0,
                vetoed: false,
                split_child: Some(child),
            });
        }
        crate::context::compact::directive::DirectiveOutcome::FellThrough => {}
    }
}
```

think.rs:1796 的 `compact_to_fit_in_place` 方法**保留为薄壳**（reactive 路径 :1651 仍在用），体内改为一行转发 `directive::compact_to_fit_and_note(...)`。参数按 Step 2 签名逐个对上（实参类型以 `cargo check` 报错为准微调 `&`/`as_ref`/`as_deref`，不改逻辑）。

- [ ] **Step 4: 编译 + 回归**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib harness && cargo test -p alephcore --lib context::compact`
Expected: 零 error；`src/harness/tests/reactive_compaction.rs` 与 driver/think 测试全 PASS（纯搬移）

- [ ] **Step 5: Commit**

```bash
git add src/context/compact/directive.rs src/context/compact/mod.rs src/harness/agent/think.rs
git commit -m "harness: sink compaction directive dispatch into context layer (R10 diet)"
```

### Task 7: agent.rs 内联测试外置

**Files:**
- Modify: `src/harness/agent.rs`（`#[cfg(test)]` 块在 :222 与 :1068 两处起始，共 ~1500 行）
- Create/Modify: `src/harness/tests/agent.rs`（若 tests/ 有 mod 声明文件则同步挂上——先看 `src/harness/tests/` 现有文件如何被引入：`grep -rn "mod tests" src/harness/mod.rs src/harness/agent.rs` 与 `ls src/harness/tests/`，镜像 `driver.rs`/`think.rs` 同款挂法）

- [ ] **Step 1: 确认测试块边界与挂载方式**

Run: `grep -n "#\[cfg(test)\]" src/harness/agent.rs && grep -rn "tests/" src/harness/mod.rs src/harness/agent/mod.rs 2>/dev/null; grep -rn "path = " src/harness/*.rs | head -5`
记录：两个测试 mod 的准确范围、tests/ 目录文件的 `#[path]`/mod 挂载模式。

- [ ] **Step 2: 剪切搬移**

把 agent.rs 两个 `#[cfg(test)] mod ...` 块整体剪切到 `src/harness/tests/agent.rs`（合并为一个文件，保留原 mod 结构），`use super::*` 按新位置改为 `use crate::harness::...`（以 `cargo check` 报错清单逐个补 import，不改测试逻辑/断言）。按 Step 1 记录的模式挂载新文件。

- [ ] **Step 3: 回归**

Run: `cargo test -p alephcore --lib harness::tests::agent 2>/dev/null || cargo test -p alephcore --lib harness`
Expected: 搬移的测试全部被发现且 PASS（数量与搬移前一致——先 `cargo test -p alephcore --lib harness 2>&1 | tail -3` 记录搬移前后测试计数对照）

- [ ] **Step 4: Commit**

```bash
git add src/harness/agent.rs src/harness/tests/agent.rs
git commit -m "harness: move agent.rs inline tests to tests/ directory convention"
```

### Task 8: 行数验收闸（测量 → 必要时二次下沉）

**Files:**
- 可能 Modify: `src/harness/agent/think.rs`（`drain_context_overflow` :1374 二次下沉，仅在测量不达标时执行）

- [ ] **Step 1: 测量生产行数（统一口径：每文件数到首个 `#[cfg(test)]` 为止）**

```bash
for f in src/harness/*.rs src/harness/agent/*.rs; do
  awk '/#\[cfg\(test\)\]/{exit} {c++} END{printf "%6d %s\n", c, FILENAME}' "$f"
done | sort -rn | awk '{s+=$1; print} END{print "TOTAL", s}'
```

Expected: TOTAL 较基线（~5267）下降 ≥150 行。**若 TOTAL ≤ 4950（~4900 红线的容差内）→ 跳到 Step 3。**

- [ ] **Step 2:（条件执行）`drain_context_overflow` 下沉**

若仍 > 4950：按 Task 6 完全相同的模式把 think.rs:1374 `drain_context_overflow` 的函数体搬到 `src/context/compact/directive.rs`（新 `pub async fn drain_context_overflow(...)`，`self.deps.X → 参数`，think.rs 保留薄壳转发；有界/幂等语义与全部注释原样）。搬移后重跑 Step 1 测量。若仍超，**停下**：把剩余差距与候选下沉项如实记入 Task 16 的文档更新，不再扩大重构（外科手术边界）。

- [ ] **Step 3: 回归 + Commit**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib harness`

```bash
git add -A src/harness/ src/context/compact/
git commit -m "harness: verify R10 line budget after diet (with measurement in message body)"
```

commit body 里贴 Step 1 的最终测量表。

---

## Workflow 3 — 事件流不变量

### Task 9: 生命周期事件降级一致化

**Files:**
- Modify: `src/gateway/event_emitter/mod.rs`（:41/:54/:68/:87/:101/:121/:138/:151/:171 九处 `let _ = self.emit(...)`）
- Modify: `src/gateway/execution_engine/execute.rs`（:254 `RunAccepted`、:1131 emitter 调用）

**分级规则（对齐 §4.5 teams 层既定策略）**：骨架事件（RunComplete/RunError/RunRetrying + execute.rs 的 RunAccepted 与 :1131）失败 → `tracing::warn!`；装饰性流事件（Reasoning/ToolStart/ToolUpdate/ToolEnd/AgentTrace/ResponseChunk）失败 → `tracing::debug!`（高频，warn 会刷屏）。

- [ ] **Step 1: 改 `event_emitter/mod.rs` 九个默认方法**

模式（以 `emit_run_complete` 为例，其余八个同构，只换事件名与级别）：

```rust
async fn emit_run_complete(&self, run_id: &str, summary: RunSummary, duration_ms: u64) {
    let seq = self.next_seq();
    if let Err(e) = self
        .emit(StreamEvent::RunComplete {
            run_id: run_id.to_string(),
            seq,
            summary,
            total_duration_ms: duration_ms,
        })
        .await
    {
        tracing::warn!(run_id, error = %e, "failed to emit RunComplete stream event");
    }
}
```

debug 级的同构写法：`tracing::debug!(run_id, error = %e, "failed to emit ToolStart stream event");`。逐一核对九处的事件名写进日志文案。

- [ ] **Step 2: 改 execute.rs 两处**

Run: `sed -n '248,260p;1125,1137p' src/gateway/execution_engine/execute.rs`
把 :254 与 :1131 的 `let _ = emitter...` 改为 `if let Err(e) = ... { tracing::warn!(...) }`，日志文案含事件名与 run/session 标识（就地取上下文可用变量）。**不动** :165（cancel best-effort）与 :523（strategy put，spec 判定有意设计）。

- [ ] **Step 3: 编译 + 定点回归**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib event_emitter`
Expected: 零 error；既有 emitter 测试 PASS（只加日志不改控制流，无行为断言变化）

- [ ] **Step 4: Commit**

```bash
git add src/gateway/event_emitter/mod.rs src/gateway/execution_engine/execute.rs
git commit -m "gateway: make lifecycle event emit failures observable (warn/debug tiers)"
```

### Task 10: seq 单调性验证 →（确认后）修复

**Files:**
- 验证: `src/gateway/event_emitter/{impls.rs,origin_fanout.rs,team_fanout.rs,instant_buffer.rs}`、消费侧 seq 排序依赖
- 可能 Modify: `origin_fanout.rs:144,178`、`team_fanout.rs`（`seq: 0` 各处 + 本地计数器）

- [ ] **Step 1: 判定 `seq: 0` 站点是生产码还是测试码**

Run: `for l in 144 178; do sed -n "$((l-12)),$((l+3))p" src/gateway/event_emitter/origin_fanout.rs; echo ...; done`
Run: `awk '/#\[cfg\(test\)\]/{print NR": TEST BOUNDARY"}' src/gateway/event_emitter/{origin_fanout.rs,team_fanout.rs}`
对照 team_fanout.rs :212/:255/:297/:337——凡落在测试边界之后的站点**排除**。

- [ ] **Step 2: 判定消费侧是否依赖 seq 排序**

Run: `grep -rn "\.seq" src/gateway/server/ src/gateway/handlers/ interfaces/webchat/src/ --include="*.rs" | grep -v test | head -20`
判据：若存在按 seq 排序/去重/断点续传的消费者，且 Step 1 有生产码 `seq: 0` 合成事件会流向它 → **确认为 bug**；否则记录「seq 仅递增标记、无排序消费者」为验证结论，跳到 Step 5 直接做文档记录（代码不动）。

- [ ] **Step 3:（确认后）写失败测试**

在 `origin_fanout.rs`（或 team_fanout.rs，按确认站点）测试模块加：

```rust
#[tokio::test]
async fn synthetic_events_carry_monotonic_seq() {
    // 构造 fan-out emitter（复用本文件既有测试的构造方式），先经正常
    // emit_* 推进内层 seq 若干次，再触发会发合成事件的路径，断言合成
    // 事件的 seq > 之前观察到的最大 seq（而非 0）。
    // 具体构造照抄本文件最近的一个 #[tokio::test] 的 harness 搭建。
}
```

Run 确认 FAIL（合成事件当前 seq==0）。

- [ ] **Step 4: 修复**

确认站点的 `seq: 0` 改为 `seq: self.next_seq()`（fan-out 装饰器的 `next_seq` 已委托 inner，天然共享单调源）；`team_fanout` 的 inner=None 本地计数器分支：若 Step 2 证明该分支有排序消费者则在 doc 注释里写明「独立序号域，仅限无 inner 的独立广播场景」，不合并计数器（避免过度设计）。重跑 Step 3 测试至 PASS。

- [ ] **Step 5: Commit（修复或验证结论二选一）**

```bash
git add src/gateway/event_emitter/
git commit -m "gateway: give fan-out synthetic events monotonic seq from the shared counter"
# 或（验证不成立时，仅在 Task 16 文档任务里记录，无代码提交）
```

---

## Workflow 4 — recall 瞬态 × 压缩边界

### Task 11: 复现测试 →（确认后）重排 recall 注入点

**Files:**
- 验证/Test: `src/harness/tests/reactive_compaction.rs`（复用其既有 harness 搭建）或 `src/context/compact/compactor.rs` 测试模块
- 可能 Modify: `src/harness/agent/think.rs:471-473`（recall push 位点）与 :506/:539（压力度量的 overhead 补偿）

- [ ] **Step 1: 写边界复现测试**

在 compactor 层写单元测试（不需要整个 harness）：

```rust
#[tokio::test]
async fn trailing_transient_message_survives_compact_to_fit() {
    // 构造一段足以触发压缩的消息历史（复用本文件既有压缩测试的
    // 消息工厂），末尾 push 一条带哨兵文本的 user 消息：
    let sentinel = "RECALL_TRANSIENT_SENTINEL_e7a1";
    // messages.push(UnifiedMessage::user(sentinel));
    // 以 fresh_tail = 0（CompactToFit 临界路径的等价配置）调用
    // compactor.compact(&mut messages, 0, Some("test-session"))。
    // 断言 1：压缩后 messages 中哨兵消息仍以原文存在（未被卷进摘要）。
    // 断言 2：任何生成的 summary 文本不包含哨兵串。
    // 具体调用形状照抄本文件最近的 compact 测试。
}
```

Run: `cargo test -p alephcore --lib compactor -- trailing_transient`
两种结果都有效：PASS = 5-2 不成立（`select_window_end`/尾部保护天然护住最后一条）→ 跳 Step 4 记录关闭；FAIL = 确认。

- [ ] **Step 2:（确认后）修复：recall 注入点后移**

think.rs 的修复形状——把 :471-473 的 recall push 从 `build_prompt` 之后**移到 2c 派发块之后**（Task 6 落地后即 `apply_budget_directive` 调用之后、grace/max-steps hint 追加之前），使压缩永远看不到瞬态消息；同时保持压力度量诚实：:506 `peek_pressure` 与 :539 `before_turn` 的 `tool_tokens` 实参改为

```rust
tool_tokens + self.deps.recall_context.as_deref()
    .map(crate::context::budget::pressure::estimate_tokens_smart)
    .unwrap_or(0)
```

（recall 计入 overhead 而非消息体——压缩决策仍感知其体积，但压缩窗口物理上碰不到它。）

- [ ] **Step 3: 回归**

Run: `cargo test -p alephcore --lib harness && cargo test -p alephcore --lib compactor`
Expected: Step 1 测试 PASS + 既有 harness/压缩测试全 PASS

- [ ] **Step 4: Commit（修复或验证结论）**

```bash
git commit -am "harness: keep transient recall context out of the compaction window"
# 或验证不成立：仅在 Task 16 文档任务记录「已验证安全」+ 保留 Step 1 测试作永久回归护栏：
git commit -am "context: regression test pinning transient-tail safety under compact-to-fit"
```

---

## Workflow 5 — hermes 三项 delta（验证优先，有缺才补）

### Task 12: tool-call 先持久化后执行 — 验证

**Files:**
- 验证: `src/harness/agent/think.rs`（assistant 轮持久化位点）、`src/harness/agent/act.rs`（执行起点）、`src/harness/agent.rs` `run()`（Think→Act 编排顺序）

- [ ] **Step 1: 定位持久化与执行的先后**

Run: `grep -n "append_event\|record_event\|session\.\|persist" src/harness/agent/think.rs | grep -vi "test\|///" | head -20`
Run: `grep -n "fn run\b" src/harness/agent.rs && sed -n "$(grep -n 'fn run\b' src/harness/agent.rs | head -1 | cut -d: -f1),+80p" src/harness/agent.rs`
判据：Think 阶段把 assistant 消息（含 tool_use 块）写入 SessionService 事件日志的调用，是否发生在 `run()` 进入 Act（工具副作用）**之前**。对照 hermes `conversation_loop.py:4506`（flush 先于执行）。

- [ ] **Step 2: 出结论**

- **已保证** → 在 Task 16 的 FEATURE_LOCATOR §3.1 增补一行：「persist-before-execute（hermes flush-before-execute parity）：assistant tool-call 轮先落事件日志再进 Act，崩溃后 resume 可见已下发的 tool_use——锚点 <file:line>」。无代码。
- **未保证** → **不要当场重排**（持久化时序牵动 orphan-pairing/streaming 多个已硬化不变量）；把证据（调用顺序、文件行号、风险面）写成一节附加到 spec 文档尾部，标记为独立后续任务。本计划内不实施。

- [ ] **Step 3: Commit（仅当产出了文档/注释变更时与 Task 16 合并提交）**

### Task 13: 压缩防抖等价性 — 验证

**Files:**
- 验证: `src/context/budget/mod.rs:521` `note_compaction_effect` + 测试 :898（`resets_breaker_when_effective`）/:926（`keeps_counting_when_ineffective`）、think.rs:558-569 调用点注释（"hermes anti-thrash"）

- [ ] **Step 1: 读并核对语义等价**

Run: `sed -n '500,560p' src/context/budget/mod.rs && sed -n '890,960p' src/context/budget/mod.rs`
核对判据（对照 hermes `should_compress` 防抖：连续两次节省 <10% → 停止压缩）：Aleph 的 breaker 是否满足「无效压缩不复位计数 → 持续无效升级为 FinalReply/终态，不会无限重压」。等价性要点是**有界**而非数值一致（10% vs breaker 阈值可不同）。

- [ ] **Step 2: 出结论**

- **等价** → Task 16 在 FEATURE_LOCATOR §2.1 增补：「压缩防抖（hermes should_compress 防抖 parity）：`note_compaction_effect` breaker——无效压缩持续计数升级 FinalReply，不无限重压；锚点 budget/mod.rs:521」。无代码。
- **不等价（存在无限重压路径）** → 写失败测试复现（构造压缩后压力不降的场景，断言第 N 轮后不再触发 compact），然后在 `note_compaction_effect`/breaker 处补有界化——纯机械计数，零语义（R10-safe）。

### Task 14: verify-on-stop 软门 — 验证 →（缺则）实现 `MutationEvidenceVerifier`

**Files:**
- 验证: `src/verification/`（现有 verifier 清单）、`grep -rn "VerifierChainBuilder\|with_verifier\|builder()" src/ --include="*.rs" | grep -v test` 找链的组装位点
- 可能 Create: `src/verification/mutation_evidence_verifier.rs`
- 可能 Modify: `src/verification/mod.rs`、链组装位点（orchestrator 层）

- [ ] **Step 1: 验证缺口**

Run: `ls src/verification/ && grep -rln "mutation\|file_write\|file_edit" src/verification/`
判据：现有 verifier（Scratchpad/StopHook/ToolLoop）是否已有「本 run 有文件变更工具成功执行、结束前无验证类动作 → 推一轮」语义。没有 → 实现；有 → Task 16 记录锚点关闭。

- [ ] **Step 2:（缺则）写失败测试**

`src/verification/mutation_evidence_verifier.rs`（新文件，测试与实现同文件）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::turn_verifier::{ToolCallSummary, TurnVerifyContext, TurnVerifier};

    fn ctx<'a>(calls: &'a [ToolCallSummary], stopping: bool) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 3,
            tool_calls_made: calls.len(),
            final_text: Some("done"),
            recent_tool_calls: calls,
            stop_reason: stopping.then_some("end_turn"),
            session_id: Some("s1"),
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        }
    }

    fn call(name: &str) -> ToolCallSummary {
        ToolCallSummary { name: name.into(), args_hash: 0 }
    }

    #[tokio::test]
    async fn vetoes_once_when_stopping_after_unverified_mutation() {
        let v = MutationEvidenceVerifier::default();
        let calls = [call("file_edit")];
        let token = tokio_util::sync::CancellationToken::new();
        // 首次：编辑后直接停 → 一次 Veto（nudge）
        assert!(v.verify(&ctx(&calls, true), &token).await.is_veto());
        // 同一 session 第二次：不再重复 nudge（一次性，nudge 非 gate）
        assert!(v.verify(&ctx(&calls, true), &token).await.is_continue());
    }

    #[tokio::test]
    async fn stays_silent_when_evidence_follows_mutation() {
        let v = MutationEvidenceVerifier::default();
        // 变更后跑了 bash（验证证据的机械代理信号）→ 不打扰
        let calls = [call("file_edit"), call("bash")];
        let token = tokio_util::sync::CancellationToken::new();
        assert!(v.verify(&ctx(&calls, true), &token).await.is_continue());
    }

    #[tokio::test]
    async fn stays_silent_mid_turn_and_without_mutations() {
        let v = MutationEvidenceVerifier::default();
        let token = tokio_util::sync::CancellationToken::new();
        let mutating = [call("file_edit")];
        assert!(v.verify(&ctx(&mutating, false), &token).await.is_continue()); // 非 stop 时不管
        let readonly = [call("file_read")];
        assert!(v.verify(&ctx(&readonly, true), &token).await.is_continue());
    }
}
```

（`CancellationToken` 的具体路径/构造照抄 `tool_loop_verifier.rs` 测试的写法；`ModelRobustnessProfile::conservative()` 若非该名字，照抄 `TurnVerifyContext` 文档注释里的默认构造。）
Run 确认编译失败（类型不存在）。

- [ ] **Step 3: 实现**

同文件实现（约 60 行）：

```rust
//! Verify-on-stop soft gate (hermes `verification_stop.py` parity, nudge form).
//!
//! Mechanical trigger only (R7-safe): the model is stopping (`end_turn`),
//! the recent tool window contains a successful file-mutation tool, and no
//! execution-evidence tool ran after the last mutation. Fires at most once
//! per session, as a `Veto` nudge — the model remains free to stop again
//! on the very next turn (nudge, NOT a gate).

use crate::sync_primitives::Arc; // 按仓内其他 verifier 的 import 风格
use std::collections::HashSet;
use std::sync::Mutex;

/// Tools whose success means "files were mutated this run".
const MUTATION_TOOLS: &[&str] = &["file_write", "file_edit", "apply_patch"];
/// Tools whose presence AFTER a mutation counts as verification evidence
/// (mechanical proxy: something was executed/observed post-edit).
const EVIDENCE_TOOLS: &[&str] = &["bash", "code_exec"];
/// Bound on the once-per-session memory (mechanical hygiene, no LRU needed:
/// entries are one small String per session; clear wholesale at capacity).
const NUDGED_SESSIONS_CAP: usize = 1024;

#[derive(Default)]
pub struct MutationEvidenceVerifier {
    nudged: Mutex<HashSet<String>>,
}

#[async_trait::async_trait]
impl crate::verification::turn_verifier::TurnVerifier for MutationEvidenceVerifier {
    fn name(&self) -> &str {
        "mutation_evidence"
    }

    async fn verify(
        &self,
        ctx: &crate::verification::turn_verifier::TurnVerifyContext<'_>,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> crate::verification::turn_verifier::VerifierVerdict {
        use crate::verification::turn_verifier::VerifierVerdict as V;
        if ctx.stop_reason != Some("end_turn") {
            return V::Continue; // only at the stop boundary
        }
        let last_mutation = ctx
            .recent_tool_calls
            .iter()
            .rposition(|c| MUTATION_TOOLS.contains(&c.name.as_str()));
        let Some(mut_idx) = last_mutation else {
            return V::Continue;
        };
        let evidence_after = ctx.recent_tool_calls[mut_idx + 1..]
            .iter()
            .any(|c| EVIDENCE_TOOLS.contains(&c.name.as_str()));
        if evidence_after {
            return V::Continue;
        }
        // Once per session: a nudge repeated every stop becomes a gate.
        if let Some(sid) = ctx.session_id {
            let mut nudged = self.nudged.lock().unwrap_or_else(|e| e.into_inner());
            if nudged.contains(sid) {
                return V::Continue;
            }
            if nudged.len() >= NUDGED_SESSIONS_CAP {
                nudged.clear();
            }
            nudged.insert(sid.to_string());
        }
        V::Veto {
            reason: "You edited files this run but nothing was executed afterwards to \
                     verify the change. Consider running a quick check (build, test, or \
                     targeted command) before finishing — or finish now if you are \
                     confident verification is unnecessary."
                .to_string(),
            class: crate::verification::turn_verifier::ErrorClass::default(), // 按枚举实际变体选最中性的一个，照抄 tool_loop_verifier 的用法
        }
    }
}
```

（`ErrorClass` 变体与 `async_trait`/token 的准确路径以 `tool_loop_verifier.rs` 为模板照抄；nudge 文案本身是模型面 prompt——若评审认为应住 `thinker::nudges`，把字符串挪过去引用，同 Task 5 模式。）

- [ ] **Step 4: 挂链**

在 Step 1 找到的 `VerifierChainBuilder` 组装位点（orchestrator 层）把 `MutationEvidenceVerifier::default()` 加入链尾（在 ToolLoopVerifier 之后——先防死循环，再谈证据）。`src/verification/mod.rs` 导出新模块。**零 harness 改动**（verifier 经 `deps.verifier_chain` 注入，R10 文件预算不涨）。

- [ ] **Step 5: 测试 + Commit**

Run: `cargo test -p alephcore --lib verification::mutation_evidence && cargo check -p alephcore`
Expected: 3 个测试 PASS

```bash
git add src/verification/ src/orchestrator/
git commit -m "verification: add verify-on-stop mutation-evidence nudge (hermes parity, nudge not gate)"
```

---

## 收尾

### Task 15: 全局定点回归

- [ ] **Step 1: 一次性中等范围回归（cargo 节制：不跑全量）**

Run: `cargo test -p alephcore --lib harness context thinker verification event_emitter 2>&1 | tail -15`
（若该过滤器形式不被接受，按 `cargo test -p alephcore --lib -- harness::`、`context::`… 分次跑，各取 tail。）
Expected: 全 PASS。任何 FAIL 回到对应任务修复后重跑。

### Task 16: 文档同步 + 发现归档

**Files:**
- Modify: `docs/reference/HARNESS_PHILOSOPHY.md:123-136`（「9 文件/~1500 行」+ 幽灵 `loop_callback.rs` → 现实：12 文件 + Task 8 实测生产行数 + 行数口径定义）
- Modify: `src/harness/CLAUDE.md`（行数预算数字与口径同步；nudge 文案与压缩派发的新家写进导览）
- Modify: `docs/reference/FEATURE_LOCATOR.md`：§1.2（prompt 预算换算单源 + Task 4 已知残留）、§2.1（Task 13 防抖结论）、§3.1（Task 5/6/8 减重记录 + Task 12 persist-before-execute 结论 + Task 14 verify-on-stop 锚点）、§4.7（Task 9/10 事件降级与 seq 结论）、§2.x recall 边界结论（Task 11）
- Memory: 更新 `~/.claude/projects/-Volumes-TBU-Workspace-Aleph/memory/`——新增本次 spec/plan 的 project 记忆条目 + MEMORY.md 索引行

- [ ] **Step 1: 按各任务实际结论逐条更新上述文档**（验证不成立的项写「已验证安全 + 回归测试锚点」，实施的项写新锚点与日期 2026-07-04）

- [ ] **Step 2: Commit**

```bash
git add docs/reference/ src/harness/CLAUDE.md
git commit -m "docs: sync harness philosophy, CLAUDE.md and feature locator after cross-layer hardening"
```
