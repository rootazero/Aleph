# Skills 接口预留文档（方案 C）

## 一、概述

### 1.1 什么是 Claude Code Skills

**Claude Code Skills** 是 Claude Code CLI 提供的模块化能力系统，每个 Skill 包含：

- **知识库** (Knowledge Base): 专业领域的文档、指南、最佳实践
- **工具集成** (Tool Integration): 可执行的 MCP Tools（文件操作、编译、测试等）
- **工作流定义** (Workflow): 多步骤自动化流程
- **Prompt 模板** (Prompt Templates): 结构化的提示词模板

**典型 Skills 示例**:

| Skill ID | 功能描述 | 工具 | 工作流步骤 |
|---------|---------|------|-----------|
| `build-macos-apps` | 构建原生 macOS 应用 | `read_files`, `swift_compile`, `xcodebuild_test` | 需求分析 → 代码生成 → 语法验证 → 测试 |
| `pdf` | PDF 文档操作 | `pdf_extract`, `pdf_merge`, `pdf_form_fill` | 读取 → 处理 → 生成 |
| `mcp-builder` | 构建 MCP 服务器 | `read_files`, `write_files`, `npm_test` | 分析需求 → 生成代码 → 测试 |
| `xlsx` | Excel 表格操作 | `xlsx_read`, `xlsx_write`, `formula_calc` | 读取 → 数据处理 → 格式化输出 |

### 1.2 Skills vs Custom Prompts

**区别对比**:

| 特性 | Custom Prompts (`Intent::Custom`) | Skills (`Intent::Skills`) |
|-----|----------------------------------|--------------------------|
| **复杂度** | 简单 Prompt 转换 | 多步骤复杂工作流 |
| **工具调用** | ❌ 无 | ✅ 支持 MCP Tools |
| **状态追踪** | ❌ 无 | ✅ WorkflowState 状态机 |
| **知识库** | ❌ 无 | ✅ 领域专业知识 |
| **配置示例** | `intent_type = "translation"` | `intent_type = "skills:build-macos-apps"` |
| **实施阶段** | ✅ MVP 实现 | 🔮 方案 C 预留 |

**示例对比**:

```toml
# Custom Prompt: 简单翻译
[[rules]]
regex = "^/en"
provider = "openai"
system_prompt = "Translate to English"
intent_type = "translation"

# Skills 工作流: 复杂 iOS 应用构建
[[rules]]
regex = "^/build-ios"
provider = "claude"
system_prompt = "你是 iOS 开发专家"
intent_type = "skills:build-macos-apps"
skill_id = "build-macos-apps"
workflow = '{"steps": [...]}'
tools = '["read_files", "swift_compile", "xcodebuild_test"]'
```

### 1.3 为什么要在本次方案中预留接口

**设计原则**: **向前兼容，避免未来重构**

**本次实施（MVP）**:
- ✅ 数据结构重构（String → AgentPayload）
- ✅ Memory 功能集成
- ⚠️ Search / MCP 接口预留（空实现）

**方案 C（未来）**:
- 🔮 Skills 系统完整实现
- 🔮 WorkflowEngine 工作流引擎
- 🔮 SkillsRegistry Skill 注册表
- 🔮 MCP Tools 完整集成

**预留的好处**:
1. **避免破坏性修改**: 未来添加 Skills 无需修改现有 AgentPayload 结构
2. **配置文件兼容**: 用户可以提前在 config.toml 中定义 Skills 规则（虽然暂不执行）
3. **API 稳定性**: UniFFI 接口在方案 C 实施时无需变更
4. **渐进式演进**: 可以逐步实现 Skills 功能，而不是一次性大重构

---

## 二、预留的数据结构

### 2.1 Intent::Skills 枚举变体

**文件**: `Aether/core/src/payload/intent.rs`

**定义**:

```rust
pub enum Intent {
    BuiltinSearch,
    BuiltinMcp,

    /// 🔮 Skills 工作流（方案 C 预留）
    ///
    /// Claude Code Skills 复杂工作流（包含多步骤 + MCP Tools + 知识库）
    ///
    /// **本次实施**: 仅定义枚举，未实现执行逻辑
    /// **方案 C**: 实现 WorkflowEngine 和 SkillsRegistry
    ///
    /// 参数: skill_id (如 "build-macos-apps", "pdf", "mcp-builder")
    Skills(String),

    Custom(String),
    GeneralChat,
}
```

**辅助方法**:

```rust
impl Intent {
    /// 🔮 是否为 Skills 工作流（方案 C 预留）
    pub fn is_skills(&self) -> bool {
        matches!(self, Intent::Skills(_))
    }

    /// 🔮 获取 Skill ID（方案 C 预留）
    pub fn skills_id(&self) -> Option<&str> {
        match self {
            Intent::Skills(id) => Some(id.as_str()),
            _ => None,
        }
    }
}
```

**解析逻辑**:

```rust
impl Intent {
    pub fn from_rule(rule: &RoutingRuleConfig) -> Self {
        if let Some(intent_type) = &rule.intent_type {
            match intent_type.as_str() {
                "search" | "web_search" => Intent::BuiltinSearch,
                "mcp" | "tool_call" => Intent::BuiltinMcp,

                // 🔮 Skills 格式: "skills:xxx"
                s if s.starts_with("skills:") => {
                    let skill_id = s.strip_prefix("skills:").unwrap_or("");
                    Intent::Skills(skill_id.to_string())
                }

                custom => Intent::Custom(custom.to_string()),
            }
        } else {
            Intent::GeneralChat
        }
    }
}
```

### 2.2 WorkflowState（工作流状态）

**文件**: `Aether/core/src/payload/workflow.rs`（新建）

**定义**:

```rust
/// 🔮 Skills 工作流执行状态（方案 C 预留）
///
/// 用于跟踪 Skills 多步骤工作流的执行状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    /// 当前工作流 ID (对应 Intent::Skills 的 skill_id)
    pub workflow_id: String,

    /// 当前执行到的步骤索引
    pub current_step: usize,

    /// 总步骤数
    pub total_steps: usize,

    /// 每个步骤的执行结果（JSON 格式）
    pub step_results: Vec<serde_json::Value>,

    /// 工作流执行状态
    pub status: WorkflowStatus,

    /// 错误信息（如果失败）
    pub error: Option<String>,
}

/// 🔮 工作流执行状态（方案 C 预留）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowStatus {
    Pending,
    Running,
    WaitingForConfirmation,
    Completed,
    Failed,
}
```

**集成到 AgentContext**:

```rust
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,

    /// 🔮 Skills 工作流状态（方案 C 预留）
    pub workflow_state: Option<WorkflowState>,
}
```

### 2.3 RoutingRuleConfig 扩展字段

**文件**: `Aether/core/src/config/mod.rs` + `aether.udl`

**UniFFI 定义** (`aether.udl`):

```rust
dictionary RoutingRuleConfig {
    // ===== 现有字段 =====
    string regex;
    string provider;
    string? system_prompt;
    boolean? strip_prefix;
    sequence<string>? capabilities;
    string? intent_type;
    string? context_format;

    // ===== 🔮 Skills 专用字段（方案 C 预留）=====

    /// Skills ID（如 "build-macos-apps", "pdf"）
    string? skill_id;

    /// Skills 版本号（语义化版本，如 "1.0.0"）
    string? skill_version;

    /// Skills 工作流定义（JSON 字符串）
    string? workflow;

    /// Skills 可用工具列表（JSON 字符串数组）
    string? tools;

    /// Skills 知识库路径或 URL
    string? knowledge_base;
}
```

**Rust 实现**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    pub regex: String,
    pub provider: String,
    pub system_prompt: Option<String>,
    pub strip_prefix: Option<bool>,
    pub capabilities: Option<Vec<String>>,
    pub intent_type: Option<String>,
    pub context_format: Option<String>,

    // 🔮 Skills 专用字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_base: Option<String>,
}
```

**辅助方法**:

```rust
impl RoutingRuleConfig {
    /// 🔮 是否为 Skills 类型的路由规则
    pub fn is_skills_rule(&self) -> bool {
        self.intent_type
            .as_ref()
            .map(|s| s.starts_with("skills:"))
            .unwrap_or(false)
    }

    /// 🔮 获取 Skills 工作流定义（解析 JSON）
    pub fn get_workflow_definition(&self) -> Option<serde_json::Value> {
        self.workflow
            .as_ref()
            .and_then(|json_str| serde_json::from_str(json_str).ok())
    }

