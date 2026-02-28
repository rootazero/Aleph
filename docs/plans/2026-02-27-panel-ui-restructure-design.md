# Panel UI Restructure Design

> **Date**: 2026-02-27
> **Status**: Approved
> **Scope**: 删除 Halo 窗口，将 Chat/Dashboard/Settings 统一到 Panel 中

---

## 1. 背景与动机

当前 Aleph 桌面端有多个独立窗口：
- **Halo 窗口** (800×80 浮动面板) — 快速输入入口，无对话历史
- **Settings 窗口** (900×650) — 独立加载 `/settings`
- **Panel 窗口** — 纯监控仪表板，5 个功能页面

问题：
1. Halo 浮窗功能单一，无法承载完整对话体验
2. Panel 无 Chat 能力，用户必须在多个窗口间切换
3. Settings 独立窗口造成不必要的窗口管理负担

## 2. 设计目标

- **单窗口体验**：所有功能统一到 Panel 中
- **Chat 为核心**：Panel 默认为 Chat 模式，对话是第一公民
- **轻量导航**：底部 3 个图标切换模式（Chat/Dashboard/Settings）
- **上下文侧栏**：侧栏内容跟随当前模式变化

## 3. 整体布局架构

### 三层布局

```
┌────────────────────────────────────────────────────┐
│                    Top Bar (h-12)                   │  ← 全局固定
│  [A] Aleph Hub              [搜索] [+新对话]        │
├────────────────┬───────────────────────────────────┤
│                │                                   │
│   Side Panel   │         Main Content              │  ← flex-1
│   (w-64)       │         (flex-1)                  │
│                │                                   │
│   根据当前模式  │    Chat: 消息流 + 输入框           │
│   动态切换内容  │    Dashboard: 子功能页面           │
│                │    Settings: 设置表单              │
│                │                                   │
├────────────────┴───────────────────────────────────┤
│              Bottom Bar (h-12)                      │  ← 全局固定
│  [💬 Chat]          [📊 Dashboard]       [⚙ Settings] │
└────────────────────────────────────────────────────┘
```

### 三种模式

| 模式 | 侧栏内容 | 主内容区 |
|------|----------|----------|
| **Chat** (默认) | 项目列表 + 会话列表（可折叠树） | 选中会话的消息流 + 底部输入框 |
| **Dashboard** | Dashboard 子导航列表 | 选中的子功能页面（Overview/Trace/Health/Memory/Social） |
| **Settings** | Settings 子导航列表 | 选中的设置表单 |

### 路由设计

```
/                    → Chat (默认，新建或恢复上次对话)
/chat/:session_id    → Chat (指定会话)
/dashboard           → Dashboard Overview
/dashboard/trace     → Agent Trace
/dashboard/health    → System Health
/dashboard/memory    → Memory Vault
/dashboard/social    → Social Connect
/settings            → Settings General
/settings/providers  → AI Providers
/settings/search     → Search Providers
/settings/embedding  → Embedding Providers
/settings/generation → Generation Providers
/settings/social     → Social Bots
/settings/extensions → Extensions
/settings/security   → Security
/settings/about      → About
```

## 4. Chat 模式详细设计

### 4.1 Chat 侧栏

```
┌──────────────────┐
│ 🔍 搜索对话...    │  ← 搜索框
├──────────────────┤
│ [+ 新对话]        │  ← 新建按钮
├──────────────────┤
│ ▼ 项目 A          │  ← 可折叠项目分组
│   💬 会话标题 1    │     (标题自动从首条消息生成)
│   💬 会话标题 2 ●  │     ● = 有未读/新回复
│   💬 会话标题 3    │
│                   │
│ ▼ 项目 B          │
│   💬 会话标题 4    │
│                   │
│ ▶ 未分组 (3)      │  ← 未关联项目的对话
└──────────────────┘
```

**会话列表行为：**
- 会话按最近活跃时间排序（最新在上）
- 右键菜单：重命名 / 移到项目 / 删除
- 拖拽可将会话移动到不同项目分组
- 点击会话 → 主内容区切换到该对话

### 4.2 Chat 主内容区

