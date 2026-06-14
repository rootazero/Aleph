# Note 层 llm_wiki 协议标准化 + 图智能增强 — 设计

- **日期**: 2026-06-14
- **分支**: `note-protocol-graph`（worktree `/Volumes/TBU4/Workspace/Aleph-wt-note`，隔离于 main）
- **参考项目**: `/Volumes/TBU4/Github/llm_wiki`（nashsu 实现的 Karpathy "LLM Wiki" 模式 → Tauri 桌面应用 + 本地 HTTP/MCP 协议）
- **目标**: 对 Aleph 记忆管理 note 层进行 ① 协议标准化（Vault 级字节兼容）② 图智能架构增强（全盘移植参考算法）③ 错误修复与功能连线
- **强制约束**: 完成后**不跑 cargo check/test**，直接提交（用户资源治理约束）；严禁触碰 main 分支。

---

## 1. 扫描摘要（Gap Analysis，已按真实代码校正）

> ⚠️ 初次分析基于 `docs/reference/memory/NOTES.md`，落 spec 前已逐文件核实真实代码，**发现该文档大面积陈旧**。下表为校正后的真实差距。

| 维度 | llm_wiki 参考协议 | Aleph 真实现状（代码实证） | 判定 |
|---|---|---|---|
| **定向文件** | index.md / log.md / schema.md / **overview.md** / **purpose.md** | index.md✓ log.md✓ SCHEMA.md✓（`orientation/` 已连线）；overview.md✗ purpose.md✗ | **真差距**：缺 overview/purpose |
| **图谱相关性** | 4 信号：直链×3 / 源重叠×4 / Adamic-Adar×1.5 / 类型亲和×1（结构拓扑） | recency + reinforcement + MMR + RRF（时序/行为，检索期），无拓扑信号 | **真差距**：缺拓扑评分 |
| **社区检测** | Louvain + 内聚度评分 | 无 | **真差距** |
| **图谱洞察** | 惊喜连接 + 知识缺口（孤岛/稀疏社区/桥节点） | 仅 `note_weave` 孤岛重连 | **真差距**：缺桥/稀疏/惊喜 |
| **Vault frontmatter** | 每页 `type` / `title` / `aliases` / `tags` / `sources` | `to_markdown` 发 `category/tags/created/updated/confidence/severity/source_notes/status/...`，**无 `type`/`title`/`aliases`** | **真差距**：vault 兼容缺字段 |
| **`.obsidian/` 配置** | 自动生成 | 无 | **真差距** |
| **schema → prompt** | schema 注入 LLM | `SchemaDoc::compact_for_prompt()` 存在但**零调用**（`orientation/schema.rs:47`，仅测试） | **真差距**：休眠未连线 |
| ~~wikilink 管道别名~~ | `[[t\|alias]]` | **已修复**：`wikilink.rs:9` 正则已支持 + `extract_wikilinks_with_alias` + 完整测试 | ✅ 非差距（NOTES.md §5.1 陈旧） |
| ~~源/关系 frontmatter 绑定~~ | `sources: []` | **已绑定**：`KnowledgeNote` 含 `source_notes` + **typed `relations`**（`to`/`type`/`confidence`，镜像进 `notes_links.relation`）— 比 wikilink 更强 | ✅ 非差距（NOTES.md §3/§8 陈旧） |
| ~~矛盾/陈旧检测~~ | "flag contradictions" | **已超越**：`note_drift`（矛盾+陈旧）+ 治理 `status/supersedes/superseded_by/contradicted` | ✅ 非差距 |
| 两步 Ingest | analysis→generation | compound ingestor（planner + 语义去重 mem0-style）| ✅ Aleph ≥ 参考 |

**结论**：Aleph note 层在**治理、provenance、typed relations、dream 维护**上已显著领先参考项目。真正的差距收敛为**图拓扑智能**（4 信号/Louvain/洞察）+ **vault 兼容外壳**（frontmatter/`.obsidian`/overview/purpose）+ **少量连线**（compact_for_prompt）。本设计据此聚焦，不重复造已有能力。

---

## 2. 三大根本决策（已与用户确认）

