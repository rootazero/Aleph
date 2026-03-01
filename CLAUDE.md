<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

# CLAUDE.md

> *"这是人类历史上第一次，赋予了机器的灵魂一个躯壳。"*
> — 攻壳机动队 / Ghost in the Shell

This file provides guidance to Claude Code when working with code in this repository.

---

## 🔮 核心哲学

**Aleph 是一个完整的智能生命体。** 它拥有五层涌现的进化灵魂 (Soul)，由 1-2-3-4 工程骨架 (Skeleton) 支撑，以 POE+DDD 思维 (Mind) 驱动决策，以具体产品约束 (R1-R7) 保障实用性。

### 五层涌现架构 (The Soul — 灵魂)

```
散落的积木 → 分类堆放 → 堆叠整齐 → 功能模块 → 多态智能体
   ↓            ↓           ↓          ↓           ↓
经验之海    领域分类    原子技能    即插即用    随需而变
(Know)     (Classify)  (Know-how)  (Compose)   (Embody)
```

| 层级 | 名称 | 本质转变 |
|------|------|----------|
| **L1** | 经验之海 | 互联网、代码、历史、常识 — AI 的预训练养料 |
| **L2** | 领域分类 | 医学、法律、编程、物理 — 知识有了学科边界 |
| **L3** | 原子技能 | **Know-what → Know-how** — 从拥有知识到拥有能力 |
| **L4** | 功能模块 | 技能封装，即插即用 — AI 可以组合能力达成目标 |
| **L5** | 多态智能体 | **灵魂获得躯壳** — 随需变身，干涉物理/数字世界 |


### 🧠 Agent 设计思想：POE 架构 (The Mind — 思维)

Aleph 的 Agent 核心采用 **POE (Principle-Operation-Evaluation)** 架构，融合双系统认知模型：

- **第一性原理** — 先定义成功契约，再开始执行
- **启发式思考** — System 1 (快速直觉) + System 2 (深度推理) 协同
- **自我学习** — 成功经验结晶化，相似任务自动借鉴

详见：[Agent 设计哲学](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) | [POE 架构设计](docs/plans/2026-02-01-poe-architecture-design.md)

### 🏛️ 领域建模：DDD 筑底 (The Mind — 思维)

Aleph 采用 **DDD (Domain-Driven Design)** 的核心概念来组织领域逻辑，通过 Rust trait 系统实现轻量级的领域规约。

#### 统一语言 (Ubiquitous Language)

| 术语 | 定义 | 示例 |
|------|------|------|
| **Entity** | 具有唯一身份标识的对象，身份在状态变化中保持不变 | `Task`, `MemoryFact` |
| **AggregateRoot** | 聚合的入口点，管理一组相关对象的一致性边界 | `TaskGraph`, `MemoryFact` |
| **ValueObject** | 由属性定义的不可变对象，无身份标识 | `TaskStatus`, `ContextAnchor` |

#### Domain Traits (`core/src/domain/`)

```rust
pub trait Entity {
    type Id: Eq + Clone + Display;
    fn id(&self) -> &Self::Id;
}

pub trait AggregateRoot: Entity {}

pub trait ValueObject: Eq + Clone {}
```

#### 限界上下文 (Bounded Contexts)

| 上下文 | 聚合根 | 职责 |
|--------|--------|------|
| **Dispatcher** | `TaskGraph` | DAG 调度、工具编排、任务状态管理 |
| **Memory** | `MemoryFact` | 事实存储、RAG 检索、知识压缩 |
| **Intent** | `AggregatedIntent` | 意图检测、L1-L3 分层过滤 |
| **POE** | `SuccessManifest` | 成功契约、验证规则、评估结果 |

详见：[领域建模指南](docs/reference/DOMAIN_MODELING.md) | [DDD+BDD 设计](docs/plans/2026-02-06-ddd-bdd-dual-wheel-design.md)

---

## 🏗️ 1-2-3-4 架构模型 (The Skeleton — 骨架)