```
┌─────────────────────────────────────────────┐
│  会话标题          [项目: A] [模型: Claude]  │  ← 顶部信息栏
├─────────────────────────────────────────────┤
│                                             │
│  👤 用户消息气泡                              │
│                                             │
│  🤖 AI 回复（Markdown 渲染）                 │
│     - 代码块语法高亮 + 复制按钮              │
│     - 工具调用展示（可折叠卡片）              │
│     - 思考过程（可折叠浅色区块）              │
│                                             │
│  🤖 AI 流式回复                              │
│     ████████░░░░ 正在思考...                │
│                                             │
├─────────────────────────────────────────────┤
│  📎 [附件]  [输入消息...]          [发送 ↑]  │  ← 底部输入区
│  模型选择 ▾  |  工具开关 🔧                  │
└─────────────────────────────────────────────┘
```

**消息流特性：**
- 流式渲染（逐 token 显示）
- Markdown 渲染（标题、列表、代码块、表格）
- 代码块语法高亮 + 复制按钮
- 工具调用以可折叠卡片展示（工具名 + 输入 + 输出）
- 思考过程（thinking）以可折叠浅色区块展示
- 自动滚动到底部（用户手动上滚时暂停自动滚动）

**输入区特性：**
- 多行文本输入（自动扩展高度）
- Enter 发送，Shift+Enter 换行
- 附件支持（文件/图片）
- 模型选择下拉
- 工具启用/禁用开关

### 4.3 RPC 通信

基于现有 DashboardContext 的 WebSocket 基础设施：

| 操作 | RPC 方法 | 说明 |
|------|----------|------|
| 发送消息 | `agent.run` | 发送用户消息，启动 agent loop |
| 流式响应 | `subscribe_topic("stream.*")` | 订阅流式 token |
| 取消生成 | `agent.cancel` | 取消当前生成 |
| 加载历史 | `sessions.list` / `session.messages` | 获取会话列表和消息 |
| 创建会话 | `session.create` | 新建会话 |
| 删除会话 | `session.delete` | 删除会话 |

## 5. Dashboard 模式详细设计

### 5.1 Dashboard 侧栏

```
┌──────────────────┐
│  📊 Dashboard     │  ← 标题
├──────────────────┤
│                   │
│  ▸ Overview       │  ← 系统总览 (原 Home 页面)
│  ▸ Agent Trace    │  ← 实时 Agent 追踪
│  ▸ System Health  │  ← 系统健康
│  ▸ Memory Vault   │  ← 记忆库
│  ▸ Social Connect │  ← 社交通道管理
│                   │
└──────────────────┘
```

- 垂直导航列表，图标 + 文字
- 当前选中项高亮
- 默认进入 Overview

### 5.2 Dashboard 主内容区

保持现有 5 个视图内容：
- **Overview**: 系统总览（Stats Grid + 最近活动 + 快捷操作）
- **Agent Trace**: 实时追踪时间线
- **System Health**: 服务状态 + 资源利用
- **Memory Vault**: 记忆搜索 + 统计
- **Social Connect**: 社交平台连接管理

视图内容不变，仅从顶级路由移到 `/dashboard/*` 嵌套路由下。

## 6. Settings 模式详细设计

### 6.1 Settings 侧栏

```
┌──────────────────┐
│  ⚙ Settings       │  ← 标题
├──────────────────┤
│                   │
│  ▸ General        │  ← 通用设置
│  ▸ AI Providers   │  ← AI 提供商配置
│  ▸ Search         │  ← 搜索提供商
│  ▸ Embedding      │  ← Embedding 配置
│  ▸ Generation     │  ← 生成配置
│  ▸ Social Bots    │  ← 社交 Bot 配置
│  ▸ Extensions     │  ← 扩展/插件管理
│  ▸ Security       │  ← 安全/权限
│  ▸ About          │  ← 关于/版本
│                   │
└──────────────────┘
```

### 6.2 Settings 主内容区

保持现有 Settings 页面内容，迁入 Panel 主内容区。

## 7. 底部导航栏 (Bottom Bar)

```
┌─────────────────────────────────────────────┐
│                                             │
│   💬            📊             ⚙            │
│  Chat       Dashboard      Settings         │
│                                             │
└─────────────────────────────────────────────┘
```

**设计规格：**
- 高度：`h-12` (48px)
- 背景：`bg-slate-900/50 backdrop-blur-xl`
- 顶部边框：`border-t border-slate-800`
- 每项：图标 + 小号文字标签，垂直排列
- 活跃状态：`text-indigo-400`，带微弱底部指示线
- 非活跃：`text-slate-500`
- Hover：`text-slate-300`
- 三项等分宽度，居中对齐

## 8. 顶部栏 (Top Bar)

```
┌─────────────────────────────────────────────┐
│  [A] Aleph Hub              [🔍] [+ 新对话]  │
└─────────────────────────────────────────────┘
```