1. **图算法**：全盘移植参考算法（Louvain 社区检测 + Adamic-Adar + 4 信号 + 图谱洞察），用 Rust 类型安全 + 标准库线程（std::thread::scope） 并发实现，定位为**检索/分析基础设施**（与向量检索同级的赋能层），性能力争超越 TS 参考。
2. **标准化目标**：**Vault 级字节兼容** — 同一 `~/.aleph/memory/note/{agent_id}/` 目录可同时被 Obsidian / llm_wiki 打开。**类目（preference/plan/...）保留为子目录**，不改名 entities/concepts（vault 兼容不需要）。
3. **overview/purpose 来源**：**新建独立 LLM 维护文档**，由专用 dream 阶段读全语料生成维护，与 Soul/Goal 解耦。
4. **集成方案**：**A+B 混合** — 独立 `graph/` 模块 + `notes_graph_cache` 物化表（标准库线程（std::thread::scope） 并发重算兑现性能超越）+ 4 信号注入检索 + insights 连 dream/tool。**暂缓 Panel 可视化**（C，另开 spec）。

---

## 3. Goals / Non-goals

### Goals
- G1 Vault 字节兼容：`to_markdown` + 解析器补 `type`/`title`/`aliases`；自动生成 `.obsidian/` 配置。
- G2 新增 `overview.md` + `purpose.md`，由新 dream 阶段 LLM 维护。
- G3 新增 `src/memory/notes/graph/` 子系统：4 信号相关性 / 手写 Louvain / 图谱洞察。
- G4 `notes_graph_cache` 物化表 + 并发重算 dream 阶段。
- G5 连线：4 信号注入 `note_retrieval` 图扩展；insights 连 `note_lint`/`note_weave` + `note_manage` 新增只读 `Insights` action；`compact_for_prompt` 注入 prompt。
- G6 熵减：删 `frontmatter_template` 死代码三分叉；刷新陈旧的 NOTES.md。

### Non-goals（YAGNI 明确排除）
- Panel/Leptos 图谱可视化（sigma.js 等价物）— 另开 spec；图数据已由 `get_graph_data` 提供。
- MinerU/PDF/DOCX 解析、Web Clipper、Deep Research、向量搜索切换 — 参考项目的桌面应用特性，非 note 协议；Aleph 自有压缩式 ingestion + 已有向量/FTS/hybrid。
- 类目改名为 entities/concepts（vault 兼容不需要，且破坏现有 agent 记忆）。
- 多语言 UI、KaTeX、思考块显示等参考 UI 特性。

---

## 4. 详细设计（7 区块）

### 区块 1 · Vault 兼容 frontmatter〔G1 · 标准化〕

**真实写路径是 `KnowledgeNote::to_markdown`**（`note/mod.rs:167`），**不是** `frontmatter_template`（后者仅测试用，见区块 7）。

- `note/mod.rs::to_markdown`：在现有字段基础上**新增发射**（保持其余字段顺序与字节不变，纯追加）：
  - `type: {category}` — llm_wiki/Obsidian 页面类型，镜像 category（单一真源仍是目录）。
  - `title: {title}` — 当前 title 仅来自文件名；显式写入 frontmatter 供 Obsidian 显示。
  - `aliases: []` — Obsidian 别名机制，默认空。
- `note/parsing.rs::Frontmatter`：新增 `#[serde(default)]` 字段 `note_type: Option<String>`(`#[serde(rename="type")]`)、`title: Option<String>`、`aliases: Vec<String>`，解析绑定进 `KnowledgeNote`（`title` 缺省回落文件名，保持向后兼容）。
- **向后兼容**：所有新字段 `#[serde(default)]`；旧笔记无这些字段 → 解析为缺省，零迁移。新字段对旧 Obsidian/llm_wiki 是良性元数据。
- `KnowledgeNote` 结构新增 `aliases: Vec<String>`、`note_type: Option<String>`（`title` 已有）。

> **源重叠语义对齐**：llm_wiki `sources: []` 指原始源文件；Aleph `source_notes` 指笔记 provenance（哪些 note/raw-memory 产出本笔记）。二者都表达"共享来源 → 相关"。本设计**复用 `source_notes`** 作为源重叠信号载体，不强行引入 llm_wiki 的文件级 sources（Aleph 笔记来自对话压缩而非文档导入，文件级 sources 无来源）。`to_markdown` 可选地同时以 `sources:` 别名发射 `source_notes` 以增强 llm_wiki 互读（reviewable）。

