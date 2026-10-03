# Aleph Context Fabric：上下文管理与压缩深度重构 — 设计 Spec

- **日期**：2026-10-01
- **分支**：`context-fabric`（会话级 worktree `.worktrees/context-fabric-2026-10-01`，完成后合并回 main 并删除）
- **分类**：architectural（Lifecycle 层新增子系统级能力）
- **参考项目**：context-mode（Admission 思想：compute outside context, retrieve into context）、billion-context（Lifecycle 思想：model-driven hierarchical compression / ACP）

---

## 1. 意图与成功标准

### 1.1 用户意图

对 Aleph 的上下文管理和压缩进行：深度架构重构和功能增强、代码逻辑细节打磨、错误修复和功能连线。吸收 context-mode（preventative：垃圾不进上下文）与 billion-context（curative：已进入的历史持续分层压缩）两个参考项目各自独有的机制，映射为 **Aleph Context Fabric = Admission 层 + Lifecycle 层** 的两层框架。

### 1.2 扫描结论（Gap Analysis 摘要）

| 维度 | Aleph 现状 | 判定 |
|---|---|---|
| 垃圾不进上下文（Admission） | ingress 清洗 + ContentKind 路由缩减器 + 中央字节守卫（FL §2.7/§3.14） | **领先** context-mode（结构化路由 > 正则启发式） |
| 大输出卸载 + 检索回读 | result_store + `[Full output persisted]` + `ctx_search`（已连线模型） | 已实现等价物 |
| 会话事件可检索记忆 | session_events 真源 + FTS + recall/session_search/记忆三支柱 | 已实现等价物 |
| 模型驱动压缩时机 | `session_compact` 工具存在但无增长步进引导 | **真实 gap（中）** |
| 分层可寻址折叠块 | running summary 单链 extend-merge，块不可寻址 | **真实 gap（中）** |
| 解压缩/区间还原 | Retire::Through 保留 FTS 可搜片段，**无法逐字还原进 prompt** | **真实 gap（核心）** |
| 折叠经济学（fold economics） | CacheMonitor + doctor cache-hit-rate，无逐次压缩回本核算 | **真实 gap（中）** |
| 压缩标记防伪（bili #717） | 压缩系统侧执行，天然免疫；无测试锁定该性质 | 补守卫测试 |

**结论**：Admission 层主要是连线与打磨；Lifecycle 层是功能增强主战场。吸收 billion-context 三个机制：**可寻址折叠块 + decompress 还原 + 折叠经济学**，外加模型驱动压缩引导哲学。

### 1.3 成功标准

1. 模型可通过 `session_decompress` 按 fold_id 把任一已折叠区间的原文逐字还原进上下文（有界、分页、预算内）。
2. 每次压缩落一条可寻址 `FoldRecorded` 记录，fold 列表可查。
3. `doctor core/fold-economics` 输出逐 fold 回本判定（惰性计算，无后台任务）。
4. 增长步进 nudge 引导模型在合适时机调用 `session_compact`；breaker 跳闸期间不 nudge。
5. 三条记录在案欠账清零：降级摘要污染跨 run 缓存（§2.1）、headroom SmartCrusher（§2.7）、tool_output 全链断线审计。
6. 全量测试通过：`cargo test -p alephcore --lib`、`just test-all`（内存受限机器用 `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`）。
7. FEATURE_LOCATOR.md 增补本轮条目（§2.21）+ 附录 E 新判据。

---

## 2. 架构总览：两层映射（不重组模块目录）

```
Aleph Context Fabric（文档框架；代码不移动目录）
Admission 层  = src/tool_output/*（ingress/hygiene/structured/result_store/ctx_search）
                + src/context/budget/cheap_passes（stale pass）
Lifecycle 层  = src/context/compact/* + budget/preflight + 前缀缓存纪律（FL §2.15/2.18）
```

现有模块已天然映射两层；「Fabric」只作为本 spec 与 FEATURE_LOCATOR 的文档框架。**明确不做**目录重组（churn 大、撞 R10 预算纪律、组织收益低）。

