# Aleph Context Fabric 实施计划

> **For agentic workers:** 本计划由主会话通过 `pi -p` 无头子进程按「双线并行」派发执行。Steps 用 checkbox 追踪。执行者必读 Spec（同目录 `../specs/2026-10-01-context-fabric-design.md`）——计划从 spec 出发论证，spec 随计划同行。

**Goal:** 为 Aleph 上下文子系统补齐 Lifecycle 层三机制（可寻址折叠块 / decompress 还原 / 折叠经济学）+ 模型驱动压缩引导，并清三条记录在案的 Admission 欠账。

**Architecture:** 不重组目录。Lifecycle 工作落在 `src/context/compact/`（新 folds.rs / fold_ledger.rs）+ `src/builtin_tools/`（新 decompress.rs）；Admission 打磨落在 `src/tool_output/` 与 `src/context/compact/compactor.rs`。折叠坐标复用现有 `SessionEvent::CompactionPerformed{from_seq,to_seq}`；decompress 复用 read-family 预算先例（逐字、backstop 兜底、不卸载）；还原内容骑普通工具结果通道回 prompt，零新瞬态注入路径。

**Tech Stack:** Rust（alephcore lib）、rusqlite（session_events / state.db）、tokio、serde。

**Spec:** `docs/superpowers/specs/2026-10-01-context-fabric-design.md`

## Global Constraints

- 所有工作在 worktree `D:/Workspace/Aleph/.worktrees/context-fabric-2026-10-1`（分支 `context-fabric`）；严禁直接在 main checkout 工作。
- rustfmt 4 空格 / 100 列；clippy `-D warnings`；库 thiserror、应用 anyhow、`?` 传播、生产代码禁 unwrap。
- **本机 pre-commit hook 的 fmt 检查因 rustfmt 版本漂移对 main 也失败**：提交用 `git commit --no-verify`，但每个任务收尾必须手动跑 `cargo fmt -p alephcore -- --check` 并确认 diff 只含本任务未触碰的预存漂移（对本任务触碰的文件必须零 diff）+ `cargo clippy -p alephcore -- -D warnings`。
- 内存纪律：跑 cargo 前查可用内存（Windows: `wmic OS get FreePhysicalMemory` 或 `systeminfo`；低于 4GB 等待重查）；测试用 `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib`。
- 提交格式 `<scope>: <description>`（英文）。
- 事件日志是唯一真源；`messages` 表是投影。`Retire::Through` 保留 FTS，`Retire::From` 必须删 FTS。
- 新 SessionEvent 变体必须 serde 向后兼容（可选字段/default），并通过 events.rs:1454 的变体名守卫测试。
- 测试断言「效果到达」而非「调用发生」；恒真谓词 = 没判。
- cache_hit_ratio 唯一合法公式：`read/(input+read)`（shared/protocol/src/events.rs::cache_hit_ratio）。

## Review Focus

1. **旧会话无 FoldRecorded**：本变更前的会话只有 `CompactionPerformed`。`list_folds` 必须从 `CompactionPerformed` 派生旧 fold（fold_id 用派生形式），decompress 对旧 fold 必须可用 → Task 1/2 各有测试。
2. **fold 区间被后续 Retire::From 硬删**（clear/rewind 后）：decompress 引用已消失的 seq 区间必须诚实报错（区分「fold 不存在」与「区间已被硬退休」），不得渲染半截垃圾 → Task 2 测试。
3. **decompress 还原的文本内含压缩标记形文本**（助手历史上写过 `[Context folded]` 之类）：还原内容骑工具结果通道，不得被 `is_synthetic_reminder` 误判、不得触发任何压缩确认路径 → Task 2 测试。
4. **nudge 不得进可缓存前缀、不得落盘**：注入在瞬态尾区；prefix_stability 契约测试（turn N 是 N+1 严格前缀）必须保持绿 → Task 4 测试。
5. **fold-economics doctor 检查在空库/全新安装上不得误报**（仿 cache_hit_rate.rs 的 MIN_CALLS 模式：无 fold 时报告 informational 而非 warn）→ Task 3 测试。

---

## 线 1：Lifecycle 增强（模型：opus/sonnet 级）

### Task 1: FoldRecorded 事件 + Fold Registry

