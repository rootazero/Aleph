# Memory Governance Surfacing 设计文档

> Aleph panel 记忆管理面板三方向重构之 #3「补齐缺失支柱+治理」的剩余两子块。
> 第①子块（检索打分透明 `memory.retrieve_with_trace`）已合并 main `751b9c7d3`。
> 本文档覆盖 ②dream insights 列表 RPC + Panel、③corrections 治理（只读）RPC + Panel。

**日期:** 2026-06-20
**分支:** `memory-governance-surfacing`（off main `751b9c7d3`）
**前置授权:** #3 特批允许 `cargo check`（异于 #1/#2 全程 no-cargo 约束）。

---

## 1. 目标 (Goal)

把已生成但 panel 看不见的两类记忆子系统状态暴露出来，让用户能在面板里观察记忆系统的"治理面"：

- **②Dream Insights**：每日摘要（daily digest）、weekly synthesis 笔记、做梦运行历史（audit trail）。当前全部生成并持久化，但从未经任何 RPC 到达 panel。
- **③Corrections 治理**：用户纠正（`flag_user_correction` 写入的 raw memory）→ 蒸馏成 `feedback/` 笔记的**生命周期与审计**。Memory Hub（#2）已经把*蒸馏后的* feedback/lessons 笔记作为 notes 展示，但 **raw correction → distillation 这条管线**（哪些待蒸馏、哪些已蒸馏）对用户不可见。

两块都是**只读暴露面**：新增后端只读 RPC + Panel 只读视图消费。沉淀/纠正的写入仍 100% 由 LLM/工具驱动。

## 2. 架构原则与红线约束

- **连线优先**：复用现有 store 读 API。只在确无 list 能力处补**一个**查询方法（daily insights 当前只有单日读）。
- **R7/R8/R10 边界**：corrections 治理**纯只读**，不提供手动 CRUD。FEATURE_LOCATOR §2.5③ 明确：纠正→反馈管线是 LLM/工具驱动的故意设计，用户手动改写会破坏该边界。本设计**不碰**写入侧。
- **R4 纯 I/O**：handler 只做 `参数校验 → 调 store 读 → 映射响应`，不含业务逻辑。
- **Panel 落点 = Settings ▸ Memory**：该页已托管 Dreaming 设置 + Retrieval Debug Panel，是天然的"记忆诊断/治理"中枢。新视图落此处，**不污染** Memory Hub（`/memory`）的 notes/graph 浏览职责。
- **熵减**：本轮以新增暴露面为主，无现成旧代码可删；若实现中发现 dead query 顺手清理并在 plan 标注。

## 3. 参考项目 (memos / MemOS 2.0)

`/Volumes/TBU4/Github/memos` 有 `mem_feedback`（`src/memos/mem_feedback/`、`api/handlers/feedback_handler.py`）与 `dream`（`src/memos/dream/`）模块，作为"反馈/做梦如何对外暴露"的概念参照。Aleph 的存储与协议是自有的（SQLite + JSON-RPC），不移植代码，仅借鉴"把 feedback 生命周期与 dream 产物作为可观测对象暴露"的产品思路。

## 4. 子块 ② — Dream Insights

### 4.1 后端 RPC `dreaming.list_insights`

- **命名**：snake_case，对齐已有 `dreaming.run_now`。
- **参数** `{ agent_id?: string, limit?: number }`，默认 `agent_id = DEFAULT_AGENT_ID`、`limit = 30`。
- **三个数据源**：
  1. **Daily insights**：`daily_insights` 表当前只有 `DreamStore::get_daily_insight(date: &str) -> Option<DailyInsight>` 单日读。**新增** `recent_daily_insights(limit: usize) -> Vec<DailyInsight>`（`DreamStore` trait + sqlite impl，`ORDER BY date DESC LIMIT ?`）。这是本子块唯一新增查询。
  2. **Synthesis notes**：复用 `NoteStore::list_notes(agent_id)` 后 filter `category == "synthesis"`。
  3. **做梦运行历史**：复用现成 `SqliteMemoryBackend::recent_dream_reports(limit) -> Vec<PersistedDreamReport>`（audit trail，免费拿）。
- **响应形状**：
  ```json
  {
    "daily":   [{ "date": "2026-06-20", "content": "...", "source_memory_count": 5, "created_at": 1718918400 }],
    "synthesis":[{ "path": "synthesis/rust-synthesis", "title": "rust Synthesis", "tags": ["rust","synthesis"], "updated_at": 1718918400 }],
    "runs":    [{ "id": "...", "pipeline_type": "daily", "started_at": 1, "finished_at": 2, "duration_ms": 3000, "synthesis_count": 2, "errors": null }]
  }
  ```
- **错误处理**：`limit` 非法/缺省走默认；agent_id 缺省走 DEFAULT_AGENT_ID。store 读失败 → `INTERNAL_ERROR`，不泄漏内部路径。

### 4.2 Panel — Settings ▸ Memory 新增 "Dream Insights" 只读区