    /// 🔮 获取 Skills 工具列表（解析 JSON）
    pub fn get_tools_list(&self) -> Vec<String> {
        self.tools
            .as_ref()
            .and_then(|json_str| serde_json::from_str::<Vec<String>>(json_str).ok())
            .unwrap_or_default()
    }
}
```

---

## 三、预留的执行方法

### 3.1 CapabilityExecutor::execute_skills_workflow()

**文件**: `Aether/core/src/capability/mod.rs`

**方法签名**:

```rust
impl CapabilityExecutor {
    // 🔮 Skills 工作流执行（方案 C 预留）
    #[allow(dead_code)]
    async fn execute_skills_workflow(&self, payload: AgentPayload) -> Result<AgentPayload> {
        // TODO: 方案 C 实现
        Ok(payload)
    }
}
```

**方案 C 实现伪代码**:

```rust
async fn execute_skills_workflow(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
    // 1. 获取 skill_id
    let skill_id = payload.meta.intent.skills_id()
        .ok_or_else(|| AetherError::InvalidIntent)?;

    // 2. 从 SkillsRegistry 加载 Skill 定义
    let skill = self.skills_registry
        .as_ref()
        .ok_or_else(|| AetherError::SkillsNotAvailable)?
        .load(skill_id)
        .await?;

    // 3. 初始化 WorkflowEngine
    let mut workflow = WorkflowEngine::new(skill.workflow);

    // 4. 执行多步骤工作流
    while let Some(step) = workflow.next_step() {
        match step.step_type {
            StepType::ToolCall => {
                // 调用 MCP Tool
                let result = self.mcp_client
                    .as_ref()
                    .ok_or_else(|| AetherError::McpNotAvailable)?
                    .call_tool(&step.tool, &step.params)
                    .await?;

                workflow.record_result(result);
            }
            StepType::Generate => {
                // LLM 生成
                let prompt = workflow.build_prompt(
                    &skill.knowledge_base,
                    &payload.user_input
                );

                let result = self.llm_provider
                    .generate(prompt)
                    .await?;

                workflow.record_result(result);
            }
            StepType::WaitConfirmation => {
                // 等待用户确认
                workflow.pause_for_confirmation();
                break;
            }
        }
    }

    // 5. 更新 payload 的 workflow_state
    payload.context.workflow_state = Some(workflow.state());

    Ok(payload)
}
```

### 3.2 CapabilityExecutor 结构体扩展

**预留字段**:

```rust
pub struct CapabilityExecutor {
    memory_db: Option<Arc<VectorDatabase>>,

    // 🔮 预留字段（第二阶段）
    // search_client: Option<Arc<SearchClient>>,
    // mcp_client: Option<Arc<McpClient>>,

    // 🔮 Skills 相关字段（方案 C 预留）
    // skills_registry: Option<Arc<SkillsRegistry>>,
    // workflow_engine: Option<Arc<WorkflowEngine>>,
}
```

---

## 四、配置示例

### 4.1 完整的 Skills 配置

**文件**: `~/.config/aether/config.toml`

```toml
# 🔮 Skills 配置示例 1: 构建 macOS 应用
[[rules]]
regex = "^/build-ios"
provider = "claude"
system_prompt = "你是 iOS 开发专家，帮助用户构建原生 macOS 应用"
strip_prefix = true
intent_type = "skills:build-macos-apps"
skill_id = "build-macos-apps"
skill_version = "1.0.0"
workflow = '''
{
  "steps": [
    {
      "type": "analyze",
      "prompt": "分析用户需求，确定应用架构"
    },
    {
      "type": "tool_call",
      "tool": "read_files",
      "params": {"pattern": "**/*.swift"}
    },
    {
      "type": "generate",
      "prompt": "根据需求生成 Swift 代码"
    },
    {
      "type": "tool_call",
      "tool": "swift_compile",
      "params": {"target": "debug"}
    },
    {
      "type": "tool_call",
      "tool": "xcodebuild_test",
      "params": {}
    },
    {
      "type": "wait_confirmation",
      "message": "代码已生成并通过测试，是否继续部署？"
    }
  ]
}
'''
tools = '["read_files", "write_files", "swift_compile", "xcodebuild_test"]'
knowledge_base = "~/.aether/skills/build-macos-apps/knowledge"

