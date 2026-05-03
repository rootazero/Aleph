---
title: Memory Evolution Roadmap (Hermes-inspired, Aleph-native)
date: 2026-04-13
status: approved
owner: @user
related_refs:
  - docs/reference/memory/NOTES.md
  - docs/reference/memory/RAW_MEMORY.md
  - docs/reference/memory/RETRIEVAL.md
  - docs/reference/memory/DREAM_DAEMON.md
---

# Memory Evolution Roadmap

> 从 hermes-agent 借鉴经验，**不是照搬**，而是补齐 Aleph 现有记忆系统的缺口并超越。
> 4 个独立 spec，按"高价值低风险优先、重构最后"排序。Spec 1 完成后继续下一个。

---

## 背景对比

### Hermes 的有效设计（值得借鉴）

1. `MemoryProvider` ABC — 可插拔后端 + 生命周期钩子
2. 完整钩子集：`prefetch` / `queue_prefetch` / `sync_turn` / `on_pre_compress` / `on_session_end` / `on_delegation` / `on_memory_write`
3. `reflect` 操作 — 跨记忆综合推理输出答案（vs `recall` 只返回条目）
4. 上下文围栏 `<memory-context>...</memory-context>` 防止召回混入用户输入
5. 三种内存模式：`context` / `tools` / `hybrid`
6. Per-context 写入规则（子代理/cron/flush 跳过）

### Aleph 已有优势（不要重复造）

- ✅ LLM 抽取 (`CompressionService` + `notes/extractor.rs`)
- ✅ 混合检索 (FTS5 + sqlite-vec + RRF)
- ✅ 事件溯源 (`MemoryEvent` / `MemoryCommandHandler`) → 时间旅行能力
- ✅ Dream Daemon 漂移/衰减/lint
- ✅ HybridAssembler（已接入）
- ✅ Wikilinks 图 + agent/namespace 双轴隔离

### 真正的缺口（Aleph 需要补的）

| # | 缺口 | 对应 Hermes 能力 |
|---|------|------------------|
| G1 | 上下文压缩前无抽取钩子 | `on_pre_compress` |
| G2 | 子代理产出不回流父记忆 | `on_delegation` |
| G3 | 无显式会话结束钩子（只有 idle/turn） | `on_session_end` |
| G4 | 召回只能返回条目，不能综合答案 | `reflect` |
| G5 | 召回内容与用户输入无视觉/语义隔离 | `<memory-context>` fencing |
| G6 | 无注入模式配置 | memory modes (context/tools/hybrid) |
| G7 | 无可插拔外部记忆后端扩展点 | `MemoryProvider` 插件 |

---

## 4-Spec 分解

### Spec 1 (NOW): Memory Capture Hooks — 堵漏

**解决缺口**: G1, G2, G3

**问题**: 3 个边界上信息会流失
- 上下文被压缩时 → 旧消息里的事实没被抽走就被丢弃
- 子代理完成时 → 父代理无法从子代理结果里学习
- 会话真正结束时 → 仅有 idle/turn 触发，没有 session boundary 语义

**方案概要**: 加 3 个钩子到现有 `CompressionService` / Multi-agent / Session 流程，**不引入新抽象**。复用现有 LLM 抽取管线 (`compression::extractor`)，定向入 notes。

**体量**: 中
**依赖**: 无
**下一步**: 本次会话进入完整 brainstorm → design doc → plan → impl 流程

---

### Spec 2 (NEXT): Reflect / Synthesis 操作 — 能力跃迁

**解决缺口**: G4

**问题**: Aleph 现在只能"召回一堆条目"，LLM 要自己拼。Hermes 的 `reflect` 让记忆子系统直接产出**综合答案**。

**方案概要**: 在 `HybridAssembler` 之上加一层 `MemoryReflector`：
1. 复用现有检索
2. LLM 综合 prompt 生成 coherent 答案
3. 作为 `memory_query` 工具的 `mode: "reflect"` 暴露

**体量**: 中小 — 站在 assembler 肩膀上
**依赖**: Spec 1 或无（可并行，但建议 Spec 1 后做以确定综合范围）

---

### Spec 3 (LATER): Context Fencing + Memory Modes — 边界清晰化

**解决缺口**: G5, G6

**问题**:
- 召回记忆混在 system prompt / user turn 里，模型可能误解为"用户最新输入"
- 没有模式配置 — 何时自动注入 vs 何时走工具调用

