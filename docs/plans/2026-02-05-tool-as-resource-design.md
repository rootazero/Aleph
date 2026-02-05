# Tool-as-Resource: 动态工具发现与按需水合

> **状态**: Draft
> **作者**: Claude & User
> **日期**: 2026-02-05
> **关联**: [Memory System](../MEMORY_SYSTEM.md), [Tool System](../TOOL_SYSTEM.md)

## 1. 背景与动机

### 1.1 问题陈述

Aleph 的愿景是成为"万物之始"的统一 AI 助手，容纳数以千计的 MCP 工具和 Skill。但当前架构面临 **"规模-上下文"悖论**：

| 工具数量 | Token 消耗 (完整 Schema) | 问题 |
|----------|--------------------------|------|
| 10 | ~2,000-5,000 | 可接受 |
| 50 | ~10,000-25,000 | 上下文拥挤 |
| 200+ | ~40,000-100,000 | **上下文爆炸** |

当前工具发现机制 (`McpSubAgent.find_matching_tools`) 使用简单的字符串匹配 (`prompt_lower.contains(kw)`)，无法进行语义理解，且所有工具 Schema 需要全量注入上下文。

### 1.2 设计目标

1. **语义检索**: 基于用户意图的向量匹配，而非关键词匹配
2. **按需加载**: 只将相关工具的完整 Schema 注入上下文
3. **零感延迟**: 检索延迟隐藏在 LLM 推理之前
4. **自我进化**: 工具描述随使用自动优化

### 1.3 核心洞察

> 工具是"过程性知识" (Procedural Knowledge)，应与"陈述性知识" (Declarative Facts) 统一存储在 Memory 系统中。

Aleph 已有成熟的 Memory 系统 (fastembed + sqlite-vec + 混合检索)，但尚未应用于工具发现。本设计将复用这套基础设施。

---

## 2. 架构设计

### 2.1 系统概览

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          用户输入                                        │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │
┌───────────────────────────────────▼─────────────────────────────────────┐
│  TaskAnalyzer (意图识别)                                                 │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │
          ┌─────────────────────────┴─────────────────────────┐
          │                    并行执行                        │
          ▼                                                   ▼
┌─────────────────────────┐                       ┌─────────────────────────┐
│   ToolRetrieval         │                       │   MemoryRetrieval       │
│   (工具语义检索)         │                       │   (上下文检索)           │
│   ~10-50ms              │                       │   ~10-50ms              │
└───────────┬─────────────┘                       └───────────┬─────────────┘
            │                                                 │
            └─────────────────────┬───────────────────────────┘
                                  │
┌─────────────────────────────────▼───────────────────────────────────────┐
│  HydrationPipeline                                                      │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │ 1. 从 UnifiedToolRegistry 获取完整 Schema                          │ │
│  │ 2. 根据置信度分级注入 (完整 Schema / 摘要 / 跳过)                   │ │
│  │ 3. 组装 PromptContext                                              │ │
│  └────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────┬───────────────────────────────────────┘
                                  │
┌─────────────────────────────────▼───────────────────────────────────────┐
│  Thinker (LLM 推理)                                                     │
│  System Prompt 包含:                                                    │
│  - 核心工具索引 (常驻, ~500 tokens)                                     │
│  - 动态水合的相关工具 Schema (~1000-2000 tokens)                        │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.2 数据模型

#### 2.2.1 ToolFact (Memory 中的工具索引)

工具以 `MemoryFact` 形式存入 Memory 系统，通过 `FactType::Tool` 区分：

```rust
MemoryFact {
    id: "tool:mcp:github:git_commit",
    content: "[Tool] git_commit: Commit staged changes with a message. Use this tool when you need to save your work to version control.",
    fact_type: FactType::Tool,
    specificity: Specificity::Principle,      // 全局能力
    temporal_scope: TemporalScope::Permanent, // 永久有效
    metadata: {
        "tool_id": "mcp:github:git_commit",
        "server": "github",
        "category": "mcp",
        "optimization_level": "L1",  // L0 / L1 / L2
        "optimized_at": null,        // L2 完成时填充
    },
}
```

**Scope 隔离策略**:
- `specificity: Principle` - 工具代表系统的全局能力
- `temporal_scope: Permanent` - 工具定义不随时间变化
- 检索时通过 `fact_type == FactType::Tool` 过滤，避免与用户偏好事实混淆