# 🔮 Skills 配置示例 2: PDF 文档处理
[[rules]]
regex = "^/pdf"
provider = "claude"
system_prompt = "你是文档处理专家，帮助用户操作 PDF 文件"
strip_prefix = true
intent_type = "skills:pdf"
skill_id = "pdf"
skill_version = "1.2.0"
workflow = '''
{
  "steps": [
    {
      "type": "tool_call",
      "tool": "pdf_extract_text",
      "params": {"path": "$input_path"}
    },
    {
      "type": "generate",
      "prompt": "基于提取的文本内容，执行用户请求的操作"
    },
    {
      "type": "tool_call",
      "tool": "pdf_write",
      "params": {"output": "$output_path"}
    }
  ]
}
'''
tools = '["pdf_extract_text", "pdf_merge", "pdf_split", "pdf_form_fill", "pdf_write"]'
knowledge_base = "~/.aether/skills/pdf/knowledge"
```

### 4.2 workflow 字段详细说明

**工作流 JSON Schema**:

```json
{
  "steps": [
    {
      "type": "analyze|tool_call|generate|wait_confirmation",
      "prompt": "string (仅 analyze/generate 类型)",
      "tool": "string (仅 tool_call 类型)",
      "params": {
        "key": "value"
      },
      "message": "string (仅 wait_confirmation 类型)"
    }
  ]
}
```

**步骤类型说明**:

| 类型 | 说明 | 必需字段 | 示例 |
|-----|------|---------|------|
| `analyze` | LLM 分析 | `prompt` | `{"type": "analyze", "prompt": "分析需求"}` |
| `tool_call` | 调用 MCP Tool | `tool`, `params` | `{"type": "tool_call", "tool": "read_files", "params": {}}` |
| `generate` | LLM 生成内容 | `prompt` | `{"type": "generate", "prompt": "生成代码"}` |
| `wait_confirmation` | 等待用户确认 | `message` | `{"type": "wait_confirmation", "message": "是否继续?"}` |

---

## 五、方案 C 实施计划

### 5.1 需要新增的模块

**文件结构**:

```
Aether/core/src/
├── skills/                     # 🔮 Skills 模块（方案 C 新建）
│   ├── mod.rs                  # SkillsRegistry
│   ├── registry.rs             # Skill 加载和管理
│   ├── workflow_engine.rs      # WorkflowEngine 工作流引擎
│   ├── step_executor.rs        # 步骤执行器
│   └── knowledge_loader.rs     # 知识库加载器
└── payload/
    └── workflow.rs             # 🔮 WorkflowState（本次预留）
```

### 5.2 SkillsRegistry（Skill 注册表）

**职责**:
- 从文件系统加载 Skills 定义
- 缓存已加载的 Skills
- 验证 Skill 版本兼容性

**API 设计**:

```rust
pub struct SkillsRegistry {
    skills_dir: PathBuf,
    cache: HashMap<String, Skill>,
}

impl SkillsRegistry {
    pub async fn load(&mut self, skill_id: &str) -> Result<Skill>;
    pub fn get_cached(&self, skill_id: &str) -> Option<&Skill>;
    pub fn list_available_skills(&self) -> Vec<String>;
}

pub struct Skill {
    pub id: String,
    pub version: String,
    pub workflow: WorkflowDefinition,
    pub tools: Vec<String>,
    pub knowledge_base: PathBuf,
}
```

### 5.3 WorkflowEngine（工作流引擎）

**职责**:
- 解析 workflow JSON 定义
- 执行多步骤工作流
- 管理工作流状态
- 处理错误和重试

**API 设计**:

```rust
pub struct WorkflowEngine {
    definition: WorkflowDefinition,
    state: WorkflowState,
    mcp_client: Arc<McpClient>,
}

impl WorkflowEngine {
    pub fn new(definition: WorkflowDefinition) -> Self;
    pub fn next_step(&mut self) -> Option<WorkflowStep>;
    pub fn record_result(&mut self, result: serde_json::Value);
    pub fn pause_for_confirmation(&mut self);
    pub fn resume(&mut self);
    pub fn state(&self) -> WorkflowState;
}
```

### 5.4 MCP Client 集成

**职责**:
- 与 MCP 服务器通信
- 调用 MCP Tools
- 处理 Tool 调用结果

**API 设计**:

```rust
pub struct McpClient {
    server_url: String,
    http_client: reqwest::Client,
}