---

## 3. 线 1（Lifecycle 增强）——四个工作项

### 1a. Fold Registry（可寻址折叠块）

**新文件**：`src/context/compact/folds.rs`（~300 行）

**核心发现**：`SessionEvent::CompactionPerformed`（src/session/events.rs:574）已携带 `from_seq/to_seq/summary_ref`——折叠坐标已存在，registry 是把它正式化。

**设计**：
- 新增事件变体 `SessionEvent::FoldRecorded`（serde 可选字段向后兼容；events.rs:1454 有变体名守卫测试，新增变体会被它覆盖）：
  - `fold_id: String`（`fold_<ulid>` 或 `<session>-<seq>` 形式，实现时取仓内已有 id 生成模式）
  - `from_seq / to_seq: EventSeq`
  - `summary_ref: String`
  - `strategy: String`（manual / auto-llm / auto-deterministic / session-memory-reuse）
  - `trigger: String`（manual-command / pressure-escalation / preventive / model-tool）
  - `folded_tokens: u64`、`summary_tokens: u64`
  - `at: Timestamp`
- 发射点：与现有 `CompactionPerformed` **同一 emit_batch**（手动压缩 manual.rs:477 附近；自动压缩 compactor.rs 落点——自动压缩目前是瞬态摘要不改事件日志，见 §6 开放问题 O1）。
- 派生视图 `list_folds(session_key) -> Vec<FoldRecord>`：从事件日志扫描 `FoldRecorded`，**不建平行表**（单一真源原则）。
- projection（store.rs:1168）：`FoldRecorded` 归 `compaction_performed` 同类，不投影 messages 行。

**守卫测试**：fold_id 唯一性；list_folds 顺序与 seq 单调；与 CompactionPerformed 同批次原子性（要么都在要么都不在）。

### 1b. `session_decompress` 工具（逐字区间还原）

**新文件**：`src/builtin_tools/decompress.rs`（~250 行），注册进 builtin tools 表。

**入参**：
```rust
pub struct SessionDecompressArgs {
    /// 省略 = 最近一次 fold
    pub fold_id: Option<String>,
    /// 区间分页：fold 内子段
    pub from_seq: Option<u64>,
    pub to_seq: Option<u64>,
    /// 输出 token 上限，默认 read_backstop_tokens 量级
    pub max_tokens: Option<usize>,
}
```

**行为**：
1. 解析 fold_id → `(from_seq, to_seq)`（缺省取最新 `FoldRecorded`；无 fold 时返回诚实空结果 + advisory，遵守「没找到 vs 没跑」判据）。
2. 从 `session_events` 读软退休区间（`Retire::Through` 保留了原文与 FTS），按事件类型渲染为可读转录（User/Assistant/ToolResult 各归其位，渲染规则复用 recall/session_search 的既有渲染器——**实现时先连线，不新写渲染**）。
3. 超 `max_tokens`：截断 + 返回显式 continuation 坐标（`next_from_seq`），模型可翻页。

**预算路径（关键连线）**：`src/tools/result_processing.rs:210` 的 `is_read_family` 扩展为 `is_verbatim_recall_family`（或等价命名），同时覆盖 `file_read` 与 `session_decompress`。理由与 file_read 先例同构（该函数 doc 注释原文）：**"the only way back from an offloaded read is another read"**——decompress 是从折叠区间回来的方式，卸载它的结果是循环的。因此：
- 不挂卸载预算（Layer 2 不给 budget）
- 不被 ContentKind 缩减器触碰（逐字是功能本体）
- 由 `read_backstop_tokens` 兜底

**架构决策**：还原内容骑**普通工具结果**进入后续 prompt，之后被既有压缩机制自然再折叠。**不引入新的瞬态注入路径**——这是与 billion-context 的关键架构差异（它必须改消息流，我们复用工具结果通道，侵入面最小）。事件日志零改动。