#### 2.2.2 FactType 扩展

```rust
// core/src/memory/database/facts/types.rs
pub enum FactType {
    Observation,    // 观察到的事实
    Preference,     // 用户偏好
    Relationship,   // 关系
    Procedure,      // 过程性知识
    Tool,           // 新增: 工具能力
}
```

### 2.3 模块划分

```
core/src/
├── dispatcher/
│   └── tool_index/                 # 新增目录
│       ├── mod.rs                  # 模块入口
│       ├── coordinator.rs          # ToolIndexCoordinator
│       ├── inference.rs            # SemanticPurposeInferrer
│       ├── retrieval.rs            # ToolRetrieval
│       └── config.rs               # ToolIndexConfig
│
├── memory/
│   └── database/facts/
│       └── types.rs                # 扩展 FactType::Tool
```

---

## 3. 核心组件设计

### 3.1 ToolIndexCoordinator

**职责**: 监听工具变更事件，同步 Memory 中的 ToolFact。

```rust
pub struct ToolIndexCoordinator {
    memory: Arc<MemoryService>,
    inferrer: SemanticPurposeInferrer,
}

impl ToolIndexCoordinator {
    /// 订阅工具变更事件
    pub async fn start(
        &self,
        mcp_events: broadcast::Receiver<McpManagerEvent>,
        skill_events: broadcast::Receiver<SkillRegistryEvent>,
    );

    /// 处理工具添加
    async fn handle_tool_added(&self, tool: UnifiedTool) -> Result<()> {
        let content = self.inferrer.generate_content(&tool).await?;
        let fact = MemoryFact {
            id: format!("tool:{}", tool.id),
            content,
            fact_type: FactType::Tool,
            specificity: Specificity::Principle,
            temporal_scope: TemporalScope::Permanent,
            metadata: self.build_metadata(&tool),
        };
        self.memory.upsert_fact(fact).await
    }

    /// 处理工具移除
    async fn handle_tool_removed(&self, tool_id: &str) -> Result<()> {
        self.memory.delete_fact(&format!("tool:{}", tool_id)).await
    }

    /// 初始化时批量同步
    pub async fn sync_all(&self, registry: &ToolRegistry) -> Result<()>;
}
```

**事件来源**:

```rust
pub enum ToolIndexEvent {
    ToolAdded { tool: UnifiedTool },
    ToolRemoved { tool_id: String },
    ToolUpdated { tool: UnifiedTool },
    BatchSync { tools: Vec<UnifiedTool> },
}
```

### 3.2 SemanticPurposeInferrer (分级推断引擎)

**职责**: 生成工具的语义描述，用于向量索引。

```rust
pub struct SemanticPurposeInferrer {
    llm_client: Option<Arc<dyn LlmClient>>,  // 用于 L2 异步补全
}

impl SemanticPurposeInferrer {
    /// 生成工具内容 (用于 MemoryFact.content)
    pub async fn generate_content(&self, tool: &UnifiedTool) -> Result<InferenceResult> {
        // Level 0: 显式结构化数据 (Zero Latency)
        if let Some(purpose) = self.try_level_0(tool) {
            return Ok(InferenceResult {
                content: self.format_content(tool, &purpose),
                level: OptimizationLevel::L0,
            });
        }

        // Level 1: 语义模板推断 (Minimal Latency)
        let purpose = self.level_1_template_inference(tool);
        let result = InferenceResult {
            content: self.format_content(tool, &purpose),
            level: OptimizationLevel::L1,
        };

        // Level 2: 异步 LLM 补全 (Eventual Consistency)
        if self.should_trigger_l2(tool) {
            self.schedule_l2_optimization(tool.id.clone());
        }

        Ok(result)
    }

    /// L0: 从 structured_meta 提取
    fn try_level_0(&self, tool: &UnifiedTool) -> Option<String> {
        let meta = tool.structured_meta.as_ref()?;
        if !meta.use_when.is_empty() {
            return Some(meta.use_when.join("; "));
        }
        if !meta.capabilities.is_empty() {
            return Some(meta.capabilities.iter()
                .map(|c| c.to_purpose_string())
                .collect::<Vec<_>>()
                .join("; "));
        }
        None
    }

    /// L1: 规则引擎推断
    fn level_1_template_inference(&self, tool: &UnifiedTool) -> String {
        let name_lower = tool.name.to_lowercase();

        // 基于动词前缀的模板
        if name_lower.starts_with("list") || name_lower.starts_with("get") || name_lower.starts_with("read") {
            return format!("retrieve information about {}", self.extract_topic(&tool.name));
        }
        if name_lower.starts_with("set") || name_lower.starts_with("update") || name_lower.starts_with("write") {
            return format!("modify or save {}", self.extract_topic(&tool.name));
        }
        if name_lower.starts_with("create") || name_lower.starts_with("add") {
            return format!("create new {}", self.extract_topic(&tool.name));
        }
        if name_lower.starts_with("delete") || name_lower.starts_with("remove") {
            return format!("remove or delete {}", self.extract_topic(&tool.name));
        }

        // 默认回退
        format!("perform {} operations", self.extract_topic(&tool.name))
    }

    /// 格式化最终内容
    fn format_content(&self, tool: &UnifiedTool, purpose: &str) -> String {
        format!(
            "[Tool] {}: {}. Use this tool when you need to {}.",
            tool.name,
            tool.description,
            purpose
        )
    }
}
```