### 区块 2 · overview.md + purpose.md〔G2 · 增强〕

- `orientation/` 新增 `overview_md.rs`（`OverviewMdGenerator`）、`purpose_md.rs`（`PurposeDoc`），读/写/tail 接口与 `index_md.rs`/`log_md.rs` 同构，落盘到 `note/{agent_id}/overview.md`、`note/{agent_id}/purpose.md`（vault 根，与 index/log 同级）。
- 新 dream 阶段 `CorpusNarrativeStage`（`dreaming/stages/corpus_narrative.rs`），注册进 **Synthesize** 策略（高增长周期，`dreaming/mod.rs:199`），可能列入 `GLOBAL_ONLY_STAGES`（overview/purpose 跟随 agent 语料，不按 project 分叉）：
  - 读 index.md + log 尾部 + 采样语料（top-N by recency/severity）+ 现有 overview/purpose。
  - 一次 LLM 调用：重写 `overview.md`（整库综述 + 演化主题），增量维护 `purpose.md`（仅实质变化才改写，幂等；首跑由 LLM 从语料推断 purpose）。
  - R7/R9 对账：综述是 LLM 擅长的语义生成，**不**用确定性代码替代。
- `fs_orientation.rs::read_snapshot` 扩展为携带 overview/purpose 摘要；prompt 注入见区块 5（H）。
- agent 可经 `note_manage`/`self_config` 手工编辑 overview/purpose（R8）。

### 区块 3 · 图智能子系统 `src/memory/notes/graph/`〔G3 · 增强 · 全盘移植〕

新模块目录 `src/memory/notes/graph/`：

- **`relevance.rs` — 4 信号相关性模型**：
  - `direct_link ×3.0` — `notes_links`（含 typed `relation` 边 + `to_raw` wikilink 边）。
  - `source_overlap ×4.0` — 两笔记 `source_notes`（经 `notes_sources` 索引，区块 4）交集大小。
  - `adamic_adar ×1.5` — `Σ 1/ln(deg(c))` 遍历公共邻居 c（二阶邻居强度）。
  - `type_affinity ×1.0` — 同 category 加成。
  - 权重走 `[memory.graph]` config 可调（默认对齐参考 3/4/1.5/1）。
  - `related(seed, k) -> Vec<(path, score)>`；候选对评分用 `std::thread::scope` 分块并行（在 `spawn_blocking` 内），无新依赖。
- **`community.rs` — 手写 Louvain**：模块度最大化双阶段（局部移动 + 社区聚合迭代至收敛）。**不引入图算法 crate**（守 R3 核心轻量化，~200 行）。`cohesion = 实际内部边 / 可能内部边`。输出 `node -> community_id` + 每社区 cohesion。
- **`insights.rs` — 图谱洞察**：
  - 孤岛 `isolated`：degree ≤ 1。
  - 稀疏社区 `sparse`：cohesion < 0.15 且 ≥ 3 节点。
  - 桥节点 `bridge`：邻接 ≥ 3 个不同社区。
  - 惊喜连接 `surprising`：跨社区 / 跨类型边 + 复合 surprise 评分。
  - 返回 `GraphInsights { isolated, sparse, bridge, surprising }`。
- 模块纯函数化、输入为图快照（节点+边+source_notes+category），便于单测且与存储解耦（P4）。

### 区块 4 · 物化缓存 + 并发重算〔G4 · 增强〕

- 新 SQLite 表（DDL 加进 `store/sqlite/schema/ddl.rs`，init 加进 `schema/mod.rs`，全 `CREATE IF NOT EXISTS` 幂等）：
  - `notes_sources(agent_id, note_path, source_ref, UNIQUE(agent_id,note_path,source_ref))` — 源重叠信号的快速 join 面，`index_note` 落盘时从 `source_notes` 填充（镜像 `notes_links` 的写法，`store/sqlite/notes.rs`）。
  - `notes_graph_cache(agent_id, node_path, community_id, cohesion, degree, updated_at, PRIMARY KEY(agent_id,node_path))` + `notes_graph_insights(agent_id, kind, payload_json, created_at)` — 物化社区/度/洞察。
