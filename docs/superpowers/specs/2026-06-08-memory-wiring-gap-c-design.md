# 记忆模块连线修复 + 熵减（增量 Gap C）

> Spec date: 2026-06-08
> Scope: surgical wiring fixes + dead-code removal inside `src/memory/`
> Branch: `fix/memory-wiring-gap-c` (worktree, off `main`)

## 1. 背景

对照参考项目 mem0 / memU / MemOS 做 Gap Analysis 后，确认 Aleph 记忆系统在
*持久化范式、事件溯源、离线整理、插件化* 上已超越三者。真正可借鉴的能力缺口收敛为三块：

- **Gap A** 自动结构化实体图谱（借鉴 mem0）— 独立 spec
- **Gap B** capture 时冲突决策 ADD/UPDATE/MERGE/NOOP（借鉴 mem0）— 独立 spec
- **Gap C** 错误修复 + 死代码/连线审计 — **本 spec**

经 5 路并行只读审计 + 二次人工核实，本增量锁定 6 项 `src/memory/` 内部、低风险、
高置信的连线/熵减修复。审计中被证伪或属故意设计的项（session_reflection 默认关、
workflow_proposal 仅 Synthesize、note_review Approve 桩、reembed 手动触发、
PostCompressionHook、NoteMetadataUpdated 保留位、X1 扩展系统）均**不在范围内**，
理由见 §6。

## 2. 设计原则

- **连线优先**：W1/W2/S1 接活已存在但断线的实现，不新写功能。
- **熵减**：D1/D2/D3 + S1 删除并行死抽象。Git 是时光机，废弃代码直接删。
- **非破坏性**：配置语义只增不减；默认值保证字节级回归（旧行为不变）。
- **R10 薄 Harness / R3 核心轻量**：不给笨循环加认知层，不引入 per-turn 额外成本。

## 3. 范围（6 项）

### W1 — `rrf_k` 配置接线 `[HIGH]`

- **现状**：`store/sqlite/notes.rs:671` 硬编码 `let k = 60.0_f32`；`config.rrf_k`
  在 `config/types/memory/mod.rs:75` 定义并解析，但全仓 **0 处读取**。文档
  `MEMORY_SYSTEM.md §10/§13` 把它列为可调旋钮——用户调了不生效。
- **改法**：`SqliteMemoryBackend` 构造时从 `MemoryConfig` 接收
  `RetrievalTuning { rrf_k: u32, bm25_bonus_weight: f32 }` 字段；
  `hybrid_search_notes` 读 `self.tuning.rrf_k` 替代字面量。
- **兼容性**：默认 `rrf_k = 60` → 行为不变。

### W2 — `bm25_bonus_weight` 接线 `[MEDIUM]`

- **现状**：`hybrid_search_notes` 融合纯 RRF，FTS 命中只贡献 RRF 名次；
  `bm25_bonus_weight`（`mod.rs:77`，默认 0.15）**0 处读取**。
- **改法**：融合循环里 FTS 命中项的 RRF 贡献乘 `(1.0 + bm25_bonus_weight)`，
  让词法匹配获得可调提升（文档承诺的 "extra BM25 lift in fusion"）。复用 W1 的
  `RetrievalTuning`。
- **兼容性**：默认 0.15 是新的有效行为；若需严格字节回归可将默认设 0.0，但文档
  既已宣称 0.15 为默认，保持 0.15 即"修复旋钮使其符合文档"。

### S1 — 信号驱动压缩接线 + 删并行死抽象 `[HIGH]`

**3a. 接线（连线优先）**

- **现状**：`gateway/execution_engine/execute.rs:511` 调
  `record_turn_and_check()`（非阻塞、不带消息）。`check_and_compress_with_signal(msg)`
  （`compression/service.rs:439`）已**完整实现**关键词信号 → Immediate/Deferred/Batch
  优先级压缩，但生产 **0 caller**（仅测试）。
- **改法**：call site 改为传 `request.input` 调 `check_and_compress_with_signal`，
  **spawn 化**（保持 turn 非阻塞，对齐 §480-508 既有 spawn 模式）。
- **必修的计数缺口**：`check_and_compress_with_signal` 的 no-signal fallback 分支
  当前不 `increment_turns()`，直接 swap 会让 turn-threshold 压缩失效。重构语义为：
  ```
  每轮: record_turn()                    // 必计, 同步
  spawn:
    detect() 关键词信号
      Immediate → compress()             // 立即, 内部已 fire PostCompressionHook
      其余      → check_and_compress()    // scheduler 判 turn/idle 阈值
  ```
  保留 `record_turn_and_check` 的 "exactly-once threshold-crossing" 原子语义
  （`fetch_add` 返回值判定），避免重复触发。
- **效果**：用户说"记住 / 记一下 / 不对 / 完成"等信号触发即时压缩——mem0 式
  capture 响应，契合 R5「AI 主动到达」。

**3b. 熵减（接线路径未触碰的并行死抽象，删除）**

`check_and_compress_with_signal` 只用 `SignalDetector::detect()`（关键词）。以下从未
被接线路径使用，且接线它们会违背 R3/R10，故删除：

- `signal_detector.rs`: `detect_with_context` / `detect_context_switch` /
  `CompressionSignal::ContextSwitch`（含 projector/match 中对应臂）——基于**逐轮
  embedding 距离**的话题切换检测，接线需每轮算 embedding（成本），当前 0 caller。
- 整个 `compression/trigger.rs`（`HybridTrigger` / `TriggerReason` / `TriggerConfig`
  / `CompressionAggressiveness`）+ `mod.rs` re-export——token-window 触发模型，与现有
  scheduler（turn/idle）**竞争重复**，从未实例化。

