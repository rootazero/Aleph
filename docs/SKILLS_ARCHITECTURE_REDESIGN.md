# Skills 架构重设计方案

> **Status**: ✅ **已实现** (2026-01-21)
>
> 本文档描述了 Skills 系统的重设计方案。该方案已完全实现。
> 具体实现细节请参考：
> - `core/src/rig_tools/skill_reader.rs` - ReadSkillTool, ListSkillsTool
> - `core/src/thinker/prompt_builder.rs` - PromptConfig.skill_mode
> - [SKILLS.md](./SKILLS.md) - 用户文档

基于 Claude 官方 Skills 规范，重新设计 Aether 的 Skills 功能。

## 1. 问题分析

### 1.1 当前实现的问题

**用户反馈**：Skill 指令被当作"参考"而非"必须执行"

**根因分析**：

| 方面 | Claude 官方架构 | Aether 当前实现 | 问题 |
|------|----------------|-----------------|------|
| 加载模式 | 渐进式 (Progressive Disclosure) | 一次性全量注入 | Token 浪费 |
| 加载时机 | Agent 主动读取 | 系统被动注入 | 心智模型错误 |
| Metadata | 系统提示只含 name + description | 完整 instructions | 无选择性 |
| Instructions | 工具调用返回 | Context Information | 被视为参考 |
| 心智模型 | "我需要读取并执行" | "这是背景信息" | **核心问题** |

### 1.2 心智模型差异图解

```
官方架构：Agent 主动获取
┌─────────────────────────────────────────────────────┐
│ System Prompt: "Available Skills: refine-text..."  │  ← 只有 metadata
└─────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────┐
│ User: "Refine this text..."                        │
└─────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────┐
│ Agent: "I'll use the refine-text skill"            │
│ → Decision: UseTool { read_skill, {id: "refine"} } │  ← Agent 主动决策
└─────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────┐
│ Tool Result: [SKILL.md full content]               │
│ → Agent 将其视为"要执行的任务指令"                  │  ← 任务指令语义
└─────────────────────────────────────────────────────┘


Aether 当前：系统被动注入
┌─────────────────────────────────────────────────────┐
│ System Prompt + Context Information:               │
│   ## Skill Instructions                            │  ← 作为上下文注入
│   [完整 SKILL.md 内容]                              │  ← 与 Memory 等混在一起
└─────────────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────────────┐
│ Agent: 把 Skill Instructions 当作"参考信息"         │
│ → 可以选择忽略，因为不是"任务指令"                    │  ← 问题！
└─────────────────────────────────────────────────────┘
```

## 2. 官方架构核心设计

### 2.1 三层渐进式加载

| Level | 加载时机 | Token 成本 | 内容 |
|-------|---------|-----------|------|
| **Level 1: Metadata** | 启动时 | ~100 tokens/skill | name + description (YAML frontmatter) |
| **Level 2: Instructions** | 触发时 | <5k tokens | SKILL.md body |
| **Level 3+: Resources** | 按需 | 无上限 | 额外文件、脚本 |

### 2.2 文件系统访问模式

```
skill-dir/
├── SKILL.md           # Level 2: 主指令文件
├── ADVANCED.md        # Level 3: 高级指南
├── REFERENCE.md       # Level 3: 参考文档
└── scripts/
    └── process.py     # Level 3: 可执行脚本
```

Agent 通过工具调用访问这些文件：
- `read_skill("refine-text")` → 读取 SKILL.md
- `read_skill("refine-text", "ADVANCED.md")` → 读取额外文件
- `run_skill_script("refine-text", "process.py", [...])` → 执行脚本

### 2.3 关键机制

1. **系统提示只包含 metadata** - Agent 知道有哪些 skills 可用
2. **Agent 主动读取** - 通过工具调用获取完整指令
3. **工具返回内容 = 任务指令** - Agent 将其视为必须执行的内容

## 3. Aether 实现方案

### 3.1 架构概览

