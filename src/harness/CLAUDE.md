# src/harness/ — 薄 Harness 护栏 (R10 本地红线)

> 本文是根 `CLAUDE.md` R10 的本地强化，编辑本目录前必读。完整哲学见
> [HARNESS_PHILOSOPHY.md](../../docs/reference/HARNESS_PHILOSOPHY.md)。

## 硬边界：12 文件 / ~4900 行

- 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
- `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`

**新增文件须在 PR 描述说明为何无法装进现有 12 个文件之一。**

**口径**：行数按"文件开头到该文件内第一个 `#[cfg(test)]` 之前"计，内联测试不计入预算（超预算就把测试搬去 `src/harness/tests/`，而不是当作行数豁免的借口）。

**当前测量（2026-07-04）：TOTAL 5077 行 — 超 ~4900 红线 177 行，超 4950 容差 127 行。** Tasks 5–7 已从 baseline 5267 减到 5077；Task 8 的下一步（把 `agent/think.rs` 的 `drain_context_overflow` + `try_reactive_compact_and_retry` + `reactive_fit_and_retry` 反应式压缩救援簇下沉）被 BLOCKED——其依赖是读写私有 harness 状态的 `&self` 方法，不是可参数化的 `self.deps.X` 字段。缺口与候选下沉项详见 [HARNESS_PHILOSOPHY.md §4.1](../../docs/reference/HARNESS_PHILOSOPHY.md) 与 `.superpowers/sdd/task-8-report.md`。

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
