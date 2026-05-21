# Capability: MCP Settings UI

## Overview

MCP Settings UI 提供 Master-Detail 布局的 MCP 服务器配置界面，支持内置服务和外部扩展服务器的统一管理。

---

## ADDED Requirements

### Requirement: Master-Detail 双栏布局

MCP 设置界面 SHALL 采用 macOS 标准的 Master-Detail 双栏布局。

**Rationale**: 提供清晰的层级结构，左侧紧凑列表便于快速定位，右侧宽敞表单便于详细配置。

#### Scenario: 用户打开 MCP 设置页面

**Given** 用户在 Settings 窗口中
**When** 用户点击 MCP 导航项
**Then** 显示双栏布局：左侧服务器列表，右侧服务器详情
**And** 左侧列表按 "Built-in Core" 和 "Extensions" 分组
**And** 默认选中第一个服务器并在右侧显示其详情

#### Scenario: 用户切换选中的服务器

**Given** MCP 设置页面已打开
**When** 用户点击左侧列表中的另一个服务器
**Then** 右侧详情面板更新为该服务器的配置
**And** 左侧对应项高亮显示选中状态

---

### Requirement: 外部 MCP 服务器管理

系统 MUST 支持用户添加、编辑和删除外部 MCP 服务器。

**Rationale**: 支持用户扩展 MCP 能力，兼容 claude_desktop_config.json 生态。

#### Scenario: 添加新的外部服务器

**Given** MCP 设置页面已打开
**When** 用户点击 "+ Add Server" 按钮
**Then** 显示添加服务器 Sheet
**And** Sheet 包含：名称、命令、参数、环境变量、工作目录字段
**When** 用户填写必填字段并点击 "Add"
**Then** 新服务器出现在 Extensions 分组
**And** 配置保存到 config.toml

#### Scenario: 编辑现有外部服务器

**Given** 用户选中一个外部服务器
**When** 用户修改右侧详情面板中的任意字段
**Then** SaveBar 出现，显示未保存更改
**When** 用户点击 "Save"
**Then** 更改保存到 config.toml
**And** SaveBar 消失

#### Scenario: 删除外部服务器

**Given** 用户选中一个外部服务器
**When** 用户点击列表底部的删除按钮或右键菜单 "Delete"
**Then** 显示确认对话框
**When** 用户确认删除
**Then** 服务器从列表和 config.toml 中移除

#### Scenario: 尝试删除内置服务器

**Given** 用户选中一个内置服务器（Built-in Core）
**Then** 删除按钮不可用或不显示
**And** 右键菜单无 "Delete" 选项

---

### Requirement: 环境变量安全显示

环境变量值 MUST 默认掩码显示，可手动切换可见性。

**Rationale**: 防止 API Key 等敏感信息在屏幕共享或截图时泄露。

#### Scenario: 查看环境变量列表

**Given** 用户选中一个配置了环境变量的服务器
**Then** 环境变量以 Key-Value 表格显示
**And** Value 列默认显示为 `••••••••••••`（掩码）
**And** 每行末尾有眼睛图标

#### Scenario: 临时查看环境变量值

**Given** 环境变量值处于掩码状态
**When** 用户点击眼睛图标
**Then** 该行的值明文显示
**And** 图标变为 "eye.slash"
**When** 用户再次点击或离开该行
**Then** 值恢复为掩码

#### Scenario: 编辑环境变量

**Given** 用户选中一个服务器
**When** 用户点击 "+ Add Variable"
**Then** 新增一行空的 Key-Value 输入
**When** 用户输入 Key 和 Value 并保存
**Then** 环境变量保存到配置

---

### Requirement: GUI/JSON 双模式编辑

系统 SHALL 支持用户在图形界面和 JSON 原始编辑两种模式间切换。

**Rationale**: GUI 模式适合普通用户，JSON 模式适合高级用户直接粘贴网上找到的配置。

#### Scenario: 默认 GUI 模式

