# Aleph Memory System v2 设计文档

> 日期: 2025-01-31
> 状态: Draft
> 作者: Human + Claude

## 1. 设计目标

**核心理念**：超越 OpenClaw，打造"AI 原生"的记忆系统。

**优先级排序**：
1. **记忆压缩效率** — 智能触发，而非机械定时
2. **检索精度** — 混合搜索 + 分层检索
3. **跨 Session 联想** — 动态聚类，零存储开销
4. **不使用 Markdown 知识库** — 纯数据库路线

**与 OpenClaw 的差异化**：
| 维度 | OpenClaw | Aleph v2 |
|------|----------|-----------|
| 知识存储 | YAML/Markdown + SQLite | 纯 SQLite |
| 检索方式 | 混合搜索 | 混合搜索 + 分层 + 动态联想 |
| 压缩触发 | Token 阈值 | 多维信号检测 |
| 人类可编辑 | 支持 | 不支持（AI 原生） |

---

## 2. 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Aleph Memory v2                         │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐     │
│  │   Layer 1   │    │   Layer 2   │    │   Layer 3   │     │
│  │  memories   │───▶│memory_facts │───▶│  (动态联想)  │     │
│  │  (原始对话)  │    │ (压缩事实)  │    │ (检索时生成) │     │
│  └─────────────┘    └─────────────┘    └─────────────┘     │
│         │                  │                               │
│         ▼                  ▼                               │
│  ┌─────────────┐    ┌─────────────┐                       │
│  │memories_vec │    │ facts_vec   │   混合检索引擎         │
│  │memories_fts │    │ facts_fts   │   (向量70% + BM25 30%) │
│  └─────────────┘    └─────────────┘                       │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              智能压缩触发器 (Signal Detector)         │   │
│  │  • 学习信号  • 纠错信号  • 里程碑信号  • 切换信号     │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

**三层架构说明**：
- **Layer 1 (memories)**: 原始对话记录，完整保留 user_input + ai_output
- **Layer 2 (memory_facts)**: LLM 压缩提炼的事实，第三人称陈述
- **Layer 3 (动态联想)**: 不存储，检索时实时计算聚类

---

## 3. 智能压缩触发器 (Signal Detector)

### 3.1 数据结构

```rust
// core/src/memory/compression/signal_detector.rs

/// 压缩信号类型
pub enum CompressionSignal {
    /// 学习信号：用户表达偏好/规则
    Learning {
        trigger_phrase: String,      // "我喜欢...", "记住...", "以后都..."
        confidence: f32,             // 0.0-1.0
    },
    /// 纠错信号：用户纠正 AI 误解
    Correction {
        original_understanding: String,
        corrected_to: String,
        confidence: f32,
    },
    /// 里程碑信号：任务/项目完成
    Milestone {
        task_description: String,
        completion_indicator: String, // "完成", "搞定", "done"
    },
    /// 上下文切换信号：话题跳转
    ContextSwitch {
        from_topic: String,
        to_topic: String,
    },
}

/// 信号检测结果
pub struct DetectionResult {
    pub signals: Vec<CompressionSignal>,
    pub should_compress: bool,
    pub priority: CompressionPriority,  // Immediate / Deferred / Batch
}

/// 压缩优先级
pub enum CompressionPriority {
    Immediate,  // 立即压缩（纠错信号）
    Deferred,   // 延迟压缩（学习信号，等对话稳定）
    Batch,      // 批量压缩（里程碑、切换信号）
}
```

### 3.2 检测策略

**两层过滤：关键词 + LLM**

| 信号类型 | 第一层：关键词匹配 | 第二层：LLM 确认 |
|---------|------------------|----------------|
| 学习 | "记住"、"以后"、"偏好"、"喜欢用" | 仅当关键词命中时调用 |
| 纠错 | "不对"、"搞错"、"我说的是" | 提取原始理解 vs 纠正内容 |
| 里程碑 | "完成"、"搞定"、"结束"、"done" | 判断是否真正完成 |
| 切换 | 向量距离突变（>阈值） | 总结旧话题 |

**设计原则**：关键词过滤 90% 噪音，LLM 只处理高置信候选，避免每轮对话都调用 LLM。

### 3.3 关键词库