**优化级别**:

| Level | 来源 | 延迟 | 质量 |
|-------|------|------|------|
| L0 | `structured_meta.use_when` / `capabilities` | 0ms | 最高 |
| L1 | 规则引擎模板推断 | <1ms | 中等 |
| L2 | 异步 LLM 补全 | 后台 | 高 |

### 3.3 ToolRetrieval

**职责**: 封装 Memory 调用，返回相关工具列表。

```rust
pub struct ToolRetrieval {
    memory: Arc<MemoryService>,
    registry: Arc<RwLock<ToolRegistry>>,
    config: ToolRetrievalConfig,
}

#[derive(Clone)]
pub struct ToolRetrievalConfig {
    pub hard_threshold: f32,    // 噪声过滤 (default: 0.4)
    pub soft_threshold: f32,    // 置信度分界 (default: 0.6)
    pub high_confidence: f32,   // 高置信度 (default: 0.7)
    pub top_k: usize,           // 最大检索数 (default: 5)
    pub core_tools: Vec<String>, // 强制核心工具
}

impl Default for ToolRetrievalConfig {
    fn default() -> Self {
        Self {
            hard_threshold: 0.4,
            soft_threshold: 0.6,
            high_confidence: 0.7,
            top_k: 5,
            core_tools: vec!["search".into(), "file_read".into(), "file_write".into()],
        }
    }
}

impl ToolRetrieval {
    /// 检索相关工具
    pub async fn retrieve(&self, intent: &str) -> Result<RetrievalResult> {
        // 1. 向量检索
        let facts = self.memory.search_facts(SearchQuery {
            query: intent,
            fact_type: Some(FactType::Tool),
            limit: self.config.top_k,
        }).await?;

        // 2. 动态双阈值过滤
        let mut high_confidence = Vec::new();
        let mut medium_confidence = Vec::new();
        let mut low_confidence = Vec::new();

        for (fact, score) in facts {
            let tool_id = fact.metadata.get("tool_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            if score > self.config.high_confidence {
                high_confidence.push(tool_id.to_string());
            } else if score > self.config.soft_threshold {
                medium_confidence.push(tool_id.to_string());
            } else if score > self.config.hard_threshold {
                low_confidence.push(tool_id.to_string());
            }
            // score <= hard_threshold: 丢弃
        }

        // 3. 获取完整 Schema
        let registry = self.registry.read().await;

        let hydrated_tools: Vec<HydratedTool> = high_confidence.iter()
            .chain(medium_confidence.iter())
            .filter_map(|id| registry.get(id))
            .map(|tool| HydratedTool::full_schema(tool))
            .collect();

        let summary_tools: Vec<HydratedTool> = low_confidence.iter()
            .take(1)  // 低置信度只取 Top-1
            .filter_map(|id| registry.get(id))
            .map(|tool| HydratedTool::summary_only(tool))
            .collect();

        Ok(RetrievalResult {
            hydrated: hydrated_tools,
            summaries: summary_tools,
            confidence: self.calculate_confidence(&high_confidence, &medium_confidence),
        })
    }
}

/// 水合后的工具表示
pub struct HydratedTool {
    pub tool: UnifiedTool,
    pub hydration_level: HydrationLevel,
}

pub enum HydrationLevel {
    FullSchema,   // 完整 JSON Schema
    SummaryOnly,  // 仅名称 + 描述
}
```