**方案概要**:
- 召回内容统一包进 `<memory-context>...</memory-context>` fenced block + 反注入清理正则
- 配置 `memory.injection_mode: context | tools | hybrid`，在 prompt assembly 和 tool registry 分叉处读取
- 顺带清理现有 prompt assembly 中散落的记忆注入历史代码

**体量**: 小
**依赖**: Spec 1/2（模式选项依赖它们定型）

---

### Spec 4 (FUTURE, YAGNI-gated): Pluggable Memory Extensions

**解决缺口**: G7

**重要说明**: Aleph 已经通过 `notes / retrieval / assembler / events` 解耦好了，**不需要重造 hermes 的 MemoryProvider ABC**。真正需要插件化的是未来接入**外部云端记忆后端**（mem0/supermemory/hindsight 等）。

**方案概要**: `MemoryExtension` trait — 不替换核心，只是**额外层**（实现 `on_retrieve` / `on_retain` 两个钩子）。

**体量**: 大（涉及 config、registry、错误隔离、生命周期）
**依赖**: Spec 1（钩子语义稳定后才知道 trait 签名）
**触发条件**: **YAGNI** — 除非有具体外部后端要接，否则不做

---

## 依赖关系

```
Spec 1 (Hooks) ─────┬─────► Spec 2 (Reflect)
                    │
                    └─────► Spec 3 (Fencing/Modes) ──► Spec 4 (Extensions, if needed)
```

---

## 进度追踪

| Spec | 状态 | 设计文档 | 实施计划 | 完成日期 |
|------|------|----------|----------|----------|
| 1. Capture Hooks | ✅ shipped | [design](2026-04-13-memory-evolution-spec1-capture-hooks-design.md) | [plan](../plans/2026-04-13-memory-evolution-spec1-capture-hooks.md) | 2026-04-13 |
| 2. Reflect | ✅ shipped | [design](2026-04-13-memory-evolution-spec2-reflector-design.md) | [plan](../plans/2026-04-13-memory-evolution-spec2-reflector.md) | 2026-04-13 |
| 3. Fencing/Modes | ✅ shipped | [design](2026-04-13-memory-evolution-spec3-fencing-modes-design.md) | [plan](../plans/2026-04-13-memory-evolution-spec3-fencing-modes.md) | 2026-04-13 |
| 4. Extensions | ✅ shipped | [design](2026-04-13-memory-evolution-spec4-extensions-design.md) | [plan](../plans/2026-04-13-memory-evolution-spec4-extensions.md) | 2026-04-13 |

### Follow-up Specs (post-roadmap)

Closes the remaining Hermes-vs-Aleph gaps surfaced after the 4-spec roadmap shipped.

| Spec | 状态 | 设计文档 | 实施计划 | 完成日期 |
|------|------|----------|----------|----------|
| A. Curated Hot Memory + Frozen Snapshot + `remember` tool | ✅ shipped | [design](2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md) | [plan](../plans/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot.md) | 2026-05-01 |
| B. `session_search` summarization pipeline | ✅ shipped | [design](2026-05-01-memory-evolution-spec-b-session-search-summarization-design.md) | [plan](../plans/2026-05-01-memory-evolution-spec-b-session-search-summarization.md) | 2026-05-01 |
| C. Cross-process safety beyond curated layer | ✅ shipped | [design](2026-05-02-memory-evolution-spec-c-cross-process-safety-design.md) | [plan](../plans/2026-05-02-memory-evolution-spec-c-cross-process-safety.md) | 2026-05-03 |

> 每个 spec 完成后回到本文档更新状态栏和文档链接。
> 下一个 session 若遗忘整个 roadmap，从本文件重新启动。

---

## 原则

1. **不照搬** — hermes 的 Provider ABC 是解决它单体记忆问题的，Aleph 架构已经分层好了
2. **不预构** — YAGNI 严守，Spec 4 除非有真实需求否则不做
3. **清理旧代码** — 每个 spec 完成要删掉被替代的旧路径，避免屎山
4. **Rust 优势发挥** — trait + 类型系统保证钩子契约，不走 Python 的"鸭子类型 + 运行期崩"路线
5. **LLM 主权 (R8)** — 所有"要不要抽 / 抽什么 / 如何综合"交给 LLM，代码层只负责调度 + 解析 + 写入