```
┌─────────────────────────────────────────────────────────────────┐
│                        System Prompt                            │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ ## Available Skills                                        │  │
│  │ - **refine-text**: Improve and polish writing              │  │
│  │ - **translate**: Translate text between languages          │  │
│  │ - **summarize**: Create concise summaries                  │  │
│  │                                                            │  │
│  │ To use a skill:                                            │  │
│  │ 1. Call read_skill(skill_id) to load its instructions     │  │
│  │ 2. Follow the instructions exactly                        │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                        Tool Registry                            │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │   read_skill    │  │ run_skill_script│  │   list_skills   │  │
│  │                 │  │                 │  │                 │  │
│  │ Read SKILL.md   │  │ Execute scripts │  │ List available  │  │
│  │ or resources    │  │ in skill dir    │  │ skills          │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                     Skills Directory                            │
│  ~/.config/aether/skills/                                       │
│  ├── refine-text/                                               │
│  │   ├── SKILL.md                                               │
│  │   └── examples/                                              │
│  ├── translate/                                                 │
│  │   └── SKILL.md                                               │
│  └── summarize/                                                 │
│      └── SKILL.md                                               │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 核心改动

#### 改动 1: 新增 `read_skill` 工具

**文件**: `core/src/rig_tools/skill_reader.rs` (新建)

```rust
pub struct ReadSkillTool {
    skills_dir: PathBuf,
    max_file_size: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReadSkillArgs {
    /// The skill identifier (directory name)
    pub skill_id: String,
    /// Optional: specific file within skill directory (default: SKILL.md)
    #[serde(default)]
    pub file_name: Option<String>,
}

#[derive(Serialize)]
pub struct ReadSkillOutput {
    pub success: bool,
    pub skill_id: String,
    pub file_name: String,
    pub content: String,
    /// Available files in this skill directory
    pub available_files: Vec<String>,
}

impl Tool for ReadSkillTool {
    const NAME: &'static str = "read_skill";
    // ...
}
```

**工具定义**（暴露给 LLM）:

```json
{
  "name": "read_skill",
  "description": "Read the instructions of an installed skill. Use this to load skill-specific guidance before executing tasks that match a skill's purpose.",
  "parameters": {
    "type": "object",
    "properties": {
      "skill_id": {
        "type": "string",
        "description": "The skill identifier (e.g., 'refine-text', 'translate')"
      },
      "file_name": {
        "type": "string",
        "description": "Optional: specific file to read (default: SKILL.md)"
      }
    },
    "required": ["skill_id"]
  }
}
```

#### 改动 2: 新增 `list_skills` 工具

**文件**: `core/src/rig_tools/skill_reader.rs`

```rust
pub struct ListSkillsTool {
    skills_registry: Arc<SkillsRegistry>,
}

#[derive(Serialize)]
pub struct ListSkillsOutput {
    pub skills: Vec<SkillSummary>,
}

#[derive(Serialize)]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
}
```

#### 改动 3: 可选 `run_skill_script` 工具

**文件**: `core/src/rig_tools/skill_runner.rs` (新建)

```rust
pub struct RunSkillScriptTool {
    skills_dir: PathBuf,
    allowed_interpreters: Vec<String>,  // ["python3", "bash", "node"]
    timeout_seconds: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct RunSkillScriptArgs {
    pub skill_id: String,
    pub script_name: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub stdin: Option<String>,
}

#[derive(Serialize)]
pub struct RunSkillScriptOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}
```

#### 改动 4: 修改 SkillsStrategy

**文件**: `core/src/capability/strategies/skills.rs`

```rust
// 旧实现：预加载 instructions
async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
    if let Some(skill) = registry.get_skill(skill_id) {
        payload.context.skill_instructions = Some(skill.instructions.clone());  // ← 移除
    }
    Ok(payload)
}