- 新 dream 阶段 `GraphRecomputeStage`（`dreaming/stages/graph_recompute.rs`），注册进 **Consolidate + Conserve**（与 `IndexRefresherStage` 同周期，`dreaming/mod.rs:179,229`）：
  - 读全图（`get_graph_data` + `notes_sources`）→ 标准库线程（std::thread::scope） 并发跑 4 信号 + Louvain + insights → upsert 物化表。
  - **这是"超越参考性能"的发力点**：参考项目在 JS 主线程串行计算，Aleph 用 标准库线程（std::thread::scope） 数据并行。
  - 纯确定性聚合，零 LLM 调用（R7/R10 安全：定位为分析基础设施）。
- `NoteIndexer::full_rebuild` 路径不受影响（物化表可由 recompute 重建）。

### 区块 5 · 连线：检索 + dream + 工具 + prompt〔G5 · 连线〕

- **检索（4 信号注入）**：`note_retrieval/mod.rs` 图扩展相改读 `notes_graph_cache` 的相关性边（或调 `graph::relevance::related`）做加权 2-hop 种子扩展；**保留** recency/reinforcement/MMR 于末端打分（互补，不替换 — 时序行为 ⊕ 结构拓扑）。
- **dream（insights 消费）**：`note_weave` 改读 insights 缓存的孤岛集（取代其临时 orphan SQL）；`note_lint` 报告桥/稀疏健康。统一健康来源（熵减）。
- **工具（R8）**：`note_manage` 新增只读 action `Insights`（`NoteManageAction::Insights`），返回 `GraphInsights`（知识缺口/桥节点/惊喜连接），让 LLM 自然语言驱动维护。`call()` 与 `record_lifecycle_event` 的 `Query|List => return` 臂一并加 `Insights`。
- **prompt（H · 连线休眠）**：`SchemaDoc::compact_for_prompt()`（`orientation/schema.rs:47`）注入系统 prompt（随 orientation 快照），消除休眠。

### 区块 6 · Obsidian vault 兼容〔G1 · 标准化〕

- `orientation/` 新增 `ensure_obsidian_config(agent_id)`：一次性写 `note/{agent_id}/.obsidian/{app.json, graph.json, core-plugins.json}`（镜像 llm_wiki 自动生成），使目录可直接以图谱视图 + wikilink 打开。在 orientation bootstrap 处调用（`fs_orientation.rs`）。

### 区块 7 · 熵减 / 死代码 / 文档〔G6 · 修复+连线〕

- **删** `note_manage.rs::frontmatter_template`（813-827）+ 其 3 个测试（`test_frontmatter_*`）：经核实是**测试用死代码**，真实写路径是 `to_markdown`；保留只会与 `to_markdown` 双源漂移。
- **连线** `compact_for_prompt`（区块 5 H）。
- **刷新** `docs/reference/memory/NOTES.md` 陈旧章节：§3（frontmatter 已含 governance/relations/source_notes）、§5.1（管道别名已支持）、§8（notes_links 已含 to_raw/relation 列）、§9（`src/wiki/` 已删除，index/log 由 orientation 生成）、§12.3；新增 graph 子系统 + overview/purpose + vault 兼容章节。

---

## 5. 数据模型与 Schema 变更汇总

| 变更 | 文件 | 类型 |
|---|---|---|
| `KnowledgeNote` 加 `aliases`/`note_type` | `notes/note/mod.rs` | 结构扩展（向后兼容 default） |
| `Frontmatter` 加 `title`/`aliases`/`note_type` | `notes/note/parsing.rs` | 解析扩展 |
| `to_markdown` 发 `type`/`title`/`aliases` | `notes/note/mod.rs` | 序列化（纯追加） |
| `notes_sources` 表 | `store/sqlite/schema/{ddl,mod}.rs` | 新表（幂等） |
| `notes_graph_cache` / `notes_graph_insights` 表 | `store/sqlite/schema/{ddl,mod}.rs` | 新表（幂等） |
| `index_note` 填充 `notes_sources` | `store/sqlite/notes.rs` | 写路径扩展 |
| `NoteStore` 加 graph/source 方法 | `notes/store.rs` + `store/sqlite/notes.rs` | trait 扩展 |

---

## 6. 新增模块布局

