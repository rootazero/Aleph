# Aether Tauri 跨平台版本设计文档

> 创建日期：2026-01-22
> 状态：已确认

## 概述

为 Aether 设计 Tauri 版本，用于支持 Windows 和 Linux 平台，最大限度复刻 macOS Swift 版本的效果。

### 核心原则

**⚠️ Windows 优先原则**：如果某个局部代码在兼容 Windows 和 macOS 时有冲突，**优先兼容 Windows**。因为 macOS 大概率会继续使用 Swift 原生方案，Tauri 版本的主要目标是支持 Windows。

### 平台策略

```
                    ┌─────────────────────────────────────┐
                    │         Rust Core (共享)             │
                    │  agent_loop / dispatcher / memory   │
                    └──────────────┬──────────────────────┘
                                   │
           ┌───────────────────────┼───────────────────────┐
           │                       │                       │
           ▼                       ▼                       ▼
    ┌─────────────┐         ┌─────────────┐         ┌─────────────┐
    │   macOS     │         │   Windows   │         │   Linux     │
    │ Swift/SwiftUI│         │   Tauri    │         │   Tauri     │
    │  (UniFFI)   │         │  (内嵌Rust) │         │  (内嵌Rust) │
    └─────────────┘         └─────────────┘         └─────────────┘
        旗舰版                   主要目标                 次要目标
```

---

## 技术选型

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 前端框架 | React 18 + TypeScript 5 | 生态成熟，shadcn/ui 支持 |
| UI 组件 | shadcn/ui + Tailwind CSS 3 | 可定制，匹配 macOS 设计 |
| 状态管理 | Zustand | 轻量，与 Tauri 集成好 |
| 动画 | Framer Motion | AnimatePresence 处理状态切换 |
| 国际化 | react-i18next | 成熟稳定 |
| Halo 架构 | 单窗口多状态 | 切换流畅，无创建延迟 |
| 窗口架构 | 多窗口 | 与 macOS 行为一致 |
| Core 集成 | 直接依赖 | 最简单，类型共享 |
| 主题 | Tailwind dark mode | 跟随系统，可手动覆盖 |
| 设置导航 | 左侧边栏 | 适合 12+ 标签页 |
| 构建工具 | Vite | 快速，HMR 支持好 |
| 桌面框架 | Tauri 2.0 | 轻量，Rust 原生 |

---

## 目录结构

```
platforms/tauri/
├── src-tauri/                    # Rust 后端
│   ├── Cargo.toml                # 依赖 aether-core
│   ├── src/
│   │   ├── main.rs               # Tauri 入口
│   │   ├── commands/             # Tauri commands（调用 core）
│   │   │   ├── mod.rs
│   │   │   ├── settings.rs
│   │   │   ├── providers.rs
│   │   │   └── mcp.rs
│   │   ├── tray.rs               # 系统托盘
│   │   ├── shortcuts.rs          # 全局快捷键
│   │   └── error.rs              # 错误处理
│   └── tauri.conf.json           # 窗口配置
├── src/                          # React 前端
│   ├── main.tsx                  # 入口
│   ├── windows/                  # 多窗口入口
│   │   ├── halo/                 # Halo 窗口
│   │   │   ├── main.tsx
│   │   │   ├── HaloWindow.tsx
│   │   │   ├── components/
│   │   │   │   ├── HaloListening.tsx
│   │   │   │   ├── HaloProcessing.tsx
│   │   │   │   ├── HaloSuccess.tsx
│   │   │   │   ├── HaloError.tsx
│   │   │   │   ├── HaloClarification.tsx
│   │   │   │   ├── HaloToolConfirmation.tsx
│   │   │   │   ├── HaloPlanConfirmation.tsx
│   │   │   │   └── ...
│   │   │   └── types.ts
│   │   ├── settings/             # 设置窗口
│   │   │   ├── main.tsx
│   │   │   ├── SettingsWindow.tsx
│   │   │   ├── SettingsSidebar.tsx
│   │   │   ├── tabs/
│   │   │   │   ├── GeneralSettings.tsx
│   │   │   │   ├── ShortcutsSettings.tsx
│   │   │   │   ├── BehaviorSettings.tsx
│   │   │   │   ├── ProvidersSettings.tsx
│   │   │   │   ├── McpSettings.tsx
│   │   │   │   ├── PluginsSettings.tsx
│   │   │   │   ├── AgentSettings.tsx
│   │   │   │   └── ...
│   │   │   └── types.ts
│   │   └── conversation/         # 会话窗口
│   │       └── ...
│   ├── components/               # 共享组件
│   │   └── ui/
│   │       ├── button.tsx
│   │       ├── switch.tsx
│   │       ├── select.tsx
│   │       ├── slider.tsx
│   │       ├── settings-card.tsx
│   │       ├── save-bar.tsx
│   │       ├── status-dot.tsx
│   │       └── ...
│   ├── stores/                   # Zustand stores
│   │   ├── haloStore.ts
│   │   └── settingsStore.ts
│   ├── hooks/                    # 自定义 hooks
│   │   ├── useTauriEvents.ts
│   │   └── useTheme.ts
│   ├── lib/                      # 工具函数
│   │   ├── commands.ts
│   │   ├── errors.ts
│   │   └── utils.ts
│   ├── locales/                  # i18n 翻译文件
│   │   ├── en.json
│   │   └── zh-CN.json
│   └── styles/
│       └── globals.css
├── index.html
├── halo.html
├── settings.html
├── conversation.html
├── package.json
├── tailwind.config.ts
├── vite.config.ts
├── vitest.config.ts
└── tsconfig.json
```