impl McpClient {
    pub async fn call_tool(
        &self,
        tool_name: &str,
        params: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value>;

    pub async fn list_available_tools(&self) -> Result<Vec<String>>;
}
```

### 5.5 实施步骤

**预估时间**: 7-10 天

| 步骤 | 任务 | 时间 |
|-----|------|------|
| 1 | 设计 SkillsRegistry 和 Skill 定义格式 | 1 天 |
| 2 | 实现 WorkflowEngine 核心逻辑 | 2 天 |
| 3 | 实现 MCP Client（JSON-RPC 2.0 协议） | 2 天 |
| 4 | 集成到 CapabilityExecutor | 1 天 |
| 5 | 实现知识库加载和 Prompt 注入 | 1 天 |
| 6 | UI 扩展（Skills 配置界面） | 1 天 |
| 7 | 测试和文档 | 1 天 |

---

## 六、UI 配置界面预留

### 6.1 Swift UI 扩展（方案 C）

**文件**: `Aether/Sources/Components/RoutingView.swift`

**新增字段**:

```swift
struct RoutingRuleConfig {
    // ===== 现有字段 =====
    var regex: String
    var provider: String
    var systemPrompt: String?
    var stripPrefix: Bool?
    var capabilities: [String]?
    var intentType: String?
    var contextFormat: String?

