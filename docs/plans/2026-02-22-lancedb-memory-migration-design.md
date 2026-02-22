# LanceDB 统一记忆存储设计

> **Date**: 2026-02-22
> **Status**: Approved
> **Scope**: 记忆系统存储层重构 — SQLite + sqlite-vec → LanceDB 统一存储

---

## 1. 背景与动机

Aleph 当前的记忆系统使用 SQLite + sqlite-vec + FTS5 三层组合：
- **SQLite**: 结构化元数据存储（15+ 张表、40+ 索引）
- **sqlite-vec**: 向量相似性搜索（L2 距离 KNN）
- **FTS5**: 全文关键词搜索（BM25 排序）

这种架构存在以下问题：
1. **数据分散** — 同一个 Fact 的内容在 `memory_facts`（BLOB 嵌入）、`facts_vec`（向量索引）、`facts_fts`（全文索引）三处冗余存储
2. **同步复杂** — 写入/更新需要同时操作三个存储位置，任一失败导致不一致
3. **能力受限** — sqlite-vec 的 ANN 索引能力有限，不支持 HNSW/IVF-PQ 等高级算法
4. **扩展困难** — 多维度嵌入需要为每个维度创建独立的 vec0 虚拟表

## 2. 方案选择

### 调研结论

LanceDB (v0.26.2) 调研结果：

| 能力 | 评估 | 说明 |
|------|------|------|
| FTS | 完全可用 | BM25、jieba 中文分词、词干提取、停用词、短语搜索、模糊搜索 |
| 混合搜索 | 原生支持 | `nearest_to()` + `full_text_search()` + RRF 融合，单次 API 调用 |
| Rust API | 成熟 | 完全 async，基于 Apache Arrow，支持多向量列不同维度 |
| 嵌入式 | 类 SQLite | 零服务器，文件目录存储 |
| 版本控制 | 一流 | 自动 MVCC，时间旅行，秒级回滚 |
| 结构化查询 | 受限 | 仅过滤表达式，无 JOIN/聚合/子查询 |

### 图谱查询复杂度分析

对现有 `graph.rs` 的代码审查表明，**实际图谱查询全部为 1-hop 单表查找**：

| 查询模式 | 跳数 | 说明 |
|---------|------|------|
| 实体名/别名查找 | 1-hop | FTS 匹配节点名 |
| 上下文边计数 | 1-hop | 过滤 `from_id` 或 `to_id` |
| 记忆-实体关联 | 1-hop | 通过 `node_id` 过滤 |
| 衰减清理 | 全表扫描 | 每日一次 |
| 共现边创建 | 仅写入 | 无查询 |

**无任何多跳遍历、递归 CTE、BFS/DFS**。这意味着 LanceDB 的元数据过滤能力完全足够。

### 方案评估

| 方案 | 描述 | 结论 |
|------|------|------|
| **A: LanceDB 统一存储** | 彻底舍弃 SQLite，所有记忆数据集中 LanceDB | **选定** |
| B: LanceDB + SQLite 图谱 | 双数据库，LanceDB 做 Facts，SQLite 做图谱 | 图谱实际无需 JOIN，过度设计 |
| C: LanceDB + Tantivy FTS | LanceDB + 独立 FTS 引擎 | LanceDB 内建 FTS 已足够成熟 |

## 3. 架构设计

### 3.1 总体架构

