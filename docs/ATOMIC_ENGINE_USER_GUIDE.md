# Atomic Engine 用户指南

> 完整的 Atomic Engine 使用文档，包含架构说明、API 参考和最佳实践

## 目录

- [概述](#概述)
- [快速开始](#快速开始)
- [核心概念](#核心概念)
- [功能模块](#功能模块)
- [API 参考](#api-参考)
- [性能优化](#性能优化)
- [最佳实践](#最佳实践)
- [故障排查](#故障排查)

---

## 概述

Atomic Engine 是 Aleph 的核心执行引擎，实现了三层路由架构（L1/L2/L3），提供高性能的原子操作执行和智能路由。

### 核心特性

- **三层路由架构**: L1 缓存 → L2 规则 → L3 LLM
- **7 种原子操作**: Read, Write, Edit, Bash, Search, Replace, Move
- **机器学习**: 自动从 L3 执行学习并生成 L2 规则
- **持久化**: 跨会话保存学习数据
- **冲突检测**: 自动识别和解决规则冲突
- **性能优化**: 缓存、索引、预编译 Regex

### 性能指标

| 指标 | 数值 |
|------|------|
| L2 路由延迟 | 143 μs |
| 缓存命中延迟 | ~1 μs |
| L2 命中率 | 95%+ |
| 执行吞吐量 | 333 ops/sec |

---

## 快速开始

### 基本使用

```rust
use alephcore::engine::{AtomicEngine, AtomicAction};

// 创建引擎实例
let engine = AtomicEngine::new(workspace_path);

// 执行原子操作
let action = AtomicAction::Read {
    path: "src/main.rs".into(),
};

let result = engine.execute(action).await?;
println!("Result: {}", result.output);
```

### 启用机器学习

```rust
use alephcore::engine::{LearningAgent, RuleLearner, ReflexLayer};
use std::sync::Arc;
use tokio::sync::RwLock;

// 创建学习组件
let learner = Arc::new(RuleLearner::new());
let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
let agent = LearningAgent::new(learner, reflex_layer);

// 监听 L3 执行
agent.on_l3_success(
    "search for TODO",
    search_action,
    Duration::from_millis(100)
).await;

// 生成和部署规则
let count = agent.generate_and_deploy_rules().await;
println!("Generated {} rules", count);
```

### 启用持久化

```rust
use alephcore::engine::Persistence;

// 创建持久化层
let persistence = Persistence::new("./data/learned_rules.db").await?;

// 保存学习模式
persistence.save_pattern(
    "search for TODO",
    &action,
    5,  // count
    5,  // successes
    0   // failures
).await?;

// 加载学习模式
let patterns = persistence.load_patterns().await?;
```

---

## 核心概念

### 三层路由架构

```
┌─────────────────────────────────────────────────────┐
│                    L1: Cache Layer                   │
│  • O(1) 查找                                         │
│  • ~1 μs 延迟                                        │
│  • 命中率: 60-70%                                    │
└─────────────────────────────────────────────────────┘
                         ↓ Cache Miss
┌─────────────────────────────────────────────────────┐
│                   L2: Reflex Layer                   │
│  • 关键词匹配                                        │
│  • ~143 μs 延迟                                      │
│  • 命中率: 87.5% → 95%+                              │
└─────────────────────────────────────────────────────┘
                         ↓ No Match
┌─────────────────────────────────────────────────────┐
│                    L3: LLM Layer                     │
│  • 完整 LLM 推理                                     │
│  • ~1-5s 延迟                                        │
│  • 命中率: 100%                                      │
└─────────────────────────────────────────────────────┘
```

### 原子操作

#### 1. Read - 读取文件

```rust
AtomicAction::Read {
    path: PathBuf::from("src/main.rs"),
}
```

#### 2. Write - 写入文件

```rust
AtomicAction::Write {
    path: PathBuf::from("output.txt"),
    content: "Hello, World!".to_string(),
    mode: WriteMode::Overwrite,
}
```

#### 3. Edit - 编辑文件

```rust
AtomicAction::Edit {
    path: PathBuf::from("src/main.rs"),
    patches: vec![
        Patch::new(10, 10, "old line", "new line")?
    ],
}
```

#### 4. Bash - 执行命令

```rust
AtomicAction::Bash {
    command: "git status".to_string(),
    cwd: Some(PathBuf::from("/project")),
}
```

#### 5. Search - 搜索文件

```rust
AtomicAction::Search {
    pattern: SearchPattern::Regex {
        pattern: "TODO:.*".to_string(),
    },
    scope: SearchScope::Workspace,
    filters: vec![FileFilter::Code],
}
```

#### 6. Replace - 替换内容

```rust
AtomicAction::Replace {
    pattern: SearchPattern::Regex {
        pattern: "foo".to_string(),
    },
    replacement: "bar".to_string(),
    scope: SearchScope::Workspace,
    mode: ReplaceMode::Global,
}
```

#### 7. Move - 移动文件

```rust
AtomicAction::Move {
    source: PathBuf::from("old.txt"),
    destination: PathBuf::from("new.txt"),
    update_imports: true,
}
```

---

## 功能模块

### 1. RuleLearner - 规则学习器

自动从 L3 执行学习并生成 L2 规则。

```rust
use alephcore::engine::RuleLearner;

let learner = RuleLearner::new();

// 学习成功案例
learner.learn_success("git status", bash_action);

// 学习失败案例
learner.learn_failure("invalid command", bash_action);

// 生成规则
let rules = learner.generate_rules();
```

**配置参数**:
- `MIN_EXECUTIONS`: 3（最少执行次数）
- `MIN_CONFIDENCE`: 0.8（最低置信度）

### 2. FeatureExtractor - 特征提取器

从用户输入提取特征用于机器学习。

```rust
use alephcore::engine::FeatureExtractor;

let extractor = FeatureExtractor::new();
let features = extractor.extract("search for TODO in file");

println!("Keywords: {:?}", features.keywords);
println!("Intent: {:?}", features.intent);
println!("Entities: {:?}", features.entities);
```

**提取的特征**:
- **Keywords**: 关键词（动词、名词）
- **Intent**: 意图（Read, Write, Execute, Search, Replace, Move）
- **Entities**: 实体（文件路径、命令名、模式）
- **Confidence**: 置信度（0.0-1.0）

### 3. NaiveBayesClassifier - 朴素贝叶斯分类器

增量学习的分类器，用于预测动作类型。

```rust
use alephcore::engine::{NaiveBayesClassifier, ActionClass};

let mut classifier = NaiveBayesClassifier::new();

// 训练
classifier.train(&features, ActionClass::Search);

// 预测
let (predicted_class, confidence) = classifier.predict(&features)?;
```

**特性**:
- 增量学习（无需重新训练）
- Laplace 平滑（处理未见特征）
- 对数空间计算（数值稳定性）

### 4. LearningAgent - 学习代理

集成 Agent Loop，自动监听和学习。

```rust
use alephcore::engine::LearningAgent;

let agent = LearningAgent::new(learner, reflex_layer);

// 监听 L3 成功
agent.on_l3_success(input, action, latency).await;

// 监听 L3 失败
agent.on_l3_failure(input, action, error).await;

// 自动生成规则（每 100 个观察或 5 分钟）
// 规则会自动部署到 ReflexLayer
```

**配置参数**:
- `MIN_OBSERVATIONS`: 100（触发规则生成的最少观察数）
- `GENERATION_INTERVAL_SECS`: 300（规则生成间隔，5 分钟）

### 5. Persistence - 持久化层

跨会话保存学习数据。

```rust
use alephcore::engine::Persistence;

let persistence = Persistence::new("./data/rules.db").await?;

// 保存模式
persistence.save_pattern(pattern, action, count, successes, failures).await?;

// 加载模式
let patterns = persistence.load_patterns().await?;

// 保存分类器
persistence.save_classifier(&classifier).await?;

// 加载分类器
let classifier = persistence.load_classifier().await?;

// 保存规则元数据
persistence.save_rule_metadata(pattern, hit_count, miss_count, avg_latency).await?;
```

**数据库表**:
- `learned_patterns`: 学习的模式
- `classifier_state`: 分类器状态
- `rule_metadata`: 规则性能元数据

### 6. ConflictDetector - 冲突检测器

检测和解决 L2 规则冲突。

```rust
use alephcore::engine::ConflictDetector;

let detector = ConflictDetector::new();
let conflicts = detector.detect(&reflex_layer);

for conflict in conflicts {
    println!("Conflict: {}", conflict.description);
    println!("Severity: {:?}", conflict.severity);
    println!("Suggestion: {}", conflict.resolution_suggestion());
}
```

**冲突类型**:
- `PatternOverlap`: 多个规则匹配同一输入
- `PriorityConflict`: 相同优先级的规则竞争
- `AmbiguousMatch`: 相似置信度的多个匹配
- `RedundantRule`: 永远不会命中的规则

**严重级别**:
- `Low`: 2 个规则冲突
- `Medium`: 3-4 个规则冲突
- `High`: 5+ 个规则冲突
- `Critical`: 系统级冲突

### 7. PerformanceOptimizer - 性能优化器

提供缓存、索引和预编译优化。

```rust
use alephcore::engine::PerformanceOptimizer;

let optimizer = PerformanceOptimizer::new(1000); // 缓存大小

// 构建索引
optimizer.build_index(patterns);

// 查询缓存
if let Some(cached) = optimizer.get_cached("git status") {
    return cached.result;
}

// 缓存结果
optimizer.cache("git status", result);

// 获取候选规则
let candidates = optimizer.get_candidates("git status");

// 获取预编译模式
let regex = optimizer.get_compiled_pattern(rule_idx);
```

**性能提升**:
- 缓存命中: ~1 μs（vs ~143 μs）
- 索引查找: ~10 μs（vs ~100 μs）
- 预编译 Regex: 10-100x 提升

---

## API 参考

### AtomicEngine

主引擎类，负责执行原子操作。

```rust
impl AtomicEngine {
    pub fn new(workspace: PathBuf) -> Self;
    pub async fn execute(&self, action: AtomicAction) -> Result<ExecutionResult>;
    pub fn route(&self, input: &str) -> RoutingResult;
    pub fn stats(&self) -> RoutingStats;
}
```

### RuleLearner

规则学习器。

```rust
impl RuleLearner {
    pub fn new() -> Self;
    pub fn learn_success(&self, input: &str, action: AtomicAction);
    pub fn learn_failure(&self, input: &str, action: AtomicAction);
    pub fn generate_rules(&self) -> Vec<KeywordRule>;
    pub fn predict(&self, input: &str) -> Option<(ActionClass, f64)>;
    pub fn stats(&self) -> LearnerStats;
    pub fn clear(&self);
}
```

### LearningAgent

学习代理。

```rust
impl LearningAgent {
    pub fn new(learner: Arc<RuleLearner>, reflex_layer: Arc<RwLock<ReflexLayer>>) -> Self;
    pub async fn on_l3_success(&self, input: &str, action: AtomicAction, latency: Duration);
    pub async fn on_l3_failure(&self, input: &str, action: AtomicAction, error: String);
    pub async fn generate_and_deploy_rules(&self) -> usize;
    pub async fn stats(&self) -> AgentStats;
    pub async fn clear(&self);
}
```

### Persistence

持久化层。

```rust
impl Persistence {
    pub async fn new<P: AsRef<Path>>(db_path: P) -> SqliteResult<Self>;
    pub async fn save_pattern(&self, pattern: &str, action: &AtomicAction, count: usize, successes: usize, failures: usize) -> SqliteResult<()>;
    pub async fn load_patterns(&self) -> SqliteResult<Vec<LearnedPattern>>;
    pub async fn save_classifier(&self, classifier: &NaiveBayesClassifier) -> SqliteResult<()>;
    pub async fn load_classifier(&self) -> SqliteResult<Option<NaiveBayesClassifier>>;
    pub async fn clear_all(&self) -> SqliteResult<()>;
    pub async fn stats(&self) -> SqliteResult<PersistenceStats>;
}
```

### ConflictDetector

冲突检测器。

```rust
impl ConflictDetector {
    pub fn new() -> Self;
    pub fn with_test_inputs(test_inputs: Vec<String>) -> Self;
    pub fn detect(&self, reflex_layer: &ReflexLayer) -> Vec<Conflict>;
    pub fn add_test_input(&mut self, input: String);
}
```

### PerformanceOptimizer

性能优化器。

```rust
impl PerformanceOptimizer {
    pub fn new(max_cache_size: usize) -> Self;
    pub fn get_cached(&self, query: &str) -> Option<CachedResult>;
    pub fn cache(&self, query: &str, result: String);
    pub fn build_index(&self, patterns: Vec<(usize, String)>);
    pub fn get_candidates(&self, query: &str) -> Vec<usize>;
    pub fn get_compiled_pattern(&self, rule_idx: usize) -> Option<Regex>;
    pub fn cache_stats(&self) -> CacheStats;
    pub fn clear_cache(&self);
}
```

---

## 性能优化

### 1. 启用缓存

```rust
let optimizer = PerformanceOptimizer::new(1000);

// 查询前检查缓存
if let Some(cached) = optimizer.get_cached(input) {
    return Ok(cached.result);
}

// 执行后缓存结果
let result = execute_query(input)?;
optimizer.cache(input, result.clone());
```

### 2. 构建规则索引

```rust
// 提取所有规则模式
let patterns: Vec<(usize, String)> = reflex_layer
    .rules()
    .iter()
    .enumerate()
    .map(|(i, rule)| (i, rule.pattern.to_string()))
    .collect();

// 构建索引
optimizer.build_index(patterns);

// 使用索引过滤候选规则
let candidates = optimizer.get_candidates(input);
```

### 3. 使用预编译 Regex

```rust
// 获取预编译的 Regex
if let Some(regex) = optimizer.get_compiled_pattern(rule_idx) {
    // 直接使用，无需重新编译
    if regex.is_match(input) {
        // ...
    }
}
```

### 4. 监控性能指标

```rust
// 缓存统计
let cache_stats = optimizer.cache_stats();
println!("Hit rate: {:.2}%", cache_stats.hit_rate * 100.0);

// 索引统计
let index_stats = optimizer.index_stats();
println!("Keywords: {}", index_stats.keyword_count);
```

---

## 最佳实践

### 1. 学习策略

**推荐配置**:
- 最少观察数: 100（平衡学习速度和准确性）
- 置信度阈值: 0.8（确保规则质量）
- 生成间隔: 5 分钟（避免过于频繁）

**示例**:
```rust
// 只学习成功的执行
if execution_successful {
    agent.on_l3_success(input, action, latency).await;
}

// 定期生成规则
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(300));
    loop {
        interval.tick().await;
        agent.generate_and_deploy_rules().await;
    }
});
```

### 2. 持久化策略

**推荐做法**:
- 定期保存（每小时或每 1000 个观察）
- 启动时加载
- 优雅关闭时保存

**示例**:
```rust
// 启动时加载
let patterns = persistence.load_patterns().await?;
for pattern in patterns {
    learner.restore_pattern(pattern);
}

// 定期保存
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    loop {
        interval.tick().await;
        persistence.save_all(&learner).await?;
    }
});

// 关闭时保存
tokio::signal::ctrl_c().await?;
persistence.save_all(&learner).await?;
```

### 3. 冲突处理

**推荐流程**:
1. 定期检测冲突（每天或每周）
2. 优先处理 High/Critical 级别
3. 应用自动解决建议
4. 人工审核 Medium 级别

**示例**:
```rust
let conflicts = detector.detect(&reflex_layer);

// 过滤高严重级别
let critical = conflicts.iter()
    .filter(|c| c.is_high_severity())
    .collect::<Vec<_>>();

// 生成解决策略
for conflict in critical {
    let strategies = ConflictResolver::resolve(conflict);
    // 应用策略...
}
```

### 4. 性能监控

**关键指标**:
- L2 命中率（目标 > 90%）
- 缓存命中率（目标 > 50%）
- 平均延迟（目标 < 200 μs）

**示例**:
```rust
// 定期输出统计
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;

        let cache_stats = optimizer.cache_stats();
        let learner_stats = learner.stats();

        info!(
            "Cache hit rate: {:.2}%, Rules: {}",
            cache_stats.hit_rate * 100.0,
            learner_stats.rules_generated
        );
    }
});
```

---

## 故障排查

### 问题 1: L2 命中率低

**症状**: L2 命中率 < 80%

**可能原因**:
- 规则数量不足
- 规则模式不够精确
- 用户输入变化大

**解决方案**:
```rust
// 1. 检查规则数量
let stats = learner.stats();
if stats.rules_generated < 10 {
    // 增加观察数据
}

// 2. 降低置信度阈值（临时）
// 修改 MIN_CONFIDENCE 从 0.8 到 0.7

// 3. 添加更多测试输入
detector.add_test_input("common user input");
```

### 问题 2: 缓存命中率低

**症状**: 缓存命中率 < 30%

**可能原因**:
- 缓存大小太小
- TTL 太短
- 查询变化大

**解决方案**:
```rust
// 1. 增加缓存大小
let optimizer = PerformanceOptimizer::new(5000); // 从 1000 增加到 5000

// 2. 调整 TTL（需要修改源码）
// cached.ttl = Duration::from_secs(600); // 从 300 增加到 600

// 3. 规范化查询
let normalized = input.trim().to_lowercase();
optimizer.get_cached(&normalized);
```

### 问题 3: 规则冲突过多

**症状**: 检测到大量 PatternOverlap 冲突

**可能原因**:
- 规则模式过于宽泛
- 优先级设置不当

**解决方案**:
```rust
// 1. 使用冲突检测器
let conflicts = detector.detect(&reflex_layer);

// 2. 应用解决策略
for conflict in conflicts {
    if conflict.conflict_type == ConflictType::PatternOverlap {
        let strategies = ConflictResolver::resolve(&conflict);
        // 应用优先级调整策略
    }
}

// 3. 手动调整规则
// 使规则模式更具体，例如：
// "git.*" → "git\\s+(status|log|diff)"
```

### 问题 4: 内存占用过高

**症状**: 内存使用持续增长

**可能原因**:
- 缓存未清理
- 学习数据累积
- 事件记录过多

**解决方案**:
```rust
// 1. 定期清理缓存
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    loop {
        interval.tick().await;
        optimizer.clear_cache();
    }
});

// 2. 限制学习数据
if learner.pattern_count() > 10000 {
    learner.clear();
}

// 3. 清理事件记录
agent.clear().await;
```

---

## 附录

### A. 配置参数参考

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `MIN_EXECUTIONS` | 3 | 生成规则的最少执行次数 |
| `MIN_CONFIDENCE` | 0.8 | 生成规则的最低置信度 |
| `MIN_OBSERVATIONS` | 100 | 触发规则生成的最少观察数 |
| `GENERATION_INTERVAL_SECS` | 300 | 规则生成间隔（秒） |
| `CACHE_SIZE` | 1000 | 缓存大小 |
| `CACHE_TTL` | 300 | 缓存 TTL（秒） |

### B. 性能基准

| 操作 | 延迟 | 吞吐量 |
|------|------|--------|
| L1 缓存命中 | ~1 μs | 1M ops/sec |
| L2 路由 | ~143 μs | 7K ops/sec |
| L3 LLM 调用 | ~1-5s | 0.2-1 ops/sec |
| 规则生成 | ~10ms | 100 rules/sec |
| 持久化保存 | ~1ms | 1K ops/sec |

### C. 错误代码

| 代码 | 说明 |
|------|------|
| `E001` | 文件不存在 |
| `E002` | 权限不足 |
| `E003` | 模式匹配失败 |
| `E004` | 规则冲突 |
| `E005` | 数据库错误 |

---

## 更新日志

### v1.0.0 (2026-02-08)

**新增功能**:
- ✅ Search/Replace/Move 原子操作
- ✅ ML 规则学习系统
- ✅ Agent Loop 集成
- ✅ 持久化存储
- ✅ 规则冲突检测
- ✅ 性能优化（缓存、索引）

**性能提升**:
- L2 命中率: 87.5% → 95%+
- 缓存命中延迟: ~1 μs
- 规则索引查找: ~10 μs

**测试覆盖**:
- 170 个单元测试
- 100% 测试通过率

---

## 贡献指南

欢迎贡献！请参考 [CONTRIBUTING.md](../CONTRIBUTING.md)。

## 许可证

MIT License - 详见 [LICENSE](../LICENSE)。

## 联系方式

- GitHub Issues: https://github.com/your-org/aleph/issues
- 文档: https://docs.aleph.ai
- 社区: https://community.aleph.ai
