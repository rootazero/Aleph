# 分层记忆溯源证据链 — 设计 (Traceable Memory Evidence Chain — Design)

> **日期 (Date)**: 2026-06-27
> **范围 (Scope)**: A — 只做溯源证据链主线（连通 L0→L1→L2→L3 provenance 回指 + 一个下钻工具/RPC）。不新建物理 L2 场景层，不引入符号短期记忆 Mermaid canvas。
> **参考项目 (Reference)**: `/Volumes/TBU4/Github/TencentDB-Agent-Memory`（符号化短期记忆 + 分层长期记忆 L0→L3 的 TS/openclaw 插件）。
> **协议 (Protocol)**: Gap Analysis 取最优 → 连线优先 → 熵减 → worktree 隔离（代码阶段）。

---

## 1. 北极星 (North Star)

任意高层记忆断言（`USER.md` 画像条目，或某条 note fact）都能**逐层下钻到地面真相**，每一跳都有**显式回指**，不靠时间戳推断：

```
L3 画像 section → L2 源 note(synthesis/cluster) → L1 note fact → L0 raw_memory 行 → 原始 transcript
```

验收口径：「用户偏好 TypeScript」能从 L3 追到 L2 场景块，再追到 L1 原子事实，最后追到 L0 原话，证据链不断。

---

## 2. Gap Analysis（参考项目 vs Aleph）

最重要的发现：**Aleph 的溯源基建几乎全部已存在，但从未被填充** —— compression pipeline 在 `PageOp` 阶段把 raw id 丢掉了，四个断链全是「连线」问题而非「造轮子」问题。

| 维度 | 参考 TencentDB-Agent-Memory | Aleph 现状 | Gap |
|---|---|---|---|
| **L0 原始对话** | JSONL 日分片，`msg_id`，retention 管理 | `raw_memories`(SQLite)，UUID id，`is_processed` 标志 | 对等 |
| **L0→L1 溯源** | L1 显式 `source_message_ids: string[]` 回指 L0 | 字段 `source_notes`/`fact_provenance` + 表 `notes_sources`/`notes_provenance`（含反向索引 `idx_notes_sources_ref`）**已存在但永不填充**（`ingest/apply.rs:125` `..Default::default()` 丢弃 raw id） | 🔴 **断链①** 基建齐全，仅缺连线 |
| **L1 原子事实** | MemoryRecord（JSONL 全量 + SQLite 向量索引层丢 source_ids） | notes markdown + index/fts/vec，**逐事实**粒度 provenance 基建在 | Aleph 设计更强，未填 |
| **L2 场景聚类** | `scene_blocks/*.md`，L2→L1 仅靠 `scene_name` 隐式反查 | 无物理 L2；只有 dreaming `NoteSynthesis` + community detection；`NoteConsolidated.source_note_paths` 有源但绕过 handler | 🟠 **断链②** |
| **L3 用户画像** | `persona.md`，**无显式回指**，靠 `last_persona_time` 时间戳推断 | `USER.md` `ProfileSynthesizer`；`SessionSignal` 带 `session_id` 但**画像不落任何 provenance** | 🔴 **断链③** 双方都弱，Aleph 可反超 |
| **下钻工具** | `node_id` grep + `conversation_search`/`memory_search` | `recall_context`（须先知 `session_id`）；**无 note→raw→transcript 反向下钻** | 🔴 **断链④** 反向索引已在，缺消费者 |
| **事件溯源** | 无 | `MemoryEvent::NoteCreated.source_memory_ids` 已定义但 compression 直写 store 绕过 handler | Aleph 独有，未接 |
| **符号短期记忆** | Mermaid canvas + `node_id` 下钻 `refs/*.md` | §2.7 ContentRouter + `ctx_search` + `ContentIndex`(FTS5) | 各有实现，**不在范围 A** |

**结论**：参考项目只有 L0→L1 一跳是显式回指，L2/L3 同样靠语义/时间戳推断；Aleph 的字段、表、反向索引、事件溯源**全部已建好**，只差把 raw id 在 ingest pipeline 里穿过去 + 一个下钻消费者。接通后 Aleph 在溯源上**超越参考项目**：逐事实粒度 + 显式回指 + 反向索引。

---

## 3. 层映射（范围 A：全部复用现有物理层，不新建）

| 概念层 | Aleph 落点 | 回指字段（已存在） |
|---|---|---|
| **L0** | `raw_memories`（UUID id） | — |
| **L1** | notes markdown | note 级 `source_notes`(→raw id) + 逐事实 `fact_provenance`(→raw id via `<!-- src: -->` marker) |
| **L2** | 现有 `NoteSynthesis` 综合笔记 + community 聚类 | 综合笔记的 `source_notes`(→源 L1 paths) |
| **L3** | `USER.md`（6 固定 section） | **新增**结构化 `## Sources` 映射(section→note/session) |

### 已验证的现有基建锚点 (Confirmed existing infrastructure)