**Aleph 是一个自托管的个人 AI 助手**，其工程实现遵循 "1-2-3-4" 架构模型：1 个核心大脑，2 种交互界面，3 类执行系统，4 层通信协议。

### 1 Core — 大脑 (The Brain)

Rust Core 是 Aleph 的灵魂，只负责三件事：

- **推理规划 (Reasoning)**: 决定下一步该干什么
- **状态管理 (State)**: 维护对话、任务上下文
- **路由分发 (Routing)**: 把任务分发给插件、MCP 或桌面能力层

核心不画界面，不写截图代码。它是纯粹的、轻量的"大脑"。

### 2 Faces — 交互界面 (The Faces)

| 界面 | 角色 | 宿主 |
|------|------|------|
| **统一 Panel (Leptos/WASM)** | 全平台唯一 UI 逻辑实现 | Web、macOS/Windows/Linux (Tauri 壳) |
| **社交 Bot 通道 (Gateway)** | 数字世界的身影，永远在线的后台智能 | Telegram、Discord 等 |

### 3 Limbs — 执行系统 (The Limbs)

| 系统 | 角色 | 示例 |
|------|------|------|
| **Native 能力 (The Muscles)** | 直接控制系统 | Bash/Shell、Desktop Bridge (Tauri-Rust) — "看"(OCR/截图) 和 "动"(点击/输入) |
| **MCP (The External Tools)** | 杠杆效应，调用社区工具 | Playwright、Google Maps 等 |
| **Skills/Plugins (The Expertise)** | 领域知识 | PPT 专家、代码审查助手 |

### 4 Nerves — 通信协议 (The Nerves)

| 编号 | 通道 | 协议 | 用途 |
|------|------|------|------|
| 1 | Core ↔ UI | WebSocket/RPC | 驱动面板展示 |
| 2 | Core ↔ Desktop Bridge | UDS/IPC | 驱动电脑控制 |
| 3 | Core ↔ Gateway | gRPC/NATS | 驱动社交 Bot |
| 4 | Core ↔ MCP | JSON-RPC | 驱动外部插件 |

### 核心子系统

| 子系统 | 描述 | 文档 |
|--------|------|------|
| **Gateway** | WebSocket 控制面，JSON-RPC 2.0 协议，30+ RPC 方法 | [Gateway](docs/reference/GATEWAY.md) |
| **Agent Loop** | Observe-Think-Act-Feedback 循环，状态机驱动 | [Agent System](docs/reference/AGENT_SYSTEM.md) |
| **Thinker** | LLM 交互，Thinking Levels，流式响应 | [Agent System](docs/reference/AGENT_SYSTEM.md) |
| **Dispatcher** | 任务编排，DAG 调度，多步执行 | [Agent System](docs/reference/AGENT_SYSTEM.md) |
| **Tool Server** | AlephTool trait，19+ 内置工具 | [Tool System](docs/reference/TOOL_SYSTEM.md) |
| **Memory** | LanceDB 统一存储，混合检索 (ANN + FTS)，MemoryStore/GraphStore/SessionStore traits | [Memory System](docs/reference/MEMORY_SYSTEM.md) |
| **Resilience** | 多 Agent 弹性系统，StateDatabase (SQLite) 管理事件/任务/追踪/会话 | — |
| **Extension** | WASM + Node.js 插件运行时 | [Extension System](docs/reference/EXTENSION_SYSTEM.md) |
| **Desktop Bridge** | UDS JSON-RPC 2.0 桌面能力 (OCR/截图/输入/窗口/Canvas) | [Design](docs/plans/2026-02-25-server-centric-build-architecture-design.md) |
| **Exec** | Shell 执行安全，审批工作流 | [Security](docs/reference/SECURITY.md) |

详见：[完整架构文档](docs/reference/ARCHITECTURE.md)

---

## 🛑 架构红线 (Architectural Redlines)

以下为最高优先级约束，所有开发决策必须遵守。违反红线的代码不得合入。

### R1. 大脑与四肢绝对分离 (Brain-Limb Separation)

