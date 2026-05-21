# Design: Refactor Skills UI Architecture

## Overview

本文档详细描述 Skills UI 架构重构的技术设计，重点关注：
1. 统一数据模型设计
2. 组件架构与通信模式
3. 状态管理策略
4. 配置迁移方案

## 1. 统一数据模型

### 1.1 核心设计原则

**Config as Code, UI as Convenience**
- 底层维护标准配置文件（TOML/JSON）
- UI 是配置的友好渲染器，不是唯一真相来源
- 支持直接编辑配置文件，UI 实时反映变更

### 1.2 数据类型定义

```rust
// aleph.udl

/// Skill 类型枚举
enum SkillType {
    "BuiltinMcp",      // 内置 MCP 服务 (Rust 原生实现)
    "ExternalMcp",     // 外部 MCP 服务 (子进程)
    "PromptTemplate",  // 提示模板 (SKILL.md)
};

/// Skill 状态枚举
enum SkillStatus {
    "Stopped",     // 已停止
    "Starting",    // 启动中
    "Running",     // 运行中
    "Error",       // 错误
};

/// 传输协议类型
enum McpTransport {
    "Stdio",       // 标准输入/输出
    "Sse",         // Server-Sent Events
};

/// 环境变量
dictionary EnvVar {
    string key;
    string value;
};

/// Skill 权限配置
dictionary SkillPermissions {
    boolean requires_confirmation;  // 工具调用前需确认
    sequence<string> allowed_paths; // 允许访问的路径
    sequence<string> allowed_commands; // 允许执行的命令 (shell)
};

/// 统一的 Skill 配置
dictionary UnifiedSkillConfig {
    // 基础信息
    string id;                      // 唯一标识 (e.g., "fs", "git", "custom-linear")
    string name;                    // 显示名称
    string description;             // 描述
    SkillType skill_type;           // 类型
    boolean enabled;                // 是否启用
    string icon;                    // 图标 (SF Symbol name)
    string color;                   // 主题色 (#RRGGBB)

    // 触发命令 (可选，用于 Halo 快捷触发)
    string? trigger_command;        // e.g., "/git", "/fs"

    // MCP 特有字段 (skill_type == BuiltinMcp || ExternalMcp)
    McpTransport? transport;        // 传输协议
    string? command;                // 命令路径 (外部 MCP)
    sequence<string> args;          // 参数列表
    sequence<EnvVar> env;           // 环境变量
    string? working_directory;      // 工作目录

    // 权限
    SkillPermissions permissions;

    // PromptTemplate 特有字段 (skill_type == PromptTemplate)
    string? skill_md_path;          // SKILL.md 文件路径
    sequence<string> allowed_tools; // 允许的工具列表
};

/// Skill 状态信息 (用于 UI 显示)
dictionary SkillStatusInfo {
    SkillStatus status;
    string? message;                // 状态消息
    string? last_error;             // 最后错误
    u64? pid;                       // 进程 ID (仅外部 MCP)
};
```

### 1.3 类型映射关系

| 原类型 | 字段 | 新类型字段 | 映射规则 |
|-------|------|-----------|---------|
| `McpServerConfig` | `id` | `UnifiedSkillConfig.id` | 直接映射 |
| `McpServerConfig` | `name` | `UnifiedSkillConfig.name` | 直接映射 |
| `McpServerConfig` | `serverType` | `UnifiedSkillConfig.skill_type` | `builtin` → `BuiltinMcp`, `external` → `ExternalMcp` |
| `McpServerConfig` | `enabled` | `UnifiedSkillConfig.enabled` | 直接映射 |
| `McpServerConfig` | `command` | `UnifiedSkillConfig.command` | 直接映射 |
| `McpServerConfig` | `args` | `UnifiedSkillConfig.args` | 直接映射 |
| `McpServerConfig` | `env` | `UnifiedSkillConfig.env` | `McpEnvVar` → `EnvVar` |
| `McpServerConfig` | `permissions` | `UnifiedSkillConfig.permissions` | 结构映射 |
| `SkillInfo` | `id` | `UnifiedSkillConfig.id` | 直接映射 |
| `SkillInfo` | `name` | `UnifiedSkillConfig.name` | 直接映射 |
| `SkillInfo` | `description` | `UnifiedSkillConfig.description` | 直接映射 |
| `SkillInfo` | `allowedTools` | `UnifiedSkillConfig.allowed_tools` | 直接映射 |
| (新增) | - | `UnifiedSkillConfig.skill_type` | 固定为 `PromptTemplate` |

