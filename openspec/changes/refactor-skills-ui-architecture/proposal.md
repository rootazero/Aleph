# Change: Refactor Skills UI Architecture

## Status

**Status**: Abandoned
**Created**: 2026-01-08
**Updated**: 2026-01-09
**Abandoned**: 2026-01-09
**Author**: AI Assistant

### Abandonment Reason

设计前提已变更。原提案基于"统一 MCP + Skills 为单一 Skill 概念"的假设，但经澄清：

- **MCP** 是 MCP（提供可执行的 tools/函数调用）
- **Skills** 是 Skills（Prompt Templates，提供指令注入）
- 二者都"使用" Tools，但概念上应保持分离
- Tools 还包括其他内置命令（search, video, chat）和用户自定义命令

**决定**：保持 `McpSettingsView` 和 `SkillsSettingsView` 独立，不进行统一。

### Completed Work (Before Abandonment)

- Phase 1: 统一数据模型（Rust）- 已完成但不再需要
- Phase 2: Swift 组件库 - 已完成后删除（与现有 MCP 组件重复）

## Why (Original)

当前 Aether 存在两个独立的设置界面来管理"后台能力"：

1. **McpSettingsView** - 管理 MCP Servers（内置 + 外部扩展）
2. **SkillsSettingsView** - 管理 Claude Agent Skills（用户安装的 prompt 模板）

这种分离导致以下问题：

1. **概念混淆**：用户不理解 "MCP Server" 和 "Skill" 的区别，两者本质都是"幽灵能力"
2. **UI 重复**：两个界面都实现了类似的列表、卡片、状态指示器、配置面板
3. **体验割裂**：用户需要在两个 Tab 间切换来管理类似功能
4. **扩展困难**：未来如需添加新的能力类型（如 Plugins），需再建一个 Tab

### 设计目标

针对 Aether 这样一个"幽灵般"的系统级 Agent，其设置界面不仅是参数配置的地方，更是 **Skills（能力）的控制塔**。设计目标是：

- **可视化不可见的服务**：让后台运行的进程状态清晰可见
- **简化复杂的参数注入**：将 CLI 参数、环境变量、权限等复杂配置简化为友好表单
- **Config as Code, UI as Convenience**：底层维护标准 JSON 配置，UI 只是友好渲染器

## What Changes

### 核心架构变更

**从**：两个独立的设置视图（MCP + Skills）
**变为**：统一的 Skills 设置视图，使用 Master-Detail + Inspector 架构

### UI Layout (完全重构)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                          Skills Settings View                                  │
├──────────────┬────────────────────────────────────────────────────────────────┤
│   FILTER     │                    Skill List (Grid/List)                       │
│  ───────────│────────────────────────────────────────────────────────────────│
│  ○ 全部      │  ┌─────────────────────────────────────────────────────────┐   │
│  ○ 已启用    │  │ [Icon] System Info       🟢 Running      [Toggle][...]  │   │
│  ○ 已停用    │  │        系统信息查询                                       │   │
│  ○ 错误      │  └─────────────────────────────────────────────────────────┘   │
│              │  ┌─────────────────────────────────────────────────────────┐   │
│   CATEGORY   │  │ [Icon] File System       🟢 Running      [Toggle][...]  │   │
│  ───────────│  │        文件系统访问                                       │   │
│  ○ 内置核心  │  └─────────────────────────────────────────────────────────┘   │
│  ○ 外部扩展  │  ┌─────────────────────────────────────────────────────────┐   │
│  ○ 提示模板  │  │ [Icon] Git Operations    🟢 Running      [Toggle][...]  │   │
│              │  │        Git 仓库操作                                       │   │
│  ───────────│  └─────────────────────────────────────────────────────────┘   │
│  [+ 添加]    │  ┌─────────────────────────────────────────────────────────┐   │
│  [{ } JSON]  │  │ [Icon] Linear            🟠 Connecting   [Toggle][...]  │   │
│              │  │        外部 MCP 扩展                                      │   │
├──────────────┴───────────────────────────────────────────────────────────────┤
│                         Inspector Panel (右侧滑出)                             │
│  ┌───────────────────────────────────────────────────────────────────────┐   │
│  │  [Icon] Git Operations                    🟢 Running    [Toggle]      │   │
│  │         内置核心服务 • 触发命令: /git                                    │   │
│  ├───────────────────────────────────────────────────────────────────────┤   │
│  │  Connection (仅外部扩展显示)                                            │   │
│  │  ─────────────────────────────────────────────────────────────────────│   │
│  │  Transport:  [ Stdio ▾ ]                                               │   │
│  │  Command:    [ ~/.cargo/bin/mcp-git    ] [Browse]                     │   │
│  │  Arguments:  ┌────────────────────────┐                                │   │
│  │              │ --path                  │ [×]                           │   │
│  │              │ /Users/zou/Project      │ [×]                           │   │
│  │              │ [+ Add Argument]        │                               │   │
│  │              └────────────────────────┘                                │   │
│  │  Working Dir: [ ~/Projects            ] [Browse]                       │   │
│  ├───────────────────────────────────────────────────────────────────────┤   │
│  │  Environment Variables                                                 │   │
│  │  ─────────────────────────────────────────────────────────────────────│   │
│  │  ┌─────────────┬──────────────────────┬───┬───┐                        │   │
│  │  │ GITHUB_TOKEN│ ••••••••••••••••••••• │ 👁 │ × │                       │   │
│  │  └─────────────┴──────────────────────┴───┴───┘                        │   │
│  │  [+ Add Variable]                                                      │   │
│  ├───────────────────────────────────────────────────────────────────────┤   │
│  │  Permissions                                                           │   │
│  │  ─────────────────────────────────────────────────────────────────────│   │
│  │  ☑ 工具调用前需确认                                                     │   │
│  │  Allowed Paths: [~/Projects] [~/Documents] [+ Add]                     │   │
│  ├───────────────────────────────────────────────────────────────────────┤   │
│  │  Tools (只读)                                                          │   │
│  │  ─────────────────────────────────────────────────────────────────────│   │
│  │  • git_status   • git_commit   • git_diff   • git_log                  │   │
│  ├───────────────────────────────────────────────────────────────────────┤   │
│  │  [View Logs]                                      [Cancel] [Save]      │   │
│  └───────────────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Skill 统一数据模型

