# Agent执行模式设计

> 解决Aleph AI助手过度询问问题，使其具备真正的Agent能力

## 问题背景

当前Aether在处理可执行任务（如文件整理）时，会退化为"询问模式"——返回大量选项让用户选择，而不是直接展示计划并执行。这与AI Agent的期望行为相悖。

**期望行为**：
- AI立即展示任务计划
- 用户只需点击"执行"或"取消"
- 执行过程中显示实时进度
- 遇到冲突时弹出单个确认

## 设计目标

1. **Prompt/System层改进** - 改变AI的决策行为
2. **架构层改进** - 增强cowork的自动触发
3. **用户交互设计** - Cursor Agent模式体验

## 整体架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        用户输入                                      │
│                          ↓                                          │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │              IntentClassifier (Router层)                      │   │
│  │   规则匹配 → 关键词检测 → 轻量LLM分类                           │   │
│  │   输出: Executable | Conversational | Ambiguous               │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                          ↓                                          │
│         ┌────────────────┼────────────────┐                        │
│         ↓                ↓                ↓                        │
│   [Executable]    [Ambiguous]    [Conversational]                  │
│         ↓                ↓                ↓                        │
│   AgentMode        单问澄清         普通对话                        │
│         ↓                ↓                                         │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │              DefaultsResolver (智能默认层)                     │   │
│  │   1. 查询用户偏好 → 2. 匹配预设场景 → 3. 上下文推断             │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                          ↓                                          │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │              CoworkEngine (执行层)                             │   │
│  │   plan() → confirm() → execute() → report()                   │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

**核心改动点**：
1. 新增 `IntentClassifier` - 在Router层前置判断
2. 新增 `DefaultsResolver` - 三层智能默认策略
3. 修改 Prompt 注入 Agent行为规则
4. 增强 CoworkEngine 的确认流程

---

## 第一部分：IntentClassifier（Router层）

### 数据结构

```rust
// 新增文件: core/src/intent/classifier.rs

pub enum ExecutionIntent {
    /// 可直接执行的任务 - 走Agent模式
    Executable(ExecutableTask),
    /// 需要澄清的任务 - 最多问一个问题
    Ambiguous { task_hint: String, clarification: String },
    /// 纯对话 - 走普通聊天流程
    Conversational,
}

pub struct ExecutableTask {
    pub category: TaskCategory,      // 任务类别
    pub action: String,              // 动作描述
    pub target: Option<String>,      // 目标路径/对象
    pub confidence: f32,             // 分类置信度
}

pub enum TaskCategory {
    FileOrganize,      // 文件整理
    FileTransfer,      // 文件移动/复制
    FileCleanup,       // 文件清理/删除
    CodeExecution,     // 代码执行
    AppAutomation,     // 应用自动化
    DocumentGenerate,  // 文档生成
    DataProcess,       // 数据处理
}
```

### 三级分类流程

| 层级 | 方法 | 延迟 | 示例 |
|------|------|------|------|
| L1 | 正则匹配 | <5ms | `整理.*文件` → FileOrganize |
| L2 | 关键词+规则 | <20ms | 动词(整理/移动/删除) + 名词(文件/文件夹) |
| L3 | 轻量LLM | ~200ms | Haiku快速分类，仅当L1/L2无法判定时 |

### L1正则示例

```rust
lazy_static! {
    static ref EXECUTABLE_PATTERNS: Vec<(Regex, TaskCategory)> = vec![
        (r"整理|归类|分类.*文件", TaskCategory::FileOrganize),
        (r"移动|复制|拷贝.*到", TaskCategory::FileTransfer),
        (r"删除|清理|清空", TaskCategory::FileCleanup),
        (r"运行|执行.*脚本|代码", TaskCategory::CodeExecution),
    ];
}
```

### 分类器实现

```rust
impl IntentClassifier {
    pub async fn classify(&self, input: &str) -> ExecutionIntent {
        // L1: 正则匹配
        if let Some(task) = self.match_regex(input) {
            return ExecutionIntent::Executable(task);
        }

        // L2: 关键词+规则
        if let Some(task) = self.match_keywords(input) {
            return ExecutionIntent::Executable(task);
        }

        // L3: 轻量LLM分类
        self.classify_with_llm(input).await
    }
}
```