---

## 2. 组件架构

### 2.1 组件层次结构

```
SkillsSettingsView (容器视图)
│
├── SkillFilterSidebar (筛选侧边栏)
│   ├── FilterSection (状态筛选)
│   ├── CategorySection (类型筛选)
│   └── ActionButtons (添加/JSON)
│
├── SkillListView (Skill 列表区)
│   ├── SearchBar (搜索框)
│   └── LazyVStack
│       └── SkillCard × N
│           ├── SkillIcon
│           ├── SkillInfo (name + description)
│           ├── SkillStatusIndicator
│           ├── Toggle
│           └── MoreButton (...)
│
└── SkillInspectorPanel (配置面板，条件显示)
    ├── SkillHeaderSection
    │   ├── SkillIcon (大)
    │   ├── SkillName + Type
    │   ├── SkillStatusIndicator
    │   └── Toggle
    │
    ├── SkillConnectionSection (仅外部 MCP)
    │   ├── TransportPicker
    │   ├── CommandField + BrowseButton
    │   ├── SkillArgsEditor
    │   └── WorkingDirectoryField
    │
    ├── SkillEnvVarEditor
    │   └── EnvVarRow × N
    │       ├── KeyField
    │       ├── SecureValueField
    │       ├── EyeButton
    │       └── DeleteButton
    │
    ├── SkillPermissionsEditor
    │   ├── ConfirmationToggle
    │   ├── AllowedPathsList
    │   └── AllowedCommandsList (仅 shell)
    │
    ├── SkillToolsSection (只读)
    │   └── ToolTag × N
    │
    └── ActionBar
        ├── ViewLogsButton
        ├── Spacer
        ├── CancelButton
        └── SaveButton
```

### 2.2 组件通信模式

采用 **单向数据流 + Binding** 模式：

```
┌─────────────────────────────────────────────────────────────────────┐
│  SkillsSettingsView (State Owner)                                    │
│                                                                      │
│  @State skills: [UnifiedSkillConfig]      ← loadSkills()            │
│  @State selectedSkillId: String?                                     │
│  @State filterState: FilterState                                     │
│  @State editingConfig: UnifiedSkillConfig? (copy for editing)       │
│                                                                      │
└──────────┬──────────────────────┬──────────────────────┬────────────┘
           │                      │                      │
           ↓                      ↓                      ↓
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────────┐
│ SkillFilterSidebar│  │ SkillListView     │  │ SkillInspectorPanel     │
│                  │  │                  │  │                          │
│ @Binding filter  │  │ skills (readonly)│  │ @Binding editingConfig  │
│ onAddClick       │  │ @Binding selected│  │ onSave: () -> Void      │
│ onJsonClick      │  │                  │  │ onCancel: () -> Void    │
└──────────────────┘  └──────────────────┘  └──────────────────────────┘
```

**关键设计决策**：
- `editingConfig` 是 `selectedSkill` 的**副本**，用户编辑不影响原数据
- 点击 Save 时将 `editingConfig` 同步回 `skills` 并调用 Core API
- 点击 Cancel 时丢弃 `editingConfig`，恢复原值

### 2.3 组件 API 设计

```swift
// SkillCard.swift
struct SkillCard: View {
    let skill: UnifiedSkillConfig
    let status: SkillStatusInfo
    let isSelected: Bool
    let onSelect: () -> Void
    let onToggle: (Bool) -> Void
    let onMoreAction: (SkillAction) -> Void
}

enum SkillAction {
    case viewLogs
    case duplicate
    case delete
    case viewInJson
}

// SkillFilterSidebar.swift
struct SkillFilterSidebar: View {
    @Binding var statusFilter: SkillStatusFilter?
    @Binding var typeFilter: SkillType?
    let onAddSkill: () -> Void
    let onShowJson: () -> Void
}

enum SkillStatusFilter {
    case enabled
    case disabled
    case error
}

// SkillInspectorPanel.swift
struct SkillInspectorPanel: View {
    @Binding var config: UnifiedSkillConfig
    let status: SkillStatusInfo
    let availableTools: [String]  // 从 Core 获取
    let onSave: () -> Void
    let onCancel: () -> Void
    let onViewLogs: () -> Void
}

// SkillEnvVarEditor.swift
struct SkillEnvVarEditor: View {
    @Binding var envVars: [EnvVar]
}

// SkillArgsEditor.swift
struct SkillArgsEditor: View {
    @Binding var args: [String]
}

// SkillPermissionsEditor.swift
struct SkillPermissionsEditor: View {
    @Binding var permissions: SkillPermissions
    let skillType: SkillType  // 根据类型显示不同选项
}
```