```rust
pub struct SignalKeywords {
    pub learning: Vec<&'static str>,
    pub correction: Vec<&'static str>,
    pub milestone: Vec<&'static str>,
}

impl Default for SignalKeywords {
    fn default() -> Self {
        Self {
            learning: vec![
                // 中文
                "记住", "以后", "偏好", "喜欢用", "习惯", "总是", "一直",
                "我喜欢", "我讨厌", "我倾向", "默认用", "优先用",
                // 英文
                "remember", "always", "prefer", "I like", "I hate",
                "from now on", "by default", "going forward",
            ],
            correction: vec![
                // 中文
                "不对", "搞错", "错了", "我说的是", "不是这个意思",
                "你理解错了", "应该是", "纠正一下",
                // 英文
                "wrong", "incorrect", "no,", "not what I meant",
                "I meant", "actually", "let me clarify",
            ],
            milestone: vec![
                // 中文
                "完成", "搞定", "结束", "做完了", "好了", "成功",
                "告一段落", "收工",
                // 英文
                "done", "finished", "completed", "that's it",
                "wrap up", "all set",
            ],
        }
    }
}
```

---

## 4. 混合检索引擎 (Hybrid Retrieval)

### 4.1 配置结构

```rust
// core/src/memory/retrieval/hybrid.rs

/// 混合检索配置
pub struct HybridSearchConfig {
    pub vector_weight: f32,           // 默认 0.7
    pub text_weight: f32,             // 默认 0.3
    pub min_score: f32,               // 默认 0.35
    pub max_results: usize,           // 默认 10
    pub candidate_multiplier: usize,  // 候选池倍数，默认 4
}

/// 分层检索策略
pub enum RetrievalStrategy {
    /// 只搜 Layer 2 (facts) — 快速模式
    FactsOnly,
    /// 先搜 facts，不够再搜 memories — 默认模式
    FactsFirst { min_facts: usize },
    /// 同时搜两层，合并结果 — 深度模式
    BothLayers,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: 0.7,
            text_weight: 0.3,
            min_score: 0.35,
            max_results: 10,
            candidate_multiplier: 4,
        }
    }
}
```

### 4.2 数据库扩展

```sql
-- 为 memories 添加全文索引
CREATE VIRTUAL TABLE memories_fts USING fts5(
    user_input,
    ai_output,
    id UNINDEXED,
    content='memories',
    content_rowid='rowid'
);

-- 为 memory_facts 添加全文索引
CREATE VIRTUAL TABLE facts_fts USING fts5(
    content,
    fact_type UNINDEXED,
    id UNINDEXED,
    content='memory_facts',
    content_rowid='rowid'
);

-- 同步触发器：memories 插入时自动更新 FTS
CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, user_input, ai_output, id)
    VALUES (new.rowid, new.user_input, new.ai_output, new.id);
END;

-- 同步触发器：memories 删除时自动更新 FTS
CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, user_input, ai_output, id)
    VALUES ('delete', old.rowid, old.user_input, old.ai_output, old.id);
END;

-- 同步触发器：memory_facts 插入时自动更新 FTS
CREATE TRIGGER facts_fts_insert AFTER INSERT ON memory_facts BEGIN
    INSERT INTO facts_fts(rowid, content, fact_type, id)
    VALUES (new.rowid, new.content, new.fact_type, new.id);
END;

-- 同步触发器：memory_facts 删除时自动更新 FTS
CREATE TRIGGER facts_fts_delete AFTER DELETE ON memory_facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, content, fact_type, id)
    VALUES ('delete', old.rowid, old.content, old.fact_type, old.id);
END;
```

### 4.3 混合查询 SQL

```sql
-- 混合检索 memory_facts
WITH vec_hits AS (
    SELECT rowid, distance FROM facts_vec
    WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2
),
fts_hits AS (
    SELECT rowid, bm25(facts_fts) as rank FROM facts_fts
    WHERE facts_fts MATCH ?3 ORDER BY rank LIMIT ?2
)
SELECT f.*,
    (COALESCE(0.7 / (1.0 + v.distance), 0) +
     COALESCE(0.3 / (1.0 - fts.rank), 0)) as combined_score
FROM memory_facts f
LEFT JOIN vec_hits v ON f.rowid = v.rowid
LEFT JOIN fts_hits fts ON f.rowid = fts.rowid
WHERE (v.rowid IS NOT NULL OR fts.rowid IS NOT NULL)
  AND f.is_valid = 1
ORDER BY combined_score DESC
LIMIT ?4;
```

---

