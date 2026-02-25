# 1-2-3-4 架构宪法设计

> Date: 2026-02-24
> Status: Approved & Implemented

## Motivation

Aleph 经历了三个架构阶段：早期单体尝试、中期激进拆分（删除 12000 行代码）、现在的"模块化回归"。架构已趋稳固，但 CLAUDE.md 缺乏一份"宪法"来约束开发决策，导致 AI 助手和开发者可能偏离架构航线。

本设计将架构师的深度复盘提炼为宪法级原则，写入 CLAUDE.md 作为所有后续开发的最高约束。

## 核心概念：生命体叙事 (Living Being Narrative)

Aleph 被定义为一个完整的智能生命体，由四层组成：

| 层 | 隐喻 | 定义 | CLAUDE.md 对应 |
|----|------|------|----------------|
| **Soul (灵魂)** | 进化目标 | 五层涌现架构：从经验之海到多态智能体 | `### 五层涌现架构 (The Soul — 灵魂)` |
| **Skin (皮肤)** | 存在状态 | 产品设计约束 (R5-R7)：菜单栏优先、AI 主动到达、一核多端 | `### R5-R7 产品设计约束` |
| **Skeleton (骨架)** | 工程实现 | 1-2-3-4 模型：大脑、脸、四肢、神经的布局 | `## 🏗️ 1-2-3-4 架构模型 (The Skeleton — 骨架)` |
| **Mind (思维)** | 决策逻辑 | POE 架构 + DDD 领域建模 | `### POE 架构 (The Mind — 思维)` |

一句话定位：

> "Aleph 拥有五层涌现的进化灵魂。在工程上，它由 1 个核心驱动，拥有 2 种交互面，通过 3 类执行系统干涉现实，并由 4 层通讯协议编织成一个完整的智能生命体。"

## 1-2-3-4 架构模型

### 1 Core — 大脑 (The Brain)

Rust Core 只负责三件事：

- **推理规划 (Reasoning)**: 决定下一步该干什么
- **状态管理 (State)**: 维护对话、任务上下文
- **路由分发 (Routing)**: 把任务分发给插件、MCP 或桌面能力层

核心不画界面，不写截图代码。它是纯粹的、轻量的"大脑"。

### 2 Faces — 交互界面 (The Faces)

| 界面 | 角色 | 宿主 |
|------|------|------|
| **统一 Panel (Leptos/WASM)** | 全平台唯一 UI 逻辑实现 | Web、macOS (Swift 壳)、Windows/Linux (Tauri 壳) |
| **社交 Bot 通道 (Gateway)** | 数字世界的身影，永远在线的后台智能 | Telegram、Discord 等 |

### 3 Limbs — 执行系统 (The Limbs)

| 系统 | 角色 | 示例 |
|------|------|------|
| **Native 能力 (The Muscles)** | 直接控制系统 | Bash/Shell、Desktop Bridge (Swift/Tauri-Rust) — "看"(OCR/截图) 和 "动"(点击/输入) |
| **MCP (The External Tools)** | 杠杆效应，调用社区工具 | Playwright、Google Maps 等 |
| **Skills/Plugins (The Expertise)** | 领域知识 | PPT 专家、代码审查助手 |

### 4 Nerves — 通信协议 (The Nerves)

| 编号 | 通道 | 协议 | 用途 |
|------|------|------|------|
| 1 | Core ↔ UI | WebSocket/RPC | 驱动面板展示 |
| 2 | Core ↔ Desktop Bridge | UDS/IPC | 驱动电脑控制 |
| 3 | Core ↔ Gateway | gRPC/NATS | 驱动社交 Bot |
| 4 | Core ↔ MCP | JSON-RPC | 驱动外部插件 |

## 架构红线 (Architectural Redlines)

四条宪法级约束，违反红线的代码不得合入：

### R1. 大脑与四肢绝对分离 (Brain-Limb Separation)

- **禁令**: 严禁在 `core/src` 中直接调用特定平台系统 API (AppKit, Vision, CoreGraphics, windows-rs)
- **原则**: 核心层只定义"能力契约 (Trait)"，物理实现由 Desktop Bridge (Swift/Tauri-Rust) 通过 IPC 提供

### R2. UI 逻辑唯一源 (Single Source of UI Truth)

- **禁令**: 严禁在 Swift 或 Tauri 中实现具有业务逻辑的复杂设置页面、表单或列表
- **原则**: 所有复杂业务 UI 在 Leptos (WASM) 中实现。原生外壳仅负责窗口容器、原生动画和菜单栏

### R3. 核心轻量化 (Core Minimalism)

- **禁令**: 严禁为单一非核心功能在 core 中引入沉重的第三方库
- **原则**: 优先实现为 Skill (Python/Bash) 或 MCP Server。内核只调度，不搬砖
- **备注**: 代码层面的奥卡姆剃刀原则和 Rust 大文件拆分规范与此不冲突

### R4. Interface 层禁止业务逻辑 (I/O-Only Interfaces)

- **禁令**: 禁止在 App/Bot/CLI 中处理数据持久化、记忆检索或任务规划逻辑
- **原则**: Interface 层是"纯 I/O"— 输入转为 JSON-RPC 发给 Server，响应渲染给用户

## 北极星 (North Star)

三条战略方向：

1. **架构已定，填充不推倒** — 当前架构已经稳固，后续工作是"填充"而非"推倒"
2. **标准化桌面操作协议** — 完成 Desktop Bridge 的协议定义，先 Swift 后 Tauri，进度完全可控
3. **Skill 驱动未来** — 架构是骨骼，Skill 才是血肉，它们决定了 Aleph 能帮省多少工作

## 产品设计约束 (R5-R7)

原 "Ghost 美学" 概念已被拆解为具体的产品设计约束，避免哲学化表述误导设计决策：

- **R5. 菜单栏优先，按需展窗**: 默认无 Dock 图标 + 菜单栏常驻 + Halo 浮窗快捷交互。复杂场景使用正常窗口
- **R6. AI 主动到达**: 减少用户切换上下文成本，不打扰用户但不拒绝必要 UI
- **R7. 一核多端**: Rust Core 唯一大脑，Leptos/WASM 统一 UI，原生壳负责窗口和系统集成

## 架构复盘

### 三个阶段

1. **单体尝试**: 所有功能写在一起，Core 膨胀
2. **激进拆分**: 删除 12000 行代码，剔除大脑中的"肌肉组织"
3. **模块化回归**: 1-2-3-4 模型确立，边界清晰，填充而非推倒

### 解决的核心问题

1. **生产力瓶颈**: Leptos 统一 UI，不再在三个前端框架里疲于奔命
2. **能力边界**: 放弃在 Rust 里死磕跨平台 UI 调用，使用 Swift 和原生驱动实现顶级环境感知力和执行力
3. **生存空间**: Gateway 让 Aleph 既是本地桌面秘书，也是云端社交管家

### 架构评价

> 架构完整，边界清晰，具备极强的工程落地可行性。

## Implementation

Changes applied to `CLAUDE.md` in commit `effc724e`:

- Added Living Being narrative as opening statement under `## 🔮 核心哲学`
- Added role annotations (Soul/Skin/Mind) to philosophy subsection headers
- Updated `Native-First` → `Native-Powered` in Ghost aesthetics table
- Replaced `## 🏗️ 架构概览` + `### Server-Centric 架构` with `## 🏗️ 1-2-3-4 架构模型`
- Added `## 🛑 架构红线` with 4 redlines (R1-R4)
- Added `## 🧭 北极星` with 3 strategic directives
- Updated Session Context to match new architecture narrative