**Files:**
- Modify: `src/session/events.rs`（enum SessionEvent 加变体，~574 行 CompactionPerformed 附近；变体名守卫测试在 ~1454）
- Create: `src/context/compact/folds.rs`
- Modify: `src/context/compact/mod.rs`（挂 `pub mod folds;`）
- Modify: `src/session/store.rs`（~1168 行 projection 归类；~1211 行 event type 名映射）

**Interfaces:**
- Consumes: `SessionEvent::CompactionPerformed { from_seq, to_seq, summary_ref, at }`；`emit_batch(.., Some(Retire::Through{..}))`（manual.rs:477 的既有批次）
- Produces:
  ```rust
  // src/session/events.rs
  SessionEvent::FoldRecorded {
      fold_id: String, from_seq: EventSeq, to_seq: EventSeq,
      summary_ref: String, strategy: String, trigger: String,
      folded_tokens: u64, summary_tokens: u64, at: Timestamp,
  }
  // src/context/compact/folds.rs
  pub struct FoldRecord { pub fold_id: String, pub from_seq: EventSeq, pub to_seq: EventSeq,
      pub summary_ref: String, pub strategy: FoldStrategy, pub trigger: FoldTrigger,
      pub folded_tokens: u64, pub summary_tokens: u64, pub at: Timestamp }
  pub fn list_folds(events: &[SessionEvent]) -> Vec<FoldRecord>  // 纯函数，事件切片→fold 列表
  pub fn derive_fold_id(session_key: &str, from_seq: EventSeq) -> String  // "fold_<session_hash8>_<from_seq>"
  ```
- 旧会话兼容：`list_folds` 对只有 `CompactionPerformed` 没有 `FoldRecorded` 的区间，派生 FoldRecord（fold_id = `derive_fold_id`，strategy 标 `Legacy`，token 账目记 0）。

- [ ] **Step 1: 写失败测试**（folds.rs `#[cfg(test)]`）：`fold_recorded_roundtrip_serde`（新变体序列化/反序列化）；`list_folds_derives_legacy_from_compaction_performed`（只有 CompactionPerformed 的事件流 → 派生 fold）；`list_folds_prefers_recorded_over_derived`（两者都有时不重复）；`fold_id_deterministic`。
- [ ] **Step 2: 跑测试确认失败** `cargo test -p alephcore --lib context::compact::folds` → 编译错（模块不存在）。
- [ ] **Step 3: 实现**：events.rs 加变体（serde 字段全有默认值/或整变体新增即可，旧日志无此变体不受影响）；folds.rs 实现三个函数；store.rs projection 把 `FoldRecorded` 归到与 `CompactionPerformed` 同类（不投影 messages 行，event type 名 `"fold_recorded"`）。
- [ ] **Step 4: 跑测试确认通过** + 变体名守卫测试（events.rs:1454 的测试应自动覆盖新变体，若该测试是列举式则需把新变体加进它的表——先读后改）。
- [ ] **Step 5: 接线发射点**：manual.rs:477 附近的 emit_batch 里，在 `CompactionPerformed` 同批追加 `FoldRecorded`（folded_tokens/summary_tokens 从本次压缩的账目取；strategy=`Manual`，trigger 按来源：用户命令/工具调用）。**同批原子性：要么都在要么都不在。**
  - **Spec 开放问题 O1 裁决**（必须裁决并把理由写回 spec）：自动压缩（compactor.rs 三策略）目前是瞬态摘要、不改事件日志。默认倾向——压力驱动 LlmSummary 落 `FoldRecorded`（它是事实上的折叠、有摘要产物可寻址）；DeterministicTruncation 不记（无摘要产物）。实现时以此为准，若代码现实推翻倾向，记录裁决理由到 spec §6。
- [ ] **Step 6: 效果断言测试**：跑一次手动压缩全流程（沿用 manual.rs 既有测试脚手架），断言事件日志里同 seq 批次同时含 `CompactionPerformed` 与 `FoldRecorded`，且 list_folds 能取回。
- [ ] **Step 7: Commit** `git add -A && git commit --no-verify -m "compact: add FoldRecorded event and fold registry"`

### Task 2: `session_decompress` 工具

**Files:**
- Create: `src/builtin_tools/decompress.rs`
- Modify: `src/builtin_tools/mod.rs`（~269 行 pub use 区加导出）
- Modify: `src/executor/builtin_registry/definitions.rs` + `src/executor/builtin_registry/builder/constructor/collab_session_tools.rs`（仿 RecallContextTool 注册路径）
- Modify: `src/tools/result_processing.rs`（~210 行 `is_read_family` → 扩展为 verbatim-recall 谓词；~151 行 `read_backstop_tokens` 复用）