```
┌───────────────────────────────────────────────────────────────┐
│                    Aleph Memory System                        │
├───────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌─────────────────────────────────────────────────────────┐  │
│  │                  Service Layer                          │  │
│  │  HybridRetrieval │ CompressionDaemon │ EmbeddingMigration │
│  │  GraphLogic │ VFS │ Augmentation │ LazyDecay            │  │
│  └──────────────────────────┬──────────────────────────────┘  │
│                             │                                 │
│  ┌──────────────────────────┴──────────────────────────────┐  │
│  │               Trait Abstraction Layer                    │  │
│  │  MemoryStore │ GraphStore │ SessionStore                │  │
│  └──────────────────────────┬──────────────────────────────┘  │
│                             │                                 │
│  ┌──────────────────────────┴──────────────────────────────┐  │
│  │            LanceMemoryBackend                           │  │
│  │  (Unified LanceDB implementation)                       │  │
│  │                                                         │  │
│  │  LanceDB (~/.aleph/memory.lance/)                       │  │
│  │  ├── facts          # Content + metadata + vectors + FTS│  │
│  │  ├── graph_nodes    # Entity nodes                      │  │
│  │  ├── graph_edges    # Entity relationships              │  │
│  │  └── memories       # Raw session logs                  │  │
│  └─────────────────────────────────────────────────────────┘  │
│                                                               │
└───────────────────────────────────────────────────────────────┘
```

### 3.2 数据模型 — LanceDB Tables

#### Table: `facts`

| Column | Type | Index | Description |
|--------|------|-------|-------------|
| id | Utf8 | Scalar | UUID |
| content | Utf8 | FTS (jieba) | Fact 文本内容 |
| fact_type | Utf8 | Scalar | Preference/Plan/Learning/... |
| fact_source | Utf8 | - | Extracted/Summary/Document/Manual |
| specificity | Utf8 | - | Principle/Pattern/Instance |
| temporal_scope | Utf8 | - | Permanent/Contextual/Ephemeral |
| path | Utf8 | Scalar | aleph:// VFS 路径 |
| parent_path | Utf8 | Scalar | 父路径 |
| namespace | Utf8 | Scalar | owner/guest:xxx/shared |
| tags | List\<Utf8\> | - | 标签列表 |
| source_memory_ids | List\<Utf8\> | - | 来源记忆 ID |
| content_hash | Utf8 | - | L1 过期检测 |
| confidence | Float32 | - | 0.0-1.0 |
| decay_score | Float32 | - | 衰减分数 |
| is_valid | Boolean | Scalar | 软删除标志 |
| invalidation_reason | Utf8 | - | 失效原因 (nullable) |
| embedding_model | Utf8 | - | 当前嵌入模型 ID |
| created_at | Int64 | Scalar | Unix timestamp |
| updated_at | Int64 | - | 最后更新 |
| decay_invalidated_at | Int64 | - | 衰减失效时间 (nullable) |
| version | Int32 | - | Fact 版本号 |
| vec_384 | FixedSizeList\<f32, 384\> | ANN (IVF-PQ) | e5-small 等 |
| vec_1024 | FixedSizeList\<f32, 1024\> | ANN (IVF-PQ) | bge-large 等 (nullable) |
| vec_1536 | FixedSizeList\<f32, 1536\> | ANN (IVF-PQ) | OpenAI 等 (nullable) |

#### Table: `graph_nodes`

| Column | Type | Index | Description |
|--------|------|-------|-------------|
| id | Utf8 | Scalar | UUID |
| name | Utf8 | FTS | 实体名称 |
| kind | Utf8 | Scalar | person/project/tech/... |
| aliases | List\<Utf8\> | FTS | 别名列表 (合并原 graph_aliases) |
| metadata | Utf8 | - | JSON 元数据 |
| decay_score | Float32 | - | 衰减分数 |
| created_at | Int64 | - | |
| updated_at | Int64 | Scalar | |

#### Table: `graph_edges`

| Column | Type | Index | Description |
|--------|------|-------|-------------|
| id | Utf8 | Scalar | UUID |
| from_id | Utf8 | Scalar | 源节点 ID |
| to_id | Utf8 | Scalar | 目标节点 ID |
| relation | Utf8 | - | co_occurs/related/entity_mention/... |
| weight | Float32 | - | 关系权重 |
| confidence | Float32 | - | 置信度 |
| context_key | Utf8 | Scalar | 上下文键（消歧用） |
| decay_score | Float32 | - | |
| created_at | Int64 | - | |
| updated_at | Int64 | - | |
| last_seen_at | Int64 | - | |

