# LLM 提示词系统重构设计

> 日期: 2026-01-21
> 状态: 已批准，待实施

## 问题背景

当前提示词系统存在以下问题：
- **打补丁式修补**：每次 AI 执行失败就添加新规则
- **僵化散乱**：规则分散在多个文件，相互矛盾
- **过于特定化**：缺乏泛化能力和通用性

### 核心症状

1. **任务理解偏差**：用户想让 AI 做事，AI 却只给建议/解释
2. **工具调用缺失**：明明有合适的工具，AI 却选择用文字描述
3. **判断标准混乱**：多种判断方式混合，没有统一标准

## 解决方案：统一决策层 + 精简提示词

### 核心思想

将"是否执行"的判断从提示词中剥离，建立单一的 `ExecutionIntentDecider` 决策点。提示词只负责"如何做"，不再负责"是否做"。

```
┌─────────────────────────────────────────────────────────────┐
│                    当前架构（问题）                          │
├─────────────────────────────────────────────────────────────┤
│  IntentClassifier ──→ 判断意图类型                          │
│        ↓                                                    │
│  Planner Prompt ──→ 再次判断是否执行 ← 冲突点1              │
│        ↓                                                    │
│  Agent Prompt ──→ 又一次强调必须执行 ← 冲突点2              │
│        ↓                                                    │
│  AI 收到混乱信号，保守选择"描述"                            │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    新架构                                    │
├─────────────────────────────────────────────────────────────┤
│  ExecutionIntentDecider (单一决策点)                        │
│        │                                                    │
│        ├─→ ExecutionMode::Execute(task_type)                │
│        │      → 注入"执行者"提示词（无需判断，直接做）       │
│        │                                                    │
│        └─→ ExecutionMode::Converse                          │
│               → 注入"对话者"提示词（纯对话，无工具）         │
│                                                             │
│  提示词职责：只描述"如何做"，不再判断"是否做"               │
└─────────────────────────────────────────────────────────────┘
```

## 详细设计

### 1. ExecutionMode 类型定义

```rust
pub enum ExecutionMode {
    /// 直接工具调用 - 斜杠命令触发
    DirectTool(ToolInvocation),

    /// 执行模式 - 提供工具，期望完成任务
    Execute(TaskCategory),

    /// 纯对话模式 - 不提供工具，不期望执行
    Converse,
}

pub enum TaskCategory {
    FileOperation,      // 文件读写、移动、搜索
    CodeExecution,      // 运行脚本、命令
    ContentGeneration,  // 生成图片、文档、音视频
    AppAutomation,      // 启动应用、AppleScript
    DataProcessing,     // 数据转换、分析
}

pub struct ToolInvocation {
    pub tool_id: String,
    pub params: HashMap<String, Value>,
}
```

### 2. 决策规则（优先级从高到低）

```
L0. 斜杠命令 (直接映射, <1ms)
    └─ "/screenshot" → DirectTool(screenshot)
    └─ "/ocr" → DirectTool(vision_ocr)
    └─ "/gen image ..." → DirectTool(image_generate)
    (跳过所有判断，零歧义)

L1. 显式指令词 (正则, <5ms)
    ├─ "打开/运行/执行/创建/生成/删除..." → Execute
    └─ "什么是/为什么/解释一下..." → Converse

L2. 上下文信号 (规则, <20ms)
    ├─ 用户选中了文件/文本 → Execute(相关操作)
    ├─ 在特定工具面板中 → Execute(该工具类型)
    └─ 纯文本输入框 → 继续判断

L3. 语义分析 (轻量LLM, <500ms)
    ├─ 只在 L0/L1/L2 无法判断时触发
    ├─ 使用小模型快速分类
    └─ 输出: Execute(category) | Converse | Ambiguous

L4. 默认策略
    └─ Ambiguous → Execute (偏向执行而非对话)
```

### 3. 提示词精简

#### 执行模式提示词