    // ===== 🔮 Skills 字段（方案 C 预留）=====
    var skillId: String?
    var skillVersion: String?
    var workflow: String?
    var tools: String?
    var knowledgeBase: String?
}
```

**UI 设计草图**:

```
┌─────────────────────────────────────────┐
│  路由规则编辑器                          │
├─────────────────────────────────────────┤
│ 基础配置                                │
│  Regex:     [^/build-ios          ]    │
│  Provider:  [Claude ▼             ]    │
│  System Prompt: [你是 iOS 开发专家  ]    │
├─────────────────────────────────────────┤
│ 意图类型                                │
│  ◉ General Chat                        │
│  ○ Custom Prompt                       │
│  ○ 🔮 Skills Workflow                  │ <-- 方案 C 添加
├─────────────────────────────────────────┤
│ 🔮 Skills 配置（仅在选择 Skills 时显示）  │
│  Skill ID:   [build-macos-apps    ]    │
│  Version:    [1.0.0               ]    │
│  Workflow:   [编辑工作流...        ] 🔘  │
│  Tools:      [read_files, ...     ]    │
│  Knowledge:  [~/.aether/skills/... ]    │
└─────────────────────────────────────────┘
```

---

## 七、测试策略

### 7.1 本次实施（MVP）

**测试目标**: 确保预留接口不影响现有功能

```rust
#[test]
fn test_intent_skills_variant_exists() {
    let intent = Intent::Skills("build-macos-apps".to_string());
    assert!(intent.is_skills());
    assert_eq!(intent.skills_id(), Some("build-macos-apps"));
}

#[test]
fn test_routing_rule_config_skills_fields_serialize() {
    let rule = RoutingRuleConfig {
        regex: "^/test".into(),
        provider: "openai".into(),
        skill_id: Some("test-skill".into()),
        skill_version: Some("1.0.0".into()),
        workflow: Some(r#"{"steps":[]}"#.into()),
        tools: Some(r#"["tool1"]"#.into()),
        knowledge_base: Some("~/.aether/skills/test".into()),
        ..Default::default()
    };

    let toml = toml::to_string(&rule).unwrap();
    let parsed: RoutingRuleConfig = toml::from_str(&toml).unwrap();

    assert_eq!(rule.skill_id, parsed.skill_id);
    assert_eq!(rule.skill_version, parsed.skill_version);
}

#[test]
fn test_workflow_state_default() {
    let state = WorkflowState::default();
    assert_eq!(state.current_step, 0);
    assert_eq!(state.status, WorkflowStatus::Pending);
}
```

### 7.2 方案 C 实施时

**完整测试用例**:

```rust
#[tokio::test]
async fn test_skills_workflow_execution() {
    let skill = create_test_skill("build-macos-apps");
    let registry = SkillsRegistry::new_with_skill(skill);
    let mcp_client = MockMcpClient::new();

    let executor = CapabilityExecutor {
        skills_registry: Some(Arc::new(registry)),
        mcp_client: Some(Arc::new(mcp_client)),
        ..Default::default()
    };

    let payload = create_test_payload_with_skills("build-macos-apps");
    let result = executor.execute_skills_workflow(payload).await.unwrap();

    assert!(result.context.workflow_state.is_some());
    let state = result.context.workflow_state.unwrap();
    assert_eq!(state.status, WorkflowStatus::Completed);
}
```

---

## 八、常见问题

### Q1: 本次实施后，Skills 配置会被忽略吗？

**A**: 是的。如果用户在 `config.toml` 中配置了 Skills 规则，当前版本会：

1. ✅ 成功解析配置（因为所有字段都是 `Option<T>`）
2. ✅ 识别出 `Intent::Skills` 枚举
3. ⚠️ 但不会执行工作流（`execute_skills_workflow` 为空实现）
4. ✅ 会记录日志: `warn!("Skills workflow not implemented yet")`

用户体验: 配置不会报错，但 Skills 功能暂不可用。

### Q2: 为什么不在本次方案中实现 Skills？

**A**: 工作量评估:

| 模块 | 工作量 |
|-----|--------|
| 本次 MVP（数据结构 + Memory） | 1-2 天 |
| Search 集成（Tavily/SearXNG） | 3-4 天 |
| MCP 集成（JSON-RPC 客户端） | 2-3 天 |
| **Skills 完整实现（方案 C）** | **7-10 天** |

**原因**:
- Skills 依赖 MCP 集成
- 需要设计 SkillsRegistry + WorkflowEngine
- 需要 UI 配置界面
- 工作量是 MVP 的 5 倍

**策略**: 先完成基础架构（本次），再逐步添加高级功能（方案 C）。

### Q3: 未来实施方案 C 时，需要修改现有代码吗？

**A**: **最小化修改**

| 模块 | 修改程度 |
|-----|---------|
| AgentPayload | ❌ 无需修改（已有 workflow_state 字段） |
| Intent 枚举 | ❌ 无需修改（已有 Skills 变体） |
| RoutingRuleConfig | ❌ 无需修改（已有 Skills 字段） |
| CapabilityExecutor | ✅ 填充 `execute_skills_workflow` 实现 |
| 新增模块 | ✅ `skills/` 目录 |

**破坏性变更**: 无

### Q4: Skills 和 MCP 是什么关系？

**A**: **Skills 是 MCP 的上层抽象**

```
┌─────────────────────────────────┐
│  Skills (高级工作流)             │
│  - 多步骤自动化                 │
│  - 知识库集成                   │
│  - Prompt 模板                  │
└─────────────┬───────────────────┘
              │ 使用
              ↓
┌─────────────────────────────────┐
│  MCP Tools (底层工具调用)        │
│  - read_files                   │
│  - swift_compile                │
│  - xcodebuild_test              │
└─────────────────────────────────┘
```

**示例**:
- MCP Tool: `read_files` - 读取文件内容
- Skill: `build-macos-apps` - 使用多个 Tools 完成完整的应用构建流程

---

## 九、总结

### 9.1 本次实施（MVP）预留的接口

| 类别 | 接口 | 状态 |
|-----|------|------|
| **枚举** | `Intent::Skills(String)` | 🔮 已定义 |
| **结构体** | `WorkflowState`, `WorkflowStatus` | 🔮 已定义 |
| **字段** | `AgentContext.workflow_state` | 🔮 已预留 |
| **配置** | `RoutingRuleConfig` 添加 5 个 Skills 字段 | 🔮 已扩展 |
| **方法** | `Intent::is_skills()`, `Intent::skills_id()` | 🔮 已实现 |
| **方法** | `CapabilityExecutor::execute_skills_workflow()` | 🔮 空实现 |
| **UniFFI** | RoutingRuleConfig Skills 字段定义 | 🔮 已添加 |

### 9.2 方案 C 实施时的工作

| 任务 | 预估时间 |
|-----|---------|
| SkillsRegistry 实现 | 1 天 |
| WorkflowEngine 实现 | 2 天 |
| MCP Client 实现 | 2 天 |
| 集成到 CapabilityExecutor | 1 天 |
| 知识库加载 | 1 天 |
| UI 扩展 | 1 天 |
| 测试和文档 | 1 天 |
| **总计** | **7-10 天** |

### 9.3 设计验证清单

- ✅ Intent::Skills 枚举变体存在
- ✅ WorkflowState 结构完整定义
- ✅ AgentContext 包含 workflow_state 字段
- ✅ RoutingRuleConfig 包含 5 个 Skills 字段
- ✅ 所有 Skills 字段都是 `Option<T>`（向后兼容）
- ✅ UniFFI 边界层已扩展
- ✅ 配置文件可以提前定义 Skills 规则
- ✅ execute_skills_workflow() 方法签名正确
- ✅ 辅助方法 is_skills() 和 skills_id() 已实现

**结论**: Skills 接口预留已完成，未来实施方案 C 时无需破坏性修改。