**Interfaces:**
- Consumes: Task 1 的 `FoldRecord` / `list_folds` / `derive_fold_id`；`SessionService::get_events`（src/session/service.rs:52，含软退休事件——确认该接口返回退休事件，若不则走 EventLog 读路径，实现时先连线）；`read_backstop_tokens()`。
- Produces:
  ```rust
  pub struct SessionDecompressTool { /* session 访问句柄，仿 RecallContextTool::new */ }
  impl AlephTool for SessionDecompressTool {
      const NAME = "session_decompress";
      // DESCRIPTION 必须写明：逐字还原、分页参数、与 ctx_search（片段检索）的分工
  }
  pub struct SessionDecompressArgs { pub fold_id: Option<String>,
      pub from_seq: Option<u64>, pub to_seq: Option<u64>, pub max_tokens: Option<usize> }
  pub struct SessionDecompressResult { pub fold_id: String, pub from_seq: u64, pub to_seq: u64,
      pub rendered: String, pub truncated: bool, pub next_from_seq: Option<u64> }
  ```
- 谓词扩展：`is_read_family` 改名/扩展为同时匹配 `FileReadTool::NAME` 与 `SessionDecompressTool::NAME`（守卫测试断言两者都在）。

- [ ] **Step 1: 写失败测试**：`decompress_roundtrip_byte_exact`（造会话→手动压缩→decompress→还原字节与事件原文逐字节相等）；`decompress_latest_fold_default`（省略 fold_id 取最近）；`decompress_pagination_reassembles`（两页拼接 == 整段）；`decompress_hard_retired_range_errors_honestly`（Review Focus #2：区间被 Retire::From 硬删 → 错误信息区分「fold 不存在」vs「区间已硬退休」）；`decompress_legacy_fold_via_compaction_performed`（Review Focus #1）；`verbatim_family_includes_both_tools`（谓词守卫）；`decompressed_marker_text_not_synthetic`（Review Focus #3：还原文本内含 `[Context folded]` 字样时 `is_synthetic_reminder` 不误判、无压缩路径副作用）。
- [ ] **Step 2: 跑测试确认失败** `cargo test -p alephcore --lib builtin_tools::decompress`。
- [ ] **Step 3: 实现 decompress.rs**：解析 fold_id（缺省=list_folds 最后一个；无 fold → 诚实空结果+advisory 字段，遵守「没找到 vs 没跑」判据）→ 读 seq 区间事件 → 渲染可读转录（渲染复用 recall/session_search 既有渲染器，先探索连线，不新写）→ max_tokens 截断 + `next_from_seq` 续页坐标。
- [ ] **Step 4: 预算连线**：result_processing.rs 谓词扩展 + 守卫测试；确认 dispatch.rs:1588 的 resolve_result_budget 对该工具返回 None（不卸载、由 backstop 兜底）。
- [ ] **Step 5: 注册**：definitions.rs + collab_session_tools.rs 仿 RecallContextTool；mod.rs 导出。
- [ ] **Step 6: 跑测试确认全过** + `cargo clippy -p alephcore -- -D warnings`。
- [ ] **Step 7: Commit** `git commit --no-verify -m "tools: add session_decompress for verbatim fold restoration"`

### Task 3: Fold Economics 账本 + doctor 检查

**Files:**
- Create: `src/context/compact/fold_ledger.rs`（惰性判定逻辑，纯函数为主）
- Create: `src/diagnostics/checks/fold_economics.rs`（仿 cache_hit_rate.rs 模式）
- Modify: `src/diagnostics/checks/mod.rs`（注册检查）

**Interfaces:**
- Consumes: Task 1 的 `FoldRecord`（folded_tokens/summary_tokens）；state.db 的 `task_traces` provider_usage 行（cache_hit_rate.rs 的既有 SQL 读路径）；`aleph_protocol::cache_hit_ratio`。
- Produces:
  ```rust
  pub struct FoldVerdict { pub fold_id: String, pub net_saved_tokens: i64,
      pub summary_cost_tokens: u64, pub payback: Payback }  // Payback::{Breakeven, NotYet, Never}
  pub fn judge_fold(fold: &FoldRecord, turns_observed: u32, summarizer_cost_tokens: u64,
      window_turns: u32) -> FoldVerdict  // 纯函数
  ```
