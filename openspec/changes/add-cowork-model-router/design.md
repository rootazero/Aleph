# Design: Cowork Multi-Model Router

## Context

Aether Cowork 需要支持多模型协作来处理复杂任务。当前架构中，所有 AI 请求都通过单一的 Router 路由到一个 provider。这在简单场景下够用，但对于 Cowork 的多步骤任务执行来说，存在以下问题：

1. 不同任务类型有不同的最优模型选择
2. 成本控制需要智能路由
3. 隐私敏感数据需要路由到本地模型
4. 多模型 pipeline 需要上下文传递

## Goals / Non-Goals

### Goals

1. 定义 ModelProfile 数据结构，描述模型能力
2. 实现 ModelMatcher 根据任务类型选择最佳模型
3. 实现 PipelineExecutor 支持多模型链式执行
4. 集成 Memory 模块实现跨任务上下文传递
5. 提供配置界面让用户自定义路由规则

### Non-Goals

1. 动态模型发现（用户需手动配置模型）
2. 跨会话的模型性能学习
3. 自动降级和重试策略（留给未来）
4. 模型 A/B 测试框架

## Decisions

### Decision 1: Capability-Based Routing

**选择**: 基于能力标签的路由，而非基于任务类型的硬编码

**原因**:
- 灵活性：用户可以为新模型添加能力标签
- 可扩展：新能力不需要修改核心代码
- 可配置：用户可以覆盖默认的能力-模型映射

**备选方案**:
- 硬编码任务-模型映射：简单但不灵活
- LLM 自动选择：延迟高，可能不稳定

### Decision 2: Profile + Matcher Architecture

```
                    ┌─────────────────────────────────────────┐
                    │              ModelRouter                 │
                    │  ┌─────────────────────────────────────┐│
                    │  │           ModelMatcher               ││
                    │  │  ┌──────────┐  ┌───────────────────┐││
                    │  │  │ Profiles │  │  Routing Rules    │││
                    │  │  │ HashMap  │  │  Vec<RoutingRule> │││
                    │  │  └──────────┘  └───────────────────┘││
                    │  └─────────────────────────────────────┘│
                    └─────────────────────────────────────────┘
                                        │
                    ┌───────────────────┼───────────────────┐
                    ▼                   ▼                   ▼
            ┌───────────────┐   ┌───────────────┐   ┌───────────────┐
            │ ModelProfile  │   │ ModelProfile  │   │ ModelProfile  │
            │ claude-opus   │   │ gpt-4o        │   │ ollama-llama  │
            │ - reasoning   │   │ - image       │   │ - privacy     │
            │ - code_gen    │   │ - code_gen    │   │ - fast        │
            └───────────────┘   └───────────────┘   └───────────────┘
```

**选择**: Profile 存储模型元数据，Matcher 执行路由决策

**原因**:
- 关注点分离：Profile 描述模型，Matcher 执行策略
- 易于测试：可以独立测试 Profile 解析和 Matcher 逻辑
- 可扩展：可以添加新的 Matcher 策略

### Decision 3: Pipeline Execution Model

```rust
pub struct PipelineExecutor {
    model_router: Arc<dyn ModelRouter>,
    task_context: TaskContextManager,
}

impl PipelineExecutor {
    pub async fn execute_pipeline(
        &self,
        stages: Vec<PipelineStage>,
    ) -> Result<Vec<StageResult>> {
        let mut results = Vec::new();
        let mut context = PipelineContext::new();

        for stage in stages {
            // Select model for this stage
            let profile = self.model_router.route(&stage.task)?;

            // Inject context from previous stages
            let enriched_task = context.enrich_task(&stage.task);

            // Execute with selected model
            let result = self.execute_with_model(&enriched_task, &profile).await?;

            // Store result in context for next stages
            context.add_result(&stage.id, &result);
            results.push(result);
        }

        Ok(results)
    }
}
```

**选择**: 顺序执行 + 上下文累积

**原因**:
- 简单可预测
- 支持依赖关系
- 便于调试

**备选方案**:
- 并行执行：复杂，上下文传递困难
- DAG 执行：与 Scheduler 重复，不必要

### Decision 4: Memory Integration Pattern