## 5. 动态联想聚类 (Dynamic Association)

### 5.1 数据结构

```rust
// core/src/memory/retrieval/association.rs

/// 联想结果
pub struct AssociationCluster {
    pub center_fact: MemoryFact,           // 聚类中心（最相关的 fact）
    pub related_facts: Vec<MemoryFact>,    // 聚类成员
    pub cluster_theme: Option<String>,      // LLM 生成的主题标签（可选）
    pub avg_similarity: f32,                // 簇内平均相似度
}

/// 联想检索器配置
pub struct AssociationConfig {
    pub expansion_radius: f32,    // 向量空间扩展半径，默认 0.4
    pub max_associations: usize,  // 最大联想数，默认 5
    pub min_cluster_size: usize,  // 最小簇大小，默认 2
    pub generate_theme: bool,     // 是否生成主题标签，默认 false
}

impl Default for AssociationConfig {
    fn default() -> Self {
        Self {
            expansion_radius: 0.4,
            max_associations: 5,
            min_cluster_size: 2,
            generate_theme: false,
        }
    }
}
```

### 5.2 算法流程

```
用户查询: "Rust 的所有权怎么理解？"
    │
    ▼
[1] 混合检索 top-K facts (K=10)
    │
    ▼
[2] 以每个 hit 为圆心，expansion_radius 为半径
    在 facts_vec 中查找邻居
    │
    ▼
[3] 合并重叠区域，形成若干"临时簇"
    │
    ▼
[4] 过滤：簇大小 < min_cluster_size 的丢弃
    │
    ▼
[5] 返回联想结果

结果示例:
┌─────────────────────────────────────────────┐
│ 直接命中: "Rust 所有权是..."                  │
├─────────────────────────────────────────────┤
│ 联想簇 1 (主题: 内存管理):                    │
│   • "C++ RAII 模式..."                       │
│   • "Go 的 GC 与 Rust 对比..."               │
├─────────────────────────────────────────────┤
│ 联想簇 2 (主题: Rust 学习):                   │
│   • "用户偏好通过实战学习 Rust"               │
│   • "用户正在做的 Aleph 项目是 Rust 写的"    │
└─────────────────────────────────────────────┘
```

### 5.3 邻居查找 SQL

```sql
-- 以某个 fact 的 embedding 为中心，找半径内的邻居
SELECT f.*, v.distance
FROM memory_facts f
INNER JOIN facts_vec v ON f.rowid = v.rowid
WHERE v.embedding MATCH ?1           -- 中心 fact 的 embedding
  AND v.distance < ?2                -- expansion_radius
  AND f.id != ?3                     -- 排除自身
  AND f.is_valid = 1
ORDER BY v.distance
LIMIT ?4;
```

---

## 6. 压缩提炼质量改进

### 6.1 结构化输出

```rust
// core/src/memory/compression/extractor.rs

/// 事实提取的结构化输出
pub struct ExtractedFact {
    pub content: String,                   // 第三人称陈述
    pub fact_type: FactType,               // 分类
    pub confidence: f32,                   // 提取置信度
    pub specificity: FactSpecificity,      // 新增：具体度
    pub temporal_scope: TemporalScope,     // 新增：时效性
    pub source_ids: Vec<String>,
}

/// 具体度（防止太泛或太细）
pub enum FactSpecificity {
    /// 原则级："用户偏好函数式编程"
    Principle,
    /// 模式级："用户处理错误时喜欢用 Result 而非 panic"
    Pattern,
    /// 实例级："用户在 2025-01-15 的项目里用了 anyhow"
    Instance,
}

/// 时效性
pub enum TemporalScope {
    /// 长期有效："用户的母语是中文"
    Permanent,
    /// 上下文相关："用户当前在做 Aleph 项目"
    Contextual,
    /// 短期有效："用户今天想专注写文档"
    Ephemeral,
}
```

### 6.2 质量控制策略

| 问题 | 解决方案 |
|------|---------|
| 太泛泛 | 要求输出 `specificity`，过滤掉纯 `Principle` 级别的废话 |
| 太细碎 | 合并同一 `Pattern` 下的多个 `Instance` |
| 丢失关键信息 | 信号检测器标记的内容强制提取，不允许跳过 |
| 过时信息污染 | `temporal_scope` 标记 + 定期清理 `Ephemeral` |

### 6.3 数据库扩展