#### Table: `memories`

| Column | Type | Index | Description |
|--------|------|-------|-------------|
| id | Int64 | Scalar | 自增 ID |
| app_bundle_id | Utf8 | Scalar | 应用标识 |
| window_title | Utf8 | - | 窗口标题 |
| user_input | Utf8 | FTS | 用户输入 |
| ai_output | Utf8 | FTS | AI 输出 |
| timestamp | Int64 | Scalar | 时间戳 |
| topic_id | Utf8 | - | 话题 ID (nullable) |
| session_key | Utf8 | Scalar | 会话键 |
| namespace | Utf8 | Scalar | 命名空间 |
| vec_384 | FixedSizeList\<f32, 384\> | ANN | 会话嵌入 |

### 3.3 Trait 架构

```rust
/// Core memory storage — Facts CRUD + search
#[async_trait]
pub trait MemoryStore: Send + Sync {
    // CRUD
    async fn insert_fact(&self, fact: &MemoryFact) -> Result<()>;
    async fn get_fact(&self, id: &str) -> Result<Option<MemoryFact>>;
    async fn update_fact(&self, fact: &MemoryFact) -> Result<()>;
    async fn delete_fact(&self, id: &str) -> Result<()>;
    async fn batch_insert_facts(&self, facts: &[MemoryFact]) -> Result<()>;

    // Vector search
    async fn vector_search(
        &self, embedding: &[f32], dim_hint: u32,
        filter: &SearchFilter, limit: usize,
    ) -> Result<Vec<ScoredFact>>;

    // Full-text search
    async fn text_search(
        &self, query: &str, filter: &SearchFilter, limit: usize,
    ) -> Result<Vec<ScoredFact>>;

    // Hybrid search (vector + FTS + RRF fusion)
    async fn hybrid_search(
        &self, embedding: &[f32], dim_hint: u32,
        query_text: &str, config: &HybridSearchConfig,
        filter: &SearchFilter,
    ) -> Result<Vec<ScoredFact>>;

    // VFS path operations
    async fn list_by_path(&self, parent_path: &str, ns: &NamespaceScope) -> Result<Vec<MemoryFact>>;
    async fn get_by_path(&self, path: &str, ns: &NamespaceScope) -> Result<Option<MemoryFact>>;

    // Statistics
    async fn count_facts(&self, filter: &SearchFilter) -> Result<usize>;
    async fn get_facts_by_type(&self, ft: FactType, ns: &NamespaceScope, limit: usize) -> Result<Vec<MemoryFact>>;
}

/// Graph storage — entity nodes and relationships
#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn upsert_node(&self, node: &GraphNode) -> Result<()>;
    async fn upsert_edge(&self, edge: &GraphEdge) -> Result<()>;
    async fn resolve_entity(&self, query: &str, context_key: Option<&str>) -> Result<Vec<ResolvedEntity>>;
    async fn get_edges_for_node(&self, node_id: &str, context_key: Option<&str>) -> Result<Vec<GraphEdge>>;
    async fn apply_decay(&self, policy: &GraphDecayPolicy) -> Result<DecayStats>;
}

/// Raw session log storage
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn insert_memory(&self, memory: &RawMemory) -> Result<i64>;
    async fn search_memories(&self, embedding: &[f32], filter: &MemoryFilter, limit: usize) -> Result<Vec<RawMemory>>;
    async fn get_memories_for_entity(&self, entity_id: &str, limit: usize) -> Result<Vec<RawMemory>>;
}
```

`LanceMemoryBackend` 统一实现上述三个 trait。

### 3.4 模块结构