**守卫测试**：
- 还原字节与事件日志原文逐字节相等（效果断言，非调用断言）
- read-family 扩展守卫：断言两个工具名都在谓词内
- 分页：跨页拼接 == 整段
- 空 fold / 非法 fold_id / 区间越界的诚实报错

### 1c. Fold Economics 账本（惰性回本判定）

**新文件**：`src/context/compact/fold_ledger.rs`（~200 行）+ doctor 检查 `core/fold-economics`。

**设计**：
- 记账随 `FoldRecorded` 落库（folded_tokens、summary_tokens；summarizer 成本由 MeteringProvider 已有通道计量，FL §2.18 两个构造点已套 `compactor:<agent>` 标签）。
- **回本判定惰性计算**（不起后台任务）：doctor 检查做 SQL join——fold 之后 N 轮（N 默认 10，可配）的 cache_read/cache_creation 增量 vs 摘要成本，输出逐 fold 净省 token、回本判定（breakeven / not-yet / never）、异常行（命中率 <85% 或单次 miss ≥5000，对标 bili LINE ITEMS 判据）。
- 健康线写入检查判据：prefix-cache 命中率 95–97%、压缩自身成本占比 ≤2%（对标 bili 健康会话指标）。
- 数据源全部现成：usage 行（cache_hit_ratio 单一源 shared/protocol/src/events.rs）、CacheMonitor 事件。

**注意口径**：`cache_hit_ratio = read/(input+read)`（disjoint 计数）是唯一合法公式，webchat 的 prefix_reuse_ratio 刻意不同——doctor 检查必须用前者。

### 1d. 模型驱动引导 + 防伪守卫

**改动**：`src/thinker/nudges.rs` + `src/builtin_tools/sessions/compact_tool.rs` DESCRIPTION + 新配置项。

**设计**：
- 新配置 `[context_budget] compress_nudge_growth_tokens`（默认 50000，对标 bili `compress.nudgeGrowthTokens`）。
  - **2026-10-02 用户翻案**：实现期曾偏离本设计把阈值落为常量（`FOLD_NUDGE_GROWTH_TOKENS`，nudges.rs doc 标注为「Spec O2 ruling」——标签松散，§6 的 O2 是 fold_id 生成格式，与此无关）。现按 R9「All configurability exposed as tools」回正为本节原设计的配置项，字段名定为 `fold_nudge_growth_tokens`（默认 50000，常量保留为默认值唯一来源）。同批修复跨 run 增长会计：新 run 基线从上次 fold 之后的状态续账（`GrowthNudgeTracker::seed_cross_run`，详见 FEATURE_LOCATOR §2.1「增长步进 fold nudge」）。
- 每轮结束（think.rs 轮尾，瞬态尾区注入点已有先例）：若 prompt token 自上次 fold 或上次 nudge 起增长 ≥ 阈值，且 CompactionCircuitBreaker 未跳闸（budget/mod.rs:232 单一源），注入 system-reminder 建议 `session_compact` 并教它写 instructions。
- fenced 常量放 nudges.rs（§2.19 纪律：分类单一源在产地；`no_fenced_const_escapes_classification` 用 include_str! 自扫，新常量自动进覆盖范围）。
- 瞬态注入 = 不落盘、不进可缓存前缀（dynamic 尾区），遵守 §2.3 前缀缓存纪律。
- `session_compact` DESCRIPTION 增补：何时该压（增长步进、任务阶段切换）、instructions 怎么写（保什么：用户目标、未决问题、文件台账）。
- **防伪守卫测试**：助手输出含压缩标记形文本（如 `[Context folded]`/`📦` 样式）时，事件日志中 `FoldRecorded`/`CompactionPerformed` 计数不变——压缩确认只认事件日志，不认助手文本（对标 bili #717；Aleph 系统侧执行天然免疫，测试锁住这个性质）。

---

## 4. 线 2（Admission 打磨 + 清账）——三个工作项

### 2a. 降级摘要污染跨 run 指纹缓存修复（FL §2.1 记录在案欠账）

