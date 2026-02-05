# Aleph 记忆系统演进设计：融合 OpenClaw 智慧

> 审核对象：Aleph Memory System
> 参考对象：OpenClaw Memory & Compaction
> 设计日期：2026-02-05

---

## 1. 背景与目标

### 1.1 现状分析

Aleph 已具备完善的双层记忆架构：
- **Raw Memories** (`memories` 表)：存储原始对话，支持向量检索
- **Compressed Facts** (`memory_facts` 表)：LLM 提取的结构化事实

**核心优势**：主动式上下文注入（Proactive Augmentation）

**关键差距**：
1. **检索入口缺失**：Agent 无法主动搜索历史记忆
2. **融合冗余**：Fact 与 Transcript 可能重复占用 Token
3. **压缩策略简单**：缺乏自适应的 Token 预算管理
4. **认知深度不足**：缺乏知识演进和冲突调和机制

### 1.2 设计目标

1. **保留主动注入优势**：Aleph 的"保姆式"体验是核心竞争力
2. **引入被动检索能力**：让 Agent 拥有 `memory_search` 工具
3. **实现 Token 精细化管理**：自适应压缩 + 上下文仲裁
4. **超越 OpenClaw**：涟漪式蒸馏 + 知识演进链条

---

## 2. 架构概览

```
┌─────────────────────────────────────────────────────────────────────┐
│                         INGESTION LAYER                              │
│   对话完成 → TranscriptIndexer (NRT) → ValueEstimator → Fact Queue  │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────────┐
│                         STORAGE LAYER                                │
│   memories + transcript_vec │ memory_facts + facts_vec │ graph_edges│
└───────────────────────────────┬─────────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────────┐
│                         RETRIEVAL LAYER                              │
│   memory_search Tool → Hybrid Retrieval → ContextComptroller        │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────────┐
│                         MAINTENANCE LAYER                            │
│   DreamDaemon: RippleTask │ RepairTask │ ConsolidationTask          │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 3. 核心组件设计

### 3.1 TranscriptIndexer（准实时索引器）

**职责**：在对话完成后立即向量化，建立可检索的 transcript 索引

**触发时机**：每次 Assistant 响应结束后（异步，不阻塞响应）

**延迟目标**：< 500ms

#### 数据模型

```sql
-- 新增 transcript_vec 虚拟表（基于 sqlite-vec）
CREATE VIRTUAL TABLE transcript_vec USING vec0(
    id INTEGER PRIMARY KEY,
    embedding FLOAT[384]
);

-- 扩展 memories 表，增加切片支持
ALTER TABLE memories ADD COLUMN chunk_index INTEGER DEFAULT 0;
ALTER TABLE memories ADD COLUMN chunk_total INTEGER DEFAULT 1;
```

#### 核心接口

```rust
pub struct TranscriptIndexer {
    db: Arc<MemoryDatabase>,
    embedder: Arc<SmartEmbedder>,
    chunk_config: ChunkConfig,
}

pub struct ChunkConfig {
    pub max_tokens: usize,      // 默认 400
    pub overlap_tokens: usize,  // 默认 80
}

impl TranscriptIndexer {
    /// 准实时索引单轮对话
    pub async fn index_turn(&self, memory_id: i64) -> Result<()>;

    /// 支持滑动窗口切片的索引
    pub async fn index_with_chunking(&self, memory_id: i64) -> Result<Vec<i64>>;
}
```

#### 与 CompressionService 的关系

- **独立服务**：TranscriptIndexer 与 CompressionService 平级
- **共享资源**：共用 MemoryDatabase 和 SmartEmbedder
- **触发频率不同**：Indexer 每轮触发，Compressor 批量触发

---

### 3.2 ContextComptroller（上下文仲裁器）

**职责**：在检索后、注入前进行资源仲裁，避免 Token 浪费

**核心能力**：
1. **溯源重组**：利用 Fact 的 `source_memory_ids` 关联原始 Transcript
2. **去重抑制**：相似度 > 0.95 的 Fact 与 Transcript 只保留一个
3. **Token 软熔断**：超预算时启动有损压缩

#### 核心接口

```rust
pub struct ContextComptroller {
    config: ComptrollerConfig,
}

pub struct ComptrollerConfig {
    pub similarity_threshold: f32,    // 去重阈值，默认 0.95
    pub token_budget: usize,          // Token 预算
    pub fold_threshold: f32,          // 折叠阈值，默认 0.8（剩余 20% 时折叠）
}

pub enum RetentionMode {
    PreferTranscript,  // 默认：保留原文
    PreferFact,        // 空间紧张时：保留事实
    Hybrid,            // 混合：关键原文 + 摘要事实
}

impl ContextComptroller {
    /// 仲裁检索结果
    pub fn arbitrate(&self, results: RetrievalResult, budget: TokenBudget) -> ArbitratedContext;

    /// 检测冗余
    fn detect_redundancy(&self, facts: &[MemoryFact], transcripts: &[MemoryEntry]) -> Vec<RedundancyPair>;