// 新实现：只提供 metadata
async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
    // 不再注入 instructions
    // 只确保 read_skill 工具可用
    // 系统提示中已包含 skill metadata
    Ok(payload)
}
```

#### 改动 5: 修改系统提示生成

**文件**: `core/src/payload/assembler.rs`

```rust
// 旧实现：完整 instructions 进入 Context
fn format_context(&self, context: &Context) -> Option<String> {
    if let Some(instructions) = &context.skill_instructions {
        sections.push(format!("## Skill Instructions\n\n{}", instructions));  // ← 移除
    }
}

// 新实现：只在系统提示中包含 metadata
fn build_skills_metadata(&self, registry: &SkillsRegistry) -> String {
    let skills = registry.list_all();
    if skills.is_empty() {
        return String::new();
    }

    let mut lines = vec!["## Available Skills\n".to_string()];
    for skill in skills {
        lines.push(format!(
            "- **{}**: {}\n",
            skill.id, skill.description
        ));
    }
    lines.push("\nTo use a skill, call `read_skill(skill_id)` to load its instructions, then follow them exactly.\n".to_string());

    lines.join("")
}
```

#### 改动 6: 修改 Payload 结构

**文件**: `core/src/payload/mod.rs`

```rust
pub struct Context {
    // 移除
    // pub skill_instructions: Option<String>,

    // 新增：skill metadata 用于系统提示
    pub available_skills: Vec<SkillMetadata>,
}

pub struct SkillMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
}
```

### 3.3 工具调用流程

```
1. Agent Loop 启动
   ├─ 加载 SkillsRegistry
   ├─ 提取所有 skill 的 metadata
   └─ 注入到系统提示的 "Available Skills" 部分

2. 用户请求: "帮我润色这段文字"
   ↓
3. Thinker 分析
   ├─ 看到 "Available Skills" 中有 refine-text
   ├─ 决策: UseTool { read_skill, { skill_id: "refine-text" } }
   └─ 发起工具调用

4. ReadSkillTool 执行
   ├─ 读取 ~/.config/aether/skills/refine-text/SKILL.md
   ├─ 返回完整内容
   └─ ActionResult::ToolSuccess { output: { content: "..." } }

5. Agent 收到工具结果
   ├─ 将返回的 instructions 视为"任务指令"
   ├─ 严格按照指令执行
   └─ 完成任务