将 `McpServerConfig` 和 `SkillInfo` 统一为 `UnifiedSkillConfig`：

```rust
// 统一的 Skill 类型
enum SkillType {
    BuiltinMcp,      // 内置 MCP 服务 (fs, git, shell, system-info)
    ExternalMcp,     // 外部 MCP 服务 (用户配置的命令行进程)
    PromptTemplate,  // 提示模板 (原 Skills，基于 SKILL.md 的 prompt)
}

// 统一的 Skill 配置
struct UnifiedSkillConfig {
    id: String,                      // 唯一标识
    name: String,                    // 显示名称
    description: String,             // 描述
    skill_type: SkillType,           // 类型
    enabled: bool,                   // 是否启用
    icon: String,                    // 图标 (SF Symbol)
    color: String,                   // 主题色

    // MCP 特有字段 (skill_type != PromptTemplate)
    transport: Option<McpTransport>, // Stdio | SSE
    command: Option<String>,         // 命令路径
    args: Vec<String>,               // 参数列表
    env: Vec<EnvVar>,                // 环境变量
    working_directory: Option<String>,

    // Permissions (所有类型共用)
    permissions: SkillPermissions,

    // Prompt Template 特有字段 (skill_type == PromptTemplate)
    skill_md_path: Option<String>,   // SKILL.md 路径
    allowed_tools: Vec<String>,      // 允许的工具列表
}
```

### Swift Layer 重构

- **DELETED**: `SkillsSettingsView.swift` (合并到统一视图)
- **REWRITTEN**: `McpSettingsView.swift` → `SkillsSettingsView.swift` (统一视图)
- **ADDED**: `SkillsSettingsComponents/` 组件目录
  - `SkillFilterSidebar.swift` - 左侧筛选栏
  - `SkillCard.swift` - Skill 卡片组件
  - `SkillInspectorPanel.swift` - 右侧配置面板
  - `SkillEnvVarEditor.swift` - 环境变量编辑器
  - `SkillArgsEditor.swift` - 参数列表编辑器
  - `SkillPermissionsEditor.swift` - 权限配置编辑器
  - `SkillLogsSheet.swift` - 日志查看器
  - `SkillJsonEditor.swift` - JSON 模式编辑器
  - `SkillAddSheet.swift` - 添加 Skill 表单

### Configuration 变更

- **MODIFIED**: `config.toml` - MCP 和 Skills 配置合并到统一格式
- **ADDED**: 配置迁移逻辑 (旧配置自动转换为新格式)

## Impact

### Affected Specs
- `settings-ui-layout` - Skills 设置遵循相同的设计语言
- (新建) `skills-settings-ui` - Skills 设置 UI 规范

### Affected Code
- `Aether/Sources/McpSettingsView.swift` → `SkillsSettingsView.swift` (重写)
- `Aether/Sources/SkillsSettingsView.swift` (删除，合并到上面)
- `Aether/Sources/SettingsView.swift` (更新 Tab 配置)
- `Aether/Sources/Components/Window/RootContentView.swift` (更新导航)
- `Aether/core/src/config/mod.rs` (统一配置模型)
- `Aether/core/src/aether.udl` (更新 UniFFI 接口)
- `Aether/Resources/*/Localizable.strings` (新增本地化字符串)

### Breaking Changes
- **配置迁移**：旧版 `[mcp]` 和 `[skills]` 配置将自动迁移到新的统一格式
- **API 变更**：`listMcpServers()` 和 `listInstalledSkills()` 合并为 `listSkills()`

## Design Decisions

### D1: 统一 Skill 概念 (采纳) ⭐

