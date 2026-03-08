# Aleph (ℵ)

> 自托管个人 AI 助手 — 一核多端。

[![Rust](https://img.shields.io/badge/Rust-1.92%2B-b7410e)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()

[English](README.md)

## 写在前面

这是一个纯粹的个人兴趣项目。我不是专业程序员，只是一个被 AI 可能性深深吸引的普通人。我一边学习 AI 辅助编程，一边构建了 Aleph。项目的设计大量参考了 [OpenClaw](https://github.com/AIChatClaw/OpenClaw) 和 [Claude Code](https://docs.anthropic.com/en/docs/claude-code)。

出于开源精神，我把它分享出来，希望对其他人有所帮助或启发。欢迎任何贡献、反馈和想法。

## Aleph 是什么？

Aleph 是一款使用 Rust 构建的自托管个人 AI 助手。它完全运行在你自己的设备上，通过统一的 Gateway 连接 15+ 消息通道（Telegram、Discord、Slack、WhatsApp、IRC、Matrix、Signal 等）。Rust 核心驱动了一个 Agent 循环，支持多供应商 LLM、30+ 内置工具、混合记忆检索和插件系统 — 可同时通过原生应用、CLI、Web 面板和社交 Bot 访问。

## 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        INTERFACE LAYER (I/O)                        │
│  macOS Native | Tauri | CLI | Panel (WASM) | WebChat | Telegram |  │
│  Discord | Slack | WhatsApp | IRC | Matrix | Signal | Nostr | ...  │
├─────────────────────────────┬───────────────────────────────────────┤
│                       GATEWAY LAYER                                 │
│  Router | Session Manager | Event Bus | Channel Registry | Reload  │
├─────────────────────────────┼───────────────────────────────────────┤
│                        AGENT LAYER                                  │
│  Agent Loop | Thinker | Dispatcher | Task Planner | Compressor     │
├─────────────────────────────┼───────────────────────────────────────┤
│                      EXECUTION LAYER                                │
│  Providers | Executor | Tool Server | MCP | Extensions | Exec      │
├─────────────────────────────┼───────────────────────────────────────┤
│                       STORAGE LAYER                                 │
│  Memory (LanceDB) | State (SQLite) | Config (~/.aleph/)            │
└─────────────────────────────┴───────────────────────────────────────┘
```

详见 [docs/reference/ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) 获取完整架构文档。

## 功能特性

### 核心能力

- 多供应商 LLM 支持（Claude、GPT-4、Gemini、DeepSeek、Ollama、Moonshot）
- 通过统一 Gateway 接入 15+ 消息通道
- 30+ 内置工具，支持 JSON Schema 自动生成
- 记忆系统，混合检索（向量 ANN + 全文检索，基于 LanceDB）
- MCP 协议支持，集成外部工具
- POE（Principle-Operation-Evaluation）Agent 架构
- Desktop Bridge 原生系统控制（OCR、截图、输入自动化）

### 开发者体验

- 配置变更热重载
- 插件系统（WASM + Node.js）
- `just` 构建流水线，一条命令完成工作流
- 58+ Gateway JSON-RPC 处理器
- 通过 schemars 自动生成 JSON Schema
- proptest 和 loom 并发测试套件

## 快速开始

### 前置条件

- **Rust** 1.92+ — 通过 [rustup](https://rustup.rs/) 安装
- **just** — `cargo install just`
- 可选：`wasm-bindgen-cli` + `npm`（用于 WASM 面板构建）
- 可选：Xcode + [XcodeGen](https://github.com/yonaskolb/XcodeGen)（用于 macOS 原生应用）

### 启动

```bash
git clone https://github.com/rootazero/Aleph.git
cd Aleph

# 启动服务
cargo run --bin aleph
```

### 配置

Aleph 将配置和数据存储在 `~/.aleph/`：

```
~/.aleph/
├── aleph.toml       # Main configuration
├── logs/            # Server logs
├── skills/          # User-installed skills
└── plugins/         # Extensions
```

在 `aleph.toml` 中配置通道示例：

```toml
[channels.telegram]
enabled = true
token = "your-bot-token"
```

## 构建

| 命令                  | 说明                                       |
|-----------------------|--------------------------------------------|
| `just dev`            | 以调试模式运行服务（重新构建 WASM）        |
| `just build`          | 以 release 模式构建服务                    |
| `just wasm`           | 仅构建 WASM Panel UI                      |
| `just macos`          | 构建 macOS 原生应用（release）             |
| `just test`           | 运行核心测试                               |
| `just test-all`       | 运行全部测试（core + desktop + proptest）  |
| `just clippy`         | 使用 clippy 检查核心代码                   |
| `just check`          | 快速编译检查                               |
| `just deps`           | 验证构建依赖是否已安装                     |
| `just clean`          | 清理所有构建产物                           |

生产构建无需指定 feature flags。

## 项目结构

```
Aleph/
├── core/                        # Rust Core (alephcore crate)
│   └── src/
│       ├── gateway/             # WebSocket control plane
│       │   ├── handlers/        # 58+ RPC method handlers
│       │   ├── interfaces/      # 15+ channel interfaces
│       │   └── security/        # Auth, pairing, device management
│       ├── agent_loop/          # Observe-Think-Act-Feedback loop
│       ├── thinker/             # LLM interaction layer
│       ├── dispatcher/          # Task orchestration (DAG scheduling)
│       ├── executor/            # Tool execution engine
│       ├── builtin_tools/       # 30+ built-in tools
│       ├── memory/              # LanceDB storage (vectors + FTS)
│       ├── resilience/          # State management (SQLite)
│       ├── extension/           # WASM + Node.js plugin system
│       ├── providers/           # AI provider integrations
│       ├── domain/              # DDD domain model
│       ├── mcp/                 # MCP protocol client
│       └── exec/                # Shell execution + security
├── crates/
│   ├── desktop/                 # DesktopCapability native impl
│   └── logging/                 # Logging infrastructure
├── shared/
│   ├── protocol/                # Shared protocol types
│   └── ui_logic/                # Shared UI logic
├── apps/
│   ├── cli/                     # CLI client
│   ├── panel/                   # Leptos/WASM Panel UI
│   ├── webchat/                 # React WebChat UI
│   ├── desktop/                 # Tauri cross-platform app
│   └── macos-native/            # Native macOS app (Swift/Xcode)
├── docs/
│   ├── reference/               # Architecture & system docs
│   └── plans/                   # Design documents
├── justfile                     # Build pipeline
└── Cargo.toml                   # Workspace root
```

## 文档

| 文档 | 链接 |
|------|------|
| 架构 | [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) |
| Agent 系统 | [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) |
| Gateway 协议 | [GATEWAY.md](docs/reference/GATEWAY.md) |
| 工具系统 | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| 记忆系统 | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| 扩展系统 | [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |
| 安全 | [SECURITY.md](docs/reference/SECURITY.md) |
| 设计模式 | [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) |
| 代码组织 | [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) |
| 领域建模 | [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) |
| Agent 设计哲学 | [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) |
| 服务端开发 | [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) |

## 贡献

在 `main` 分支上进行单分支开发。提交格式：`<scope>: <description>`（英文）。

示例：`gateway: add WebSocket server foundation`

## 许可证

MIT。详见 [LICENSE](LICENSE)。

## 致谢

- [攻壳机动队](https://en.wikipedia.org/wiki/Ghost_in_the_Shell) — 人机共生的愿景
- [豪尔赫·路易斯·博尔赫斯](https://en.wikipedia.org/wiki/The_Aleph_(short_story)) — Aleph 的隐喻：包含一切的一点