### 3.4 HydrationPipeline 集成

**集成点**: TaskAnalyzer 之后，PromptBuilder 之前。

```rust
// core/src/dispatcher/pipeline.rs

impl Dispatcher {
    pub async fn dispatch(&self, input: UserInput) -> Result<Response> {
        // 1. 意图识别
        let analysis = self.analyzer.analyze(&input).await?;

        // 2. Pre-flight Hydration (并行执行)
        let (tool_result, memory_result) = tokio::join!(
            self.tool_retrieval.retrieve(&input.text),
            self.memory_retrieval.retrieve(&input.text),
        );

        // 3. 组装上下文
        let context = match analysis {
            AnalysisResult::SingleStep { intent } => {
                self.prompt_builder
                    .with_intent(intent)
                    .with_tools(tool_result?)
                    .with_memory(memory_result?)
                    .build()
            },
            AnalysisResult::MultiStep { mut task_graph, .. } => {
                // 为 DAG 中每个节点水合工具
                self.hydrate_task_graph(&mut task_graph, &tool_result?).await?;
                self.prompt_builder
                    .with_task_graph(task_graph)
                    .build()
            }
        };

        // 4. LLM 推理
        self.thinker.think(context).await
    }
}
```

---

## 4. 检索策略

### 4.1 动态双阈值

```
相似度分布:
     │
 1.0 ┼─────────────────────────────────────
     │                              ████ 高置信度 (> 0.7)
 0.7 ┼─────────────────────────────█████   → 完整 Schema
     │                         █████████
 0.6 ┼────────────────────────██████████   中置信度 (0.6-0.7)
     │                    ███████████████   → 完整 Schema
 0.5 ┼───────────────────████████████████
     │               █████████████████████ 低置信度 (0.4-0.6)
 0.4 ┼──────────────██████████████████████  → 仅 Top-1 摘要
     │          ███████████████████████████
 0.0 ┼─────────████████████████████████████ 噪声 (< 0.4)
     │      ████████████████████████████████ → 丢弃
     └────────────────────────────────────────
```

### 4.2 强制核心工具

标记为 `is_core` 的工具享有特殊待遇：

1. **常驻索引**: 以 `ToolIndexEntry` 形式始终存在于 System Prompt
2. **按需水合**: 当向量检索命中且得分 > 0.5 时，升级为完整 Schema

```rust
// PromptBuilder 中的核心工具处理
fn build_tools_section(&self, result: &RetrievalResult) -> String {
    let mut sections = Vec::new();

    // 1. 核心工具索引 (常驻)
    sections.push("### Core Tools (always available)");
    for tool in &self.core_tools {
        sections.push(&format!("- {}: {}", tool.name, tool.summary));
    }

    // 2. 动态水合的工具 (完整 Schema)
    if !result.hydrated.is_empty() {
        sections.push("\n### Relevant Tools (use these for the current task)");
        for ht in &result.hydrated {
            sections.push(&ht.tool.to_prompt_line());
            if let Some(schema) = &ht.tool.parameters_schema {
                sections.push(&format!("```json\n{}\n```", schema));
            }
        }
    }

    // 3. 低置信度工具 (仅摘要 + 提示)
    if !result.summaries.is_empty() {
        sections.push("\n### Possibly Relevant (evaluate carefully)");
        for ht in &result.summaries {
            sections.push(&format!("- {}: {}", ht.tool.name, ht.tool.description));
        }
    }

    sections.join("\n")
}
```

---

## 5. 缓存一致性

### 5.1 事件驱动同步

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      ToolIndexCoordinator                               │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ 订阅事件
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           ▼
┌───────────────────┐   ┌───────────────────┐   ┌───────────────────┐
│  McpManager       │   │  SkillRegistry    │   │  NativeToolLoader │
│  Event::ServerUp  │   │  Event::SkillAdd  │   │  (future)         │
│  Event::ServerDown│   │  Event::SkillDel  │   │                   │
└───────────────────┘   └───────────────────┘   └───────────────────┘
```

### 5.2 启动时全量同步

```rust
impl ToolIndexCoordinator {
    pub async fn initialize(&self, registry: &ToolRegistry) -> Result<()> {
        // 1. 获取所有现有工具
        let tools = registry.list_all().await;

        // 2. 获取 Memory 中已有的 ToolFact IDs
        let existing_ids: HashSet<_> = self.memory
            .list_facts_by_type(FactType::Tool)
            .await?
            .iter()
            .map(|f| f.id.clone())
            .collect();

        // 3. 增量同步
        for tool in tools {
            let fact_id = format!("tool:{}", tool.id);
            if !existing_ids.contains(&fact_id) {
                self.handle_tool_added(tool).await?;
            }
        }

        // 4. 清理已删除的工具
        for id in existing_ids {
            let tool_id = id.strip_prefix("tool:").unwrap_or(&id);
            if registry.get(tool_id).is_none() {
                self.memory.delete_fact(&id).await?;
            }
        }

        Ok(())
    }
}
```

---

## 6. 配置

```toml
# ~/.aleph/config.toml