---

## 第二部分：Prompt层 Agent行为规则

当 `IntentClassifier` 判定为 `Executable` 时，系统会注入以下 Prompt 块：

### Agent执行模式Prompt

```markdown
## Agent执行模式

你已进入Agent执行模式。当前任务已被识别为可执行任务。

### 行为规则（必须遵守）

1. **禁止询问选项** - 不要列出A/B/C选项让用户选择
2. **立即制定计划** - 调用 cowork.plan() 分解任务
3. **展示计划摘要** - 用简洁格式展示将要执行的操作
4. **等待确认** - 写入/移动/删除操作必须等用户确认
5. **执行并报告** - 确认后执行，实时反馈进度

### 输出格式

📋 任务计划：{一句话描述}

将执行以下操作：
• {操作1}
• {操作2}
• ...

影响范围：{N}个文件，{M}个文件夹
[等待确认...]

### 确认边界（保守模式）

| 操作类型 | 是否需要确认 |
|---------|-------------|
| 扫描/分析/预览 | ❌ 自动执行 |
| 创建文件夹 | ✅ 需要确认 |
| 移动/复制文件 | ✅ 需要确认 |
| 重命名 | ✅ 需要确认 |
| 删除 | ✅ 需要确认 |
| 覆盖已有文件 | ✅ 单独确认 |
```

### Prompt注入位置

在 `payload/assembler.rs` 的 `format_system_prompt()` 中，根据 Intent 动态拼接：

```rust
impl PromptAssembler {
    pub fn format_system_prompt(&self, intent: &ExecutionIntent) -> String {
        let mut prompt = self.base_system_prompt.clone();

        if matches!(intent, ExecutionIntent::Executable(_)) {
            prompt.push_str(&self.agent_mode_prompt);
        }

        prompt
    }
}
```

---

## 第三部分：DefaultsResolver（智能默认分层策略）

### 核心结构

```rust
// 新增文件: core/src/intent/defaults.rs

pub struct DefaultsResolver {
    preferences: PreferenceStore,    // 用户偏好存储
    presets: PresetRegistry,         // 预设场景库
}

impl DefaultsResolver {
    /// 三层分级解析，返回执行参数
    pub async fn resolve(&self, task: &ExecutableTask, context: &TaskContext) -> TaskParameters {
        // 第一层：查询用户历史偏好
        if let Some(params) = self.preferences.get_for_task(&task.category, &context.path) {
            return params.with_source(ParameterSource::UserPreference);
        }

        // 第二层：匹配预设场景
        if let Some(preset) = self.presets.match_scenario(task, context) {
            return preset.parameters.with_source(ParameterSource::Preset);
        }

        // 第三层：上下文推断
        self.infer_from_context(task, context).await
    }
}
```

### 第一层：用户偏好存储

```toml
# ~/.aleph/preferences.toml（自动生成）
[file_organize]
default_method = "by_extension"    # 用户上次选择
conflict_resolution = "rename"     # 冲突时自动重命名

[file_organize."/Users/xxx/Downloads"]
default_method = "by_category"     # 特定目录的偏好
```

```rust
pub struct PreferenceStore {
    path: PathBuf,
    cache: RwLock<HashMap<String, TaskParameters>>,
}

impl PreferenceStore {
    /// 记录用户选择
    pub fn record_choice(&self, category: &TaskCategory, path: &str, params: &TaskParameters) {
        // 保存到 preferences.toml
    }

    /// 查询偏好
    pub fn get_for_task(&self, category: &TaskCategory, path: &str) -> Option<TaskParameters> {
        // 先查特定路径偏好，再查通用偏好
    }
}
```

### 第二层：预设场景库