```rust
pub struct TaskContextManager {
    memory_store: Arc<dyn MemoryStore>,
    current_graph_id: String,
}

impl TaskContextManager {
    /// Store task result in memory for future reference
    pub async fn store_result(
        &self,
        task_id: &str,
        result: &TaskResult,
    ) -> Result<()> {
        let memory = Memory::new()
            .with_key(format!("cowork:{}:{}", self.current_graph_id, task_id))
            .with_content(serde_json::to_string(&result)?)
            .with_metadata(MemoryMetadata {
                source: "cowork".to_string(),
                task_type: result.task_type.to_string(),
                timestamp: chrono::Utc::now(),
            });

        self.memory_store.store(memory).await
    }

    /// Retrieve context from previous tasks
    pub async fn get_context(
        &self,
        task_id: &str,
        dependencies: &[String],
    ) -> Result<TaskContext> {
        let mut context = TaskContext::default();

        for dep_id in dependencies {
            let key = format!("cowork:{}:{}", self.current_graph_id, dep_id);
            if let Some(memory) = self.memory_store.get(&key).await? {
                context.add_dependency_result(dep_id, &memory.content);
            }
        }

        Ok(context)
    }
}
```

**选择**: 使用现有 Memory 模块存储任务结果

**原因**:
- 复用现有基础设施
- 支持跨会话持久化
- 支持语义检索

## Data Structures