- 顶部：最近一次做梦运行状态（从 `runs[0]` 渲染 pipeline_type + 时间 + duration + synthesis_count，errors 非空时红字）。
- 中部：daily insights 列表（date 标题 + content 正文 + source_memory_count 角标）。
- 底部：synthesis 笔记卡片（title + tags + updated_at；点击可跳 Memory Hub 对应 note，复用 #2 的 path 联动——**可选增强，不阻断**）。
- 加载态/空态/错误态齐备。

## 5. 子块 ③ — Corrections 治理（只读）

### 5.1 后端 RPC `memory.list_corrections`

- **命名**：snake_case，对齐 `memory.retrieve_with_trace`。
- **参数** `{ agent_id?: string, limit?: number, include_distilled?: boolean }`，默认 `agent_id = DEFAULT_AGENT_ID`、`limit = 50`、`include_distilled = true`。
- **数据源**：复用 `RawMemoryStore::get_raw_by_path_prefix("aleph://correction/", agent_id, limit) -> Vec<RawMemory>`。
- **映射**：每条 `RawMemory` → 从 `RawMemorySource::Correction { severity, suggested_rule }` 提字段；`status = if is_processed { "distilled" } else { "pending" }`；`include_distilled == false` 时过滤掉 `is_processed == true` 的条目。
- **响应形状**：
  ```json
  {
    "corrections": [
      { "id": "uuid", "content": "...", "severity": "high", "suggested_rule": "...", "status": "pending", "created_at": 1718918400 }
    ]
  }
  ```
- **纯只读**：handler 不调用任何写入/删除 API。

### 5.2 Panel — Settings ▸ Memory 新增 "Corrections" 只读区

- 条目列表：每条带 **status 徽章**（pending=琥珀 / distilled=绿）、severity 标签、content、suggested_rule（有则展示）、created_at。
- 顶部小结："N 条待蒸馏 / M 条已蒸馏"。
- include_distilled 切换（默认显示全部）。
- 加载/空/错误态齐备。

## 6. 注册与文件触点

### 后端（新增/修改）
- `src/memory/store/mod.rs`：`DreamStore` trait 加 `recent_daily_insights`。
- `src/memory/store/sqlite/sessions.rs`（或 dream 相关 sqlite 文件）：impl `recent_daily_insights`。
- `src/gateway/handlers/dreaming.rs`：加 `handle_list_insights`。
- `src/gateway/handlers/memory.rs`（或新文件 `memory_corrections.rs`）：加 `handle_list_corrections`。
- `src/bin/aleph-server/commands/start/builder/handlers/memory.rs`：在 `register_memory_handlers` 手动 closure 注册两个新 method（同 block 1 `memory.retrieve_with_trace` 模式）。

### 前端（新增/修改）
- `interfaces/webchat/src/api/memory_config.rs`（或新 api 模块）：加 `list_insights` / `list_corrections` RPC client + 响应 DTO。
- `interfaces/webchat/src/views/settings/memory.rs`：挂载两个新只读组件。
- 新组件文件（按 #2 风格，每文件单一职责，<400 行）。

### 文档
- 合并后同步 `docs/reference/FEATURE_LOCATOR.md`：§2.5③ 补"治理可见性已暴露 via `memory.list_corrections`"；dreaming 相关段补 `dreaming.list_insights` 锚点。**确保文档反映真实代码架构。**

## 7. 测试策略

- **后端单测**：
  - `recent_daily_insights`：插入多条 daily insight，断言按 date DESC + limit 截断。
  - `handle_list_corrections`：构造 raw correction（pending + distilled 各一），断言 status 映射 + include_distilled 过滤。
  - `handle_list_insights`：构造三源数据，断言响应形状映射。
- **前端**：wasm check 干净（按 #2 no-cargo 习惯，前端不强制跑测试，编译通过即可）。
- **cargo check**（#3 特批）：每子块完成后 `cargo check -p alephcore --lib` + 必要时 `--bin aleph-server`。
- **预存无关 E0063**：`tests/worktree_isolation.rs` 缺 `SpawnRequest.strategy` → 用 `--lib` 过滤绕开（block 1 已知）。

## 8. 明确不做 (YAGNI / 边界)

- ❌ 手动 CRUD corrections（守 R7/R8）。
- ❌ dream 触发/调度改动（已有 `dreaming.run_now`）。
- ❌ 把 insights 塞进 Memory Hub graph/table（保持各自职责）。
- ❌ 跨 agent 聚合（按 agent_id 隔离，同所有 memory API）。
- ❌ 分页（首版 limit 截断足够；真有需要再加 offset）。

## 9. 执行方式

- 单 worktree `memory-governance-surfacing`，off main `751b9c7d3`。
- Subagent 驱动 SDD（同 block 1）：每 task fresh implementer + per-task review（spec + quality）+ 终审 opus 全分支。
- 模型选择：haiku 转录类、sonnet 集成类、opus 终审。
- 合并授权沿用 #3 既定流程（实施完成后等用户授权合并）。