**问题**：压缩摘要器失败时 truncation fallback 产物（降级摘要）仍写入 COMPACTION_CARRYOVER 跨 run 指纹缓存；两条现存回归测试断言「必须缓存」（FL 标注「待新设计」——本轮即新设计）。

**新设计**：
- 缓存键升级为 `(fingerprint, quality_tag)`，quality_tag ∈ {`full`（LLM 摘要成功）, `degraded`（truncation fallback）}。
- `degraded` 条目**可命中同 run 内复用**（保住两条回归测试的语义：同 run 不重复付摘要成本），但**不写跨 run carryover**（或写入但 TTL=本 run——实现时取最小 diff 方案）。
- accept_summary（三处侧信道调用点收敛的单一入口）负责打 quality_tag，守卫断言降级产物不出现在跨 run 缓存读路径。

**守卫测试改写**：两条现存回归测试改为断言「同 run 命中 + 跨 run 不命中」。

### 2b. headroom SmartCrusher（FL §2.7 欠账）

**问题**：ContentKind 缩减器跑完仍超预算时，当前只有均匀 head/tail 截断；缺有损行选择启发式（headroom 的 SmartCrusher 思想：按行重要性打分选择保留）。

**设计**（落在 `src/tool_output/structured/` Profile 体系内）：
- 新增 `crush_within(text, kind, budget)`：缩减器产出仍超预算时调用，按 kind 特定的行重要性启发式（error/warning/匹配行/结构行优先）选择保留行，其余以省略标记折叠。
- 遵守 E.2 判据：缩减器不判定「是否更小」（中央 `is_meaningful_shrink` 按字节裁决）；量纲进类型；形状前提用 `ContentKind::min_lines`。
- **YAGNI 闸**：只在 log/search 两 kind 上实现（diff/json 的结构敏感性使行删除风险高，先不做，spec 记录理由）。

### 2c. tool_output 全链断线审计（severed-wire-audit 技能）

**链路**：`tools/scoped/dispatch.rs:1543-1637`（result_processing）→ `hygiene::clean_result_value` → `structured/*` → `cheap_passes/tool_result_pruning.rs` → result_store/ctx_search。

**产出**：CONNECT（接回断线）/ CUT（删死代码，熵减原则）/ DECIDE（呈报用户）三分类清单；发现的每条附证据（生产者/消费者计数——「先数一遍生产者和消费者」判据）。分类判断用 Jev 辅助，人（主会话）终审。

---

## 5. 错误处理与守卫总则

- 库代码 thiserror，应用 anyhow，`?` 传播，生产代码禁止 unwrap（AGENTS.md）。
- 每条新功能带**效果断言**守卫（附录 E.1：断言效果到达，非调用发生；恒真谓词 = 没判）。
- 新增配置项进 `[context_budget]` 族，带默认值与钳位测试。
- 事件变体新增走 events.rs:1454 变体名守卫；serde 向后兼容（老事件能读，新字段 optional/default）。

## 6. 开放问题（实现时裁决，均在 spec 记录裁决结果）

- **O1**：自动压缩（compactor.rs 三策略）目前是瞬态摘要、不改事件日志——`FoldRecorded` 是否覆盖自动压缩？倾向：覆盖压力驱动 LlmSummary（它是事实上的折叠），deterministic truncation 不记（无摘要产物可寻址）。~~实现 agent 裁决并记录理由~~ **已裁决（T1，2026-10-01）：不覆盖自动压缩，compactor.rs 零改动**。代码现实推翻了倾向：(a) 自动压缩操作的是瞬态 `Vec<UnifiedMessage>`（prompt 重建时的内存消息列表），没有 SessionService 句柄、不写事件日志、不退休任何事件——`FoldRecorded` 的 `from_seq/to_seq` 语义是「被 `Retire::Through` 软退休的事件区间」，自动压缩在事件日志里没有对应坐标，落库会制造谎言（fold 声称区间已折叠，但事件仍 live，decompress/list_folds 语义随之崩坏）；(b) 为它补写路径需要给 compactor 新增事件写句柄并伪造坐标，违背「不引入新瞬态注入路径」的最小侵入架构决策（§3.1b）；(c) 自动压缩产物的跨轮可寻址性已由 COMPACTION_CARRYOVER 指纹缓存承担（T5 领域），`FoldRecorded` 只覆盖真正编辑事件日志的压缩（manual 路径）。DeterministicTruncation 不记（无摘要产物），与原倾向一致。
- **O2**：fold_id 生成格式取仓内既有 id 模式（实现时连线，不新造轮子）。
- **O3**：decompress 渲染器复用 recall/session_search 的哪个渲染路径，以实现时连线结果为准（先探索再写码）。

