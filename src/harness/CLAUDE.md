# src/harness/ — 薄 Harness 护栏 (R10 本地红线)

> 本文是根 `CLAUDE.md` R10 的本地强化，编辑本目录前必读。完整哲学见
> [HARNESS_PHILOSOPHY.md](../../docs/reference/HARNESS_PHILOSOPHY.md)。

## 硬边界：12 文件 / ~4900 行

- 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
- `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`

**新增文件须在 PR 描述说明为何无法装进现有 12 个文件之一。**

**口径**：行数按"文件开头到该文件内第一个**顶层（第 0 列）** `#[cfg(test)]` 之前"计，内联测试不计入预算（超预算就把测试搬去 `src/harness/tests/`，而不是当作行数豁免的借口）。

**「顶层」二字是本条最重要的部分**——见下方警告。**口径现在由测试执行**：`src/harness/tests/budget.rs`（跑在 `cargo test -p alephcore --lib` 里），同时守 12 文件与行数；出现第 13 个文件或行数上涨即 FAIL。**改这里的数字就得改那里的 `CEILING`，反之亦然。**

**当前测量（2026-07-14）：5994 行 — 超 ~4900 红线 1094 行。**

> ⚠️ **旧状态行「2026-07-04：TOTAL 5077 行 — 超 177 行」是错的，真实值约为 5923（超 ~1000）。** 错因不是笔误，是口径本身有歧义：`agent.rs` 在**生产 `impl` 中间**（第 215 行 / 全文 1060 行）有一个挂在 4 行测试专用取值器上的**缩进** `#[cfg(test)]`。按"第一个 `#[cfg(test)]`"的朴素读法，整个文件在第 214 行被截断，**846 行生产代码被静默排除在预算之外**——这正是 5077 与真实值之间的全部差额。旧 baseline（5267 → 5077）出自同一套读法，一并作废。
>
> 教训：**红线的状态行如果靠人手算、且规则有歧义，它迟早会说谎，而且是往好听的方向说谎。** 这就是 `budget.rs` 存在的理由。

> 唯一的自动检查曾是 `scripts/graph-audit.mjs` 的 `redline-r10` —— 它只数**文件数**（自红线写下之日起恒为 12，是唯一不会动的量），从不数行数，且未接入任何门（还需要一个生成的知识图谱产物才能跑）。

Task 8 的下一步（把 `agent/think.rs` 的 `drain_context_overflow` + `try_reactive_compact_and_retry` + `reactive_fit_and_retry` 反应式压缩救援簇下沉）仍 BLOCKED——其依赖是读写私有 harness 状态的 `&self` 方法，不是可参数化的 `self.deps.X` 字段。缺口与候选下沉项详见 [HARNESS_PHILOSOPHY.md §4.1](../../docs/reference/HARNESS_PHILOSOPHY.md) 与 `.superpowers/sdd/task-8-report.md`。

**下沉去处（新增代码先看这里，而不是塞回 harness）**：
- Nudge / 护栏文案 → `src/thinker/nudges.rs`
- 压缩指令派发（`LoopDirective` → 具体动作）→ `src/context/compact/directive.rs`

## 加代码前必答 3 问

1. 这是脚手架还是认知？认知必须搬到 prompt。
2. 模型升级一档还需要它吗？不需要就删。
3. 现在有几个真实消费者？零个就撤回。

## 循环里的 5 个"不"

1. ❌ 不判断意图分类
2. ❌ 不做工具过滤 / 相关性评分
3. ❌ 不做完成度判断（除模型显式 stop）
4. ❌ 不做内容审查 / 安全打分
5. ❌ 不做错误恢复策略选择

任何"零现有消费者"的抽象立即撤回，绝不"为未来留口"。
