# src/harness/ — 薄 Harness 护栏 (R10 本地红线)

> 本文是根 `CLAUDE.md` R10 的本地强化，编辑本目录前必读。完整哲学见
> [HARNESS_PHILOSOPHY.md](../../docs/reference/HARNESS_PHILOSOPHY.md)。

## 硬边界：12 文件 / ~4900 行

- 顶层 (8)：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs`
- `agent/` 子目录 (4)：`think.rs` / `act.rs` / `guardrails.rs` / `prompt.rs`

**新增文件须在 PR 描述说明为何无法装进现有 12 个文件之一。**

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