**设计规格：**
- 高度：`h-12` (48px)
- 背景：`bg-slate-900/50 backdrop-blur-xl`
- 底部边框：`border-b border-slate-800`
- 左侧：Logo "A" 图标 + "Aleph Hub" 文字
- 右侧：搜索按钮 + 新对话按钮（仅在 Chat 模式显示新对话按钮）
- macOS 需为交通灯按钮留出空间 (左上角 padding)

## 9. 需要删除的内容

| 组件 | 位置 | 原因 |
|------|------|------|
| HaloWindow | `apps/macos-native/Aleph/UI/HaloWindow.swift` | 完全删除 Halo 浮窗 |
| SettingsWindow | `apps/macos-native/Aleph/UI/SettingsWindow.swift` | Settings 并入 Panel |
| Halo 路由 | Panel 中的 `/halo` 路由（如有） | 不再需要 |
| 菜单栏 "Show Halo" | `MenuBarController.swift` | 改为 "Show Chat" |
| 菜单栏 "Settings..." | `MenuBarController.swift` | 改为 "Show Settings"（打开 Panel 并切换到 Settings） |

## 10. 需要修改的内容

| 组件 | 文件 | 变更 |
|------|------|------|
| App 根布局 | `apps/dashboard/src/app.rs` | 重构为 Top Bar + Side Panel + Main Content + Bottom Bar |
| Sidebar | `apps/dashboard/src/components/sidebar.rs` | 重构为上下文感知侧栏（Chat/Dashboard/Settings 三模式） |
| 路由 | `apps/dashboard/src/app.rs` | 顶级路由改为嵌套路由 `/chat/*`, `/dashboard/*`, `/settings/*` |
| MenuBarController | `apps/macos-native/.../MenuBarController.swift` | "Show Halo" → "Show Chat"，Cmd+Opt+/ 打开 Panel Chat |
| DashboardContext | `apps/dashboard/src/context.rs` | 扩展：增加会话管理、消息流式处理能力 |

## 11. 新增组件

| 组件 | 说明 |
|------|------|
| `BottomBar` | 底部三图标导航栏 |
| `TopBar` | 顶部全局信息栏 |
| `ChatSidebar` | Chat 模式侧栏（项目+会话树） |
| `DashboardSidebar` | Dashboard 模式侧栏（子功能导航） |
| `SettingsSidebar` | Settings 模式侧栏（子设置导航） |
| `ChatView` | Chat 主视图（消息流 + 输入区） |
| `MessageBubble` | 单条消息组件（用户/AI/系统） |
| `ChatInput` | 聊天输入框（多行、附件、模型选择） |
| `SessionList` | 会话列表组件（树形，支持分组） |
| `ProjectGroup` | 项目分组组件（可折叠） |

## 12. 状态管理扩展

在现有 `DashboardContext` 基础上扩展：

```rust
// 新增状态
pub struct PanelState {
    // 当前活跃模式
    pub active_mode: RwSignal<PanelMode>,  // Chat | Dashboard | Settings

    // Chat 状态
    pub current_session_id: RwSignal<Option<String>>,
    pub sessions: RwSignal<Vec<Session>>,
    pub messages: RwSignal<Vec<Message>>,
    pub is_streaming: RwSignal<bool>,

    // Dashboard 子页面
    pub dashboard_tab: RwSignal<DashboardTab>,  // Overview | Trace | Health | Memory | Social

    // Settings 子页面
    pub settings_tab: RwSignal<SettingsTab>,
}

pub enum PanelMode {
    Chat,
    Dashboard,
    Settings,
}
```

## 13. 快捷键

| 快捷键 | 行为 |
|--------|------|
| `Cmd+Opt+/` | 打开/聚焦 Panel，切换到 Chat 模式 |
| `Cmd+,` | 打开/聚焦 Panel，切换到 Settings 模式 |
| `Cmd+N` | 新建对话 |
| `Cmd+[` / `Cmd+]` | 切换上/下一个对话 |
| `Escape` | 取消当前 AI 生成 |

## 14. 设计约束

- **R2 遵守**：所有 UI 逻辑在 Leptos/WASM 中实现，macOS native 只负责窗口容器
- **R4 遵守**：Panel 是纯 I/O 层，不做业务逻辑，通过 RPC 与 Core 通信
- **R5 调整**：删除 Halo 后，菜单栏仍然常驻，快捷键打开 Panel Chat
- **响应式**：侧栏在窄窗口下可折叠隐藏，移动端侧栏以抽屉形式弹出