---

## Halo 窗口设计

### 状态枚举（对齐 macOS 15 种状态）

```typescript
type HaloState =
  | { type: 'idle' }
  | { type: 'listening' }
  | { type: 'retrievingMemory' }
  | { type: 'processingWithAI'; provider: string }
  | { type: 'processing'; content: string }
  | { type: 'typewriting'; content: string; progress: number }
  | { type: 'success'; message?: string }
  | { type: 'error'; message: string; canRetry: boolean }
  | { type: 'toast'; message: string; level: 'info' | 'warning' | 'error' }
  | { type: 'clarification'; question: string; options?: string[] }
  | { type: 'conversationInput'; placeholder?: string }
  | { type: 'toolConfirmation'; tool: string; args: Record<string, unknown> }
  | { type: 'planConfirmation'; steps: PlanStep[] }
  | { type: 'planProgress'; steps: PlanStep[]; currentIndex: number }
  | { type: 'taskGraphConfirmation'; graph: TaskGraph }
  | { type: 'taskGraphProgress'; graph: TaskGraph }
  | { type: 'agentPlan'; plan: AgentPlan }
  | { type: 'agentProgress'; progress: AgentProgress }
  | { type: 'agentConflict'; conflict: ConflictInfo };
```

### 组件结构

```
HaloWindow (窗口容器)
└── HaloContainer (状态路由)
    ├── HaloListening        # 脉动紫色圆圈
    ├── HaloProcessing       # 旋转 spinner + 内容
    ├── HaloTypewriting      # 键盘图标 + 进度条
    ├── HaloSuccess          # 绿色勾号 + 弹簧动画
    ├── HaloError            # 错误信息 + 重试/关闭按钮
    ├── HaloToast            # 通知提示
    ├── HaloClarification    # 文本输入或选项列表
    ├── HaloToolConfirmation # 工具信息 + 执行/取消
    ├── HaloPlanConfirmation # 计划步骤列表 + 确认
    ├── HaloPlanProgress     # 计划执行进度
    └── ... (其他状态组件)
```

### 窗口行为

| 行为 | 实现方式 |
|------|----------|
| 光标位置弹出 | `invoke('get_cursor_position')` → `window.setPosition()` |
| 透明背景 | Tailwind `bg-transparent` + Tauri `transparent: true` |
| 无焦点窃取 | Tauri `focus: false` + 不调用 `setFocus()` |
| 自动隐藏 | 成功/错误后延时 `window.hide()` |
| 动态大小 | `useResizeObserver` 监听内容 → `window.setSize()` |

### 窗口配置

```json
{
  "label": "halo",
  "transparent": true,
  "decorations": false,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "visible": false,
  "resizable": false,
  "shadow": false,
  "focus": false,
  "width": 400,
  "height": 300
}
```

---

## 设置窗口设计

### 标签页结构（对齐 macOS 12+ 页面）

```typescript
type SettingsTab =
  | 'general'      // 通用设置
  | 'providers'    // AI 提供商
  | 'generation'   // 生成（图像/视频/音频）
  | 'shortcuts'    // 快捷键
  | 'behavior'     // 行为设置
  | 'memory'       // 内存管理
  | 'search'       // 搜索后端
  | 'mcp'          // MCP 服务器
  | 'skills'       // 技能管理
  | 'plugins'      // 插件管理
  | 'agent'        // Agent 配置
  | 'policies';    // 安全策略

// 分组显示
const tabGroups = [
  { label: '基础', tabs: ['general', 'shortcuts', 'behavior'] },
  { label: 'AI', tabs: ['providers', 'generation', 'memory'] },
  { label: '扩展', tabs: ['mcp', 'plugins', 'skills'] },
  { label: '高级', tabs: ['agent', 'search', 'policies'] },
];
```