- 判定公式（实现者不得自行发明别的）：`per_turn_saving = folded_tokens − summary_tokens`；`Never` 当 `per_turn_saving <= 0`；`Breakeven` 当 `turns_observed * per_turn_saving >= summarizer_cost_tokens`；否则 `NotYet`。summarizer_cost_tokens 由 MeteringProvider 的 `compactor:<agent>` 通道计量（FL §2.18），读 state.db usage 行。
- doctor 检查 ID：`core/fold-economics`；无 fold 或样本不足 → informational（仿 MIN_CALLS 模式，Review Focus #5）。

- [ ] **Step 1: 写失败测试**：`judge_fold_breakeven_when_reads_dominate`；`judge_fold_never_when_summary_exceeds_savings`；`empty_db_yields_informational_not_warn`；`health_lines_documented`（命中率 95–97% / 压缩成本 ≤2% 写进 Finding detail 文案，断言文案含阈值）。
- [ ] **Step 2: 确认失败**。
- [ ] **Step 3: 实现 fold_ledger.rs**（纯函数判定）+ fold_economics.rs（rusqlite 只读 state.db、spawn_blocking、24h 窗口对齐 cache_hit_rate.rs）。
- [ ] **Step 4: 注册进 checks/mod.rs**；手动跑 `aleph doctor` 或对应测试确认检查出现。
- [ ] **Step 5: Commit** `git commit --no-verify -m "compact: fold economics ledger and doctor check"`

### Task 4: 增长步进 nudge + session_compact 引导 + 防伪守卫

**Files:**
- Modify: `src/thinker/nudges.rs`（新 fenced 构造函数 + 常量，遵守 §2.19 单一源纪律；include_str! 自扫测试自动覆盖）
- Modify: `src/harness/agent/think.rs`（~373 行 build_prompt 附近/压力评估点：增长步进判定 → 瞬态尾区注入）
- Modify: `src/builtin_tools/sessions/compact_tool.rs`（DESCRIPTION 增补引导哲学）
- Modify: 配置类型（`[context_budget] compress_nudge_growth_tokens`，默认 50000；找 context_budget 配置族所在文件，带钳位测试）

**Interfaces:**
- Consumes: `CompactionCircuitBreaker`（src/context/budget/mod.rs:232，跳闸期间不 nudge）；`SYSTEM_REMINDER_OPEN`（nudges.rs:337）；每轮 prompt token 估计（think.rs 压力评估点已有的估计值，实现时连线，不新造估计器）。
- Produces: `pub fn compact_growth_nudge(grown_tokens: u64, threshold: u64) -> String`（nudges.rs，fenced system-reminder 文本，教模型调 session_compact 并写明 instructions 要点：用户目标/未决问题/文件台账）。

- [ ] **Step 1: 写失败测试**：`nudge_fires_at_growth_threshold`；`nudge_suppressed_when_breaker_tripped`；`nudge_is_transient_not_persisted`（Review Focus #4：注入后事件日志无新增、且 prefix_stability 契约测试保持绿）；`nudge_resets_after_fold`；`assistant_marker_text_creates_no_fold_events`（防伪：助手输出含 `[Context folded]` 形文本 → 事件日志 FoldRecorded/CompactionPerformed 计数不变）。
- [ ] **Step 2: 确认失败**。
- [ ] **Step 3: 实现**：配置项 → nudges.rs 构造函数 → think.rs 注入点（增长会计：上次 fold 或上次 nudge 以来的 prompt token 增量 ≥ 阈值才发，发完重置基线）。
- [ ] **Step 4: compact_tool.rs DESCRIPTION 增补**（何时压：增长步进、任务阶段切换；instructions 怎么写）。
- [ ] **Step 5: 全量域测试 + clippy**。
- [ ] **Step 6: Commit** `git commit --no-verify -m "thinker: growth-step compaction nudge with breaker coordination"`

---

## 线 2：Admission 打磨 + 清账（模型：kimi/minimax 级）

### Task 5: 降级摘要跨 run 缓存污染修复（§2.1 欠账）

**Files:**
- Modify: `src/context/compact/compactor.rs`（accept_summary 在 :1178；COMPACTION_CARRYOVER 使用点）
- Modify: `src/context/compact/compaction_cache.rs`（缓存键升级）

