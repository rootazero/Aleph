# Change: Redesign MCP Settings UI

## Status

**Status**: Applied
**Created**: 2026-01-08
**Updated**: 2026-01-08
**Applied**: 2026-01-08
**Author**: AI Assistant

## Why

当前的 `McpSettingsView` 是一个简单的单页面设计，仅支持内置服务的开关配置。随着 MCP 功能的完善（支持外部服务器、环境变量、命令行参数等），需要重构为更专业的 **Master-Detail（主从视图）** 布局，以：

1. **清晰区分服务类型**：内置核心（Built-in Core）vs 已安装扩展（Installed Extensions）
2. **支持外部 MCP 服务器配置**：兼容 `claude_desktop_config.json` 格式
3. **提供完整的服务配置界面**：命令、参数、环境变量、权限设置
4. **支持 GUI/JSON 双模式**：兼顾普通用户和高级用户
5. **遵循 macOS 原生设计语言**：参考 Xcode 和系统设置的设计风格

## What Changes

### UI Layout (完全重构)

**从**：单页面 ScrollView 列表布局
**变为**：Master-Detail 双栏布局

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           MCP Settings View                                  │
├─────────────────────┬───────────────────────────────────────────────────────┤
│  Server List        │           Server Detail View                           │
│  (Sidebar)          │                                                        │
├─────────────────────┤  ┌─────────────────────────────────────────────────┐  │
│                     │  │  [Icon] Server Name        [Status] [Toggle]   │  │
│  BUILT-IN CORE      │  │         Trigger: /mcp/git                       │  │
│  ┌───────────────┐  │  └─────────────────────────────────────────────────┘  │
│  │⚡ System Info │  │                                                        │
│  │📂 File System│  │  ┌─────────────────────────────────────────────────┐  │
│  │🐙 Git        │  │  │  Command & Arguments                             │  │
│  │💻 Shell      │  │  │  Command: [/usr/bin/node          ] [Browse]    │  │
│  └───────────────┘  │  │  Args:    [-y, @mcp/server-git]                │  │
│                     │  └─────────────────────────────────────────────────┘  │
│  EXTENSIONS         │                                                        │
│  ┌───────────────┐  │  ┌─────────────────────────────────────────────────┐  │
│  │🤖 Linear     │  │  │  Environment Variables                          │  │
│  │🐘 PostgreSQL │  │  │  ┌────────────┬────────────────────┐            │  │
│  └───────────────┘  │  │  │ LINEAR_KEY │ ••••••••••••••••   │ [Eye]     │  │
│                     │  │  └────────────┴────────────────────┘            │  │
│  [+ Add Server]     │  │  [+ Add Variable]                                │  │
│                     │  └─────────────────────────────────────────────────┘  │
│                     │                                                        │
│                     │  ┌─────────────────────────────────────────────────┐  │
│                     │  │  Permissions                                    │  │
│                     │  │  ☑ Ask for confirmation before tool execution  │  │
│                     │  │  □ Auto-approve all tool calls (dangerous)      │  │
│                     │  │  Allowed Paths: [~/Projects, ~/Documents]       │  │
│                     │  └─────────────────────────────────────────────────┘  │
│                     │                                                        │
│                     │  [Show Logs]                        [{}JSON] [Save]   │
└─────────────────────┴───────────────────────────────────────────────────────┘
```

### Core Changes (Rust)

- **MODIFIED**: `McpConfig` - 添加外部服务器配置结构
- **ADDED**: `McpServerConfig` struct - 服务器配置（command, args, env, permissions）
- **ADDED**: `McpServerStatus` enum - 服务器状态（Running, Stopped, Error, Initializing）
- **MODIFIED**: UniFFI 接口 - 添加服务器 CRUD 方法

### Swift Layer

- **REWRITTEN**: `McpSettingsView` - 从 ScrollView 改为 HSplitView
- **ADDED**: `McpServerListView` - 左侧服务器列表（侧边栏）
- **ADDED**: `McpServerDetailView` - 右侧服务器详情（表单）
- **ADDED**: `McpEnvVarEditor` - 环境变量键值对编辑器
- **ADDED**: `McpArgsEditor` - 命令行参数编辑器
- **ADDED**: `McpJsonEditor` - JSON 模式编辑器
- **ADDED**: `McpServerLogView` - 服务器日志查看 Sheet

### Configuration

- **MODIFIED**: `config.toml` - 外部服务器配置格式兼容 `claude_desktop_config.json`

## Impact

### Affected Specs
- `settings-ui-layout` - MCP 设置遵循相同的 Master-Detail 模式

### Affected Code
- `Aether/Sources/McpSettingsView.swift` (完全重写)
- `Aether/core/src/config/mod.rs` (扩展 McpConfig)
- `Aether/core/src/aether.udl` (添加 UniFFI 接口)
- `Aether/Resources/*/Localizable.strings` (新增本地化字符串)

### Breaking Changes
- **NONE** - 现有配置格式向后兼容

## Design Decisions

### D1: Master-Detail 布局 (采纳) ⭐

**决策**：采用 macOS 标准的 Master-Detail（主从视图）双栏布局。

**理由**：
- 与 macOS 系统设置、Xcode 配置界面保持一致
- 清晰的层级结构：列表 → 详情
- 更好的空间利用：左侧紧凑列表，右侧宽敞表单

**替代方案**：
- 单页面列表展开 - **否决**，配置项太多时体验差
- Tab 分页 - **否决**，服务器数量不确定

### D2: 内置 vs 扩展分组 (采纳)

**决策**：在侧边栏将服务分为 "Built-in Core" 和 "Extensions" 两组。

**理由**：
- 明确区分系统级服务（不可删除）和用户扩展（可删除）
- 用户一眼可识别哪些是官方支持的

### D3: GUI + JSON 双模式 (采纳)

**决策**：提供 GUI 图形界面和 JSON 原始编辑两种模式切换。

**理由**：
- GUI 模式：普通用户友好，有表单校验
- JSON 模式：高级用户可直接粘贴网上找到的配置

### D4: 环境变量安全显示 (采纳)

**决策**：环境变量值默认使用 `SecureField` 掩码显示，可点击眼睛图标查看。

**理由**：
- API Key 等敏感信息不应明文显示
- 防止屏幕共享时泄露

### D5: 确认对话框默认开启 (采纳)

**决策**：默认所有工具调用需要用户确认，可选择"自动批准"。

**理由**：
- MCP 协议强调 "Human in the loop"
- 防止恶意或错误的工具调用

## Implementation Phases

### Phase 1: 数据模型扩展

**目标**：扩展 Rust 配置模型支持外部服务器

- 添加 `McpServerConfig` 结构
- 添加 `McpServerStatus` 枚举
- 扩展 UniFFI 接口
- 兼容 `claude_desktop_config.json` 格式

### Phase 2: Master-Detail 布局

**目标**：重构 UI 为双栏布局

- 创建 `McpServerListView` (侧边栏)
- 创建 `McpServerDetailView` (详情面板)
- 实现服务器选择状态管理

### Phase 3: 详情编辑器

**目标**：实现完整的服务器配置编辑

- 环境变量编辑器（Key-Value 表格）
- 命令行参数编辑器
- 权限设置面板
- GUI/JSON 模式切换

### Phase 4: 日志与调试

**目标**：提供调试能力

- 服务器状态指示器
- 实时日志 Sheet
- 错误提示与修复建议

## References

- [MCP Official Specification](https://modelcontextprotocol.io/)
- [claude_desktop_config.json Format](https://modelcontextprotocol.io/docs/tools/desktop)
- macOS Human Interface Guidelines - Master-Detail Layout
- Existing `implement-mcp-capability` proposal
- Existing `settings-ui-layout` spec