**Given** 用户选中一个服务器
**Then** 右侧详情以 GUI 表单形式显示
**And** 底部有模式切换器：`[GUI] | [JSON]`

#### Scenario: 切换到 JSON 模式

**Given** 当前为 GUI 模式
**When** 用户点击 "JSON" 按钮
**Then** 详情面板切换为 JSON 编辑器
**And** 显示当前服务器的 JSON 配置
**And** JSON 格式与 claude_desktop_config.json 兼容

#### Scenario: 在 JSON 模式编辑后切换回 GUI

**Given** 用户在 JSON 模式修改了配置
**When** 用户点击 "GUI" 按钮
**Then** 系统尝试解析 JSON
**And** 如果 JSON 有效，GUI 表单更新为新值
**And** 如果 JSON 无效，显示错误提示并阻止切换

---

### Requirement: 服务器状态可视化

服务器列表 MUST 显示每个服务器的运行状态。

**Rationale**: 用户需要快速了解哪些服务正在运行、哪些出错。

#### Scenario: 显示服务器状态指示器

**Given** MCP 设置页面已打开
**Then** 每个服务器名称旁显示状态指示器
**And** 绿色圆点表示 Running
**And** 灰色圆点表示 Stopped
**And** 红色圆点表示 Error
**And** 旋转图标表示 Starting

#### Scenario: 服务器启动失败

**Given** 用户启用一个外部服务器
**When** 服务器启动失败（如命令不存在）
**Then** 状态指示器变为红色
**And** 详情面板顶部显示错误消息
**And** 提供修复建议（如 "检查命令路径是否正确"）

---

### Requirement: 服务器日志查看

系统 SHALL 提供服务器运行日志查看功能。

**Rationale**: 便于调试和排查问题。

#### Scenario: 打开服务器日志

**Given** 用户选中一个服务器
**When** 用户点击 "Show Logs" 按钮
**Then** 显示日志 Sheet
**And** 日志按时间倒序排列
**And** 每行包含时间戳、日志级别、消息

#### Scenario: 清除日志

**Given** 日志 Sheet 已打开
**When** 用户点击 "Clear" 按钮
**Then** 当前服务器的日志被清空

---

### Requirement: claude_desktop_config.json 导入

系统 MUST 支持导入 claude_desktop_config.json 格式的配置。

**Rationale**: 便于从 Claude Desktop 迁移配置，兼容现有生态。

#### Scenario: 导入配置文件

**Given** MCP 设置页面已打开
**When** 用户点击 "Import from Claude Desktop" 按钮
**Then** 显示文件选择对话框
**When** 用户选择一个有效的 claude_desktop_config.json 文件
**Then** 解析 mcpServers 字段
**And** 对于每个新服务器，添加到 Extensions 分组
**And** 对于已存在的同名服务器，提示用户选择覆盖或跳过

#### Scenario: 导入无效文件

**Given** 文件选择对话框已打开
**When** 用户选择一个无效的 JSON 文件
**Then** 显示错误提示 "Invalid configuration file"
**And** 不修改现有配置

---

## MODIFIED Requirements

### Requirement: McpSettingsView 布局

McpSettingsView SHALL 从单页面 ScrollView 列表布局重构为 HSplitView Master-Detail 双栏布局。

**From**: 单页面 ScrollView 列表布局，仅支持内置服务开关
**To**: HSplitView Master-Detail 双栏布局，支持内置和外部服务器完整配置

#### Scenario: 布局重构后的基本功能

**Given** 用户打开 MCP 设置
**When** 界面加载完成
**Then** 显示 HSplitView 双栏布局
**And** 左侧为服务器列表（支持分组）
**And** 右侧为服务器详情面板
**And** 原有的内置服务开关功能保持可用

---

## Related Capabilities

- `settings-ui-layout` - 遵循相同的 Master-Detail 设计模式
- `mcp-capability` - MCP 核心功能实现
- `localization` - 所有 UI 文本需本地化