- **禁令**: 严禁在 `core/src` 中直接调用特定平台系统 API (AppKit, Vision, CoreGraphics, windows-rs)
- **原则**: 核心层只定义"能力契约 (Trait)"，物理实现由 Desktop Bridge (Tauri-Rust) 通过 IPC 提供

### R2. UI 逻辑唯一源 (Single Source of UI Truth)

- **禁令**: 严禁在 Tauri 中实现具有业务逻辑的复杂设置页面、表单或列表
- **原则**: 所有复杂业务 UI 在 Leptos (WASM) 中实现。原生外壳仅负责窗口容器、原生动画和菜单栏

### R3. 核心轻量化 (Core Minimalism)

- **禁令**: 严禁为单一非核心功能在 core 中引入沉重的第三方库
- **原则**: 优先实现为 Skill (Python/Bash) 或 MCP Server。内核只调度，不搬砖
- **备注**: 代码层面的奥卡姆剃刀原则和 Rust 大文件拆分规范与此不冲突

### R4. Interface 层禁止业务逻辑 (I/O-Only Interfaces)

- **禁令**: 禁止在 App/Bot/CLI 中处理数据持久化、记忆检索或任务规划逻辑
- **原则**: Interface 层是"纯 I/O"— 输入转为 JSON-RPC 发给 Server，响应渲染给用户

### R5. 菜单栏优先，按需展窗 (Menu Bar First)

- **默认形态**: macOS 无 Dock 图标，菜单栏常驻，Halo 浮窗为主要快捷交互入口
- **允许窗口**: 复杂场景（设置、长对话、调试面板）应使用正常窗口，不要为"隐形"牺牲可用性
- **原则**: 轻量入口 + 按需展开，而非"绝对无窗口"

### R6. AI 主动到达 (AI Comes to You)

- **原则**: 减少用户切换上下文的成本，AI 尽量在用户当前工作环境中提供帮助
- **实现**: Halo 浮窗、通知、内联建议等
- **边界**: 不打扰用户 (不抢焦点、不弹模态对话框)，但不要因此拒绝提供必要的 UI

### R7. 一核多端 (One Core, Many Shells)

- **原则**: Rust Core 是唯一大脑，UI 通过 Leptos/WASM 统一，原生壳只负责窗口容器和系统集成
- **备注**: 这已在 R1 和 R2 中体现，此条作为产品层面的重申

---

## 🧭 北极星 (North Star)

### 架构已定，填充不推倒

当前架构已经稳固。后续工作是"填充"而非"推倒"。

### 标准化桌面操作协议

Desktop Bridge 协议已定义完成，Tauri 版已全面实现。持续完善跨平台覆盖和性能。

### Skill 驱动未来

架构是骨骼，Skill 才是血肉。未来重点放在 Skills 上，它们决定了 Aleph 能帮省多少工作。

---

## 📁 项目结构

