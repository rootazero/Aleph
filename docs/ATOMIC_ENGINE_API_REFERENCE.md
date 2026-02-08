# Atomic Engine API 参考文档

> 完整的 API 文档，包含所有公开接口、参数说明和使用示例

## 目录

- [核心模块](#核心模块)
- [学习模块](#学习模块)
- [持久化模块](#持久化模块)
- [优化模块](#优化模块)
- [类型定义](#类型定义)
- [错误处理](#错误处理)

---

## 核心模块

### AtomicEngine

主引擎类，负责执行原子操作和路由。

#### 构造函数

```rust
pub fn new(workspace: PathBuf) -> Self
```

创建新的 AtomicEngine 实例。

**参数**:
- `workspace`: 工作目录路径

**示例**:
```rust
let engine = AtomicEngine::new(PathBuf::from("/project"));
```

#### execute

```rust
pub async fn execute(&self, action: AtomicAction) -> Result<ExecutionResult>
```

执行原子操作。

**参数**:
- `action`: 要执行的原子操作

**返回**:
- `Ok(ExecutionResult)`: 执行结果
- `Err(AlephError)`: 执行错误

**示例**:
```rust
let action = AtomicAction::Read {
    path: PathBuf::from("src/main.rs"),
};

let result = engine.execute(action).await?;
println!("Output: {}", result.output);
println!("Layer: {:?}", result.layer);
println!("Latency: {:?}", result.latency);
```

#### route

```rust
pub fn route(&self, input: &str) -> RoutingResult
```

路由用户输入到合适的层级。

**参数**:
- `input`: 用户输入字符串

**返回**:
- `RoutingResult`: 路由结果（L1/L2/L3）

**示例**:
```rust
let result = engine.route("git status");
match result.layer {
    RoutingLayer::L1 => println!("Cache hit!"),
    RoutingLayer::L2 => println!("Reflex match!"),
    RoutingLayer::L3 => println!("LLM required"),
}
```

#### stats

```rust
pub fn stats(&self) -> RoutingStats
```

获取路由统计信息。

**返回**:
- `RoutingStats`: 统计数据

**示例**:
```rust
let stats = engine.stats();
println!("L1 hits: {}", stats.l1_hits);
println!("L2 hits: {}", stats.l2_hits);
println!("L3 hits: {}", stats.l3_hits);
println!("Hit rate: {:.2}%", stats.hit_rate() * 100.0);
```

---

## 学习模块

### RuleLearner

规则学习器，从执行历史学习并生成规则。

#### 构造函数

```rust
pub fn new() -> Self
```

创建新的 RuleLearner 实例。

**示例**:
```rust
let learner = RuleLearner::new();
```

#### learn_success

```rust
pub fn learn_success(&self, input: &str, action: AtomicAction)
```

从成功的执行中学习。

**参数**:
- `input`: 用户输入
- `action`: 执行的动作

**示例**:
```rust
learner.learn_success(
    "search for TODO",
    AtomicAction::Search {
        pattern: SearchPattern::Regex {
            pattern: "TODO".to_string(),
        },
        scope: SearchScope::Workspace,
        filters: vec![],
    }
);
```

#### learn_failure

```rust
pub fn learn_failure(&self, input: &str, action: AtomicAction)
```

从失败的执行中学习。

**参数**:
- `input`: 用户输入
- `action`: 尝试的动作

**示例**:
```rust
learner.learn_failure(
    "invalid command",
    AtomicAction::Bash {
        command: "invalid".to_string(),
        cwd: None,
    }
);
```

#### generate_rules

```rust
pub fn generate_rules(&self) -> Vec<KeywordRule>
```

生成 L2 路由规则。

**返回**:
- `Vec<KeywordRule>`: 生成的规则列表

**示例**:
```rust
let rules = learner.generate_rules();
for rule in rules {
    println!("Pattern: {:?}", rule.pattern);
    println!("Priority: {}", rule.priority);
}
```

#### predict

```rust
pub fn predict(&self, input: &str) -> Option<(ActionClass, f64)>
```

预测输入的动作类型。

**参数**:
- `input`: 用户输入

**返回**:
- `Some((ActionClass, f64))`: 预测的动作类型和置信度
- `None`: 无法预测

**示例**:
```rust
if let Some((action_class, confidence)) = learner.predict("search for TODO") {
    println!("Predicted: {:?} (confidence: {:.2})", action_class, confidence);
}
```

#### stats

```rust
pub fn stats(&self) -> LearnerStats
```

获取学习统计信息。

**返回**:
- `LearnerStats`: 统计数据

**示例**:
```rust
let stats = learner.stats();
println!("Total observations: {}", stats.total_observations);
println!("Rules generated: {}", stats.rules_generated);
```

#### clear

```rust
pub fn clear(&self)
```

清除所有学习数据。

**示例**:
```rust
learner.clear();
```

### FeatureExtractor

特征提取器，从用户输入提取特征。

#### 构造函数

```rust
pub fn new() -> Self
```

创建新的 FeatureExtractor 实例。

**示例**:
```rust
let extractor = FeatureExtractor::new();
```

#### extract

```rust
pub fn extract(&self, input: &str) -> FeatureVector
```

提取特征向量。

**参数**:
- `input`: 用户输入

**返回**:
- `FeatureVector`: 提取的特征

**示例**:
```rust
let features = extractor.extract("search for TODO in file");

println!("Keywords: {:?}", features.keywords);
// Output: ["search", "todo", "file"]

println!("Intent: {:?}", features.intent);
// Output: Search

println!("Entities: {:?}", features.entities);
// Output: [Pattern("TODO")]

println!("Confidence: {:.2}", features.confidence);
// Output: 0.85
```

### NaiveBayesClassifier

朴素贝叶斯分类器，用于动作预测。

#### 构造函数

```rust
pub fn new() -> Self
```

创建新的 NaiveBayesClassifier 实例。

**示例**:
```rust
let mut classifier = NaiveBayesClassifier::new();
```

#### train

```rust
pub fn train(&mut self, features: &FeatureVector, action_class: ActionClass)
```

训练分类器（增量学习）。

**参数**:
- `features`: 特征向量
- `action_class`: 动作类型

**示例**:
```rust
let features = extractor.extract("search for TODO");
classifier.train(&features, ActionClass::Search);
```

#### predict

```rust
pub fn predict(&self, features: &FeatureVector) -> Option<(ActionClass, f64)>
```

预测动作类型。

**参数**:
- `features`: 特征向量

**返回**:
- `Some((ActionClass, f64))`: 预测的动作类型和概率
- `None`: 无训练数据

**示例**:
```rust
let features = extractor.extract("find TODO");
if let Some((action_class, prob)) = classifier.predict(&features) {
    println!("Predicted: {:?} (probability: {:.2})", action_class, prob);
}
```

#### sample_count

```rust
pub fn sample_count(&self) -> usize
```

获取训练样本数量。

**返回**:
- `usize`: 样本数量

**示例**:
```rust
println!("Trained on {} samples", classifier.sample_count());
```

#### clear

```rust
pub fn clear(&mut self)
```

清除所有训练数据。

**示例**:
```rust
classifier.clear();
```

### LearningAgent

学习代理，集成 Agent Loop 自动学习。

#### 构造函数

```rust
pub fn new(
    learner: Arc<RuleLearner>,
    reflex_layer: Arc<RwLock<ReflexLayer>>
) -> Self
```

创建新的 LearningAgent 实例。

**参数**:
- `learner`: 规则学习器
- `reflex_layer`: 反射层

**示例**:
```rust
let learner = Arc::new(RuleLearner::new());
let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
let agent = LearningAgent::new(learner, reflex_layer);
```

#### on_l3_success

```rust
pub async fn on_l3_success(
    &self,
    input: &str,
    action: AtomicAction,
    latency: Duration
)
```

处理 L3 执行成功事件。

**参数**:
- `input`: 用户输入
- `action`: 执行的动作
- `latency`: 执行延迟

**示例**:
```rust
agent.on_l3_success(
    "search for TODO",
    search_action,
    Duration::from_millis(100)
).await;
```

#### on_l3_failure

```rust
pub async fn on_l3_failure(
    &self,
    input: &str,
    action: AtomicAction,
    error: String
)
```

处理 L3 执行失败事件。

**参数**:
- `input`: 用户输入
- `action`: 尝试的动作
- `error`: 错误信息

**示例**:
```rust
agent.on_l3_failure(
    "invalid command",
    bash_action,
    "Command not found".to_string()
).await;
```

#### generate_and_deploy_rules

```rust
pub async fn generate_and_deploy_rules(&self) -> usize
```

生成并部署 L2 规则。

**返回**:
- `usize`: 部署的规则数量

**示例**:
```rust
let count = agent.generate_and_deploy_rules().await;
println!("Deployed {} rules", count);
```

#### stats

```rust
pub async fn stats(&self) -> AgentStats
```

获取代理统计信息。

**返回**:
- `AgentStats`: 统计数据

**示例**:
```rust
let stats = agent.stats().await;
println!("L3 successes: {}", stats.l3_successes);
println!("L3 failures: {}", stats.l3_failures);
println!("Rules deployed: {}", stats.rules_deployed);
println!("Success rate: {:.2}%", stats.success_rate() * 100.0);
```

#### clear

```rust
pub async fn clear(&self)
```

清除所有学习数据。

**示例**:
```rust
agent.clear().await;
```

---

## 持久化模块

### Persistence

持久化层，保存和加载学习数据。

#### 构造函数

```rust
pub async fn new<P: AsRef<Path>>(db_path: P) -> SqliteResult<Self>
```

创建新的 Persistence 实例。

**参数**:
- `db_path`: 数据库文件路径

**返回**:
- `Ok(Persistence)`: 持久化实例
- `Err(SqliteError)`: 数据库错误

**示例**:
```rust
let persistence = Persistence::new("./data/learned_rules.db").await?;
```

#### save_pattern

```rust
pub async fn save_pattern(
    &self,
    pattern: &str,
    action: &AtomicAction,
    count: usize,
    successes: usize,
    failures: usize
) -> SqliteResult<()>
```

保存学习模式。

**参数**:
- `pattern`: 模式字符串
- `action`: 关联的动作
- `count`: 观察次数
- `successes`: 成功次数
- `failures`: 失败次数

**示例**:
```rust
persistence.save_pattern(
    "search for TODO",
    &search_action,
    5,  // count
    5,  // successes
    0   // failures
).await?;
```

#### load_patterns

```rust
pub async fn load_patterns(&self) -> SqliteResult<Vec<LearnedPattern>>
```

加载所有学习模式。

**返回**:
- `Ok(Vec<LearnedPattern>)`: 模式列表
- `Err(SqliteError)`: 数据库错误

**示例**:
```rust
let patterns = persistence.load_patterns().await?;
for pattern in patterns {
    println!("Pattern: {}", pattern.pattern);
    println!("Confidence: {:.2}", pattern.confidence);
}
```

#### save_classifier

```rust
pub async fn save_classifier(
    &self,
    classifier: &NaiveBayesClassifier
) -> SqliteResult<()>
```

保存分类器状态。

**参数**:
- `classifier`: 分类器实例

**示例**:
```rust
persistence.save_classifier(&classifier).await?;
```

#### load_classifier

```rust
pub async fn load_classifier(&self) -> SqliteResult<Option<NaiveBayesClassifier>>
```

加载分类器状态。

**返回**:
- `Ok(Some(NaiveBayesClassifier))`: 分类器实例
- `Ok(None)`: 无保存的状态
- `Err(SqliteError)`: 数据库错误

**示例**:
```rust
if let Some(classifier) = persistence.load_classifier().await? {
    println!("Loaded classifier with {} samples", classifier.sample_count());
}
```

#### save_rule_metadata

```rust
pub async fn save_rule_metadata(
    &self,
    pattern: &str,
    hit_count: usize,
    miss_count: usize,
    avg_latency_ms: f64
) -> SqliteResult<()>
```

保存规则元数据。

**参数**:
- `pattern`: 规则模式
- `hit_count`: 命中次数
- `miss_count`: 未命中次数
- `avg_latency_ms`: 平均延迟（毫秒）

**示例**:
```rust
persistence.save_rule_metadata(
    "git.*status",
    100,  // hit_count
    5,    // miss_count
    50.0  // avg_latency_ms
).await?;
```

#### load_rule_metadata

```rust
pub async fn load_rule_metadata(&self) -> SqliteResult<Vec<RuleMetadata>>
```

加载规则元数据。

**返回**:
- `Ok(Vec<RuleMetadata>)`: 元数据列表
- `Err(SqliteError)`: 数据库错误

**示例**:
```rust
let metadata = persistence.load_rule_metadata().await?;
for meta in metadata {
    println!("Pattern: {}", meta.pattern);
    println!("Hit count: {}", meta.hit_count);
    println!("Avg latency: {:.2}ms", meta.avg_latency_ms);
}
```

#### clear_all

```rust
pub async fn clear_all(&self) -> SqliteResult<()>
```

清除所有数据。

**示例**:
```rust
persistence.clear_all().await?;
```

#### stats

```rust
pub async fn stats(&self) -> SqliteResult<PersistenceStats>
```

获取持久化统计信息。

**返回**:
- `Ok(PersistenceStats)`: 统计数据
- `Err(SqliteError)`: 数据库错误

**示例**:
```rust
let stats = persistence.stats().await?;
println!("Patterns: {}", stats.pattern_count);
println!("Rules: {}", stats.rule_count);
println!("Total samples: {}", stats.total_samples);
```

---

## 优化模块

### ConflictDetector

冲突检测器，检测规则冲突。

#### 构造函数

```rust
pub fn new() -> Self
```

创建新的 ConflictDetector 实例（使用默认测试输入）。

**示例**:
```rust
let detector = ConflictDetector::new();
```

#### with_test_inputs

```rust
pub fn with_test_inputs(test_inputs: Vec<String>) -> Self
```

创建带自定义测试输入的检测器。

**参数**:
- `test_inputs`: 测试输入列表

**示例**:
```rust
let detector = ConflictDetector::with_test_inputs(vec![
    "git status".to_string(),
    "search for TODO".to_string(),
]);
```

#### detect

```rust
pub fn detect(&self, reflex_layer: &ReflexLayer) -> Vec<Conflict>
```

检测规则冲突。

**参数**:
- `reflex_layer`: 反射层

**返回**:
- `Vec<Conflict>`: 检测到的冲突列表

**示例**:
```rust
let conflicts = detector.detect(&reflex_layer);
for conflict in conflicts {
    println!("Type: {:?}", conflict.conflict_type);
    println!("Severity: {:?}", conflict.severity);
    println!("Description: {}", conflict.description);
    println!("Suggestion: {}", conflict.resolution_suggestion());
}
```

#### add_test_input

```rust
pub fn add_test_input(&mut self, input: String)
```

添加测试输入。

**参数**:
- `input`: 测试输入

**示例**:
```rust
detector.add_test_input("custom test input".to_string());
```

### ConflictResolver

冲突解决器，生成解决策略。

#### resolve

```rust
pub fn resolve(conflict: &Conflict) -> Vec<ResolutionStrategy>
```

生成解决策略。

**参数**:
- `conflict`: 冲突

**返回**:
- `Vec<ResolutionStrategy>`: 解决策略列表

**示例**:
```rust
let strategies = ConflictResolver::resolve(&conflict);
for strategy in strategies {
    match strategy {
        ResolutionStrategy::AdjustPriorities { rule_index, new_priority } => {
            println!("Adjust rule {} priority to {}", rule_index, new_priority);
        }
        ResolutionStrategy::RefinePattern { rule_index, new_pattern } => {
            println!("Refine rule {} pattern to {}", rule_index, new_pattern);
        }
        _ => {}
    }
}
```

### PerformanceOptimizer

性能优化器，提供缓存和索引。

#### 构造函数

```rust
pub fn new(max_cache_size: usize) -> Self
```

创建新的 PerformanceOptimizer 实例。

**参数**:
- `max_cache_size`: 最大缓存大小

**示例**:
```rust
let optimizer = PerformanceOptimizer::new(1000);
```

#### get_cached

```rust
pub fn get_cached(&self, query: &str) -> Option<CachedResult>
```

获取缓存结果。

**参数**:
- `query`: 查询字符串

**返回**:
- `Some(CachedResult)`: 缓存的结果
- `None`: 缓存未命中

**示例**:
```rust
if let Some(cached) = optimizer.get_cached("git status") {
    println!("Cache hit! Result: {}", cached.result);
    println!("Age: {:?}", cached.age());
} else {
    println!("Cache miss");
}
```

#### cache

```rust
pub fn cache(&self, query: &str, result: String)
```

缓存查询结果。

**参数**:
- `query`: 查询字符串
- `result`: 结果

**示例**:
```rust
optimizer.cache("git status", "On branch main...".to_string());
```

#### build_index

```rust
pub fn build_index(&self, patterns: Vec<(usize, String)>)
```

构建规则索引。

**参数**:
- `patterns`: (规则索引, 模式) 元组列表

**示例**:
```rust
let patterns = vec![
    (0, r"git\s+status".to_string()),
    (1, r"git\s+log".to_string()),
    (2, r"read\s+.*".to_string()),
];
optimizer.build_index(patterns);
```

#### get_candidates

```rust
pub fn get_candidates(&self, query: &str) -> Vec<usize>
```

获取候选规则索引。

**参数**:
- `query`: 查询字符串

**返回**:
- `Vec<usize>`: 候选规则索引列表

**示例**:
```rust
let candidates = optimizer.get_candidates("git status");
println!("Found {} candidate rules", candidates.len());
```

#### get_compiled_pattern

```rust
pub fn get_compiled_pattern(&self, rule_idx: usize) -> Option<Regex>
```

获取预编译的 Regex 模式。

**参数**:
- `rule_idx`: 规则索引

**返回**:
- `Some(Regex)`: 预编译的模式
- `None`: 模式不存在

**示例**:
```rust
if let Some(regex) = optimizer.get_compiled_pattern(0) {
    if regex.is_match("git status") {
        println!("Pattern matches!");
    }
}
```

#### cache_stats

```rust
pub fn cache_stats(&self) -> CacheStats
```

获取缓存统计信息。

**返回**:
- `CacheStats`: 统计数据

**示例**:
```rust
let stats = optimizer.cache_stats();
println!("Cache size: {}/{}", stats.cache_size, stats.max_cache_size);
println!("Total queries: {}", stats.total_queries);
println!("Cache hits: {}", stats.cache_hits);
println!("Hit rate: {:.2}%", stats.hit_rate * 100.0);
```

#### index_stats

```rust
pub fn index_stats(&self) -> IndexStats
```

获取索引统计信息。

**返回**:
- `IndexStats`: 统计数据

**示例**:
```rust
let stats = optimizer.index_stats();
println!("Keywords: {}", stats.keyword_count);
println!("Compiled patterns: {}", stats.compiled_patterns);
```

#### clear_cache

```rust
pub fn clear_cache(&self)
```

清除缓存。

**示例**:
```rust
optimizer.clear_cache();
```

#### clear_all

```rust
pub fn clear_all(&self)
```

清除所有数据（缓存、索引、统计）。

**示例**:
```rust
optimizer.clear_all();
```

---

## 类型定义

### AtomicAction

原子操作枚举。

```rust
pub enum AtomicAction {
    Read { path: PathBuf },
    Write { path: PathBuf, content: String, mode: WriteMode },
    Edit { path: PathBuf, patches: Vec<Patch> },
    Bash { command: String, cwd: Option<PathBuf> },
    Search { pattern: SearchPattern, scope: SearchScope, filters: Vec<FileFilter> },
    Replace { pattern: SearchPattern, replacement: String, scope: SearchScope, mode: ReplaceMode },
    Move { source: PathBuf, destination: PathBuf, update_imports: bool },
}
```

### SearchPattern

搜索模式枚举。

```rust
pub enum SearchPattern {
    Regex { pattern: String },
    Fuzzy { text: String, threshold: f32 },
    Ast { query: String, language: String },
}
```

### SearchScope

搜索范围枚举。

```rust
pub enum SearchScope {
    File { path: PathBuf },
    Directory { path: PathBuf, recursive: bool },
    Workspace,
}
```

### ActionClass

动作类型枚举。

```rust
pub enum ActionClass {
    Read,
    Write,
    Edit,
    Bash,
    Search,
    Replace,
    Move,
}
```

### ConflictType

冲突类型枚举。

```rust
pub enum ConflictType {
    PatternOverlap,
    PriorityConflict,
    AmbiguousMatch,
    RedundantRule,
}
```

### ConflictSeverity

冲突严重级别枚举。

```rust
pub enum ConflictSeverity {
    Low,
    Medium,
    High,
    Critical,
}
```

---

## 错误处理

### AlephError

主错误类型。

```rust
pub enum AlephError {
    IoError(std::io::Error),
    ParseError(String),
    ExecutionError(String),
    DatabaseError(rusqlite::Error),
    // ...
}
```

### 错误处理示例

```rust
use alephcore::error::{AlephError, Result};

async fn example() -> Result<()> {
    let engine = AtomicEngine::new(workspace)?;

    match engine.execute(action).await {
        Ok(result) => {
            println!("Success: {}", result.output);
        }
        Err(AlephError::IoError(e)) => {
            eprintln!("IO error: {}", e);
        }
        Err(AlephError::ExecutionError(msg)) => {
            eprintln!("Execution error: {}", msg);
        }
        Err(e) => {
            eprintln!("Other error: {}", e);
        }
    }

    Ok(())
}
```

---

## 完整示例

### 端到端学习系统

```rust
use alephcore::engine::*;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. 初始化组件
    let workspace = PathBuf::from("/project");
    let engine = AtomicEngine::new(workspace);
    let learner = Arc::new(RuleLearner::new());
    let reflex_layer = Arc::new(RwLock::new(ReflexLayer::new()));
    let agent = LearningAgent::new(learner.clone(), reflex_layer.clone());
    let persistence = Persistence::new("./data/rules.db").await?;
    let optimizer = PerformanceOptimizer::new(1000);

    // 2. 加载持久化数据
    if let Some(classifier) = persistence.load_classifier().await? {
        // 恢复分类器状态
    }

    // 3. 执行循环
    loop {
        let input = read_user_input();

        // 检查缓存
        if let Some(cached) = optimizer.get_cached(&input) {
            println!("Cache hit: {}", cached.result);
            continue;
        }

        // 路由和执行
        let result = engine.route(&input);
        match result.layer {
            RoutingLayer::L3 => {
                // L3 执行
                let action = llm_generate_action(&input).await?;
                let exec_result = engine.execute(action.clone()).await?;

                // 学习
                agent.on_l3_success(&input, action, exec_result.latency).await;

                // 缓存
                optimizer.cache(&input, exec_result.output.clone());
            }
            _ => {
                // L1/L2 命中
            }
        }

        // 定期生成规则
        if should_generate_rules() {
            let count = agent.generate_and_deploy_rules().await;
            println!("Generated {} rules", count);

            // 保存到数据库
            persistence.save_classifier(&classifier).await?;
        }
    }

    Ok(())
}
```

---

## 版本历史

### v1.0.0 (2026-02-08)

初始发布，包含所有核心功能。

---

## 许可证

MIT License

---

## 贡献

欢迎贡献！请提交 Pull Request 或创建 Issue。
