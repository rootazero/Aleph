# Aether Window System - Complete Design Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 完整的 Aether 窗口系统设计与实现规划,包括 Liquid Glass 视觉效果、统一对话窗口、命令/主题列表、Halo 状态窗口等所有 UI 组件。

**Architecture:** 基于 Metal + SwiftUI 的分层架构,Metal 层处理 Liquid Glass 视觉效果(极光背景、气泡融合、玻璃折射),SwiftUI 层处理文字内容和交互逻辑。所有窗口共享统一的"幽灵"美学设计语言。

**Tech Stack:** Metal Shading Language, MetalKit (MTKView), SwiftUI, Core Graphics (壁纸采样), Accelerate (K-means)

**Design Philosophy:**
- **Invisible First**: 无 Dock 图标,无常驻窗口,仅后台进程 + 菜单栏
- **De-GUI**: 短暂 UI 在光标处出现后消散
- **Frictionless**: AI 智能直接在光标处,无上下文切换
- **Native-First**: 100% 原生代码 - Rust 核心 + 平台特定 UI

---

## 目录

- [总体架构](#总体架构)
- [窗口组件详细设计](#窗口组件详细设计)
  - [1. 统一对话窗口 (UnifiedConversationWindow)](#1-统一对话窗口-unifiedconversationwindow)
  - [2. Liquid Glass 渲染层](#2-liquid-glass-渲染层)
  - [3. 输入区域 (InputAreaView)](#3-输入区域-inputareaview)
  - [4. 对话消息列表 (ConversationAreaView)](#4-对话消息列表-conversationareaview)
  - [5. 命令列表 (/CommandListView)](#5-命令列表-commandlistview)
  - [6. 主题列表 (//TopicListView)](#6-主题列表-topiclistview)
  - [7. Halo 状态窗口 (HaloWindow)](#7-halo-状态窗口-halowindow)
- [交互设计](#交互设计)
- [状态管理](#状态管理)
- [实施路线图](#实施路线图)

---

## 总体架构

### 窗口层级结构

```
Aether Window System
├── HaloWindow (level: .floating)
│   ├── 总是位于最上层
│   ├── 光标位置的临时状态提示
│   └── 不抢焦点,短暂显示后自动消失
│
└── UnifiedConversationWindow (level: .normal)
    ├── Metal 渲染层 (LiquidGlassMetalView)
    │   ├── Aurora Background Shader
    │   ├── Metaball Fusion Shader
    │   └── Glass Refraction Shader
    │
    └── SwiftUI 内容层 (透明覆盖)
        ├── ConversationAreaView (对话消息列表)
        │   ├── MessageBubbleView (用户/AI 消息气泡)
        │   ├── PlanConfirmationBubbleView (计划确认)
        │   ├── UserInputBubbleView (用户输入请求)
        │   └── BubbleGeometryReporter (几何信息上报)
        │
        ├── CommandListView (/命令列表)
        │   ├── 快速命令搜索与选择
        │   └── 键盘导航支持
        │
        ├── TopicListView (//主题列表)
        │   ├── 主题历史浏览
        │   └── 主题切换
        │
        └── InputAreaView (输入区域)
            ├── 文本输入框
            ├── 附件预览
            ├── 提交/取消按钮
            └── 输入焦点状态上报
```

### 渲染流水线

```
SwiftUI Layer (UI & Interaction)
    ↓ Geometry & State
Metal Layer (Liquid Glass Rendering)
    ↓ Triple-Pass Rendering
GPU (Final Composite)
```

**Triple-Pass Rendering Pipeline:**
1. **Pass 1: Aurora Background** → Texture A
2. **Pass 2: Metaball Fusion** → Texture B (uses Texture A + Bubble Data)
3. **Pass 3: Final Composite** → Screen (blends Texture A + B)

---

## 窗口组件详细设计

### 1. 统一对话窗口 (UnifiedConversationWindow)

**文件:** `platforms/macos/Aether/Sources/MultiTurn/UnifiedConversationWindow.swift`

#### 职责
- 管理对话窗口的生命周期和定位
- 协调 Metal 渲染层与 SwiftUI 内容层
- 处理窗口尺寸动态调整(保持底部锚点不变)
- 处理 ESC 键取消和焦点管理

#### 关键特性
```swift
// 窗口配置
- size: 800x(动态高度)
- styleMask: [.borderless, .fullSizeContentView]
- level: .normal
- backgroundColor: .clear (透明背景)
- isOpaque: false
- alphaValue: 0 (初始隐藏,淡入显示)

// 定位策略
- 输入框底部锚定在屏幕 30% 高度处
- 向上扩展以容纳内容
- 内容高度变化时保持底部不动

// 状态
enum DisplayState {
    case empty              // 仅输入框
    case conversation       // 对话消息列表
    case commandList(String) // 命令列表 (/前缀)
}
```

#### 高度计算逻辑
```swift
// 基础高度
baseHeight = inputAreaHeight (60pt) + padding (32pt) = 92pt

// 动态内容高度
switch displayState {
case .empty:
    totalHeight = baseHeight
case .conversation:
    totalHeight = baseHeight + min(messagesHeight, 600pt)
case .commandList(prefix):
    totalHeight = baseHeight + min(listHeight, 600pt)
}

// 最大内容高度限制: 600pt
```

#### 窗口生命周期
```
Show:
  1. 计算初始高度
  2. 定位在屏幕 30% 高度处
  3. alphaValue 0→1 淡入 (0.15s)
  4. 激活并成为焦点

Update:
  1. 监听 viewModel.onHeightChanged
  2. 计算新高度
  3. 保持底部锚点,向上/向下调整
  4. 动画过渡 (setFrame animate: true)

Hide:
  1. alphaValue 1→0 淡出 (0.15s)
  2. orderOut(nil)
  3. viewModel.reset()
```

---

### 2. Liquid Glass 渲染层

**文件路径:**
```
platforms/macos/Aether/Sources/LiquidGlass/
├── Metal/
│   ├── Shaders/
│   │   ├── LiquidGlassShaders.metal      # 三层 Shader 实现
│   │   └── ShaderTypes.h                 # Swift/Metal 共享类型
│   ├── LiquidGlassRenderer.swift         # Metal 渲染器
│   └── LiquidGlassMetalView.swift        # SwiftUI 封装
├── ColorSampling/
│   ├── WallpaperColorSampler.swift       # 壁纸色采样
│   └── DominantColorExtractor.swift      # K-means 主色提取
├── Physics/
│   └── BubbleFusionCalculator.swift      # 气泡融合权重计算
└── LiquidGlassConfiguration.swift        # 全局配置
```

#### Shader 层次结构

**Layer 1: Aurora Background (auroraBackgroundFragment)**
- 输入: time, dominantColors[5], accentColor, breathPhase
- 输出: Texture A (极光背景)
- 效果:
  - 3D 柏林噪声 (Perlin Noise) 生成流动场
  - FBM (Fractal Brownian Motion) 4 octaves
  - 混合 5 种主色 + accent color
  - 呼吸动画 (1.0 + 0.15 * sin(breathPhase))
  - 边缘渐变淡出

**Layer 2: Metaball Fusion (metaballFusionFragment)**
- 输入: Texture A, BubbleData[], time, hoveredIndex, scrollVelocity
- 输出: Texture B (气泡融合层)
- 效果:
  - Metaball SDF (Signed Distance Field) 计算
  - 多气泡势能场叠加
  - 融合阈值动态调整(基于滚动速度)
  - Fresnel 边缘高光
  - 顶部高光(模拟光源)
  - UV 扭曲实现折射

**Layer 3: Final Composite (liquidGlassCompositeFragment)**
- 输入: Texture A, Texture B
- 输出: Screen (最终画面)
- 效果:
  - Alpha blending: aurora + bubble layer
  - 保持透明度层次

#### Bubble Geometry Reporting

**SwiftUI → Metal 数据流:**
```swift
MessageBubbleView
    ↓ .reportBubbleGeometry(id, isUser, timestamp, index)
PreferenceKey Collector
    ↓ onPreferenceChange
BubbleDataCollector
    ↓ Calculate Fusion Weights
LiquidGlassRenderer.updateBubbles([BubbleData])
    ↓ Copy to MTLBuffer
GPU Shader (metaballPotential)
```

**BubbleData Structure:**
```c
typedef struct {
    vector_float2 center;        // 气泡中心 (屏幕坐标)
    vector_float2 size;          // 气泡尺寸
    float cornerRadius;          // 圆角半径
    float fusionWeight;          // 融合权重 (0-1)
    float timestamp;             // 时间戳
    bool isUser;                 // 是否为用户消息
    bool isHovered;              // 是否悬停
    bool isPressed;              // 是否按下
} BubbleData;
```

#### 壁纸色采样 (Wallpaper Color Sampling)

**采样策略:**
1. 截取主屏幕壁纸 (CGWindowListCreateImage)
2. 下采样至 32x32 提升性能
3. K-means 聚类提取 5 种主色
4. 按 vibrancy (饱和度 × 亮度) 排序
5. 混合系统 accent color (40% accent, 60% wallpaper)

**更新触发:**
- 定时采样: 每 5 秒
- 窗口移动: NSWindow.didMoveNotification
- 壁纸更换: com.apple.desktop.background.changed

**颜色过渡:**
- 平滑插值: 30 steps over 0.8s
- Lerp: `color = prev * (1 - t) + target * t`

#### 气泡融合计算 (Bubble Fusion Calculator)

**融合权重影响因素:**
1. **距离因素** (主要):
   - fusionStartDistance: 60px (开始融合)
   - fusionCompleteDistance: 8px (完全融合)
   - 平滑插值: smoothstep((d - complete) / (start - complete))

2. **时间因素** (次要):
   - temporalWindow: 5 秒
   - 时间差越小,融合越强
   - 权重: timeFusion * 0.3

3. **角色因素**:
   - sameTurnBonus: 0.2 (不同角色间额外融合)
   - 用户与 AI 消息更易融合

4. **交互因素**:
   - hoverIsolationFactor: 0.5 (悬停时减少融合)
   - scrollVelocity: 影响融合阈值 (快速滚动时减少融合)

**计算公式:**
```swift
fusionWeight = min(
    distanceFusion + timeFusion * 0.3 + sameTurnBonus,
    1.0
)
if isHovered {
    fusionWeight *= hoverIsolationFactor
}
```

---

### 3. 输入区域 (InputAreaView)

**文件:** `platforms/macos/Aether/Sources/MultiTurn/Views/InputAreaView.swift`

#### 布局结构
```
InputAreaView
└── HStack
    ├── TextEditor (文本输入)
    │   ├── placeholder: "Ask or /command"
    │   ├── 自动高度调整 (最小 1 行,最大 6 行)
    │   └── 提交快捷键: ⌘+Return
    │
    ├── AttachmentGridView (附件预览)
    │   ├── 最多 5 个附件
    │   ├── 缩略图 + 删除按钮
    │   └── Drag & Drop 支持
    │
    └── VStack (操作按钮)
        ├── SubmitButton (✓)
        └── CancelButton (✕)
```

#### 视觉效果
- **背景**: 透明,让 Metal Glass 层显示
- **边框**:
  - 正常: 微弱白色渐变边框
  - 焦点: primary.opacity(0.2)
  - 拖拽目标: cyan.gradient (2px)
- **Geometry Reporting**: 上报输入框几何信息用于 Glass 渲染

#### 交互状态
```swift
@State var text: String = ""
@State var isFocused: Bool = false
@State var isTargeted: Bool = false  // Drag & Drop
@State var attachments: [PendingAttachment] = []

// 状态上报到 Metal 层
.reportBubbleGeometry(
    id: "input-area",
    isUser: true,
    timestamp: Date().timeIntervalSince1970,
    index: -1  // 特殊标记
)

// 焦点状态触发 Glass Glow
.onChange(of: isFocused) {
    inputFocusedBinding.wrappedValue = isFocused
}
```

#### 附件处理
```swift
struct PendingAttachment: Identifiable {
    let id: UUID
    let url: URL
    let type: AttachmentType
    let previewImage: NSImage?
}

enum AttachmentType {
    case image      // PNG, JPG
    case document   // PDF, TXT
    case code       // Swift, Python, etc.
}
```

#### 快捷键
- **⌘+Return**: 提交
- **ESC**: 取消 (清空输入或关闭窗口)
- **⌘+V**: 粘贴图片
- **拖拽**: 文件拖入添加附件

---

### 4. 对话消息列表 (ConversationAreaView)

**文件:** `platforms/macos/Aether/Sources/MultiTurn/Views/ConversationAreaView.swift`

#### 布局结构
```
ConversationAreaView
└── ScrollView
    └── LazyVStack (spacing: 8)
        ├── ForEach(messages)
        │   └── MessageBubbleView
        │       ├── RichMessageContentView (文本/Markdown)
        │       ├── ToolCallPartView (工具调用)
        │       ├── AttachmentGridView (附件)
        │       └── .reportBubbleGeometry(...)
        │
        ├── PlanConfirmationBubbleView (计划确认)
        │   ├── 计划标题与任务列表
        │   ├── 风险等级标识
        │   └── Approve/Reject 按钮
        │
        └── UserInputBubbleView (用户输入请求)
            ├── 问题文本
            ├── 选项列表 (单选/多选)
            └── Submit 按钮
```

#### 消息类型

**1. 文本消息 (MessageBubbleView)**
```swift
struct ConversationMessage: Identifiable {
    let id: String
    let role: Role  // user / assistant
    let content: [ContentBlock]
    let timestamp: Date?
}

enum ContentBlock {
    case text(String)
    case toolUse(ToolUseBlock)
    case toolResult(ToolResultBlock)
}
```

**样式:**
- **用户消息**: 右对齐,淡蓝色玻璃气泡
- **AI 消息**: 左对齐,浅灰玻璃气泡
- **融合效果**: 相邻消息通过 Metaball 融合

**2. 计划确认消息 (PlanConfirmationBubbleView)**
```swift
struct PendingPlanConfirmation {
    let planId: String
    let title: String
    let tasks: [(id: String, name: String, riskLevel: String)]
}
```

**布局:**
```
╭─────────────────────────────────────╮
│ 📋 Plan Confirmation                │
│                                     │
│ Title: Implement Authentication     │
│                                     │
│ Tasks:                              │
│  1. [High] Modify login flow        │
│  2. [Med]  Add JWT validation       │
│  3. [Low]  Update UI                │
│                                     │
│ [✓ Approve]  [✕ Reject]             │
╰─────────────────────────────────────╯
```

**3. 用户输入请求 (UserInputBubbleView)**
```swift
struct PendingUserInputRequest {
    let requestId: String
    let question: String
    let options: [String]
}
```

**布局:**
```
╭─────────────────────────────────────╮
│ ❓ User Input Required               │
│                                     │
│ Question: Which database to use?    │
│                                     │
│ Options:                            │
│  ○ PostgreSQL                       │
│  ○ MySQL                            │
│  ● MongoDB (selected)               │
│                                     │
│ [Submit]                            │
╰─────────────────────────────────────╯
```

#### 滚动行为
```swift
// 滚动监听
ScrollViewReader { proxy in
    LazyVStack { ... }
        .onChange(of: messages) {
            // 自动滚动到底部
            proxy.scrollTo(messages.last?.id, anchor: .bottom)
        }
}

// 滚动速度上报 (用于 Fusion 调整)
.onAppear { startTrackingScrollVelocity() }
.onDisappear { stopTrackingScrollVelocity() }
```

#### 几何上报
每个 MessageBubbleView 必须上报几何信息:
```swift
.reportBubbleGeometry(
    id: message.id,
    isUser: message.role == .user,
    timestamp: message.timestamp?.timeIntervalSince1970 ?? 0,
    index: messageIndex
)
```

---

### 5. 命令列表 (/CommandListView)

**文件:** `platforms/macos/Aether/Sources/MultiTurn/Views/CommandListView.swift`

#### 布局结构
```
CommandListView
└── VStack
    ├── SearchField ("Search commands...")
    │   └── .focused($searchFocused)
    │
    └── List
        └── ForEach(filteredCommands)
            └── CommandRowView
                ├── Icon
                ├── Name ("/commit")
                └── Description
```

#### 触发条件
- 用户输入 "/" 开头
- 显示所有可用命令
- 实时过滤: 输入 "/co" → 显示 "/commit", "/config"

#### 命令结构
```swift
struct Command: Identifiable {
    let id: String
    let name: String           // "/commit"
    let description: String    // "Create a git commit"
    let category: String       // "Git"
    let icon: String           // SF Symbol
}
```

#### 样式
- **背景**: 透明,显示 Liquid Glass
- **行高**: 44pt
- **选中状态**: 高亮边框 + 轻微缩放动画
- **键盘导航**: ↑↓ 选择, Return 确认, ESC 取消

#### 交互流程
```
1. 用户输入 "/"
   → displayState = .commandList("/")
   → 窗口展开显示命令列表

2. 继续输入 "/co"
   → 过滤命令: ["/commit", "/config"]
   → 实时更新列表

3. 按 Return
   → 执行选中命令
   → 关闭窗口

4. 按 ESC
   → 退出命令模式
   → displayState = .empty
```

---

### 6. 主题列表 (//TopicListView)

**文件:** `platforms/macos/Aether/Sources/MultiTurn/Views/TopicListView.swift` (需新建)

#### 布局结构
```
TopicListView
└── VStack
    ├── Header ("Recent Topics")
    │
    └── List
        └── ForEach(filteredTopics)
            └── TopicRowView
                ├── Icon (📁)
                ├── Title
                ├── Last Message Preview
                └── Timestamp
```

#### 主题结构
```swift
struct Topic: Identifiable {
    let id: String
    let title: String
    let createdAt: Date
    let lastMessageAt: Date
    let messageCount: Int
    let preview: String  // 最后一条消息预览
}
```

#### 触发条件
- 用户输入 "//" 开头
- 显示所有主题历史
- 实时过滤: 输入 "//auth" → 过滤包含 "auth" 的主题

#### 样式
- **背景**: 透明,显示 Liquid Glass
- **行高**: 56pt (比命令行稍高,容纳预览文本)
- **布局**:
  ```
  ╭────────────────────────────────╮
  │ 📁 Authentication Refactor     │
  │ "Let's update the login..."    │
  │ 2h ago · 12 messages           │
  ╰────────────────────────────────╯
  ```

#### 交互流程
```
1. 用户输入 "//"
   → displayState = .commandList("//")
   → 窗口展开显示主题列表

2. 选择主题
   → 加载该主题的对话历史
   → displayState = .conversation
   → 显示历史消息

3. ESC 取消
   → 退出主题模式
   → displayState = .empty
```

---

### 7. Halo 状态窗口 (HaloWindow)

**文件:** `platforms/macos/Aether/Sources/HaloWindow.swift`

#### 定位策略
- **level**: .floating (总是在最上层)
- **位置**: 光标位置偏移 (x+20, y+20)
- **尺寸**: 300x200 (根据状态动态调整)
- **焦点**: 不抢焦点 (canBecomeKey: false)
- **生命周期**: 短暂显示 (1-3 秒) 后自动淡出

#### 状态类型

**1. Idle (空闲)**
- 不显示窗口

**2. Listening (监听)**
```
╭───────────────╮
│   🎤          │
│  Listening... │
╰───────────────╯
```
- 显示时长: 持续显示直到识别完成

**3. Thinking (思考)**
```
╭───────────────╮
│   ◐          │
│  Thinking...  │
╰───────────────╯
```
- 16x16 紫色旋转动画
- 显示时长: 持续显示直到响应开始

**4. Responding (响应)**
```
╭─────────────────────────────╮
│ Let me help you with that...│
│ ▌                           │
╰─────────────────────────────╯
```
- 打字机效果显示响应文本
- 光标闪烁动画
- 自动换行

**5. Toast (提示)**
```
╭─────────────────╮
│ ✓ Task Complete │
╰─────────────────╯
```
- 成功/错误/信息提示
- 显示时长: 2 秒
- 图标: ✓ (成功), ✕ (错误), ℹ (信息)

**6. Error (错误)**
```
╭───────────────────────────╮
│ ✕ Error                   │
│ Network connection failed │
│                           │
│ [Retry]  [Dismiss]        │
╰───────────────────────────╯
```
- 显示错误信息
- 提供操作按钮
- 可交互 (canBecomeKey: true)

**7. ToolConfirmation (工具确认)**
```
╭────────────────────────────╮
│ 🔧 Tool Use Confirmation   │
│ Execute: git commit -m "…" │
│                            │
│ [Allow]  [Deny]            │
╰────────────────────────────╯
```
- 危险操作前确认
- 可交互 (canBecomeKey: true)

**8. PlanProgress (计划进度)**
```
╭──────────────────────────╮
│ 📋 Executing Plan (2/5)  │
│ ● Setup database         │
│ ◐ Create migrations      │
│ ○ Update models          │
│ ○ Write tests            │
│ ○ Deploy                 │
╰──────────────────────────╯
```
- 显示多步骤计划执行进度
- 实时更新状态

#### 视觉效果
- **背景**: 半透明毛玻璃
- **阴影**: color: .black.opacity(0.2), radius: 15
- **边框**: 白色渐变边框
- **动画**:
  - 淡入/淡出: 0.2s
  - 旋转动画: 0.3 rad/s
  - 高度变化: 平滑过渡 0.25s

#### 状态机
```swift
enum HaloState {
    case idle
    case listening
    case thinking
    case responding(text: String, progress: Double)
    case toast(message: String, type: ToastType)
    case error(message: String, actions: [HaloAction])
    case toolConfirmation(ToolConfirmationData)
    case planProgress(PlanProgressData)
}
```

---

## 交互设计

### 窗口显示流程

**1. 唤起对话窗口**
```
用户按下快捷键 (⌥+Space)
    ↓
AppDelegate.showUnifiedConversation()
    ↓
计算窗口位置 (屏幕中心,底部锚定 30%)
    ↓
UnifiedConversationWindow.showPositioned()
    ↓
淡入动画 (0.15s, alphaValue 0→1)
    ↓
聚焦输入框
```

**2. 输入处理**
```
用户输入文本
    ↓
实时检测前缀
    ├─ "/" → displayState = .commandList
    ├─ "//" → displayState = .commandList (topics)
    └─ 普通文本 → displayState = .empty
    ↓
窗口高度动态调整 (保持底部锚点)
```

**3. 提交与响应**
```
用户按 ⌘+Return
    ↓
viewModel.submitInput()
    ↓
onSubmit?(text, attachments)
    ↓
HaloWindow 显示 "Thinking..."
    ↓
Rust Core 处理
    ↓
流式响应返回
    ├─ HaloWindow 切换到 "Responding"
    └─ 对话窗口添加消息气泡
    ↓
Metal 层更新气泡几何
    ↓
Liquid Glass 效果实时渲染
```

**4. 关闭窗口**
```
用户按 ESC 或点击取消
    ↓
viewModel.handleEscape()
    ↓
UnifiedConversationWindow.hide()
    ↓
淡出动画 (0.15s, alphaValue 1→0)
    ↓
orderOut(nil)
    ↓
viewModel.reset()
```

### 焦点管理

**原则:**
- **UnifiedConversationWindow**: 可成为焦点 (canBecomeKey: true)
- **HaloWindow**: 仅在交互状态可成为焦点 (error, toolConfirmation, planConfirmation)
- **输入框**: 窗口显示时自动聚焦
- **ESC**: 总是优先处理,清空输入或关闭窗口

### 键盘快捷键

| 快捷键 | 功能 | 作用范围 |
|--------|------|----------|
| ⌥+Space | 唤起/隐藏对话窗口 | 全局 |
| ⌘+Return | 提交输入 | 输入框 |
| ESC | 取消/关闭 | 对话窗口 |
| ↑/↓ | 导航列表 | 命令/主题列表 |
| ⌘+V | 粘贴图片 | 输入框 |
| ⌘+, | 打开设置 | 全局 |

---

## 状态管理

### UnifiedConversationViewModel

**职责:**
- 管理窗口显示状态 (empty, conversation, commandList)
- 管理消息列表
- 处理用户输入
- 协调 Rust Core 调用

**状态:**
```swift
@Observable
class UnifiedConversationViewModel {
    // Display state
    var displayState: DisplayState = .empty

    // Content
    var messages: [ConversationMessage] = []
    var commands: [Command] = []
    var filteredTopics: [Topic] = []

    // Input
    var inputText: String = ""
    var attachments: [PendingAttachment] = []

    // Pending interactions
    var pendingPlanConfirmation: PendingPlanConfirmation?
    var pendingUserInputRequest: PendingUserInputRequest?

    // Progress tracking
    var planSteps: [String] = []
    var currentToolCall: String?

    // Callbacks
    var onHeightChanged: ((CGFloat) -> Void)?
    var onSubmit: ((String, [PendingAttachment]) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?
}
```

**方法:**
```swift
// Input handling
func submitInput()
func handleEscape()
func addAttachment(_ url: URL)
func removeAttachment(_ id: UUID)

// Message management
func addMessage(_ message: ConversationMessage)
func clearMessages()

// Plan confirmation
func setPendingPlanConfirmation(_ confirmation: PendingPlanConfirmation, core: AetherCore)
func approvePlan()
func rejectPlan()

// User input request
func setPendingUserInputRequest(requestId: String, question: String, options: [String], core: AetherCore)
func submitUserInput(_ selectedOptions: [String])

// Progress tracking
func setPlanSteps(_ steps: [String])
func setToolCallStarted(_ toolName: String)
func setToolCallCompleted()
func setToolCallFailed()
```

### BubbleDataCollector

**职责:**
- 收集 SwiftUI 层的气泡几何信息
- 计算融合权重
- 更新 Metal 层的 BubbleData 数组

**状态:**
```swift
@Observable
class BubbleDataCollector {
    var bubbles: [BubbleData] = []
    var hoveredIndex: Int = -1

    private var geometries: [BubbleGeometry] = []
    private let startTime: TimeInterval
}
```

**方法:**
```swift
func updateGeometries(_ newGeometries: [BubbleGeometry], viewportSize: CGSize)
func setHoveredBubble(id: String?)
private func recalculateBubbles(viewportSize: CGSize, scrollVelocity: Float)
```

### WallpaperColorSampler

**职责:**
- 采样系统壁纸主色
- 监听壁纸/窗口位置变化
- 平滑过渡颜色

**状态:**
```swift
@Observable
class WallpaperColorSampler {
    @Published var accentColor: SIMD4<Float>
    @Published var dominantColors: [SIMD4<Float>]

    private var sampleTimer: Timer?
    private var targetColors: [SIMD4<Float>]
    private var transitionProgress: Float
}
```

---

## 实施路线图

### Phase 1: Liquid Glass 基础设施 ✅ (已完成)
- [x] 创建 Metal Shader 类型定义 (ShaderTypes.h)
- [x] 实现三层 Shader (aurora, metaball, composite)
- [x] 创建 LiquidGlassRenderer (triple buffering)
- [x] 创建 LiquidGlassMetalView (SwiftUI 封装)
- [x] 实现壁纸色采样 (K-means)
- [x] 实现气泡融合计算器
- [x] 创建 BubbleGeometryReporter
- [x] 集成 Metal 层到 UnifiedConversationView

### Phase 2: 窗口组件完善 (当前)

#### Task 2.1: 优化 InputAreaView
- [ ] 移除旧的 VisualEffectBackground
- [ ] 实现透明背景让 Glass 层显示
- [ ] 添加 Geometry Reporting
- [ ] 实现焦点状态上报
- [ ] 优化附件预览布局
- [ ] 添加拖拽目标高亮

#### Task 2.2: 优化 MessageBubbleView
- [ ] 移除旧的 .glassBubble modifier
- [ ] 添加 Geometry Reporting
- [ ] 实现消息索引传递
- [ ] 优化 RichMessageContentView 渲染
- [ ] 添加悬停状态检测

#### Task 2.3: 实现 TopicListView
- [ ] 创建 Topic 数据结构
- [ ] 创建 TopicRowView 组件
- [ ] 实现主题列表布局
- [ ] 添加搜索过滤逻辑
- [ ] 集成到 UnifiedConversationView
- [ ] 实现 "//" 触发逻辑

#### Task 2.4: 优化 ConversationAreaView
- [ ] 实现滚动速度监听
- [ ] 实现滚动偏移上报
- [ ] 优化 LazyVStack 性能
- [ ] 添加自动滚动到底部
- [ ] 实现消息索引管理

### Phase 3: 交互动效增强

#### Task 3.1: Aurora Background 调优
- [ ] 调整噪声参数 (scale, octaves)
- [ ] 优化颜色混合算法
- [ ] 实现呼吸动画 fine-tuning
- [ ] 添加边缘渐变优化
- [ ] 性能测试 (60 FPS 目标)

#### Task 3.2: Metaball Fusion 细化
- [ ] 调整融合阈值参数
- [ ] 优化 SDF 计算性能
- [ ] 实现滚动速度响应
- [ ] 添加悬停隔离效果
- [ ] 测试多气泡场景 (10+ 气泡)

#### Task 3.3: Glass Refraction 增强
- [ ] 优化 Fresnel 计算
- [ ] 添加顶部高光动态调整
- [ ] 实现 UV 扭曲参数调优
- [ ] 添加深度感渐变
- [ ] 测试不同壁纸场景

#### Task 3.4: 交互响应实现
- [ ] 实现悬停状态传递 (SwiftUI → Metal)
- [ ] 实现按下状态传递
- [ ] 添加点击涟漪效果
- [ ] 实现输入焦点 glow
- [ ] 添加滚动惯性效果

### Phase 4: HaloWindow 状态增强

#### Task 4.1: 实现新状态类型
- [ ] 实现 PlanProgress 状态
- [ ] 优化 ToolConfirmation 布局
- [ ] 添加多步骤进度指示器
- [ ] 实现状态切换动画
- [ ] 添加自动隐藏逻辑

#### Task 4.2: 视觉效果优化
- [ ] 优化玻璃背景效果
- [ ] 添加阴影动画
- [ ] 实现打字机效果优化
- [ ] 添加旋转动画 fine-tuning
- [ ] 实现高度变化平滑过渡

### Phase 5: 性能优化

#### Task 5.1: Metal 渲染优化
- [ ] 实现 LOD (Level of Detail) 系统
- [ ] 优化 Shader 计算复杂度
- [ ] 实现动态帧率调整
- [ ] 添加 GPU 占用监控
- [ ] 实现降级策略 (低端设备)

#### Task 5.2: SwiftUI 性能优化
- [ ] 优化 LazyVStack 渲染
- [ ] 实现消息虚拟化 (超过 50 条)
- [ ] 优化 Geometry Reporting 频率
- [ ] 减少不必要的重绘
- [ ] 添加性能监控工具

#### Task 5.3: 内存管理
- [ ] 实现 Texture 缓存管理
- [ ] 优化 Buffer 复用
- [ ] 添加内存占用监控
- [ ] 实现旧消息释放策略
- [ ] 测试长时间运行稳定性

### Phase 6: 测试与完善

#### Task 6.1: 功能测试
- [ ] 测试所有窗口状态切换
- [ ] 测试键盘快捷键
- [ ] 测试拖拽附件
- [ ] 测试命令/主题列表
- [ ] 测试计划确认流程

#### Task 6.2: 视觉测试
- [ ] 测试不同壁纸场景
- [ ] 测试不同屏幕尺寸
- [ ] 测试多显示器场景
- [ ] 测试暗色/亮色模式
- [ ] 测试边缘情况 (超长消息等)

#### Task 6.3: 性能测试
- [ ] 测试 60 FPS 稳定性
- [ ] 测试 GPU 占用
- [ ] 测试内存占用
- [ ] 测试电池消耗
- [ ] 测试多窗口场景

---

## 配置参数参考

### LiquidGlassConfiguration

```swift
struct LiquidGlassConfiguration {
    // Animation
    static let auroraFlowSpeed: Float = 0.1
    static let auroraNoiseScale: Float = 2.0
    static let breathPeriod: Float = 4.0
    static let breathAmplitude: Float = 0.15

    // Fusion
    static let fusionStartDistance: Float = 60
    static let fusionCompleteDistance: Float = 8
    static let temporalWindow: Float = 5.0
    static let sameTurnBonus: Float = 0.2

    // Glass
    static let transparency: Float = 0.85
    static let refractionStrength: Float = 0.02
    static let fresnelIntensity: Float = 0.6
    static let fresnelPower: Float = 2.0

    // Color
    static let accentBlendRatio: Float = 0.4
    static let sampleInterval: TimeInterval = 5.0
    static let transitionDuration: TimeInterval = 0.8

    // Performance
    static let maxBubbles: Int = 50
    static let targetFrameRate: Int = 60
}
```

### Window Layout Constants

```swift
enum Layout {
    static let windowWidth: CGFloat = 800
    static let inputAreaHeight: CGFloat = 60
    static let maxContentHeight: CGFloat = 600
    static let bottomAnchorRatio: CGFloat = 0.30  // 30% from bottom

    static let bubbleSpacing: CGFloat = 8
    static let bubbleCornerRadius: CGFloat = 12
    static let bubblePadding: CGFloat = 12

    static let commandRowHeight: CGFloat = 44
    static let topicRowHeight: CGFloat = 56
}
```

---

## 总结

这份设计规划涵盖了 Aether 窗口系统的完整架构:

1. **Liquid Glass 视觉效果**: Metal 三层渲染管线,实现极光背景、气泡融合、玻璃折射
2. **统一对话窗口**: 动态高度调整,底部锚点定位,支持多种显示模式
3. **窗口组件**: 输入区域、对话列表、命令列表、主题列表,所有组件共享 Glass 美学
4. **Halo 状态窗口**: 光标位置临时状态提示,8 种状态类型
5. **交互设计**: 完整的焦点管理、键盘快捷键、状态机流转
6. **状态管理**: ViewModel + Collector 架构,清晰的职责分离
7. **实施路线图**: 6 个 Phase,从基础设施到性能优化

**下一步行动:**
1. 执行 Phase 2: 窗口组件完善 (优化 InputAreaView, MessageBubbleView, 实现 TopicListView)
2. 执行 Phase 3: 交互动效增强 (Aurora, Metaball, Glass 调优)
3. 测试与迭代

**原则:**
- 保持"幽灵"美学: 短暂、透明、无干扰
- 60 FPS 流畅性优先
- 原生体验: 键盘优先,快捷键友好
- 渐进增强: 低端设备降级策略