```
aleph/
├── core/                           # Rust Core (alephcore crate)
│   └── src/
│       ├── gateway/                # WebSocket 控制面 (34 files)
│       │   ├── handlers/           # RPC 方法处理器 (33 handlers)
│       │   ├── interfaces/          # 交互接口 (Telegram, Discord, iMessage)
│       │   └── security/           # 认证、配对、设备管理
│       ├── agent_loop/             # Observe-Think-Act-Feedback (15 files)
│       ├── thinker/                # LLM 交互层 (9 files)
│       ├── domain/                 # DDD 领域模型 (Entity, AggregateRoot traits)
│       ├── dispatcher/             # 任务编排 (22 subdirs)
│       ├── executor/               # 工具执行引擎
│       ├── providers/              # AI 提供商 (21 files)
│       ├── tools/                  # AlephTool trait
│       ├── builtin_tools/          # 内置工具 (19 files)
│       ├── memory/                 # 记忆系统 (纯 LanceDB)
│       │   └── store/             # LanceDB 存储抽象层 (MemoryStore, GraphStore, SessionStore)
│       ├── resilience/            # 任务弹性系统 (recovery, governance)
│       │   └── database/          # StateDatabase (SQLite) + CRUD 操作
│       ├── extension/              # 插件系统 (17 files)
│       ├── exec/                   # Shell 执行安全 (17 files)
│       ├── mcp/                    # MCP 协议客户端
│       ├── routing/                # Session Key 路由 (6 variants)
│       ├── config/                 # 配置系统 + 热重载
│       └── lib.rs                  # 60+ public modules
├── crates/
│   └── desktop/                    # aleph-desktop crate (DesktopCapability native impl)
├── apps/
│   ├── cli/                        # Rust CLI 客户端
│   ├── desktop/                    # Tauri Bridge - Linux/Windows (aleph-bridge)
│   └── macos-native/              # Native macOS app (Swift/Xcode)
├── docs/                           # 文档
│   ├── reference/                  # 核心架构文档
│   │   ├── ARCHITECTURE.md         # 完整架构
│   │   ├── AGENT_SYSTEM.md         # Agent 系统
│   │   ├── GATEWAY.md              # Gateway 协议
│   │   ├── TOOL_SYSTEM.md          # 工具系统
│   │   ├── MEMORY_SYSTEM.md        # 记忆系统
│   │   ├── EXTENSION_SYSTEM.md     # 扩展系统
│   │   ├── SECURITY.md             # 安全系统
│   │   ├── DESIGN_PATTERNS.md      # 设计模式
│   │   ├── CODE_ORGANIZATION.md    # 文件组织规范
│   │   ├── DOMAIN_MODELING.md      # 领域建模
│   │   ├── AGENT_DESIGN_PHILOSOPHY.md # 设计思想
│   │   └── SERVER_DEVELOPMENT.md   # Server 开发与发布
│   └── plans/                      # 设计规划文档
├── Cargo.toml                      # Workspace root
└── CLAUDE.md                       # 本文档
```

---

## ⚙️ 技术栈

| Layer | Technology |
|-------|------------|
| **Runtime** | Rust + Tokio (async/await) |
| **Gateway** | tokio-tungstenite + axum |
| **Database** | LanceDB (记忆：向量+元数据+FTS) + rusqlite (弹性状态：事件/任务/追踪) |
| **Embedding** | Remote OpenAI-compatible APIs (SiliconFlow/OpenAI/Ollama) |
| **Providers** | Claude, GPT-4, Gemini, Ollama, DeepSeek, Moonshot |
| **Plugins** | Extism (WASM), Node.js IPC |
| **Desktop App** | macOS: Native Swift/Xcode, Linux/Windows: Tauri |
| **Schema** | schemars (JSON Schema 自动生成) |

---

## 🔧 开发指南

### 构建命令

```bash
# Rust Core
cd core && cargo build && cargo test

# 启动 Server (不含 Control Plane UI)
cargo run --bin aleph-server

# 启动 Server (含 Control Plane UI)
cargo run --bin aleph-server --features control-plane

# 启动 Server (含原生桌面能力 + UI)
cargo run --bin aleph-server --features desktop,control-plane

# Tauri App
cd apps/desktop && pnpm install && pnpm tauri dev

# Build Bridge (cross-platform)
cd apps/desktop && cargo tauri build
```

---

## 🚀 Server 开发与发布

详见：[Server 开发与发布指南](docs/reference/SERVER_DEVELOPMENT.md)

快速参考：
- 不含 UI：`cargo run --bin aleph-server`
- 含 UI：`cargo run --bin aleph-server --features control-plane`
- 含原生桌面：`cargo run --bin aleph-server --features desktop`
- 全功能：`cargo run --bin aleph-server --features desktop,control-plane`
- Release：`cargo build --bin aleph-server --features desktop,control-plane --release`

---

### Feature Flags

```toml
[features]
default = ["gateway"]
gateway = ["tokio-tungstenite", "axum"]
telegram = ["teloxide", "gateway"]
discord = ["serenity", "gateway"]
cron = ["cron", "gateway"]
browser = ["chromiumoxide", "gateway"]
cli = ["inquire"]
plugin-wasm = ["extism"]
desktop-native = ["aleph-desktop"]  # In-process desktop capabilities (xcap/enigo)
desktop = ["desktop-native"]        # Server-friendly alias
```

