# Note 关系网络"接通"重构 — Design Spec

> Date: 2026-06-26 · Topic: memory note 层关系连线（让孤立 note 形成有用的关系网络）
> Reference project: `codebase-memory-mcp` (C, code-graph MCP server)
> Status: design approved, pending plan

## 0. 北极星 (North Star)

Aleph 的 note 关系网络**算得很足，却在两端漏水**。本重构不堆新算法，只做两件事：

1. **接通 (Wire)** — 把已计算的 typed / directed / weighted 边，从投影端到消费端**全程贯通**，让今天"写入即弃 / 物化即弃"的死数据变活。
2. **一项 Rust 增强 (Surpass)** — 新增 MinHash + LSH 零嵌入相似边发现，喂进同一张图，复用既有消费链。

所有改动落在 `src/memory/notes/graph/`、`src/memory/note_retrieval/`、`src/memory/notes/orientation/`、`src/memory/store/sqlite/notes/`。**不碰 `src/harness/`（R10）、不碰 core 调度、不引新依赖（R3）。**

## 1. Gap Analysis（扫描结论，已核实 file:line）

| # | 维度 | 参考 codebase-memory-mcp | Aleph 现状 | 裁定 |
|---|------|----|----|----|
| 1 | 边模型 | typed+directed+confidence+strategy | `notes_links{relation,to_raw}` + frontmatter `Relation{to,rel_type,confidence}` 全存 | 数据齐 ✅ |
| 2 | **图投影** | 全程保留类型/方向/权重 | `load_graph_snapshot` (`store_impl.rs:1100`) 只 `SELECT from_note,to_note`，丢 `relation`；`GraphIndex.adj` 无向化 (`mod.rs:60-61`)；`GraphNode` 不带 rel_type | 🔴 **核心管涌点** |
| 3 | 相关性 | 11-signal + diffusion | 4-signal (direct×3 / source-overlap-IDF×4 / Adamic-Adar×1.5 / type-affinity×1)；direct-link 是布尔 (`relevance.rs:42`) | 算法强、输入被阉割 |
| 4 | 自动连线 | 多 pass | 显式 `[[wikilink]]` + `relations` frontmatter + keyword-overlap(LLM) + 写入期语义去重 | 语义层领先 ✅ |
| 5 | 零嵌入相似 | MinHash+LSH | 仅嵌入余弦 + keyword overlap，**无 MinHash** | 🟡 可移植增强 |
| 6 | 社区检测 | Leiden | Louvain (手写/确定性) | 对齐 ✅ |
| 7 | **图洞察消费** | degree ranking / impact | insights(isolated/sparse/**bridge/surprising**) 已物化，**只有 isolated 被消费，其余物化即弃** | 🔴 消费端孤儿 |
| 8 | **反向链接** | inbound degree=hub | `get_incoming_links_any` 已算，**从不进 retrieval/orientation/prompt** | 🔴 消费端孤儿 |
| 9 | 检索图扩展 | include_connected | `graph_expand` 默认开 (top-3 seed→8 peer)，物化 4-signal (`note_retrieval/mod.rs:366`) | Aleph 领先 ✅ |

**核心**：瓶颈不在算法，在连线。`notes_links.relation`（rel_type）+ `Relation.confidence`（confidence）+ `notes_graph_insights` 的 bridge/surprising/sparse + backlinks——四样东西都被计算出来，却从不到达 LLM。

## 2. 设计决策（已与用户敲定）

- **D1 范围 = B**：连线全部 + 一项 Rust 增强（MinHash/LSH），不做 Cypher 查询面。
- **D2 rel_type 角色 = 权重 + 结构性强边必出**：边权重 = `direct_link × confidence`，方向保留；极小白名单 `{supersedes, superseded_by, contradicts}` 的目标在检索命中时**无论分数强制带出并标注**。自由 rel_type 仍 LLM 自选（R7），仅这 3 个受特殊待遇。
- **D3 呈现位置 = 检索为主 + orientation 一行健康**：关系信号随被检索到的 note 随查呈现（scoped、零常驻税）；orientation `index.md` 只加一行图健康摘要。不做 per-note prompt dump。
- **D4 confidence 持久化 = 加列**：`notes_links` 加 `confidence REAL NOT NULL DEFAULT 1.0`，幂等 `ALTER TABLE ADD COLUMN` 迁移（仿 `migrate_recall_signals_note_path`）。

## 3. 模块设计

### 模块 1 — Edge un-flatten（typed/directed/weighted 贯通）★核心

| 改动点 | 文件:行 | 改法 |
|---|---|---|
| SQL 投影拉 relation+confidence | `store/sqlite/notes/store_impl.rs:1100` | `SELECT from_note,to_note,relation,confidence FROM notes_links WHERE ...` |
| 加 confidence 列 + 迁移 | `store/sqlite/schema.rs` | `ALTER TABLE notes_links ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0`（幂等，旧库安全） |
| index_note 落 confidence | `store/sqlite/notes/store_impl.rs`（index_note 写 notes_links 处） | 写入 `Relation.confidence`；body wikilink 默认 1.0 |
| edge 升级结构体 | `notes/graph/mod.rs:29` | `edges: Vec<(String,String)>` → `Vec<GraphEdge{from,to,rel_type:Option<String>,confidence:f32}>` |
| GraphIndex 双视图 | `notes/graph/mod.rs:33-84` | **保留**无向 `adj`（Adamic-Adar/community 不动）；**新增**有向带权 `adj_out`/`adj_in: Vec<HashMap<usize, EdgeMeta{rel_type,confidence}>>` |
| 4-signal 吃权重 | `notes/graph/relevance.rs:42` | direct-link `boolean` → `w.direct_link × max_confidence(a↔b)` |

**不变量**：community.rs / Adamic-Adar 继续用无向 `adj`（语义本就对称，零回归）。方向与权重是**并行新增层**。`GraphSnapshot` 仍 `Clone`/`Default`。

### 模块 2 — 结构性强边必出

- `STRUCTURAL_STRONG: &[&str] = &["supersedes","superseded_by","contradicts"]`（常量放 `notes/note/relation.rs`）。
- `note_retrieval/mod.rs:366` `graph_expand` 之后：对每个命中 hit，查 `adj_out` 中 rel_type ∈ 白名单的目标，**无论分数强制并入结果集**，渲染时打标注（如 `⚠ superseded_by → {target}`）。
- 复用既有 hydrate 路径（`expansion.rs` 已有 content 合并），不另写。

### 模块 3 — 消费端连线（backlinks + 死信号 surface）

- **检索结果标注**：每个 hit 渲染附 `← N 篇反链` + typed 出边摘要。数据源 `get_incoming_links_any`（已存在）+ 模块 1 的 `adj_out`。scoped 到被检索到的 note，零常驻 token。
- **orientation 一行健康**：`orientation/index_md.rs` 加一行 `图健康: 孤立 N · 桥接 N · 意外关联 N`，直读 `notes_graph_insights`（bridge/surprising/sparse 终于到达 LLM）。不做 per-note dump。

### 模块 4 — Rust 增强：MinHash+LSH 零嵌入相似边

- 新文件 `notes/graph/minhash.rs`：
  - K=64 MinHash over note body **word-level k-shingles (k=3)**：body 小写化、按空白切词、滑动 3 词成 shingle（Broder 文档相似经典做法，非 AST——note 是散文不是代码）。空/极短 body (<3 词) 退化为整体 token 集合。
  - LSH 分带 b=32 / r=2，O(n) 候选生成。
  - 精确 Jaccard ≥ 阈值（默认 **0.82**，配置 `memory.graph.minhash_threshold`）才出边；每节点封顶 **8 条**防 hub 爆炸。
- **并发**：走现有 `std::thread::scope` 模式（同 `relevance.rs:119` `all_related`），裹 `tokio::task::spawn_blocking`。**不引 rayon**（守 R3 不加依赖）。
- **落点**：`dreaming/stages/graph_recompute.rs` 增一个 MinHash 边源，与 4-signal 边一起写入 `notes_graph_related` → **自动流进 `graph_expand`**，零检索侧改动（OCP）。
- **副产物（可选、最小）**：near-dup 候选可喂 `dreaming/stages/note_consolidate.rs`，但主线只做相似边，不在本次扩张。

## 4. 熵减 / 清理（Entropy Reduction）

子代理核实**零孤儿函数**。这里的"死"是**死数据**而非死代码：

- `notes_links.relation` + `Relation.confidence`：今天写入即弃（从不被图读取）→ 本重构接通后变活。
- `notes_graph_insights` 的 bridge/surprising/sparse：物化即弃 → 模块 3 surface 后不再浪费计算。
- **无新增孤儿**：MinHash 边复用 `notes_graph_related` 表与 `graph_expand` 消费链，不另起平行管线。

## 5. 风险与约束（诚实声明）

- **不跑 cargo（用户强制约束）**：模块 1 的 edge 结构体 + schema 迁移有编译风险。缓解：改动外科化；为纯函数（relevance 加权 / minhash / structural-strong 过滤）加单测**（写但不运行）**；依赖 rust-analyzer 静态核对；提交说明如实标注"未编译验证"。
- **schema 迁移**：幂等 `ALTER TABLE ADD COLUMN`，旧库默认 1.0，安全。
- **方向引入**：必须确认 community/Adamic-Adar 仍走无向 `adj`，否则破坏既有社区检测语义。
- **worktree 路径 footgun**：worktree 内 Write 必须用 worktree 相对/真实路径，勿用 main 绝对路径误落 main。

## 6. Out of Scope (YAGNI)

- ❌ Cypher 风格图查询语言 / 新 LLM-facing `note_graph` 工具（D1 否决）。
- ❌ 完整 typed-affinity 矩阵（违 P6/R7）。
- ❌ orientation 常驻关系图概览（D3 否决，token 税）。
- ❌ MinHash 驱动的自动合并（仅留候选副产物，不做合并逻辑）。
- ❌ 任何 harness / core 调度改动。

## 7. 提交计划（4 commit，worktree 隔离）

全程在新建 worktree 分支，严禁触碰 main。

1. `graph: un-flatten typed/directed/weighted edges`（模块 1 + schema 迁移）
2. `retrieval: structural-strong must-surface + backlink annotation`（模块 2 + 模块 3 检索侧）
3. `orientation: graph-health line from materialized insights`（模块 3 orientation 侧）
4. `graph: MinHash/LSH zero-embedding similarity edges`（模块 4）

## 8. 验收标准 (Success Criteria)

- `load_graph_snapshot` 携带 rel_type + confidence；`GraphIndex` 暴露有向带权 `adj_out/adj_in`，无向 `adj` 保留。
- `relevance::score_pair` 的 direct-link 项随 confidence 缩放（单测覆盖）。
- 检索命中带结构性强关系时，目标无论分数必现并标注（单测覆盖过滤逻辑）。
- 检索结果含 backlink 计数；orientation `index.md` 含图健康行。
- `minhash.rs` 纯函数有单测：MinHash 估计 Jaccard、LSH 候选、阈值出边、节点封顶。
- 无新依赖、无 harness 改动、无孤儿函数。