```sql
-- 为 memory_facts 添加新字段
ALTER TABLE memory_facts ADD COLUMN specificity TEXT DEFAULT 'pattern';
ALTER TABLE memory_facts ADD COLUMN temporal_scope TEXT DEFAULT 'contextual';

-- 创建索引
CREATE INDEX idx_facts_specificity ON memory_facts(specificity);
CREATE INDEX idx_facts_temporal ON memory_facts(temporal_scope);
```

---

## 7. 冲突处理

### 7.1 三种策略

```rust
// core/src/memory/compression/conflict.rs

pub enum ConflictResolution {
    /// 新事实覆盖旧事实（默认：纠错信号触发时）
    Override {
        invalidated_id: String,
        reason: String,
    },
    /// 旧事实保留，新事实丢弃（置信度对比）
    Reject {
        rejected_content: String,
        reason: String,
    },
    /// 合并为更精确的表述
    Merge {
        old_id: String,
        new_content: String,  // 合并后的内容
        merge_strategy: MergeStrategy,
    },
}

pub enum MergeStrategy {
    /// 抽象化："喜欢 Rust" + "喜欢 Go" → "喜欢系统编程语言"
    Generalize,
    /// 具体化："喜欢咖啡" + "喜欢深烘" → "喜欢深烘咖啡"
    Specialize,
    /// 枚举化："喜欢 Rust、Go、Zig 等系统语言"
    Enumerate,
}
```

### 7.2 检测流程

```
新 fact 进入
    │
    ▼
[1] 向量相似度 > 0.85 的现有 facts → 冲突候选
    │
    ▼
[2] LLM 判断：是矛盾还是补充？
    │
    ├─── 矛盾 + 纠错信号 → Override
    ├─── 矛盾 + 无纠错 → 比较 confidence，高的留
    ├─── 补充 + 可合并 → Merge
    └─── 补充 + 独立 → 都保留
```

---

## 8. 记忆衰减机制 (Memory Decay)

### 8.1 数据结构

```rust
// core/src/memory/decay.rs

/// 记忆强度追踪
pub struct MemoryStrength {
    pub access_count: u32,        // 被检索命中次数
    pub last_accessed: i64,       // 最后访问时间戳
    pub creation_time: i64,       // 创建时间戳
    pub strength_score: f32,      // 综合强度 0.0-1.0
}

/// 艾宾浩斯衰减配置
pub struct DecayConfig {
    pub half_life_days: f32,      // 半衰期，默认 30 天
    pub access_boost: f32,        // 每次访问增加的强度，默认 0.2
    pub min_strength: f32,        // 低于此值触发清理，默认 0.1
    pub protected_types: Vec<FactType>,  // 永不衰减的类型
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            half_life_days: 30.0,
            access_boost: 0.2,
            min_strength: 0.1,
            protected_types: vec![FactType::Personal],
        }
    }
}
```

### 8.2 衰减公式

```rust
impl MemoryStrength {
    /// 计算当前强度（艾宾浩斯曲线简化版）
    pub fn calculate_strength(&self, config: &DecayConfig, now: i64) -> f32 {
        let days_since_access = (now - self.last_accessed) as f32 / 86400.0;

        // 基础衰减：指数衰减曲线
        // strength = 0.5 ^ (days / half_life)
        let base_decay = 0.5_f32.powf(days_since_access / config.half_life_days);

        // 访问加成：每次访问 +0.2，上限 2.0
        let access_boost = (self.access_count as f32 * config.access_boost).min(2.0);

        // 最终强度 = 基础衰减 × (1 + 访问加成)
        (base_decay * (1.0 + access_boost)).min(1.0)
    }
}
```

### 8.3 数据库扩展

```sql
-- 为 memory_facts 添加衰减字段
ALTER TABLE memory_facts ADD COLUMN access_count INTEGER DEFAULT 0;
ALTER TABLE memory_facts ADD COLUMN last_accessed INTEGER;
ALTER TABLE memory_facts ADD COLUMN strength_score REAL DEFAULT 1.0;

-- 创建衰减索引（方便定期清理）
CREATE INDEX idx_facts_strength ON memory_facts(strength_score, is_valid);
```

### 8.4 生命周期