- `KnowledgeNote.source_notes: Vec<String>` — `src/memory/notes/note/mod.rs:68`；`to_markdown` 写 frontmatter `source_notes:`（`mod.rs:208`）；`from_markdown` 读回（`mod.rs:154`）。
- `KnowledgeNote.fact_provenance: Vec<FactProvenance>` — `mod.rs:81`；`extract_provenance_markers` 从正文 marker 解析（`mod.rs:136`）。
- 逐事实 marker 通道 **C2.8** 已存在：`src/memory/notes/ingest/apply.rs:23` `ensure_origin_marker`，正则已支持 `<!-- src: <id>, origin: raw_source, inferred: false -->`（`apply.rs:26`）——目前 LLM 从不给 `src:`，于是一律盖 `inferred: true`。
- 表 `notes_sources`(含 `idx_notes_sources_ref` 反向索引) / `notes_provenance`(含 `idx_prov_source`) — `src/memory/store/sqlite/schema/ddl.rs:118` / `:180`。
- 写侧已连：`index_note` 从 `source_notes` 写 `notes_sources`（`store_impl.rs:186-201`）、从 `fact_provenance` 写 `notes_provenance`（`store_impl.rs:251-285`，`upsert_provenance` at `:1324`）。
- 读侧已有：`SELECT source_ref FROM notes_sources …`（`store_impl.rs:1081`）、`get_provenance`（`store_impl.rs:1366`）。
- L3：`USER.md` 6 固定 section 解析/渲染在 `src/memory/notes/profile/store.rs`（`PROFILE_FILENAME` at `:10`，section heading 解析 `## ` at `:135`，render at `:167`）；`ProfileSynthesizer::update(agent_id, SessionSignal)`（`synthesizer.rs`）。
- L0 取数：`recall_context`（`src/builtin_tools/recall_context.rs`）按 `session_id` + path 前缀取 raw。

---

## 4. 四个连线修复 + 一个守护 (Wiring Fixes)

### ① L0→L1 溯源（keystone，断链①）

- `PageOp::Create` / `PageOp::Append` 加 `#[serde(default)] source_ids: Vec<String>`（`src/memory/notes/ingest/plan.rs`，向后兼容）。
- 把 ingest 批次的 raw id 透传进 `CompoundApplyTx`（`apply.rs`）作**确定性兜底集**。
- `apply.rs::stage()`（`apply.rs:100`）：用 `op.source_ids` 填 `KnowledgeNote.source_notes`（替换 `..Default::default()`）；LLM 未给则回退批次 raw id → 经现有 `index_note` 自动落 `notes_sources`。**保证链不空**（= 混合策略的 note 级确定性那一半）。
- **逐事实（best-effort）**：扩 ingest LLM prompt，使模型对能归因的 fact 内联 `<!-- src: <raw-id>, origin: raw_source, inferred: false -->`。`ensure_origin_marker` 已透传、`extract_provenance_markers` 已解析 → 落 `notes_provenance`。归因不了的 fact 保持 `inferred: true`（诚实，现有行为不变）。
- **prompt 改动**：ingest 抽取 prompt 需 (a) 把每条 raw 的 id 暴露给模型、(b) 指示模型每个 op 产出 `source_ids`、可选每条 fact 内联 `src:` marker。（守 R7：抽取判断仍归 LLM，代码只穿 id。）

### ② L1→L2 溯源（断链②）

- `NoteSynthesis` 造综合笔记时，把聚类成员的 L1 note paths 填进该综合笔记的 `source_notes`（聚类成员在 stage 内已知）。**复用同一 `source_notes` 字段/表** —— 综合笔记→源 L1。今天这步被丢弃。
- 锚点：`src/memory/dreaming/stages/note_synthesis.rs`、community 在 `src/memory/notes/graph/`。

### ③ L3 溯源（断链③，USER.md 结构化 Sources 区）

- `profile/store.rs` 的 USER.md 解析/渲染加机器可读 `## Sources` 块：每个 section ← 贡献的 note paths + session ids。（解析复用现有 `## ` section 机制；`UserProfile` 类型在 `profile/types.rs` 加 `sources: Map<section, {notes, sessions}>` 之类的承载。）
- `ProfileSynthesizer::update` 已收到 `SessionSignal{session_id}`；把本次合成检索到的 note paths + session_id 记到被触及的 section。沿用现有 hash-guarded 原子写（`store.rs:62` 冲突检查 + `:73` 原子 rename），Sources 区随同一文件落盘。
- 粒度 = section 级（与决策一致）。

### ④ 下钻工具 + 只读 RPC（断链④）

- 新 builtin tool `memory_trace`（LLM 可调）：输入 = note path / 画像 section / raw id；输出 = 尽力下钻的完整证据链（profile section → source notes → 各自 source_notes/provenance → raw ids → raw content/transcript 摘要）。
- **全部复用现有反向能力**：`notes_sources` 读（`store_impl:1081`）、`get_provenance`（`:1366`）、反向索引 `idx_notes_sources_ref`、`recall_context` 按 id/session 取 raw。
- 仅补 `NoteStore` 上缺的薄读方法（如 `sources_of(agent, note) -> Vec<String>`、`notes_citing(agent, raw_id) -> Vec<String>`）——若现有读 API 已覆盖则直接复用。
- 新只读 gateway RPC `memory.trace`，对齐现有 `memory.list_corrections`（`src/gateway/handlers/memory.rs`）/ `dreaming.list_insights`（`src/gateway/handlers/dreaming.rs`）模式（守 R4 纯 I/O，panel-ready，**UI 不在范围 A**）。