    /// 有损压缩
    fn fold_to_facts(&self, transcripts: Vec<MemoryEntry>) -> Vec<MemoryFact>;
}
```

#### MVP 策略（保守替换）

1. 相似度 > 0.95 时，默认保留 Transcript（原文更具说服力）
2. Context 窗口剩余 < 20% 时，启动折叠压缩
3. 折叠时将 Transcript 替换为对应的 Fact

---

### 3.3 ValueEstimator（价值评估器）

**职责**：在存储后、Fact 提取前评估对话价值，过滤低价值内容

**评估维度**：
- 是否包含第一人称陈述（"我喜欢..."、"我决定..."）
- 是否包含时间锚点（"从现在开始..."、"以后..."）
- 是否与现有 Facts 产生冲突（向量相似度检测）

#### 信号驱动的触发层级

| 信号类型 | 触发条件 | 响应 |
|---------|---------|------|
| **Immediate** | 用户明确说"记住这个" / 纠正性陈述 | 立即提取该轮 Fact |
| **Eager** | 检测到高价值信息（决策、偏好、项目变更） | 下一个 idle 窗口提取 |
| **Scheduled** | 每 20 轮 / 5 分钟 idle | 批量提取未处理的轮次 |
| **Pressure** | Context 使用率 > 75% | 激进压缩 + 历史清理 |

#### 核心接口

```rust
pub struct ValueEstimator {
    signal_detector: SignalDetector,
    conflict_checker: ConflictChecker,
}

pub struct ValueScore {
    pub score: f32,           // 0.0 - 1.0
    pub signals: Vec<Signal>,
    pub should_extract: bool, // score > 0.6
}

impl ValueEstimator {
    /// 评估单轮对话的价值
    pub async fn estimate(&self, memory: &MemoryEntry) -> ValueScore;

    /// 批量评估
    pub async fn estimate_batch(&self, memories: &[MemoryEntry]) -> Vec<ValueScore>;
}
```

---

### 3.4 DreamDaemon（涟漪式蒸馏）

**职责**：后台维护知识图谱，实现认知蒸馏和冲突调和

#### 涟漪式更新策略

**第一层涟漪：局部探索 (RippleTask)**
- 触发：每次 CompressionService 提取出新 Facts 后
- 范围：新 Fact 的 1-hop 邻居
- 操作：冲突检测、关系发现、graph_edges 更新

**第二层涟漪：标记修复 (RepairTask)**
- 触发：每次 Dreaming 窗口
- 范围：被 ContextComptroller 标记的异常节点
- 操作：高冲突/低清晰度/孤岛节点的修复

**第三层涟漪：周期扫描 (ConsolidationTask)**
- 触发：每周一次或用户手动触发
- 范围：全量（采样策略）
- 操作：用户画像蒸馏、过期清理、全局一致性检查

#### Fact Evolution Chain（演进链条）

```sql
-- 扩展 memory_facts 表
ALTER TABLE memory_facts ADD COLUMN superseded_by INTEGER REFERENCES memory_facts(id);
ALTER TABLE memory_facts ADD COLUMN evolution_type TEXT; -- 'supersede', 'refine', 'merge'
```

**示例**：
```
Fact #1: "用户喜欢 Python" (2025-12-01)
    ↓ superseded_by (evolution_type: 'supersede')
Fact #2: "用户从 Python 迁移到 Rust" (2026-01-15)
    ↓ evolved_to (evolution_type: 'refine')
Fact #3: "用户主要使用 Rust，但仍用 Python 做数据分析" (2026-02-01)
```

#### 核心接口

```rust
pub struct DreamDaemon {
    scheduler: DreamScheduler,
    distillation_engine: DistillationEngine,
}

pub struct DistillationEngine {
    conflict_resolver: ConflictResolver,
    relation_discoverer: RelationDiscoverer,
    profile_synthesizer: ProfileSynthesizer,
}

impl DreamDaemon {
    /// 执行涟漪式更新
    pub async fn ripple_update(&self, new_fact_ids: Vec<i64>) -> Result<()>;

    /// 修复标记节点
    pub async fn repair_marked(&self) -> Result<()>;

    /// 周期性深度扫描
    pub async fn consolidate(&self) -> Result<()>;
}
```

---

## 4. memory_search 工具设计

**职责**：为 Agent 提供主动检索历史记忆的能力

#### 工具 Schema

```rust
#[derive(JsonSchema, Deserialize)]
pub struct MemorySearchInput {
    /// 搜索查询
    pub query: String,

    /// 最大返回数量
    #[serde(default = "default_max_results")]
    pub max_results: u32,  // 默认 6

    /// 最小相似度阈值
    #[serde(default = "default_min_score")]
    pub min_score: f32,    // 默认 0.35

    /// 检索模式
    #[serde(default)]
    pub mode: RetrievalMode,
}