```markdown
# Role
You are a task executor. Your job is to complete the user's request using the provided tools.

# Tools Available
{tools}

# Response Format
1. Briefly acknowledge the task (one sentence)
2. Execute using tool calls
3. Report the result

# Example
User: "把下载文件夹里的图片移到图片文件夹"
Assistant: 好的，我来移动图片文件。
[tool_call: file_operation(action: "move", ...)]
完成，已移动 12 张图片。
```

#### 对话模式提示词

```markdown
# Role
You are a helpful assistant. Answer questions, explain concepts, and have conversations.

# Guidelines
- Be concise and direct
- Use examples when helpful
- Ask for clarification if needed
```

#### 精简效果

| 指标 | 当前 | 精简后 |
|------|------|--------|
| 系统提示词长度 | ~2000 tokens | ~300 tokens |
| "不要"类指令 | 12+ 条 | 0 条 |
| 职责 | 判断+执行 | 仅执行 |

### 4. 工具描述管理

```rust
/// 工具元信息 - 统一定义
pub struct ToolMeta {
    /// 工具ID - 代码调用用
    pub id: &'static str,

    /// 显示名称 - 给 AI 看
    pub name: &'static str,

    /// 一句话描述 - 用于工具选择
    pub brief: &'static str,

    /// 参数说明 - 用于参数填充
    pub params: &'static [ParamMeta],

    /// 所属类别 - 用于按需注入
    pub category: TaskCategory,
}
```

按 `TaskCategory` 注入相关工具，减少干扰：
- `FileOperation` → 只注入 file_read, file_write, file_move, file_search, file_list
- `ContentGeneration` → 只注入 image_generate, document_create, audio_generate

## 文件改动清单

### 新增文件

```
core/src/
├── intent/
│   └── execution_intent.rs       # 统一决策器
│
├── prompt/                        # 统一提示词管理模块
│   ├── mod.rs
│   ├── executor.rs               # 执行模式提示词
│   ├── conversational.rs         # 对话模式提示词
│   └── builder.rs                # 提示词组装器
│
└── tools/
    ├── registry.rs               # 工具元信息注册表
    └── categories.rs             # 工具分类定义
```

### 修改文件

```
core/src/
├── intent/
│   ├── mod.rs                    # 导出新模块
│   └── classifier.rs             # 简化，移除执行判断逻辑
│
├── planner/
│   └── prompt.rs                 # 大幅精简，移除判断逻辑
│
└── dispatcher/planner/
    └── prompt.rs                 # 精简，只保留任务分解逻辑
```

### 删除/废弃

```
core/src/intent/agent_prompt.rs   # 功能合并到 prompt/ 模块
```

## 实施阶段

### Phase 1: 基础设施 (不影响现有功能)

1. 新建 `prompt/` 模块，定义新提示词
2. 新建 `tools/registry.rs`，统一工具元信息
3. 新建 `intent/execution_intent.rs`，实现决策器

### Phase 2: 切换决策层

1. 修改入口点，使用 `ExecutionIntentDecider`
2. 根据决策结果选择不同提示词
3. 保留旧逻辑作为 fallback，可配置切换

### Phase 3: 清理旧代码

1. 删除 `agent_prompt.rs` 中的冗余指令
2. 精简 `planner/prompt.rs`
3. 移除散落各处的判断逻辑

### Phase 4: 验证与调优

1. 对比测试：旧提示词 vs 新提示词
2. 收集失败案例，调优决策规则
3. 迭代优化提示词措辞

## 回滚策略

```toml
# config.toml
[experimental]
use_unified_intent_decider = true  # 可随时切回 false
```

## 成功标准

1. **执行率提升**：用户请求执行任务时，AI 实际执行的比例 > 95%
2. **提示词精简**：系统提示词 token 数减少 80%+
3. **判断延迟**：90% 请求在 L0/L1 解决 (<5ms)
4. **可维护性**：新增场景只需修改决策规则，无需改提示词