### 布局结构

```
┌──────────────────────────────────────────┐
│ Settings                            ✕    │
├────────────┬─────────────────────────────┤
│ 基础       │                             │
│  🔧 General│      [Content Area]         │
│  ⌨️ Shortcuts                            │
│  📝 Behavior                             │
│ ─────────── │                            │
│ AI         │                             │
│  🤖 Providers                            │
│  🎨 Generation                           │
│ ─────────── │                            │
│ 扩展       │                             │
│  🔌 Plugins │                            │
│  🛠 MCP     │                            │
│ ─────────── │                            │
│ 高级       │                             │
│  🤖 Agent   │                            │
└────────────┴─────────────────────────────┘
```

### 关键设置页面

#### General 设置
- 声音效果开关
- 开机启动开关
- 语言选择（系统默认/English/简体中文）
- 更新检查
- 日志查看
- 版本信息

#### Shortcuts 设置
- 替换热键（Double Modifier）
- 追加热键（Double Modifier）
- 命令完成热键（修饰键组合 + 字符键）
- OCR 捕获热键
- 权限请求按钮

#### Behavior 设置
- 输出模式（Typewriter / Instant）
- 打字速度滑块（50-400 chars/sec）
- 速度预览窗口
- PII 数据清洗（邮箱、电话、SSN、信用卡）

#### MCP 设置（Master-Detail 布局）
- 左侧：服务器列表
- 右侧：详情编辑（GUI / JSON 模式）
- 服务器配置：命令、参数、环境变量、权限

#### Plugins 设置
- Git / ZIP 安装方式
- 插件卡片：图标、名称、描述、统计（Skills/Agents/Hooks）
- 启用/禁用开关
- 删除按钮

#### Agent 设置
- 基础配置：启用、确认、并行度、重试、Dry run
- 文件操作：允许/禁止路径、最大文件大小
- 代码执行：沙箱、网络、超时、运行时

---

## 系统集成

### 系统托盘菜单

```
菜单栏状态项 (System Tray)
├─ About Aether
├─ ───────────
├─ Default Provider ──→ 子菜单
│  ├─ OpenAI
│  ├─ Claude
│  └─ Ollama
├─ ───────────
├─ Settings...     Ctrl+,
├─ ───────────
└─ Quit            Ctrl+Q
```

### 全局快捷键

```rust
// 双击修饰键检测（替换/追加模式）
// 命令完成快捷键（如 Ctrl+Alt+/）
// OCR 捕获快捷键（如 Ctrl+Alt+O）
```

### Tauri Commands

```rust
#[tauri::command] async fn process_input(input: String, mode: ProcessMode) -> Result<ProcessResult, String>
#[tauri::command] async fn get_settings() -> Result<Settings, String>
#[tauri::command] async fn save_settings(settings: Settings) -> Result<(), String>
#[tauri::command] async fn get_providers() -> Result<Vec<ProviderConfig>, String>
#[tauri::command] async fn test_provider(provider: ProviderConfig) -> Result<bool, String>
#[tauri::command] async fn list_mcp_servers() -> Result<Vec<McpServerStatus>, String>
#[tauri::command] async fn start_mcp_server(server_id: String) -> Result<(), String>
#[tauri::command] fn get_cursor_position() -> Position
```

---

## 状态管理

### Halo Store

```typescript
interface HaloStore {
  state: HaloState;
  position: { x: number; y: number };
  visible: boolean;

  setState: (state: HaloState) => void;
  show: (position: { x: number; y: number }) => void;
  hide: () => void;
  processInput: (input: string, mode: ProcessMode) => Promise<void>;
  confirmTool: (approved: boolean) => void;
  confirmPlan: (approved: boolean) => void;
  submitClarification: (response: string) => void;
}
```

### Settings Store

```typescript
interface SettingsStore {
  general: GeneralSettings;
  shortcuts: ShortcutsSettings;
  behavior: BehaviorSettings;
  providers: ProviderConfig[];
  mcp: McpSettings;
  plugins: Plugin[];
  agent: AgentSettings;

  isDirty: boolean;
  isLoading: boolean;

  load: () => Promise<void>;
  updateGeneral: (partial: Partial<GeneralSettings>) => void;
  // ... 其他 update 方法
  save: () => Promise<void>;
  discard: () => void;
}
```

---

## 设计系统

### Design Tokens（对齐 macOS）