pub enum RetrievalMode {
    #[default]
    Auto,       // 自动选择
    Facts,      // 只检索 Facts
    Transcripts,// 只检索原始对话
    Hybrid,     // 混合检索
}
```

#### 返回格式

```rust
pub struct MemorySearchResult {
    pub items: Vec<MemoryItem>,
    pub total_found: u32,
    pub token_usage: u32,
}

pub struct MemoryItem {
    pub uri: String,           // aleph://fact/123 或 aleph://transcript/456#chunk=2
    pub content: String,
    pub score: f32,
    pub source: MemorySource,
    pub timestamp: DateTime<Utc>,
    pub metadata: Option<serde_json::Value>,
}
```

---

## 5. 实施路线图

### Phase 1: MVP（核心闭环）

**目标**：解决"健忘"与"冗余"问题

| 任务 | 优先级 | 预估复杂度 |
|------|--------|-----------|
| 实现 TranscriptIndexer 基础版 | P0 | Medium |
| 建立 transcript_vec 索引表 | P0 | Low |
| 实现 memory_search 工具 | P0 | Medium |
| 实现 ContextComptroller 基础版（去重） | P1 | Medium |
| 集成到 Agent 工具链 | P1 | Low |

**验收标准**：
- Agent 可以通过 memory_search 工具检索历史对话
- 检索结果不会出现 Fact 与 Transcript 的明显重复

### Phase 2: 体验进阶

**目标**：解决"效率"与"精度"问题

| 任务 | 优先级 | 预估复杂度 |
|------|--------|-----------|
| 实现 ValueEstimator | P2 | Medium |
| 集成信号驱动的触发逻辑 | P2 | Medium |
| 优化 TranscriptIndexer 支持滑动窗口切片 | P2 | Low |
| 增强 ContextComptroller 的 Token 预算管理 | P2 | Medium |

**验收标准**：
- Facts DB 增长速度明显放缓（低价值内容被过滤）
- 检索精度提升（切片索引带来更精准的匹配）

### Phase 3: 超越极限

**目标**：解决"认知深度"与"知识演进"问题

| 任务 | 优先级 | 预估复杂度 |
|------|--------|-----------|
| 实现 DreamDaemon 调度器 | P3 | Medium |
| 实现 RippleTask（局部探索） | P3 | High |
| 实现 Fact Evolution Chain | P3 | Medium |
| 实现 ConsolidationTask（用户画像蒸馏） | P3 | High |

**验收标准**：
- 矛盾的 Facts 被自动调和，生成演进记录
- 高频访问的 Facts 被蒸馏为用户画像

---

## 6. 技术细节

### 6.1 Token 估算

沿用现有的 4 字符/Token 估算，但增加安全边际：

```rust
const CHARS_PER_TOKEN: usize = 4;
const SAFETY_MARGIN: f32 = 1.2;  // 20% 安全边际

pub fn estimate_tokens(text: &str) -> usize {
    ((text.len() / CHARS_PER_TOKEN) as f32 * SAFETY_MARGIN) as usize
}
```

### 6.2 混合检索算法

采用 RRF (Reciprocal Rank Fusion) 进行结果融合：

```rust
pub fn rrf_merge(
    vector_results: Vec<SearchResult>,
    keyword_results: Vec<SearchResult>,
    k: f32,  // 默认 60.0
) -> Vec<SearchResult> {
    // RRF score = 1 / (k + rank)
    // 合并后按 RRF score 降序排列
}
```

### 6.3 统一资源标识符

```
aleph://fact/{id}                    # Facts DB 记录
aleph://transcript/{id}              # 原始对话记录
aleph://transcript/{id}#chunk={n}    # 对话切片
file://{path}#chunk={hash}           # 外部文件（Phase 2+）
```

---

## 7. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| NRT 索引延迟过高 | 用户感知到"刚说的话找不到" | 设置 500ms 硬限制，超时降级为同步 |
| Token 估算不准确 | Context 溢出或浪费 | 引入 20% 安全边际 |
| DreamDaemon LLM 开销过大 | 本地资源耗尽 | 涟漪式策略 + 采样扫描 |
| Fact Evolution Chain 过长 | 检索效率下降 | 定期归档，只保留最近 3 代 |

---

## 8. 参考资料

- OpenClaw `src/agents/compaction.ts` - 自适应压缩算法
- OpenClaw `src/memory/hybrid.ts` - 混合检索实现
- Aleph `core/src/memory/` - 现有记忆系统
- Aleph `core/src/compressor/` - 现有压缩逻辑

---

## 附录：与 OpenClaw 的对比

| 维度 | OpenClaw | Aleph (设计后) |
|------|----------|---------------|
| 记忆存储 | 文件索引 + Transcripts | 双层架构 + 统一 URI |
| 检索策略 | 被动工具 | 主动注入 + 被动工具 |
| 压缩算法 | 自适应分块 | 信号驱动 + 价值评估 |
| Token 管理 | Context Guard | ContextComptroller |
| 知识演进 | DreamDaemon (简单摘要) | 涟漪式蒸馏 + 演进链条 |
