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

### 五层涌现架构

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

### Ghost 美学

| 原则 | 实现 |
|------|------|
| **Invisible First** | 无 Dock 图标、无常驻窗口，只有后台进程 + 菜单栏 |
| **Frictionless** | AI 来到你身边，而不是你去找 AI |
| **Native-First** | 100% 原生代码 (Rust + Swift) |
| **Polymorphic** | 一个灵魂，无限形态 |

### 🧠 Agent 设计思想：POE 架构

Aleph 的 Agent 核心采用 **POE (Principle-Operation-Evaluation)** 架构，融合双系统认知模型：

- **第一性原理** — 先定义成功契约，再开始执行
- **启发式思考** — System 1 (快速直觉) + System 2 (深度推理) 协同
- **自我学习** — 成功经验结晶化，相似任务自动借鉴

详见：[Agent 设计哲学](docs/AGENT_DESIGN_PHILOSOPHY.md) | [POE 架构设计](docs/plans/2026-02-01-poe-architecture-design.md)

---

## 🏗️ 架构概览

**Aleph 是一个自托管的个人 AI 助手**，通过 WebSocket Gateway 统一管理多渠道消息、Agent 执行、工具调用和记忆系统。

```
┌─────────────────────────────────────────────────────────────────┐
│                         CLIENT LAYER                             │
│   macOS App │ Tauri App │ CLI │ Telegram │ Discord │ WebChat   │
└───────────────────────────────┬─────────────────────────────────┘
                                │ WebSocket (JSON-RPC 2.0)
                                │ ws://127.0.0.1:18789
┌───────────────────────────────┴─────────────────────────────────┐
│                         GATEWAY LAYER                            │
│   Router │ Session Manager │ Event Bus │ Channels │ Hot Reload  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                          AGENT LAYER                             │
│            Observe → Think → Act → Feedback → Compress           │
│   Agent Loop │ Thinker │ Dispatcher │ Guards │ Overflow         │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                        EXECUTION LAYER                           │
│   Providers │ Executor │ Tool Server │ MCP │ Extensions │ Exec  │
└───────────────────────────────┬─────────────────────────────────┘
                                │
┌───────────────────────────────┴─────────────────────────────────┐
│                         STORAGE LAYER                            │
│          Memory (Facts DB + Vector) │ Config │ Keychain          │
└─────────────────────────────────────────────────────────────────┘
```

### 核心子系统

| 子系统 | 描述 | 文档 |
|--------|------|------|
| **Gateway** | WebSocket 控制面，JSON-RPC 2.0 协议，30+ RPC 方法 | [Gateway](docs/GATEWAY.md) |
| **Agent Loop** | Observe-Think-Act-Feedback 循环，状态机驱动 | [Agent System](docs/AGENT_SYSTEM.md) |
| **Thinker** | LLM 交互，Thinking Levels，流式响应 | [Agent System](docs/AGENT_SYSTEM.md) |
| **Dispatcher** | 任务编排，DAG 调度，多步执行 | [Agent System](docs/AGENT_SYSTEM.md) |
| **Tool Server** | AlephTool trait，19+ 内置工具 | [Tool System](docs/TOOL_SYSTEM.md) |
| **Memory** | Facts DB + sqlite-vec，混合检索 (Vector + BM25) | [Memory System](docs/MEMORY_SYSTEM.md) |
| **Extension** | WASM + Node.js 插件运行时 | [Extension System](docs/EXTENSION_SYSTEM.md) |
| **Exec** | Shell 执行安全，审批工作流 | [Security](docs/SECURITY.md) |

详见：[完整架构文档](docs/ARCHITECTURE.md)

---

## 📁 项目结构