```typescript
// Spacing
spacing: { xs: '4px', sm: '8px', md: '12px', lg: '16px', xl: '24px' }

// Corner Radius
borderRadius: { small: '4px', medium: '8px', large: '12px', card: '10px' }

// Typography
fontSize: {
  body: '14px',
  caption: '12px',
  heading: '16px',
  title: '20px',
  code: '13px'
}

// Colors (Light/Dark)
colors: {
  textPrimary, textSecondary,
  accentBlue, warning, success, error, info,
  cardBackground, surfaceSecondary, sidebarBackground, border
}
```

### 毛玻璃效果

```css
.glass {
  @apply bg-white/70 dark:bg-black/50 backdrop-blur-xl;
}

.halo-container {
  @apply bg-card-background/80 backdrop-blur-xl rounded-large shadow-lg;
  border: 1px solid hsl(var(--border) / 0.5);
}
```

---

## 构建配置

### Cargo.toml

```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon", "image-png"] }
tauri-plugin-global-shortcut = "2"
tauri-plugin-shell = "2"
tauri-plugin-dialog = "2"
tauri-plugin-fs = "2"
aether-core = { path = "../../../core" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### tauri.conf.json

```json
{
  "productName": "Aether",
  "identifier": "com.aether.app",
  "bundle": {
    "targets": ["nsis", "deb", "appimage"],
    "windows": { "nsis": { "installMode": "currentUser" } }
  }
}
```

### package.json 主要依赖

```json
{
  "dependencies": {
    "@tauri-apps/api": "^2.0.0",
    "react": "^18.3.0",
    "framer-motion": "^11.0.0",
    "zustand": "^4.5.0",
    "react-i18next": "^14.0.0"
  }
}
```

---

## 错误处理

### 错误码枚举

```typescript
enum ErrorCode {
  NETWORK_ERROR,
  API_TIMEOUT,
  INVALID_CONFIG,
  MISSING_API_KEY,
  PROVIDER_ERROR,
  MCP_SERVER_ERROR,
  PLUGIN_ERROR,
  PERMISSION_DENIED,
  UNKNOWN
}
```

### 错误显示

- 可恢复错误：显示重试按钮
- 不可恢复错误：显示关闭按钮
- 错误消息国际化

---

## 测试策略

```
测试层级
├── 单元测试（Rust Core）     # 已有，继续维护
├── 单元测试（React 组件）    # Vitest + React Testing Library
├── 集成测试（Tauri Commands）# Rust 集成测试
└── E2E 测试（可选）          # Playwright + Tauri Driver
```

---

## 实施计划

### Phase 1: 基础骨架
- 创建 platforms/tauri/ 目录结构
- 配置 Tauri 2.0 + Vite + React
- 集成 aether-core 依赖
- 实现系统托盘基础功能
- 验证 Windows/Linux 构建

### Phase 2: Halo 窗口
- 实现透明无边框窗口
- 光标位置弹出逻辑
- 15 种状态组件
- Framer Motion 动画
- 全局快捷键集成

### Phase 3: 设置窗口
- 侧边栏导航框架
- General / Shortcuts / Behavior 页面
- Providers 配置页面
- MCP / Plugins / Agent 复杂页面
- 统一保存栏组件
- 国际化集成

### Phase 4: 系统集成
- 完善托盘菜单
- 快捷键配置持久化
- 开机启动（Windows/Linux）
- 主题跟随系统
- 错误处理完善

### Phase 5: 测试与优化
- 单元测试覆盖
- Windows/Linux 平台测试
- 性能优化（窗口预热、懒加载）
- 包体积优化
- 文档编写

---

## 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| WebView 首次白屏 | 窗口预热 + 骨架屏 |
| Windows 快捷键冲突 | 提供自定义快捷键 |
| Linux 托盘兼容性 | 检测桌面环境，降级处理 |
| 包体积过大 | 代码分割 + Tree shaking |

---

## 附录：与 macOS Swift 版本对比

| 特性 | macOS Swift | Tauri |
|------|-------------|-------|
| 窗口透明 | ✅ 原生 | ✅ WebView2/webkit2gtk |
| 毛玻璃效果 | ✅ NSVisualEffectView | ⚠️ CSS backdrop-blur |
| 系统托盘 | ✅ NSStatusItem | ✅ Tauri tray |
| 全局快捷键 | ✅ Carbon API | ✅ tauri-plugin-global-shortcut |
| 无焦点窃取 | ✅ NSPanel | ⚠️ 需要平台特定代码 |
| 动画流畅度 | ✅ 60fps | ⚠️ 依赖 WebView 性能 |
| 包体积 | ~15MB | ~25MB (含 WebView2) |