```

### 3.4 向后兼容

保留现有的 `/skill` 命令语法：

```rust
// 当用户使用 /refine-text 时
// Intent::Skills("refine-text") 触发
// 但不再预加载 instructions
// 而是在提示中提示 Agent: "User has requested the refine-text skill. Use read_skill to load it."
```

## 4. 实现步骤

### Phase 1: 核心工具 (Day 1-2) ✅ 已完成

1. 创建 `core/src/rig_tools/skill_reader.rs`
   - [x] `ReadSkillTool` 结构体和参数定义
   - [x] 实现 `Tool` trait
   - [x] 路径安全检查
   - [x] 文件大小限制

2. 创建 `core/src/rig_tools/skill_lister.rs` → 合并到 `skill_reader.rs`
   - [x] `ListSkillsTool` 结构体
   - [x] 返回所有 skill 的 metadata

### Phase 2: 注册和集成 (Day 2-3) ✅ 已完成

3. 修改 `core/src/dispatcher/registry.rs`
   - [x] 注册 `read_skill` 工具
   - [x] 注册 `list_skills` 工具
   - [x] 确保工具出现在 LLM 可见的工具列表中

4. 修改 `core/src/rig_tools/mod.rs`
   - [x] 导出新工具模块
   - [x] 添加到内置工具列表

### Phase 3: 提示词改造 (Day 3-4) ✅ 已完成

5. 修改 `core/src/thinker/prompt_builder.rs`
   - [x] 支持 `skill_mode` 严格工作流执行
   - [x] 支持 `tool_index` 智能工具发现
   - [x] 在系统提示中包含 skill metadata

6. 修改 `core/src/ffi/tool_discovery.rs`
   - [x] 智能工具过滤
   - [x] `infer_required_tools()` 关键词分析
   - [x] `filter_tools_by_categories()` 工具筛选

### Phase 4: Payload 清理 (Day 4) ✅ 已完成

7. 系统提示结构更新
   - [x] 通过 `PromptConfig` 配置
   - [x] 运行时能力注入
   - [x] 生成模型注入

8. 更新相关测试
   - [x] 更新 `skill_reader.rs` 测试
   - [x] 更新 `prompt_builder.rs` 测试
   - [x] 添加新工具的单元测试

### Phase 5: 可选增强 (Day 5+) 🔮 部分完成

9. 可选：创建 `run_skill_script` 工具
   - [ ] 安全的脚本执行
   - [ ] 解释器白名单
   - [ ] 超时控制

10. 资源文件支持 ✅ 已完成
    - [x] 读取 skill 目录下的任意文件 (via `file_name` 参数)
    - [x] 目录列表功能 (`available_files` 返回)

## 5. 代码改动清单

| 文件 | 改动类型 | 描述 |
|------|---------|------|
| `core/src/rig_tools/skill_reader.rs` | 新建 | ReadSkillTool, ListSkillsTool |
| `core/src/rig_tools/skill_runner.rs` | 新建 | RunSkillScriptTool (可选) |
| `core/src/rig_tools/mod.rs` | 修改 | 导出新模块 |
| `core/src/dispatcher/registry.rs` | 修改 | 注册新工具 |
| `core/src/payload/assembler.rs` | 修改 | 移除 instructions 注入，添加 metadata |
| `core/src/payload/mod.rs` | 修改 | 更新 Context 结构 |
| `core/src/capability/strategies/skills.rs` | 修改 | 移除预加载逻辑 |
| `core/src/intent/support/agent_prompt.rs` | 修改 | 更新 /skill 命令处理 |

## 6. 测试验证

### 6.1 单元测试

```rust
#[tokio::test]
async fn test_read_skill_returns_instructions() {
    // 创建测试 skill
    // 调用 read_skill
    // 验证返回完整 SKILL.md 内容
}

#[tokio::test]
async fn test_agent_uses_skill_after_reading() {
    // 模拟完整流程
    // 1. Agent 看到 "Available Skills"
    // 2. Agent 调用 read_skill
    // 3. Agent 按指令执行
}
```

### 6.2 集成测试

```rust
#[tokio::test]
async fn test_skill_execution_flow() {
    // 启动 Agent Loop
    // 发送 "请帮我润色这段文字"
    // 验证 Agent 调用了 read_skill
    // 验证 Agent 遵循了 skill 指令
}
```

## 7. 预期效果

### 7.1 修复前 vs 修复后

| 场景 | 修复前 | 修复后 |
|------|--------|--------|
| Skill 指令执行 | 可能被忽略 | 作为任务指令执行 |
| Token 消耗 | 全量注入 | 按需加载 |
| 多 Skill 支持 | 一次只能用一个 | 可组合使用 |
| 资源文件 | 不支持 | 支持 Level 3 |

### 7.2 符合官方规范

- ✅ 渐进式加载 (Progressive Disclosure)
- ✅ 工具调用模式 (Agent 主动读取)
- ✅ Metadata 与 Instructions 分离
- ✅ 资源文件支持
- ✅ 脚本执行支持 (可选)

## 8. 风险和缓解

| 风险 | 缓解措施 |
|------|---------|
| Agent 不调用 read_skill | 在系统提示中明确指导 |
| 路径遍历攻击 | 严格的路径验证 |
| 脚本执行安全 | 解释器白名单 + 沙箱 |
| 向后兼容 | 保留 /skill 命令语法 |

## 9. 后续优化

1. **Skill 缓存** - 避免重复读取同一 skill
2. **Skill 版本** - 支持 skill 版本管理
3. **Skill 依赖** - 支持 skill 之间的依赖关系
4. **Skill 市场** - 支持从远程仓库安装 skill