[tool_retrieval]
# 以下为高级配置，普通用户无需修改
hard_threshold = 0.4      # 噪声过滤阈值
soft_threshold = 0.6      # 置信度分界阈值
high_confidence = 0.7     # 高置信度阈值
top_k = 5                 # 最大检索数量
core_tools = ["search", "file_read", "file_write"]  # 强制核心工具
```

---

## 7. 性能预估

| 操作 | 延迟 | 备注 |
|------|------|------|
| 向量检索 (sqlite-vec) | 10-50ms | 取决于工具数量 |
| Schema 获取 (内存) | <1ms | HashMap 查找 |
| 总水合延迟 | ~20-60ms | 并行执行，隐藏在 LLM 推理前 |
| LLM 推理 (首 token) | 500-2000ms | 主要延迟来源 |

**工具检索延迟占比 < 5%**，对用户无感。

---

## 8. 实施计划

### Phase 1: 基础设施 (Week 1)

- [ ] 扩展 `FactType::Tool`
- [ ] 实现 `SemanticPurposeInferrer` (L0 + L1)
- [ ] 实现 `ToolIndexCoordinator` 基础同步

### Phase 2: 检索集成 (Week 2)

- [ ] 实现 `ToolRetrieval` 双阈值逻辑
- [ ] 集成 `HydrationPipeline` 到 Dispatcher
- [ ] 修改 `PromptBuilder` 支持动态工具注入

### Phase 3: 异步优化 (Week 3)

- [ ] 实现 L2 异步 LLM 补全
- [ ] 添加 `optimization_level` 可观测性
- [ ] 性能测试与阈值调优

### Phase 4: 生产就绪 (Week 4)

- [ ] MCP Server 事件监听集成
- [ ] Skill Registry 事件监听集成
- [ ] 配置热重载支持
- [ ] 文档更新

---

## 9. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 向量检索误召回 | LLM 误调用工具 | 动态双阈值 + 低置信度提示语 |
| Memory 与 Registry 不一致 | 工具失效 | 事件驱动同步 + 启动全量校验 |
| L2 LLM 补全失败 | 描述质量下降 | L1 兜底，异步重试 |
| 冷启动时无索引 | 工具不可发现 | 强制核心工具 + 启动时同步 |

---

## 10. 未来扩展

1. **Ad-hoc MCP 挂载**: 支持运行时动态挂载 MCP Server，自动触发索引更新
2. **Phantom Execution**: 在沙箱中预览工具执行结果，实现语义级审批
3. **Skill 自演进**: 基于 MCP 工具 Schema 自动生成高层 Skill 流程图

---

## 附录 A: 数据流示例

**用户输入**: "把代码存一下"

```
1. TaskAnalyzer
   → intent: "save code changes"

2. ToolRetrieval.retrieve("save code changes")
   → Vector search in Memory (fact_type=Tool)
   → Results:
     - git_commit (0.89) → high confidence
     - git_add (0.85) → high confidence
     - save_file (0.72) → medium confidence

3. HydrationPipeline
   → Fetch full schemas from Registry
   → Inject into PromptContext

4. Thinker receives:
   """
   ### Core Tools (always available)
   - search: Web search for information
   - file_read: Read file contents

   ### Relevant Tools (use these for the current task)
   - **git_commit** [MCP:github]: Commit staged changes
     ```json
     {"type": "object", "properties": {"message": {"type": "string"}}}
     ```
   - **git_add** [MCP:github]: Stage files for commit
     ```json
     {"type": "object", "properties": {"files": {"type": "array"}}}
     ```
   - **save_file** [Native]: Save content to file
     ```json
     {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}}
     ```
   """

5. LLM Decision
   → call_tool("git_commit", {"message": "save work"})
```