```
新 fact 创建
    │ strength = 1.0
    ▼
30天无访问 ──────────────────▶ strength ≈ 0.5
    │
    │ 被检索命中 (+0.2 boost)
    ▼
strength ≈ 0.7 ◀──────────────── 越用越强
    │
60天无访问
    ▼
strength ≈ 0.25
    │
90天无访问
    ▼
strength < 0.1 ──────────────▶ 触发软删除候选
    │
    ▼
[定期清理任务] is_valid = 0, invalidation_reason = "decay"
```

### 8.5 类型保护策略

| FactType | 衰减策略 |
|----------|---------|
| `Preference` | 半衰期 × 2（偏好应该更持久）|
| `Personal` | 永不自动衰减（个人信息珍贵）|
| `Ephemeral` | 半衰期 × 0.5（本就是临时的）|
| 其他 | 正常衰减 |

---

## 9. 实现路径

### 9.1 文件结构变更

```
core/src/memory/
├── compression/
│   ├── mod.rs
│   ├── service.rs          # 修改：集成 SignalDetector
│   ├── extractor.rs        # 修改：结构化提取 + Specificity/TemporalScope
│   ├── conflict.rs         # 修改：三种冲突策略 + Merge
│   ├── signal_detector.rs  # 新增：智能压缩触发器
│   └── scheduler.rs        # 修改：信号驱动调度
├── retrieval/
│   ├── mod.rs              # 新增模块
│   ├── hybrid.rs           # 新增：混合检索引擎
│   ├── association.rs      # 新增：动态联想聚类
│   └── strategy.rs         # 新增：分层检索策略
├── decay.rs                # 新增：衰减机制
├── database/
│   ├── core.rs             # 修改：新增 FTS5 表 + 衰减字段
│   ├── memory_ops.rs       # 修改：混合查询 SQL
│   └── facts.rs            # 修改：衰减字段读写
└── retrieval.rs            # 重构：拆分到 retrieval/ 目录
```

### 9.2 优先级排序

| 阶段 | 内容 | 依赖 | 预计文件 |
|------|------|------|---------|
| **P0** | SignalDetector + 智能压缩触发 | 无 | signal_detector.rs, service.rs |
| **P1** | FTS5 索引 + 混合检索 | P0 可并行 | core.rs, hybrid.rs |
| **P2** | 结构化提取 + 冲突处理改进 | P0 | extractor.rs, conflict.rs |
| **P3** | 动态联想聚类 | P1 | association.rs |
| **P4** | 衰减机制 | P2 | decay.rs, facts.rs |

### 9.3 与现有代码的融合点

| 现有模块 | 融合方式 |
|---------|---------|
| `compression/service.rs` | 注入 `SignalDetector`，保留原有定时/定量触发作为兜底 |
| `compression/extractor.rs` | 扩展输出结构，Prompt 模板升级 |
| `database/core.rs` | `init_database()` 中添加 FTS5 + 触发器 |
| `retrieval.rs` | 重构为 `retrieval/` 目录，原接口保持兼容 |

---

## 10. 设计对比总结

| 维度 | 现状 (v1) | 升级后 (v2) |
|------|----------|------------|
| 压缩触发 | 定时/定量 | 信号检测 + 兜底 |
| 检索方式 | 纯向量 | 混合 (向量70% + BM25 30%) |
| 检索层级 | 单层 | 分层 (facts → memories) |
| 联想能力 | 无 | 动态聚类 |
| 冲突处理 | 软删除旧的 | Override/Reject/Merge 三策略 |
| 生命周期 | 永久保留 | 艾宾浩斯衰减 |
| 事实分类 | 6 种 FactType | + Specificity + TemporalScope |

---

## 11. 开放问题

1. **信号检测的 LLM 成本**：如何在检测精度和 API 成本之间平衡？
   - 当前方案：关键词预过滤，只对高置信候选调用 LLM

2. **动态聚类的延迟**：检索时计算聚类会增加多少延迟？
   - 待实测，可能需要限制 expansion_radius 或 max_associations

3. **衰减的用户控制**：用户是否需要手动"固定"某些记忆？
   - 可考虑增加 `pinned` 字段，pinned 的记忆永不衰减

---

## 12. 参考资料

- [OpenClaw Memory 实现](https://github.com/openclaw/openclaw/tree/main/src/memory)
- [sqlite-vec 文档](https://github.com/asg017/sqlite-vec)
- [SQLite FTS5 文档](https://www.sqlite.org/fts5.html)
- [艾宾浩斯遗忘曲线](https://en.wikipedia.org/wiki/Forgetting_curve)