---

## 3. 状态管理策略

### 3.1 状态分类

| 状态类型 | 存储位置 | 持久化 | 说明 |
|---------|---------|-------|------|
| 配置数据 | Rust Core | TOML 文件 | Skills 配置、权限等 |
| 运行时状态 | Rust Core | 内存 | 服务运行状态、PID、日志 |
| UI 状态 | Swift @State | 内存 | 选中项、筛选条件、编辑副本 |
| 窗口状态 | UserDefaults | Disk | 窗口位置、面板宽度 |

### 3.2 状态同步机制

```
┌─────────────────────────────────────────────────────────────────────┐
│  Config File (~/.aleph/config.toml)                          │
│  ───────────────────────────────────────────────────────────────────│
│  [skills.fs]                                                         │
│  enabled = true                                                      │
│  ...                                                                 │
└────────────────────────────────┬────────────────────────────────────┘
                                 │ read/write
                                 ↓
┌─────────────────────────────────────────────────────────────────────┐
│  Rust Core (AlephCore)                                              │
│  ───────────────────────────────────────────────────────────────────│
│  skills: Vec<UnifiedSkillConfig>                                     │
│  skill_statuses: HashMap<String, SkillStatusInfo>                    │
│                                                                      │
│  + list_skills() -> Vec<UnifiedSkillConfig>                          │
│  + update_skill(config) -> Result<()>                                │
│  + get_skill_status(id) -> SkillStatusInfo                           │
└────────────────────────────────┬────────────────────────────────────┘
                                 │ UniFFI
                                 ↓
┌─────────────────────────────────────────────────────────────────────┐
│  Swift Layer                                                         │
│  ───────────────────────────────────────────────────────────────────│
│  @State skills: [UnifiedSkillConfig]      <- core.listSkills()      │
│  @State statuses: [String: SkillStatusInfo]                          │
│                                                                      │
│  Timer (1s) → refresh statuses                                       │
│  onChange(skills) → save to Core                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.3 编辑-保存流程

```
用户选中 Skill A
    │
    ↓
editingConfig = skills.first { $0.id == selectedId }?.copy()
    │
    ↓
用户编辑 editingConfig (通过 Binding)
    │
    ├── 点击 Save
    │   │
    │   ↓
    │   core.updateSkill(editingConfig!)
    │   skills = core.listSkills()  // 刷新列表
    │   editingConfig = nil
    │
    └── 点击 Cancel
        │
        ↓
        editingConfig = nil (丢弃修改)
```

---

## 4. 配置迁移方案

### 4.1 旧配置格式

**MCP 配置 (config.toml)**：
```toml
[mcp]
enabled = true
fs_enabled = true
git_enabled = true
shell_enabled = true
system_info_enabled = true

[mcp.servers.custom-linear]
transport = "stdio"
command = "npx"
args = ["-y", "@mcp/linear"]
env = { LINEAR_API_KEY = "..." }
```

**Skills 配置 (config.toml)**：
```toml
[skills]
enabled = true
skills_dir = "~/.aleph/skills"
auto_match_enabled = true
```

### 4.2 新配置格式

```toml
[skills]
enabled = true

# 内置 MCP 服务
[skills.builtin.fs]
enabled = true
permissions.requires_confirmation = false
permissions.allowed_paths = ["~"]

[skills.builtin.git]
enabled = true
permissions.requires_confirmation = true

[skills.builtin.shell]
enabled = true
permissions.requires_confirmation = true
permissions.allowed_commands = ["ls", "pwd", "git"]

[skills.builtin.system-info]
enabled = true