### Environment

### Git Worktree 操作规范

**⚠️ 致命陷阱：`EnterWorktree` 会锁定 CWD，无法在同一会话内安全删除 worktree**

`EnterWorktree` 会在每次 Bash 命令后**强制重置 CWD 到 worktree 目录**，即使你用 `cd` 切回主仓库也无效。因此在同一会话内执行 `git worktree remove` 必然导致 Shell 永久损坏（exit 1 且无法恢复）。

**正确做法：不在使用 `EnterWorktree` 的会话内删除 worktree**

```bash
# ✅ 方案 A：在会话内只合并，不删除 worktree
cd /Volumes/TBU4/Workspace/Aleph          # 切回主仓库（仅在本命令内有效）
git merge worktree-xxx                     # 合并分支
# 结束会话 → 提示清理 worktree

# ✅ 方案 B：用新终端/新会话清理
cd /Volumes/TBU4/Workspace/Aleph
git worktree remove .claude/worktrees/xxx
git branch -D worktree-xxx
git worktree prune

# ✅ 方案 C：不用 EnterWorktree，手动管理（CWD 始终在主仓库）
git worktree add .claude/worktrees/xxx -b branch-xxx
# 用绝对路径操作 worktree 内文件，CWD 从不切换
git merge branch-xxx
git worktree remove .claude/worktrees/xxx  # 安全，因为 CWD 不在 worktree 内
git branch -D branch-xxx

# ❌ 错误 — EnterWorktree 后在同一会话内删除 worktree
git worktree remove .claude/worktrees/xxx  # CWD 被锁定在这里，Shell 永久损坏
```

### 分支策略

**单分支开发模式**：所有开发工作直接在 main 分支进行。

### 提交规范

English commit messages. Format: `<scope>: <description>`

Example: `gateway: add WebSocket server foundation`

### 语言规范

- Reply in Chinese
- Code comments in English
- Documentation in both

---

## 📚 文档索引

### 架构文档

| 文档 | 描述 |
|------|------|
| [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) | 完整系统架构、模块依赖、数据流 |
| [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) | 设计模式：Context、Newtype、FromStr、Builder |
| [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) | 文件组织规范：拆分原则、命名约定、反面示例、重构 Backlog |
| [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) | Agent Loop、Thinker、Dispatcher |
| [GATEWAY.md](docs/reference/GATEWAY.md) | WebSocket 协议、RPC 方法、Channels |
| [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) | AlephTool trait、内置工具、开发指南 |
| [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) | Facts DB、混合检索、压缩策略 |
| [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) | WASM/Node.js 插件、manifest 格式 |
| [SECURITY.md](docs/reference/SECURITY.md) | Exec 审批、权限规则、allowlist |
| [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) | DDD 领域建模、Entity/AggregateRoot traits |
| [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) | Server 开发、发布、部署、故障排查 |

### 设计文档

| 文档 | 描述 |
|------|------|
| [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) | 四大设计思想：第一性原理、启发式、自学习、POE |
| [POE Architecture](docs/plans/2026-02-01-poe-architecture-design.md) | POE 架构详细设计 |
| [Server-Centric Architecture](docs/plans/2026-02-23-server-centric-architecture-design.md) | Server-centric 架构设计 |
| [Server-Centric Build](docs/plans/2026-02-25-server-centric-build-architecture-design.md) | Daemon + Bridge 架构设计 |

---


## 📝 Session Context

### Key Context

- **项目定位**: 自托管个人 AI 助手，1-2-3-4 架构模型 (1 Core, 2 Faces, 3 Limbs, 4 Nerves)
- **核心循环**: Observe → Think → Act → Feedback → Compress
- **技术栈**: Rust Core (大脑) + Leptos/WASM (统一 UI) + Tauri (原生壳) + Gateway (社交通道)
- **当前阶段**: 架构稳固期 — 填充而非推倒，Skill 驱动未来

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.

## 📝 语言
使用中文对话
