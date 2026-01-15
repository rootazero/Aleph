# Change: Add Cowork Multi-Model Router

## Why

Phase 1-3 实现了任务编排、文件操作和代码执行基础设施。但当前所有任务都发送到同一个 AI provider，这不是最优方案：

1. **成本问题**：简单任务用昂贵的大模型浪费资源
2. **能力限制**：单一模型无法覆盖所有场景（代码生成、图像理解、长文档处理）
3. **隐私需求**：敏感数据应该路由到本地模型（Ollama）
4. **延迟优化**：快速响应任务应该用轻量级模型

Multi-Model Router 让 Cowork 能够根据任务特性自动选择最合适的模型，实现成本、能力和隐私的最优平衡。

## What Changes

### Core Components

#### 1. Model Profiles Config

定义每个模型的能力标签和特性：

```toml
[cowork.model_profiles.claude-opus]
provider = "anthropic"
model = "claude-opus-4"
capabilities = ["reasoning", "code_generation", "long_context"]
cost_tier = "high"
latency_tier = "slow"
max_context = 200000

[cowork.model_profiles.claude-haiku]
provider = "anthropic"
model = "claude-haiku"
capabilities = ["fast_response", "simple_tasks"]
cost_tier = "low"
latency_tier = "fast"

[cowork.model_profiles.gpt-4o]
provider = "openai"
model = "gpt-4o"
capabilities = ["image_understanding", "code_generation"]
cost_tier = "medium"

[cowork.model_profiles.gemini-pro]
provider = "google"
model = "gemini-1.5-pro"
capabilities = ["video_understanding", "long_document"]
cost_tier = "medium"
max_context = 1000000

[cowork.model_profiles.ollama-llama]
provider = "ollama"
model = "llama3.2"
capabilities = ["local_privacy", "fast_response"]
cost_tier = "free"
local = true
```

#### 2. Model Matcher

根据任务类型和要求自动匹配最佳模型：

```rust
pub struct ModelMatcher {
    profiles: Vec<ModelProfile>,
    routing_rules: ModelRoutingRules,
}

impl ModelRouter for ModelMatcher {
    fn route(&self, task: &Task) -> Result<ModelProfile> {
        match &task.task_type {
            TaskType::CodeExecution(_) =>
                self.find_best_for(Capability::CodeGeneration),
            TaskType::AiInference(t) if t.has_images =>
                self.find_best_for(Capability::ImageUnderstanding),
            TaskType::AiInference(t) if t.requires_privacy =>
                self.find_best_for(Capability::LocalPrivacy),
            _ => self.find_balanced(),
        }
    }
}
```

#### 3. Pipeline Executor

支持多模型链式执行：

- 任务 A（代码生成）→ Claude Opus
- 任务 B（代码审查）→ Claude Sonnet
- 任务 C（文档生成）→ Gemini Pro
- 结果聚合 → 返回给用户

#### 4. Memory Integration

跨任务上下文传递：

- 任务结果存入 Memory 模块
- 后续任务可检索前序任务的输出
- 支持任务间依赖的自动上下文注入

### Configuration

```toml
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
cost_strategy = "balanced"  # "cheapest" | "balanced" | "best_quality"

# Enable multi-model pipelines
enable_pipelines = true

# Default model when no specific routing rule matches
default_model = "claude-sonnet"

[cowork.model_routing.overrides]
# User-specific overrides
# code_generation = "gpt-4-turbo"
```

## Impact

- **Affected specs**:
  - cowork-model-routing (new)
  - ai-routing (extends with model profiles)

- **Affected code**:
  - `core/src/cowork/model_router/` - 新增模块
    - `profiles.rs` - ModelProfile 定义
    - `matcher.rs` - ModelMatcher 实现
    - `pipeline.rs` - PipelineExecutor
    - `memory_ctx.rs` - Memory 集成
  - `core/src/config/types/cowork.rs` - 扩展配置
  - `core/src/providers/` - 添加 capability 标签
  - Swift Settings UI - 新增模型路由配置面板

- **Dependencies**:
  - Phase 1 (TaskGraph, Scheduler) - Required
  - Phase 2 (File Operations) - Optional
  - Phase 3 (Code Execution) - Optional
  - Existing Provider system - Required
  - Existing Memory module - Required for context passing

- **Breaking Changes**: None - additive feature

- **Performance Considerations**:
  - Model profile lookup: O(1) with HashMap
  - Routing decision: O(n) with n = number of rules
  - Pipeline execution: Sequential with parallel optimization for independent tasks