**决策**：将 MCP Server、Prompt Template 统一为 "Skill" 概念。

**理由**：
- 用户角度：都是"让 AI 获得某种能力"的扩展
- 技术角度：都是后台服务/配置，都有启用/禁用、权限控制
- 体验角度：一个入口管理所有扩展，降低认知负担

**替代方案**：
- 保持分离 - **否决**，增加用户困惑
- 改名但保持分离 - **否决**，未解决根本问题

### D2: Filter Sidebar + Grid Layout (采纳)

**决策**：采用左侧筛选栏 + 中间卡片网格的布局。

**理由**：
- 筛选栏：快速定位想要的 Skill 类型/状态
- 卡片网格：一览所有 Skill，状态清晰
- 参考：macOS System Settings、Xcode Capabilities

**替代方案**：
- 纯列表 - **否决**，信息密度不足
- Tab 分组 - **否决**，频繁切换

### D3: Inspector Panel (采纳)

**决策**：选中 Skill 后从右侧滑出 Inspector Panel 进行配置。

**理由**：
- 不打断浏览：用户可同时看到列表和详情
- 符合 macOS HIG：类似 Finder Inspector、Xcode Inspector
- 表单空间充足：400-500px 宽度足够复杂配置

**替代方案**：
- 弹出 Sheet - **否决**，遮挡列表
- 跳转到详情页 - **否决**，丢失上下文

### D4: 环境变量安全显示 (采纳)

**决策**：环境变量值默认使用 `SecureField` 掩码显示，可点击眼睛图标查看。

**理由**：
- API Key 等敏感信息不应明文显示
- 防止屏幕共享时泄露
- 符合安全最佳实践

### D5: JSON 模式逃生舱 (采纳)

**决策**：在筛选栏底部提供 `{ } JSON` 按钮，点击后显示底层配置文件编辑器。

**理由**：
- 高级用户可直接编辑配置
- 方便复制粘贴网上找到的配置
- 批量导入/导出

### D6: 内置预设简化 (采纳)

**决策**：为常用内置服务提供简化配置界面：

| Skill | 简化 UI |
|-------|---------|
| local-fs | 仅显示 `[+ Add Folder]` 按钮，自动生成 args |
| git-rust | 仅显示 `[Select Repo]` 按钮 |
| system-info | 仅显示 `[Enable]` 开关 |

**理由**：
- 降低普通用户配置门槛
- 隐藏不必要的复杂性
- 高级用户可通过 JSON 模式完全自定义

### D7: 组件化低耦合设计 (采纳) ⭐

**决策**：将 UI 拆分为独立组件，每个组件只负责单一职责。

**组件结构**：
```
SkillsSettingsView (容器)
├── SkillFilterSidebar (筛选栏)
├── SkillList (卡片列表)
│   └── SkillCard (单个卡片)
└── SkillInspectorPanel (配置面板)
    ├── SkillHeaderSection
    ├── SkillConnectionSection (外部 MCP)
    ├── SkillEnvVarEditor
    ├── SkillPermissionsEditor
    └── SkillToolsSection (只读)
```

**理由**：
- **低耦合**：组件间通过 Binding 通信，不直接依赖
- **高内聚**：每个组件完整负责一个功能区域
- **可复用**：EnvVarEditor 等可在其他设置页面复用
- **可测试**：组件可独立预览和测试

## Implementation Phases

### Phase 1: 数据模型统一

**目标**：在 Rust Core 中统一 MCP 和 Skills 配置模型

- 定义 `UnifiedSkillConfig` 结构
- 实现 `SkillType` 枚举
- 添加配置迁移逻辑（旧配置 → 新配置）
- 更新 UniFFI 接口

### Phase 2: Swift 组件库

**目标**：创建可复用的组件库

- `SkillCard` - 卡片组件
- `SkillFilterSidebar` - 筛选栏
- `SkillEnvVarEditor` - 环境变量编辑器
- `SkillArgsEditor` - 参数编辑器
- `SkillPermissionsEditor` - 权限编辑器

### Phase 3: 主视图集成

**目标**：组装统一的 Skills 设置视图

- `SkillsSettingsView` - 主容器
- `SkillInspectorPanel` - Inspector 面板
- 删除旧的 `McpSettingsView` 和 `SkillsSettingsView`
- 更新 `RootContentView` 导航

### Phase 4: 高级功能

**目标**：添加高级用户功能

- JSON 模式编辑器
- 配置导入/导出
- 日志查看器优化
- 自动发现（扫描 npm/pip 包）

### Phase 5: 本地化与测试

**目标**：完善用户体验

- 新增本地化字符串（中英文）
- UI 自动化测试
- 文档更新

## References

- [MCP Official Specification](https://modelcontextprotocol.io/)
- [macOS Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines/macos)
- 现有提案: `redesign-mcp-settings-ui` (已完成)
- 现有提案: `implement-mcp-capability` (进行中)
- 现有提案: `add-skills-capability` (进行中)
- 现有 spec: `settings-ui-layout`