# 外部 MCP 服务
[skills.external.custom-linear]
name = "Linear"
description = "Linear 项目管理"
enabled = true
transport = "stdio"
command = "npx"
args = ["-y", "@mcp/linear"]
env = { LINEAR_API_KEY = "..." }
icon = "list.bullet.rectangle"
color = "#5E6AD2"

# 提示模板 Skills
[skills.templates]
dir = "~/.aleph/skills"
auto_match_enabled = true
```

### 4.3 迁移算法

```rust
fn migrate_legacy_config(old_config: &Config) -> Config {
    let mut new_config = old_config.clone();

    // 1. 迁移内置 MCP 服务
    if let Some(mcp) = &old_config.mcp {
        new_config.skills.builtin.fs.enabled = mcp.fs_enabled;
        new_config.skills.builtin.git.enabled = mcp.git_enabled;
        new_config.skills.builtin.shell.enabled = mcp.shell_enabled;
        new_config.skills.builtin.system_info.enabled = mcp.system_info_enabled;

        // 2. 迁移外部 MCP 服务器
        for (id, server) in mcp.servers.iter() {
            new_config.skills.external.insert(id.clone(), ExternalSkillConfig {
                name: server.name.clone(),
                description: server.description.clone().unwrap_or_default(),
                enabled: server.enabled,
                transport: server.transport.clone(),
                command: server.command.clone(),
                args: server.args.clone(),
                env: server.env.clone(),
                icon: server.icon.clone().unwrap_or("puzzlepiece".to_string()),
                color: server.color.clone().unwrap_or("#808080".to_string()),
                permissions: server.permissions.clone(),
            });
        }
    }

    // 3. 迁移 Skills 配置
    if let Some(skills) = &old_config.skills {
        new_config.skills.templates.dir = skills.skills_dir.clone();
        new_config.skills.templates.auto_match_enabled = skills.auto_match_enabled;
    }

    // 4. 备份旧配置
    // backup_config(&old_config)?;

    // 5. 移除旧字段
    new_config.mcp = None;
    // 保留 skills 字段但转换为新格式

    new_config
}
```

### 4.4 迁移触发条件

```rust
impl AlephCore {
    fn load_config() -> Result<Config> {
        let config = Config::load_from_file()?;

        // 检测是否需要迁移
        if config.needs_migration() {
            let migrated = migrate_legacy_config(&config);
            migrated.save_to_file()?;
            return Ok(migrated);
        }

        Ok(config)
    }
}

impl Config {
    fn needs_migration(&self) -> bool {
        // 如果存在旧格式的 [mcp] 配置，需要迁移
        self.mcp.is_some() && self.mcp.as_ref().unwrap().servers.is_some()
    }
}
```

---

## 5. UI 布局详细规范

### 5.1 尺寸与间距

| 元素 | 尺寸 | 说明 |
|-----|------|-----|
| Filter Sidebar | 180px 宽 | 固定宽度 |
| Skill Card | 100% 宽 × 72px 高 | 卡片高度固定 |
| Inspector Panel | 400px 宽 | 最小宽度，可拉伸 |
| Section Padding | 16px | `DesignTokens.Spacing.lg` |
| Card Gap | 8px | `DesignTokens.Spacing.sm` |

### 5.2 颜色规范

| 状态 | 颜色 | 用途 |
|-----|------|-----|
| Running | `#34C759` (绿) | 状态指示器 |
| Stopped | `#8E8E93` (灰) | 状态指示器 |
| Starting | `#FF9F0A` (黄) | 状态指示器 |
| Error | `#FF3B30` (红) | 状态指示器 |
| Selected | `.accentColor` | 卡片边框 |
| Hover | `.primary.opacity(0.05)` | 卡片背景 |

### 5.3 动画规范

| 动画 | 时长 | 曲线 | 说明 |
|-----|------|-----|-----|
| Inspector 滑入 | 250ms | `.easeInOut` | 从右侧滑入 |
| Inspector 滑出 | 200ms | `.easeIn` | 滑出到右侧 |
| Card Hover | 150ms | `.easeInOut` | 背景色渐变 |
| Status Pulse | 1000ms | `.easeInOut.repeatForever` | Running 状态呼吸灯 |

---

## 6. 错误处理策略

### 6.1 错误类型