```
core/src/memory/
├── mod.rs                    # Module exports
├── context.rs                # MemoryFact, GraphNode, GraphEdge (domain models)
├── namespace.rs              # NamespaceScope (unchanged)
│
├── store/                    # NEW: Storage abstraction layer
│   ├── mod.rs                # Trait definitions (MemoryStore, GraphStore, SessionStore)
│   ├── types.rs              # SearchFilter, ScoredFact, HybridSearchConfig
│   └── lance/                # LanceDB implementation
│       ├── mod.rs            # LanceMemoryBackend constructor & init
│       ├── facts.rs          # MemoryStore impl (CRUD + search)
│       ├── graph.rs          # GraphStore impl (nodes + edges)
│       ├── sessions.rs       # SessionStore impl (session logs)
│       ├── schema.rs         # Arrow Schema definitions
│       └── arrow_convert.rs  # MemoryFact <-> RecordBatch conversion
│
├── embedding_provider.rs     # EmbeddingProvider trait (unchanged)
├── smart_embedder.rs         # SmartEmbedder (unchanged)
├── embedding_migration.rs    # Refactor: adapt to new MemoryStore trait
│
├── hybrid_retrieval/         # Refactor: use MemoryStore::hybrid_search
│   ├── hybrid.rs             # HybridRetrieval (calls trait methods)
│   └── strategy.rs           # RetrievalStrategy (unchanged)
│
├── retrieval.rs              # Refactor: use new traits
├── graph.rs                  # Refactor: business logic over GraphStore trait
├── vfs/                      # Refactor: use MemoryStore::list_by_path
├── compression/              # Refactor: adapt to new interface
├── lazy_decay.rs             # Refactor: adapt to new interface
├── augmentation.rs           # Unchanged
└── ...                       # Other modules: incremental adaptation
```

### 3.5 搜索流程

#### 混合检索

```
Query: "Aleph 的 Gateway 用了什么协议？"
         │
         ▼
  EmbeddingProvider::embed(query) → vec[384]
         │
         ▼
  LanceDB single hybrid search:
    facts_table.query()
      .nearest_to(vec)                    // ANN on vec_384
      .full_text_search("Gateway 协议")   // BM25 on content
      .only_if("is_valid = true AND namespace = 'owner'")
      .rerank(RRFReranker::new())         // Score fusion
      .limit(10)
      .execute()
         │
         ▼
  Optional: Graph-assisted enrichment
    If query contains @Entity hints:
      1. graph_nodes FTS → resolve to node_id
      2. graph_edges filter(from_id|to_id) → related entity IDs
      3. Expand search filter with related IDs
         │
         ▼
  Return Vec<ScoredFact>
```

#### 图谱实体解析

```rust
// Entity resolution via LanceDB metadata filtering
async fn resolve_entity(&self, query: &str, context_key: Option<&str>) -> Result<Vec<ResolvedEntity>> {
    // Step 1: FTS lookup on node name + aliases
    let candidates = self.nodes_table.query()
        .full_text_search(FullTextSearchQuery::new(query.to_owned()))
        .limit(5)
        .execute().await?;

    // Step 2: Context-based disambiguation (if multiple candidates)
    if candidates.len() > 1 {
        if let Some(ctx) = context_key {
            for candidate in &mut candidates {
                let edge_count = self.edges_table.query()
                    .only_if(format!(
                        "context_key = '{}' AND (from_id = '{}' OR to_id = '{}')",
                        ctx, candidate.id, candidate.id
                    ))
                    .execute().await?.count();
                candidate.context_score = edge_count as f32;
            }
            candidates.sort_by(|a, b| b.context_score.total_cmp(&a.context_score));
        }
    }

    Ok(candidates)
}
```

### 3.6 多维度嵌入策略

**单表多向量列**：`facts` 表包含 `vec_384`、`vec_1024`、`vec_1536` 三个向量列。

- 每个 Fact 至少一个向量列非 null
- `embedding_model` 字段记录当前活跃模型
- 搜索时通过 `.column(&format!("vec_{}", dim))` 选择对应向量列
- 模型切换时，`EmbeddingMigration` 懒加载式填充新维度列
- 新增维度通过 LanceDB `add_columns` 动态扩展，无需重建表