### ⑤ 留存守护（Q4 的诚实再范围化）

- **现状核实**：今天**没有** `raw_memories` 留存 sweep（grep 全仓仅 `recall_signals` / notes GC / session facts / logs 有 retention，`raw_memories` 仅在 schema init 有一次 dedup-by-path delete）→ L0 尾巴本就安全。
- **所以不造 sweep**（守 YAGNI / 熵减）。改为：
  - (a) `memory_trace` 遇到已不存在的 raw 优雅报「证据已归档/清理」，而非报错；
  - (b) 提供 `is_raw_referenced(raw_id)` 能力（反向索引已支持）；
  - (c) 在 `docs/reference/memory/RAW_MEMORY.md` 写下不变量：**任何未来的 raw GC 必须排除被 `notes_sources` / `notes_provenance` 引用的行**。
- = 前瞻性 pin，零投机代码。

---

## 5. 熵减 (Entropy Reduction) — 已确认决策

`MemoryEvent::NoteCreated.source_memory_ids` / `CreateNoteCommand.source_memory_ids` / `NoteConsolidated.source_note_paths` 已定义但 compression 走直写 `index_note` 路径、从不填充（半死代码）。

**已确认（推荐项）**：**保留直写路径**，只在直写处填 `source_notes` / `fact_provenance`；事件字段维持现状，并在文档注明其消费者是 event-log 路径而非 compression。理由：改动面小、不碰 R10 笨循环、不引入大重构风险。（改道 event handler 的方案明确**不采用**。）

其余熵减：
- `ensure_origin_marker` 的 blanket `inferred: true` 从「所有 fact」收窄为「真正未归因 fact」的兜底——语义变实，无新增死代码。
- 实施中若发现因连线而产生的孤儿 import / 未用变量，随手清理（仅限本次改动产生的）。

---

## 6. 红线与原则对齐 (Redline / Principle Alignment)

- **R7 LLM 主权 / P8 LLM 优先**：抽取与归因判断仍由 LLM 做，代码只透传 id、不做语义判断。
- **R3 核心轻量化**：零新第三方依赖（复用 serde / sqlite / 现有表）。
- **R4 Interface 纯 I/O**：下钻只读 RPC 不含业务逻辑，仅转发。
- **R10 薄 Harness**：不碰 `src/harness/`；不加错误恢复 hook。
- **P6 KISS/YAGNI**：不造 L2 物理层、不造 retention sweep、不改道 event handler。

---

## 7. 验证 (Verification — goal-driven 成功标准)

1. **单测**：`PageOp::Create{source_ids}` → frontmatter `source_notes` 有值 → `notes_sources` 有行 → `sources_of()` 取回 raw id。
2. **单测**：带 `<!-- src: -->` 的 fact → `notes_provenance` `origin=raw_source, inferred=false`；无 marker 的 fact → `inferred=true`。
3. **单测**：synthesis 笔记 `source_notes` 记录聚类源 L1 paths。
4. **单测**：`USER.md` 渲染/解析 round-trip 保留 `## Sources` 映射；`update` 后被触及 section 的 sources 含本次 session_id。
5. **集成测（北极星）**：`memory_trace(画像 section)` 返回不断链直达某 raw id 的链路 —— 即「用户偏好 TypeScript：L3→L2→L1→L0」。
6. **降级测**：raw 已删时 `memory_trace` 返回「证据已清理」而非报错。
7. **回归**：现有 ingest/apply/profile 单测全绿（`source_ids` 为 `#[serde(default)]`，旧 plan JSON 仍解析）。

构建纪律：极度节制 cargo（系统负担）；高风险合并至多一次 `cargo check -p alephcore --lib`。

---

## 8. 实施纪律 (Implementation Discipline)

- **worktree 隔离**：所有**代码**改动在新建 worktree 分支进行，严禁直接触碰 main（本 spec 文档除外，按仓库约定落 main）。
- **连线优先**：先接通现有字段/表/索引，确认无法复用才允许新增。
- 进入 `writing-plans` 拆分为可独立验证的 task。

---

## 9. 不做清单 (Explicitly Out of Scope)

- 新建物理 L2 「场景块」聚类层（参考项目式 scene_blocks）。
- 符号短期记忆 Mermaid canvas（Aleph 已有 §2.7 ContentRouter / ctx_search 对等物）。
- `raw_memories` 留存 sweep（当前无 sweep，不投机造）。
- compression 改道 event handler。
- panel 前端证据链浏览 UI（RPC 已 panel-ready，UI 留作后续 spec）。