## 7. 非目标（明确不做）

- 不动模块目录结构（方案 C 已否决）。
- 不动 ingress 清洗管线主干。
- 不做 codex 式模型自管上下文工具（FL §2.14 判为单独提案）。
- 不引入 HTML ContentKind（FL §2.7 刻意不做，src/fetch/ 已承担）。
- 不做 pi 的 22-pattern silent-overflow 表（FL §2.14 刻意不做）。
- 不碰 `SessionManager::compact_session`（纯存储卫生，保留不动）。
- decompress 不做跨 session / 父子会话血缘链（bili derivedFrom；Aleph 已有 read_scope_keys 跨 epoch 等价物，增强留待后续提案）。

## 8. 编排计划

| 项 | 安排 |
|---|---|
| Worktree | `.worktrees/context-fabric-2026-10-01`，分支 `context-fabric`，会话级，完成合并回 main 并删除 |
| 派发机制 | `pi -p "<任务>" --model <m>` 无头子进程，bash 后台并行，各自独立 session |
| 线 1 模型 | opus/sonnet 级（1a+1b 深推理）；1c+1d 可降 sonnet/kimi |
| 线 2 模型 | kimi/minimax 级（模式执行类打磨） |
| 文件隔离 | 线 1：`src/context/compact/`、`src/builtin_tools/`、`src/thinker/nudges.rs`、`src/tools/result_processing.rs`、`src/session/events.rs`、`src/diagnostics/`；线 2：`src/tool_output/`、`src/context/budget/cheap_passes/`——**1 与 2 唯一共享文件是 `src/context/compact/compactor.rs`（2a 碰）与 `src/builtin_tools/`（1b 新增文件，2a 改缓存逻辑在不同文件），合并时注意**（见下） |
| 冲突裁决 | 2a 实际落点若在 compactor.rs 内部，则线 2 的 2a 项排到线 1 的 1a 完成之后串行，或划给线 1——派发前以代码核实为准 |
| 内存纪律 | 线 2 agent 每轮 cargo 前查 MemAvailable，<4G 等待重查 |
| 编译纪律 | 每线内部 2-3 个任务完成后合并编译，不逐任务编译 |
| 合并顺序 | 线 2 先合并回 main（低风险）；线 1 分批（1a+1b → 编译验证 → 1c+1d） |
| Jev 使用点 | 断线审计 CONNECT/CUT/DECIDE 分类、欠账优先级筛选（分类/筛选/路由类判断） |
| 收尾 | FEATURE_LOCATOR.md 新增 §2.21 条目 + 附录 E 新判据；docs/reference 同步 |

## 9. 测试策略

- 每工作项：单元测试随代码走（`cargo test -p alephcore --lib <name>`）。
- 域级：compact 域全量（`cargo test -p alephcore --lib context::compact`）、tool_output 域全量。
- 合并前：`just test-all`（内存受限用 `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib`）。
- clippy：`-D warnings` 全绿才许合并。
- prefix 稳定性：decompress 不改变 build_prompt 的持久化前缀结构（工具结果通道），prefix_stability.rs 契约测试必须保持绿。

---

*Spec 批准后将任务拆给 subagent；实现中对本 spec 的任何偏离必须回写本文件（spec 是活文档）。*