### 3.7 初始化与降级

```rust
impl LanceMemoryBackend {
    pub async fn open_or_create(data_dir: &Path, config: &MemoryConfig) -> Result<Self> {
        let db = lancedb::connect(data_dir.join("memory.lance")).execute().await?;

        // Idempotent table creation
        let facts_table = ensure_table(&db, "facts", facts_schema(config)).await?;
        let nodes_table = ensure_table(&db, "graph_nodes", nodes_schema()).await?;
        let edges_table = ensure_table(&db, "graph_edges", edges_schema()).await?;
        let memories_table = ensure_table(&db, "memories", memories_schema(config)).await?;

        // Idempotent index creation
        ensure_fts_index(&facts_table, &["content"]).await?;
        ensure_fts_index(&nodes_table, &["name"]).await?;
        ensure_ann_index(&facts_table, config).await?;

        Ok(Self { db, facts_table, nodes_table, edges_table, memories_table })
    }
}
```

**降级策略**：
- LanceDB 连接失败 → 记忆系统标记 disabled，Agent 无记忆增强继续运行
- ANN 索引未就绪 → 自动回退暴力搜索
- FTS 索引未就绪 → 仅向量搜索

### 3.8 错误处理

```rust
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("LanceDB error: {0}")]
    Lance(#[from] lancedb::Error),

    #[error("Arrow conversion error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Fact not found: {0}")]
    NotFound(String),

    #[error("Schema mismatch: expected dim {expected}, got {actual}")]
    DimensionMismatch { expected: u32, actual: u32 },
}
```

### 3.9 配置

```toml
[memory]
enabled = true
backend = "lancedb"

[memory.embedding]
provider = "local"
model = "multilingual-e5-small"
dimension = 384

[memory.lancedb]
data_dir = "~/.aleph/memory.lance"
ann_index_type = "IVF_PQ"
ann_index_threshold = 50000
fts_tokenizer = "jieba"
fts_stem = true
fts_remove_stop_words = true
```

### 3.10 测试策略

1. **单元测试** — 每个 Trait 方法的独立测试，使用临时 LanceDB 目录
2. **集成测试** — 完整的 write → search → retrieve 流程
3. **图谱测试** — 实体创建 → 边创建 → 解析 → 消歧
4. **性能基准** — 对比旧 SQLite 方案在不同数据量下的延迟

## 4. 性能预期

| 操作 | 预期延迟 | 说明 |
|------|---------|------|
| 向量搜索 (10K facts) | < 5ms | 暴力搜索 |
| 向量搜索 (100K+ facts) | < 20ms | IVF-PQ 索引 |
| FTS 搜索 | < 10ms | BM25 索引 |
| 混合搜索 (ANN + FTS + RRF) | < 30ms | 单次 LanceDB 调用 |
| 实体解析 | < 15ms | FTS + 可选上下文过滤 |
| Fact 写入 | < 5ms | 追加写入 |

## 5. 风险与缓解

| 风险 | 缓解 |
|------|------|
| LanceDB 0.x API breaking changes | 锁定版本 + CI 监控 |
| FTS 中文分词质量不及 FTS5 | POC 阶段验证，退路为 Tantivy |
| 图谱未来需要复杂遍历 | Trait 抽象允许后续引入专用图谱后端 |
| 并发写冲突 | Aleph 为单用户系统，冲突概率极低 |
| 大数据量索引构建延迟 | `ann_index_threshold` 控制自动建索引时机 |

## 6. 非目标

- 不做 SQLite 数据迁移（从零开始）
- 不实现多跳图遍历算法
- 不构建自定义 Reranker（使用 LanceDB 内置 RRF）
- 不引入第三方 FTS 引擎（除非 LanceDB FTS 验证失败）