| 场景关键词 | 默认行为 |
|-----------|---------|
| 整理文件 | 按扩展名分组到子文件夹 |
| 清理下载 | 按大类分组（Images/Documents/Videos/Archives/Others） |
| 备份项目 | 压缩为 `{项目名}_{日期}.zip` |
| 导出报告 | 生成Markdown文档到 ~/Documents |

```rust
pub struct PresetRegistry {
    presets: Vec<ScenarioPreset>,
}

pub struct ScenarioPreset {
    pub keywords: Vec<String>,
    pub category: TaskCategory,
    pub parameters: TaskParameters,
}

impl PresetRegistry {
    pub fn match_scenario(&self, task: &ExecutableTask, _ctx: &TaskContext) -> Option<&ScenarioPreset> {
        self.presets.iter().find(|p| {
            p.category == task.category &&
            p.keywords.iter().any(|k| task.action.contains(k))
        })
    }
}
```

### 第三层：上下文推断

```rust
async fn infer_from_context(&self, task: &ExecutableTask, ctx: &TaskContext) -> TaskParameters {
    // 扫描目标目录
    let scan = self.scan_directory(&ctx.path).await;

    // 分析文件构成
    let composition = scan.analyze_composition();

    // 推断最佳策略
    match composition.dominant_type {
        FileType::Images if composition.count > 50 => {
            // 大量图片 → 按日期分组更合理
            TaskParameters::file_organize_by_date()
        }
        _ => {
            // 默认按扩展名
            TaskParameters::file_organize_by_extension()
        }
    }
}

pub struct DirectoryScan {
    pub files: Vec<FileInfo>,
    pub total_size: u64,
}

pub struct FileComposition {
    pub count: usize,
    pub dominant_type: FileType,
    pub type_distribution: HashMap<FileType, usize>,
}
```

---

## 第四部分：用户交互流程（Cursor Agent模式）

### 完整交互时序

```
用户: "帮我把文件夹'/Downloads/未命名文件夹'里的文件按文件类型整理"
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Phase 1: 意图识别 (< 50ms)                                          │
│   IntentClassifier → Executable(FileOrganize)                       │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Phase 2: 扫描 + 智能默认 (200-500ms)                                 │
│   • 扫描目标目录：发现 23 个文件                                     │
│   • DefaultsResolver：无用户偏好 → 匹配预设"整理文件" → 按扩展名    │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Phase 3: 展示计划 (Halo UI)                                         │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  📋 整理文件到分类文件夹                                        │  │
│  │                                                               │  │
│  │  将执行以下操作：                                              │  │
│  │  • 创建 5 个文件夹 (PDF, Images, Videos, Documents, Others)   │  │
│  │  • 移动 23 个文件到对应文件夹                                  │  │
│  │                                                               │  │
│  │  📁 PDF (8)         📁 Images (6)       📁 Videos (3)         │  │
│  │  📁 Documents (4)   📁 Others (2)                             │  │
│  │                                                               │  │
│  │           [ 执行 ]                [ 取消 ]                     │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                          用户点击 [执行]
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Phase 4: 执行 + 实时进度                                            │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  ⏳ 正在整理文件...                                            │  │
│  │  ████████████░░░░░░░░  12/23                                  │  │
│  │                                                               │  │
│  │  ✓ 创建 PDF/                                                  │  │
│  │  ✓ 移动 report.pdf → PDF/                                     │  │
│  │  → 移动 photo.jpg → Images/                                   │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                          遇到同名文件冲突
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Phase 4.1: 冲突确认 (单个弹出)                                       │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  ⚠️ 文件已存在                                                 │  │
│  │                                                               │  │
│  │  "photo.jpg" 在 Images/ 中已存在                               │  │
│  │                                                               │  │
│  │  [ 覆盖 ]   [ 重命名为 photo_1.jpg ]   [ 跳过 ]                │  │
│  │                                                               │  │
│  │  □ 对后续冲突应用相同选择                                      │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Phase 5: 完成报告                                                   │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  ✅ 整理完成                                                   │  │
│  │                                                               │  │
│  │  • 创建了 5 个文件夹                                          │  │
│  │  • 移动了 23 个文件                                           │  │
│  │  • 跳过 1 个文件 (已存在)                                     │  │
│  │                                                               │  │
│  │  📂 点击打开文件夹                        [完成]               │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### Swift UI 状态机

```swift
enum AgentExecutionState {
    case planning           // 扫描分析中
    case awaitingConfirm    // 展示计划，等待确认
    case executing(progress: Float, currentTask: String)
    case conflict(ConflictInfo)  // 冲突确认
    case completed(summary: ExecutionSummary)
    case failed(error: String)
}