**Interfaces:**
- Consumes: 现有 COMPACTION_CARRYOVER 指纹缓存、accept_summary 单一入口、llm_retry::classify_exhausted 的降级信号。
- Produces: `enum SummaryQuality { Full, Degraded }`；缓存条目带 quality_tag；**Degraded 可同 run 命中、不写跨 run carryover**。

- [ ] **Step 1: 先读现状**：找到断言「降级产物必须缓存」的两条现存回归测试（rg "carryover" src/context/compact/ 的测试区），理解它们断言的语义边界（同 run 还是跨 run）。
- [ ] **Step 2: 写失败测试**：`degraded_summary_reused_within_run`（同 run 指纹命中仍工作）；`degraded_summary_not_carried_across_runs`（新 run 不命中降级条目，重新走摘要路径）；`full_summary_carried_across_runs`（Full 条目行为不变）。
- [ ] **Step 3: 实现**：accept_summary 打 quality_tag（降级路径=truncation fallback 携带 prior summary body 的那条）；缓存键/条目带 tag；跨 run 读路径过滤 Degraded。
- [ ] **Step 4: 改写两条现存回归测试**为新语义（同 run 命中 + 跨 run 不命中）。
- [ ] **Step 5: 域测试全过 + Commit** `git commit --no-verify -m "compact: tag degraded summaries, keep them out of cross-run carryover"`

**注意**：此任务与线 1 共享 compactor.rs——线 2 派发时若线 1 Task 1 Step 5（发射点接线）尚未合并，本任务只在 compaction_cache.rs + accept_summary 函数体内改动，不碰 manual.rs，冲突面可控。

### Task 6: headroom SmartCrusher（§2.7 欠账，只做 log/search）

**Files:**
- Modify: `src/tool_output/structured/mod.rs`（Profile 体系挂新入口）
- Create: `src/tool_output/structured/crush.rs`

**Interfaces:**
- Consumes: `ContentKind`、`reduce_within` 产出、`Profile::for_token_budget`、中央 `is_meaningful_shrink`。
- Produces: `pub(crate) fn crush_within(text: &str, kind: ContentKind, budget_tokens: usize) -> Option<String>`——仅 Log/Search 两 kind 实现行重要性打分（error/warning/匹配行/结构行优先，其余省略标记折叠）；Diff/Json 返回 None（结构敏感，spec 已记录理由）。缩减器不判定「是否更小」，交中央字节守卫。

- [ ] **Step 1: 写失败测试**：`crush_keeps_error_lines_over_noise`（log）；`crush_keeps_match_lines`（search）；`crush_diff_returns_none`；`crush_respects_byte_guard`（产出经 is_meaningful_shrink 裁决）。
- [ ] **Step 2-4: 实现 + 通过 + 域测试**（`cargo test -p alephcore --lib tool_output`）。
- [ ] **Step 5: Commit** `git commit --no-verify -m "tool_output: lossy line-selection crusher for log/search headroom"`

### Task 7: tool_output 全链断线审计

**Files:** 只读审计 + 报告；CONNECT/CUT 修复落代码。

**链路**：`tools/scoped/dispatch.rs:1543-1637` → `result_processing::apply_result_budget` → `hygiene::clean_result_value` → `structured/*` → `cheap_passes/tool_result_pruning.rs` → result_store/ctx_search。

- [ ] **Step 1: 用 severed-wire-audit 技能方法**：对每个导出符号数生产者和消费者（rg 计数），产出 CONNECT/CUT/DECIDE 清单（每条附 rg 证据行）。
- [ ] **Step 2: CUT 项直接删**（熵减原则），CONNECT 项修复，DECIDE 项写进报告留给主会话裁决。
- [ ] **Step 3: 报告落盘** `docs/superpowers/reports/2026-10-01-tool-output-wire-audit.md` + Commit `git commit --no-verify -m "tool_output: severed-wire audit + dead code cleanup"`

---

## 合并与收尾（主会话执行，不派发）

- [ ] 线 2（Task 5-7）完成后先合并回 main：`git checkout main && git merge context-fabric` 分批cherry-pick或整体合并（视冲突），编译验证。
- [ ] 线 1（Task 1-4）完成后合并；`just test-all` 全量。
- [ ] FEATURE_LOCATOR.md 新增 §2.21（本轮条目：fold registry / decompress / fold economics / growth nudge / 2a 修复 / SmartCrusher / 审计结果）+ 附录 E 新判据。
- [ ] 删除 worktree：`git worktree remove .worktrees/context-fabric-2026-10-01 && git branch -d context-fabric`。