> **已确认决策**（用户审阅通过）：删除 `ContextSwitch` 与 `trigger.rs`。若未来需要
> 话题切换触发，作为独立功能立项（需逐轮 embedding + prev-embedding 状态）。

### D1 — 删死字段 `memory_store` `[LOW, 熵减]`

- `events/handler.rs:26` 的 `memory_store: Option<MemoryBackend>` 仅 `new()` 赋值、
  `self.memory_store` **0 处读取**（原注释 "reserved, not yet consumed"）。
- 删字段 + 删 `new()` 该参数 + 更新 2 处生产调用点
  （`bin/aleph-server/commands/start/builder/handlers/memory.rs:46`、
  `executor/builtin_registry/builder/constructor.rs`）去掉实参。

### D2 — 删孤儿 `resolve_wikilink` `[LOW, 熵减]`

- `notes/wikilink.rs` 的 `resolve_wikilink()` 仅测试调用。索引期 `notes_links` 表 +
  Ripple 多跳已取代运行期解析。
- 删函数 + `notes/mod.rs:28` re-export（保留 `extract/rewrite/remove`）+ 其测试。

### D3 — 删死字段 `RerankResponse::reasoning` `[LOW, 熵减]`

- `assembler/rerank.rs:82` 的 `reasoning: Option<String>` 解析后从不读（已挂
  `#[allow(dead_code)]`）。删字段 + 该 allow 标注。

## 4. 数据流（S1 接线后）

```
gateway turn 结束 (execute.rs:511)
  ├─ record_turn()                    [同步, 每轮必计]
  └─ spawn: check_and_compress_with_signal(user_msg)
        ├─ detect() 关键词信号
        ├─ Immediate → compress()     [立即, 内部 fire PostCompressionHook]
        └─ 无/低信号 → check_and_compress() → scheduler 判 turn/idle 阈值
```

## 5. 测试策略

- **W1/W2**：单测——同一候选集，不同 `rrf_k` / `bm25_bonus_weight` 产出不同排序；
  默认值（60 / 0.15）回归。
- **S1**：单测——Immediate 信号 → `compress()` 被调；no-signal → turn 计数仍增、
  阈值跨越仍触发（覆盖修复的计数缺口）；删除项在编译期消失（无 caller 回归 = 编译器
  强制证明）。
- **D1/D2/D3**：`cargo check` 全绿即证无遗漏 caller（编译器强制）。

> **项目协议约束**：按用户「资源并发治理」要求，**完成任务后不进行 cargo check /
> 测试校验，直接提交**。本 spec 仍列出测试期望作为正确性参照；是否实跑由实施阶段
> 依协议决定。

## 6. 明确不做（本增量外，附理由）

| 项 | 理由 |
|---|---|
| **X1** 扩展系统补完（3 dispatch 点 + UnboundMcpCaller 绑定 / Task 11） | 体量大、触碰 plugin/MCP 层、风险高，独立 spec |
| **context-switch 触发** | 需逐轮 embedding + 状态，独立功能（违本增量"surgical"） |
| **C1** PostCompressionHook fire-and-forget | 非 bug：`compress()` 内部已在所有成功路径 fire hook（service.rs:192-195） |
| **NoteMetadataUpdated emitter** | 已被 projector 消费但无 emitter；补 emitter 是"事件溯源 rename 路径"的功能，非连线。保留为 reserved |
| session_reflection 默认关 / workflow_proposal 仅 Synthesize / note_review Approve 桩 / reembed 手动 | 审计确认均为故意设计且有文档，非 bug |
| **Gap A** 实体图谱 / **Gap B** capture 冲突决策 | 各自独立 spec |

## 7. 安全重构守则

- **分支隔离**：全程 worktree 分支 `fix/memory-wiring-gap-c`，不直接触碰 main。
- **非破坏性**：W1/W2 默认值保证向后兼容；D1/D2/D3 删除项已证无 live caller。
- **熵减**：S1-3b + D1/D2/D3 同步清理过期代码，不留"为未来留口"。

## 8. 涉及文件清单

| 文件 | 改动 |
|---|---|
| `src/memory/store/sqlite/notes.rs` | W1/W2: `hybrid_search_notes` 读 `RetrievalTuning` |
| `src/memory/store/sqlite/mod.rs` (或构造处) | W1/W2: 新增 `RetrievalTuning` 字段 + 构造注入 |
| `src/gateway/execution_engine/execute.rs` | S1-3a: call site 改 `check_and_compress_with_signal` (spawn) |
| `src/memory/compression/service.rs` | S1-3a: `check_and_compress_with_signal` 计数缺口重构 |
| `src/memory/compression/signal_detector.rs` | S1-3b: 删 `detect_with_context`/`detect_context_switch`/`ContextSwitch` |
| `src/memory/compression/trigger.rs` | S1-3b: **整文件删除** |
| `src/memory/compression/mod.rs` | S1-3b: 删 trigger re-export |
| `src/memory/events/handler.rs` | D1: 删 `memory_store` 字段 + new() 参数 |
| `src/bin/aleph-server/.../handlers/memory.rs` + `executor/builtin_registry/builder/constructor.rs` | D1: 调用点去实参 |
| `src/memory/notes/wikilink.rs` + `src/memory/notes/mod.rs` | D2: 删 `resolve_wikilink` + re-export |
| `src/memory/assembler/rerank.rs` | D3: 删 `reasoning` 字段 |