struct ConflictInfo {
    let fileName: String
    let targetPath: String
    let options: [ConflictOption]
}

enum ConflictOption {
    case overwrite
    case rename(suggested: String)
    case skip
}

struct ExecutionSummary {
    let foldersCreated: Int
    let filesMoved: Int
    let filesSkipped: Int
    let errors: [String]
}
```

### HaloState 扩展

```swift
extension HaloState {
    static func agentPlan(plan: AgentPlan, onExecute: @escaping () -> Void, onCancel: @escaping () -> Void) -> HaloState
    static func agentProgress(progress: Float, currentTask: String) -> HaloState
    static func agentConflict(info: ConflictInfo, onResolve: @escaping (ConflictOption) -> Void) -> HaloState
    static func agentComplete(summary: ExecutionSummary, onDismiss: @escaping () -> Void) -> HaloState
}
```

---

## 第五部分：文件结构

### 新增文件

```
core/src/
├── intent/
│   ├── mod.rs              # 模块导出
│   ├── classifier.rs       # IntentClassifier 实现
│   ├── defaults.rs         # DefaultsResolver 实现
│   ├── preferences.rs      # PreferenceStore 用户偏好
│   └── presets.rs          # PresetRegistry 预设场景
```

### 修改文件

| 文件 | 修改内容 |
|------|---------|
| `core/src/lib.rs` | 导出 intent 模块 |
| `core/src/router/mod.rs` | 集成 IntentClassifier |
| `core/src/payload/assembler.rs` | 动态注入 Agent Prompt |
| `core/src/aether.udl` | 新增 UniFFI 类型定义 |
| `Aether/Sources/HaloState.swift` | 新增 Agent 状态 |
| `Aether/Sources/Components/AgentPlanView.swift` | 新增计划展示组件 |
| `Aether/Sources/Components/AgentProgressView.swift` | 新增进度组件 |
| `Aether/Sources/Components/AgentConflictSheet.swift` | 新增冲突确认组件 |

---

## 第六部分：实现步骤

### Phase 1: Intent层 (预计工作量: 中)
1. 创建 `intent/` 模块结构
2. 实现 `IntentClassifier` 三级分类
3. 实现 `DefaultsResolver` 三层策略
4. 添加单元测试

### Phase 2: Prompt层 (预计工作量: 小)
1. 编写 Agent模式 Prompt
2. 修改 `PromptAssembler` 动态注入
3. 测试不同场景的 Prompt 输出

### Phase 3: UI层 (预计工作量: 中)
1. 扩展 `HaloState` Agent状态
2. 实现 `AgentPlanView` 计划展示
3. 实现 `AgentProgressView` 进度展示
4. 实现 `AgentConflictSheet` 冲突确认
5. 集成测试完整流程

### Phase 4: 集成测试 (预计工作量: 小)
1. 端到端测试：文件整理场景
2. 测试偏好记录和复用
3. 测试冲突处理各分支

---

## 设计决策记录

| 决策 | 选择 | 理由 |
|------|------|------|
| 用户交互模式 | Cursor Agent | 立即展示计划，用户确认后执行 |
| 确认边界 | 保守模式 | 初期版本，建立用户信任 |
| 智能默认策略 | 三层分级 | 偏好优先 → 预设默认 → 推断+单问 |
| 架构设计 | Router + Prompt双层 | 架构约束 + 行为引导的双保险 |

---

## 参考

- [COWORK.md](../COWORK.md) - Cowork任务编排系统文档
- [ARCHITECTURE.md](../ARCHITECTURE.md) - 整体架构文档