### ModelProfile

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Unique identifier (e.g., "claude-opus", "gpt-4o")
    pub id: String,

    /// Provider name (anthropic, openai, google, ollama)
    pub provider: String,

    /// Model name for API calls
    pub model: String,

    /// Capability tags
    pub capabilities: Vec<Capability>,

    /// Cost tier for cost-aware routing
    pub cost_tier: CostTier,

    /// Latency tier for latency-sensitive tasks
    pub latency_tier: LatencyTier,

    /// Maximum context window
    pub max_context: Option<u32>,

    /// Whether this is a local model
    pub local: bool,

    /// Custom parameters
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    CodeGeneration,
    CodeReview,
    TextAnalysis,
    ImageUnderstanding,
    VideoUnderstanding,
    LongContext,
    Reasoning,
    LocalPrivacy,
    FastResponse,
    SimpleTask,
    LongDocument,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CostTier {
    Free,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LatencyTier {
    Fast,
    Medium,
    Slow,
}
```

### ModelRoutingRules

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingRules {
    /// Task type to model profile mapping
    pub task_type_mappings: HashMap<String, String>,

    /// Capability to model profile mapping (fallback)
    pub capability_mappings: HashMap<Capability, String>,

    /// Cost optimization strategy
    pub cost_strategy: CostStrategy,

    /// Default model when no rule matches
    pub default_model: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostStrategy {
    Cheapest,
    Balanced,
    BestQuality,
}
```

### PipelineStage

```rust
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub id: String,
    pub task: Task,
    pub model_override: Option<String>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PipelineContext {
    pub results: HashMap<String, StageResult>,
    pub accumulated_tokens: u32,
    pub total_cost: f64,
}

#[derive(Debug, Clone)]
pub struct StageResult {
    pub stage_id: String,
    pub model_used: String,
    pub output: serde_json::Value,
    pub tokens_used: u32,
    pub duration: std::time::Duration,
}
```

## Configuration Schema

```toml
# Model Profiles Definition
[cowork.model_profiles]
# Each profile defines a model's capabilities

[cowork.model_profiles.claude-opus]
provider = "anthropic"
model = "claude-opus-4"
capabilities = ["reasoning", "code_generation", "long_context"]
cost_tier = "high"
latency_tier = "slow"
max_context = 200000

[cowork.model_profiles.claude-sonnet]
provider = "anthropic"
model = "claude-sonnet-4"
capabilities = ["code_generation", "code_review", "reasoning"]
cost_tier = "medium"
latency_tier = "medium"
max_context = 200000

[cowork.model_profiles.claude-haiku]
provider = "anthropic"
model = "claude-haiku"
capabilities = ["fast_response", "simple_task"]
cost_tier = "low"
latency_tier = "fast"

[cowork.model_profiles.gpt-4o]
provider = "openai"
model = "gpt-4o"
capabilities = ["image_understanding", "code_generation"]
cost_tier = "medium"
latency_tier = "medium"

[cowork.model_profiles.gemini-pro]
provider = "google"
model = "gemini-1.5-pro"
capabilities = ["video_understanding", "long_document", "long_context"]
cost_tier = "medium"
latency_tier = "medium"
max_context = 1000000

[cowork.model_profiles.ollama-llama]
provider = "ollama"
model = "llama3.2"
capabilities = ["local_privacy", "fast_response"]
cost_tier = "free"
latency_tier = "fast"
local = true

# Model Routing Rules
[cowork.model_routing]
# Task type to model mapping
code_generation = "claude-opus"
code_review = "claude-sonnet"
image_analysis = "gpt-4o"
video_understanding = "gemini-pro"
long_document = "gemini-pro"
quick_tasks = "claude-haiku"
privacy_sensitive = "ollama-llama"
reasoning = "claude-opus"

# Cost optimization strategy
cost_strategy = "balanced"

# Enable multi-model pipelines
enable_pipelines = true

# Default model
default_model = "claude-sonnet"

# User overrides (optional)
[cowork.model_routing.overrides]
# code_generation = "gpt-4-turbo"
```

## Module Structure

```
Aether/core/src/cowork/
├── mod.rs                    # Public API exports
├── model_router/
│   ├── mod.rs                # Module exports
│   ├── profiles.rs           # ModelProfile, Capability, CostTier
│   ├── matcher.rs            # ModelMatcher implementation
│   ├── pipeline.rs           # PipelineExecutor
│   ├── context.rs            # TaskContextManager
│   └── config.rs             # Configuration parsing
└── ...
```

## API Design

### Public API

```rust
// lib.rs - UniFFI exports
pub use cowork::model_router::{
    ModelProfile,
    ModelMatcher,
    PipelineExecutor,
    Capability,
    CostTier,
    LatencyTier,
};

// Initialize model router with config
pub fn create_model_router(config: &Config) -> Result<Arc<dyn ModelRouter>> {
    let profiles = parse_model_profiles(&config.cowork.model_profiles)?;
    let rules = parse_routing_rules(&config.cowork.model_routing)?;
    Ok(Arc::new(ModelMatcher::new(profiles, rules)))
}

// Route a task to optimal model
pub async fn route_task(
    router: &dyn ModelRouter,
    task: &Task,
) -> Result<ModelProfile> {
    router.route(task)
}

// Execute pipeline with multiple models
pub async fn execute_pipeline(
    executor: &PipelineExecutor,
    stages: Vec<PipelineStage>,
) -> Result<Vec<StageResult>> {
    executor.execute_pipeline(stages).await
}
```

### Trait Definition

```rust
#[async_trait]
pub trait ModelRouter: Send + Sync {
    /// Route task to optimal model profile
    fn route(&self, task: &Task) -> Result<ModelProfile>;

    /// Get model profile by ID
    fn get_profile(&self, id: &str) -> Option<&ModelProfile>;

    /// List all available profiles
    fn profiles(&self) -> &[ModelProfile];

    /// Check if model supports capability
    fn supports_capability(&self, profile_id: &str, capability: &Capability) -> bool;
}
```

## Risks / Trade-offs

### Risk 1: Configuration Complexity

**风险**: 用户需要配置大量的模型 profiles 和路由规则

**缓解**:
- 提供合理的默认配置
- 在 Settings UI 提供模型发现和推荐
- 支持从模板导入配置

### Risk 2: Model Availability

**风险**: 配置的模型可能不可用（API key 缺失、服务宕机）

**缓解**:
- 实现 fallback 机制
- 路由前检查模型可用性
- 提供清晰的错误信息

### Risk 3: Cost Tracking Accuracy

**风险**: 不同 provider 的计费方式不同，难以准确估算成本

**缓解**:
- 使用 token 数量作为统一度量
- 在 StageResult 中记录实际用量
- 允许用户设置成本上限

## Migration Plan

### Phase 4.1: Core Infrastructure
1. 实现 ModelProfile 数据结构
2. 实现配置解析
3. 单元测试

### Phase 4.2: ModelMatcher
1. 实现 ModelMatcher
2. 实现 capability-based routing
3. 集成测试

### Phase 4.3: Pipeline Executor
1. 实现 PipelineExecutor
2. 实现上下文传递
3. 端到端测试

### Phase 4.4: Memory Integration
1. 实现 TaskContextManager
2. 集成 Memory 模块
3. 持久化测试

### Phase 4.5: Settings UI
1. 实现模型配置界面
2. 实现路由规则配置
3. UI 测试

## Open Questions

1. **模型性能监控**: 是否需要跟踪每个模型的响应时间和成功率？
2. **动态路由调整**: 是否需要根据运行时性能自动调整路由？
3. **用户偏好学习**: 是否需要根据用户反馈优化路由决策？