```
aether/
├── core/                           # Rust Core (alephcore crate)
│   └── src/
│       ├── gateway/                # WebSocket 控制面 (34 files)
│       │   ├── handlers/           # RPC 方法处理器 (33 handlers)
│       │   ├── channels/           # 消息渠道 (Telegram, Discord, iMessage)
│       │   └── security/           # 认证、配对、设备管理
│       ├── agent_loop/             # Observe-Think-Act-Feedback (15 files)
│       ├── thinker/                # LLM 交互层 (9 files)
│       ├── dispatcher/             # 任务编排 (22 subdirs)
│       ├── executor/               # 工具执行引擎
│       ├── providers/              # AI 提供商 (21 files)
│       ├── tools/                  # AlephTool trait
│       ├── builtin_tools/          # 内置工具 (19 files)
│       ├── memory/                 # 记忆系统 (18 files)
│       ├── extension/              # 插件系统 (17 files)
│       ├── exec/                   # Shell 执行安全 (17 files)
│       ├── mcp/                    # MCP 协议客户端
│       ├── routing/                # Session Key 路由 (6 variants)
│       ├── runtimes/               # 运行时管理 (uv, fnm, yt-dlp)
│       ├── config/                 # 配置系统 + 热重载
│       └── lib.rs                  # 60+ public modules
├── platforms/
│   ├── macos/                      # macOS App (Swift/SwiftUI, 45+ dirs)
│   └── tauri/                      # Cross-platform Tauri App
├── docs/                           # 文档
│   ├── ARCHITECTURE.md             # 完整架构
│   ├── AGENT_SYSTEM.md             # Agent 系统
│   ├── GATEWAY.md                  # Gateway 协议
│   ├── TOOL_SYSTEM.md              # 工具系统
│   ├── MEMORY_SYSTEM.md            # 记忆系统
│   ├── EXTENSION_SYSTEM.md         # 扩展系统
│   ├── SECURITY.md                 # 安全系统
│   ├── AGENT_DESIGN_PHILOSOPHY.md  # 设计思想
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
| **Database** | rusqlite + sqlite-vec (向量搜索) |
| **Embedding** | fastembed (bge-small-zh-v1.5, 本地) |
| **Providers** | Claude, GPT-4, Gemini, Ollama, DeepSeek, Moonshot |
| **Plugins** | Extism (WASM), Node.js IPC |
| **macOS App** | Swift + SwiftUI + AppKit |
| **Cross-platform** | Tauri + React |
| **Schema** | schemars (JSON Schema 自动生成) |

---

## 🔧 开发指南

### 构建命令

```bash
# Rust Core
cd core && cargo build && cargo test

# 启动 Gateway
cargo run -p alephcore --features gateway

# macOS App
cd platforms/macos && xcodegen generate && open Aleph.xcodeproj

# Tauri App
cd platforms/tauri && pnpm install && pnpm tauri dev
```

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
```

### Environment

- Python path: ~/.uv/python3/bin/python
- Install Python package: cd ~/.uv/python3 && uv pip install <package>
- Xcode generation: cd platforms/macos && xcodegen generate
- Syntax validation: ~/.uv/python3/bin/python Scripts/verify_swift_syntax.py <file.swift>
- Xcode build cache cleanup: rm -rf ~/Library/Developer/Xcode/DerivedData/(Aleph)-*
- This project uses XcodeGen to manage the Xcode project. See docs/XCODEGEN_README.md for detailed workflow instructions.

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
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | 完整系统架构、模块依赖、数据流 |
| [AGENT_SYSTEM.md](docs/AGENT_SYSTEM.md) | Agent Loop、Thinker、Dispatcher |
| [GATEWAY.md](docs/GATEWAY.md) | WebSocket 协议、RPC 方法、Channels |
| [TOOL_SYSTEM.md](docs/TOOL_SYSTEM.md) | AlephTool trait、内置工具、开发指南 |
| [MEMORY_SYSTEM.md](docs/MEMORY_SYSTEM.md) | Facts DB、混合检索、压缩策略 |
| [EXTENSION_SYSTEM.md](docs/EXTENSION_SYSTEM.md) | WASM/Node.js 插件、manifest 格式 |
| [SECURITY.md](docs/SECURITY.md) | Exec 审批、权限规则、allowlist |

### 设计文档

| 文档 | 描述 |
|------|------|
| [AGENT_DESIGN_PHILOSOPHY.md](docs/AGENT_DESIGN_PHILOSOPHY.md) | 四大设计思想：第一性原理、启发式、自学习、POE |
| [POE Architecture](docs/plans/2026-02-01-poe-architecture-design.md) | POE 架构详细设计 |

---


## 📝 Session Context

### Key Context

- **项目定位**: 自托管个人 AI 助手，Gateway 控制面架构
- **核心循环**: Observe → Think → Act → Feedback → Compress
- **技术栈**: Rust (Gateway + Agent) + Swift (macOS) + React (Tauri)
- **当前状态**: Phase 8 (Multi-Channel)，Gateway 完整实现

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.