```swift
enum SkillError: LocalizedError {
    case loadFailed(underlying: Error)
    case saveFailed(underlying: Error)
    case invalidConfig(message: String)
    case skillNotFound(id: String)
    case permissionDenied(path: String)

    var errorDescription: String? {
        switch self {
        case .loadFailed(let error):
            return L("skills.error.load_failed", error.localizedDescription)
        case .saveFailed(let error):
            return L("skills.error.save_failed", error.localizedDescription)
        case .invalidConfig(let message):
            return L("skills.error.invalid_config", message)
        case .skillNotFound(let id):
            return L("skills.error.not_found", id)
        case .permissionDenied(let path):
            return L("skills.error.permission_denied", path)
        }
    }
}
```

### 6.2 错误显示

- **加载错误**：显示在列表区域的 Empty State
- **保存错误**：Inspector Panel 底部 Alert
- **状态错误**：Skill Card 显示红色状态 + Tooltip 显示错误详情

---

## 7. 性能考虑

### 7.1 列表优化

- 使用 `LazyVStack` 而非 `VStack` 进行虚拟化
- 状态轮询使用 1s 间隔，避免过于频繁
- 大量 Skills 时考虑分页加载

### 7.2 状态同步优化

- Inspector 编辑时暂停状态轮询
- 使用 debounce 避免频繁保存
- 批量更新时合并请求

### 7.3 内存优化

- 日志显示使用 Ring Buffer，限制最大行数
- 长时间不使用的 Inspector 释放 editingConfig

---

## 8. 可访问性 (Accessibility)

### 8.1 VoiceOver 支持

```swift
struct SkillCard: View {
    var body: some View {
        HStack {
            // ...
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(skill.name), \(status.localizedDescription)")
        .accessibilityHint(L("skills.card.accessibility_hint"))
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }
}
```

### 8.2 键盘导航

- `Tab` / `Shift+Tab`：在筛选栏、列表、面板间切换
- `↑` / `↓`：在 Skill 列表中移动选择
- `Enter`：打开 Inspector
- `Escape`：关闭 Inspector / 取消编辑
- `Cmd+S`：保存编辑

---

## 9. 测试策略

### 9.1 单元测试 (Rust)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_config_migration() {
        let old_config = r#"
        [mcp]
        enabled = true
        fs_enabled = true
        [mcp.servers.linear]
        command = "npx"
        "#;

        let config = Config::from_toml(old_config).unwrap();
        let migrated = migrate_legacy_config(&config);

        assert!(migrated.skills.builtin.fs.enabled);
        assert!(migrated.skills.external.contains_key("linear"));
        assert!(migrated.mcp.is_none());
    }
}
```

### 9.2 UI Preview 测试 (Swift)

```swift
#Preview("SkillCard - Running") {
    SkillCard(
        skill: .mock(status: .running),
        status: .mock(status: .running),
        isSelected: false,
        onSelect: {},
        onToggle: { _ in },
        onMoreAction: { _ in }
    )
}

#Preview("SkillCard - Error") {
    SkillCard(
        skill: .mock(status: .error),
        status: .mock(status: .error, error: "Connection refused"),
        isSelected: true,
        onSelect: {},
        onToggle: { _ in },
        onMoreAction: { _ in }
    )
}
```

### 9.3 集成测试

- 配置加载 → UI 显示一致性
- 编辑 → 保存 → 重新加载一致性
- 启用/禁用 → 服务状态变化
- JSON 模式编辑 → GUI 模式同步

---

## 10. 未来扩展点

### 10.1 Skill 商店 (Future)

预留 `Discover` Tab，未来可接入：
- Anthropic 官方 MCP Server 目录
- 社区 Skill 仓库
- 一键安装机制

### 10.2 Skill 分组 (Future)

允许用户创建自定义分组：
- 工作项目组
- 个人工具组
- 临时测试组

### 10.3 Skill 依赖 (Future)

支持 Skill 间依赖声明：
- `requires: ["fs", "git"]`
- 自动启用依赖 Skill

---

## References

- [MCP Official Specification](https://modelcontextprotocol.io/)
- [macOS Human Interface Guidelines - Sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars)
- [macOS Human Interface Guidelines - Inspectors](https://developer.apple.com/design/human-interface-guidelines/inspectors)
- [SwiftUI State Management](https://developer.apple.com/documentation/swiftui/managing-user-interface-state)