```
src/memory/notes/graph/
├── mod.rs            # GraphSnapshot + 对外 API
├── relevance.rs      # 4 信号（标准库线程（std::thread::scope） 并行）
├── community.rs      # 手写 Louvain + cohesion
├── insights.rs       # isolated/sparse/bridge/surprising
└── tests.rs

src/memory/notes/orientation/
├── overview_md.rs    # 新增
└── purpose_md.rs     # 新增

src/memory/dreaming/stages/
├── corpus_narrative.rs   # 新增（LLM 维护 overview/purpose，Synthesize）
└── graph_recompute.rs    # 新增（标准库线程（std::thread::scope） 并发重算物化表，Consolidate+Conserve）
```

---

## 7. 红线对账（Redline Accounting）

| 红线 | 对账 |
|---|---|
| **R3 核心轻量化** | 手写 Louvain，不引图算法 crate；并发用**标准库 `std::thread::scope`**（在 `tokio::task::spawn_blocking` 内），**零新增依赖**（rayon 不在依赖树，刻意不引）。 |
| **R7 LLM 主权** | 图算法定位为**检索/分析基础设施**（与向量检索同级赋能层，用户已确认全盘移植）；overview/purpose **语义综述仍 LLM 驱动**，不用确定性代码替代推理。 |
| **R8 工具即一切** | insights 经 `note_manage(action=insights)` 暴露给 LLM 自然语言驱动。 |
| **R10 笨循环** | 全部改动在 `src/memory/`，**零触碰 `src/harness/`**；新 dream 阶段是离线维护非循环内推理。 |
| **P4 依赖倒置** | graph 模块输入为图快照，与存储解耦；`NoteStore` trait 扩展，SQLite 为可替换实现。 |
| **P6 简洁性** | 删 frontmatter_template 死代码；复用 source_notes/notes_links 既有基础设施，不另造源系统。 |

---

## 8. 构建顺序（细节交由 writing-plans 展开）

1. **修复+熵减先行**（低风险）：删 frontmatter_template 死代码；连线 compact_for_prompt；刷新 NOTES.md。
2. **Vault 兼容外壳**：to_markdown/parser frontmatter 字段；`.obsidian` 配置。
3. **图模块（纯函数）**：graph/{relevance,community,insights} + 单测（输入图快照，不碰存储）。
4. **物化层**：notes_sources/notes_graph_cache 表 + index_note 填充 + NoteStore 方法。
5. **并发重算阶段**：GraphRecomputeStage（标准库线程（std::thread::scope））注册。
6. **overview/purpose**：orientation 生成器 + CorpusNarrativeStage。
7. **连线**：4 信号注入检索；insights 连 note_weave/note_lint + note_manage Insights action；prompt 注入。

---

## 9. 测试与风险

- **测试**：按 TDD 为 graph 纯函数（4 信号数值、Louvain 收敛、insights 分类）、frontmatter 往返、Insights action 写单测。**但遵用户约束不跑 cargo**，测试随代码提交，由后续 CI/人工验证。
- **风险 R1**：`to_markdown` 改动影响所有笔记序列化字节 → 纯追加字段 + 保持既有字段顺序，旧笔记 round-trip 仅多出 type/title/aliases（Obsidian/llm_wiki 良性）。
- **风险 R2**：未跑 cargo → 实施时逐字面量核对 match 臂（Insights action 需改 `call()` + `record_lifecycle_event` 两处）、trait 新方法的所有 impl、新 enum 变体的穷尽匹配；提交前静态自审。
- **风险 R3**：物化表与 markdown 漂移 → recompute 幂等重建 + `full_rebuild` 兜底。
- **风险 R4**：并发 main 推进 → 手动 worktree 隔离，合并时 merge main 入分支再 --no-ff（不用 EnterWorktree，避免 fresh-base 丢提交 / CWD 重置）。

---

## 附：被证伪的初始假设（教训）

落 spec 前以代码为准核实，推翻了 4 处基于陈旧 NOTES.md 的假设：① 管道别名已修；② source/relations 已绑定且 typed-relations 比参考更强；③ frontmatter_template 是死代码非真实写路径；④ notes_links 已含 to_raw/relation 列。**未读真实代码就照文档落 spec 会凭空造出已存在的能力。**
