# Aleph (ℵ)

> 自托管个人 AI 助手 — 一核多端。

[![Rust](https://img.shields.io/badge/Rust-1.92%2B-b7410e)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()

[English](README.md)

## Aleph 是什么？

Aleph 是一款使用 Rust 构建的自托管个人 AI 助手。它完全运行在你自己的设备上，通过统一的 Gateway 连接 15+ 消息通道（Telegram、Discord、Slack、WhatsApp、IRC、Matrix、Signal 等）。Rust 核心驱动了一个 Agent 循环，支持多供应商 LLM、30+ 内置工具、混合记忆检索和插件系统 — 可同时通过原生应用、CLI、Web 面板和社交 Bot 访问。

## 核心亮点

### 认知记忆架构

Aleph 的记忆系统超越了简单的 RAG：

- **笔记层** — 基于 Markdown 的记忆，支持 Obsidian 兼容的 `[[wikilink]]` 语法，形成可遍历的知识图谱
- **混合检索** — 向量 ANN（sqlite-vec）+ 全文搜索（FTS5）+ wikilink 图谱遍历，支持多跳推理
- **自我学习** — 从笔记中自动生成技能；系统观察笔记中的模式并建议或自动生成技能
- **梦境守护进程** — 后台记忆压缩与综合，将记忆提炼为更高层次的抽象

### 解耦的 Agent 架构

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│ Orchestrator│────▶│   Harness   │────▶│   Thinker   │
│             │     │  (Think→Act)│     │  (LLM Call) │
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
           ┌───────────────┼───────────────┐
           ▼               ▼               ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐
    │  Session │    │  Tools   │    │ Sandbox  │
    │ (History)│    │(Execution│    │(Isolation│
    └──────────┘    └──────────┘    └──────────┘
```

- **Orchestrator** — 解析 AgentDef + FlowSpec，组装 Harness 依赖，调度执行
- **Harness** — Think→Act 循环，包含停止钩子、上下文预算和紧急压缩
- **Sandbox** — 每会话工作空间，带能力账本和 OS 原生隔离（`OsSandboxDriver`）
- **Session Service** — 追加式事件日志，通过进程内 actor 实现权威状态管理
- **Tool Service** — 统一门面，覆盖内置工具、MCP 服务器和扩展，带分层中间件（审计 → 权限 → 上下文规则 → 超时）

### 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                        INTERFACE LAYER (I/O)                        │
│  macOS Native | Tauri | CLI | Panel (WASM) | Telegram |           │  │
│  Discord | Slack | WhatsApp | IRC | Matrix | Signal | ...          │
├─────────────────────────────┬───────────────────────────────────────┤
│                       GATEWAY LAYER                                 │
│  Router | Session Manager | Event Bus | Channel Registry | Reload  │
├─────────────────────────────┼───────────────────────────────────────┤
│                        AGENT LAYER                                  │
│  Agent Loop | Thinker | Dispatcher | Task Planner | Compressor     │
├─────────────────────────────┼───────────────────────────────────────┤
│                      EXECUTION LAYER                                │
│  Providers | Engine | Tool Server | MCP | Extensions | Exec        │
├─────────────────────────────┼───────────────────────────────────────┤
│                       STORAGE LAYER                                 │
│  Memory (SQLite+vec0) | State (SQLite) | Config (~/.aleph/)        │
└─────────────────────────────┴───────────────────────────────────────┘
```

详见 [docs/reference/ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) 获取完整架构文档。

## 功能特性

### 核心能力

- 多供应商 LLM 支持（Claude、GPT-4、Gemini、DeepSeek、Ollama、Moonshot、Kimi）
- 通过统一 Gateway 接入 15+ 消息通道
- 30+ 内置工具，支持 JSON Schema 自动生成
- **认知记忆** — 笔记层 + wikilink 知识图谱，混合检索（向量 + FTS + 图谱），后台梦境守护进程
- **自我学习** — 从笔记模式自动生成技能
- MCP 协议支持，集成外部工具
- **解耦 Agent 循环** — Orchestrator + Harness + Sandbox + Session + Tool Service 架构
- Desktop Bridge 原生系统控制（OCR、截图、输入自动化）

### 开发者体验

- 配置变更热重载
- 插件系统（WASM + Node.js）
- `just` 构建流水线，一条命令完成工作流
- 58+ Gateway JSON-RPC 处理器
- 通过 schemars 自动生成 JSON Schema
- proptest 和 loom 并发测试套件

## 与 OpenClaw 的关系

Aleph 是受 [OpenClaw](https://github.com/AIChatClaw/OpenClaw) 启发的 Rust 重新实现。相比原始 TypeScript 实现的关键优势包括：编译性能（~100ms 启动，~20MB 内存）、编译时安全保证（无 null/undefined，基于所有权）、多线程异步并发（tokio）、纵深防御分层安全、认知记忆架构（分层存储和后台综合），以及一等 MCP 协议支持。

## 安装

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/rootazero/Aleph/main/install.sh | bash
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/rootazero/Aleph/main/install.ps1 | iex
```

安装器自动检测平台和架构（x86_64 / ARM64），下载最新 release 二进制文件，安装到 PATH，并可选设置为系统服务自动启动。

安装完成后运行：

```bash
aleph
```

### 从源码构建

如果你更喜欢从源码构建：

```bash
# 前置条件：Rust 1.92+、just (cargo install just)
git clone https://github.com/rootazero/Aleph.git
cd Aleph
cargo run --bin aleph
```

### 配置

Aleph 将配置和数据存储在 `~/.aleph/`：

```
~/.aleph/
├── aleph.toml       # 主配置
├── logs/            # 服务日志
├── skills/          # 用户安装的技能
└── plugins/         # 扩展
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
| `just wasm`           | 仅构建 WASM Panel UI                       |
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
├── src/                         # Rust Core (alephcore crate)
│   ├── gateway/                 # WebSocket 控制平面
│   │   ├── handlers/            # 58+ RPC 方法处理器
│   │   ├── interfaces/          # 15+ 通道接口
│   │   ├── security/            # 认证、配对、设备管理
│   │   └── ...                  # 路由、会话、事件、语音、Webhooks
│   ├── orchestrator/            # AgentDef 解析 + Harness 调度
│   ├── harness/                 # Think→Act 循环、停止钩子、上下文预算
│   ├── thinker/                 # LLM 交互层
│   ├── dispatcher/              # 任务编排（DAG 调度）
│   ├── engine/                  # 工具执行引擎
│   ├── builtin_tools/           # 30+ 内置工具
│   ├── memory/                  # SQLite+sqlite-vec 存储（向量 + FTS）
│   ├── resilience/              # 状态管理（SQLite）
│   ├── extension/               # WASM + Node.js 插件系统
│   ├── providers/               # AI 供应商集成
│   ├── domain/                  # DDD 领域模型
│   ├── mcp/                     # MCP 协议客户端
│   ├── sandbox/                 # Sandbox trait + WorkspaceSandbox
│   ├── exec/                    # Shell 执行 + 安全
│   ├── agents/                  # Agent 运行时、子 Agent 生成
│   ├── a2a/                     # A2A 协议适配器
│   ├── acp/                     # ACP 协议
│   ├── approval/                # 审批系统
│   ├── arena/                   # Arena 功能
│   ├── browser/                 # 浏览器自动化
│   ├── capability/              # 能力系统
│   ├── clawhub/                 # ClawHub 集成
│   ├── components/              # 共享组件
│   ├── compressor/              # 上下文压缩
│   ├── core/                    # 核心类型和原语
│   ├── daemon/                  # 后台守护进程
│   ├── discovery/               # 服务发现
│   ├── event/                   # 事件系统
│   ├── generation/              # 媒体生成
│   ├── group_chat/              # 群聊管理
│   ├── intent/                  # 意图识别
│   ├── logging/                 # 日志基础设施
│   ├── markdown/                # Markdown 处理
│   ├── media/                   # 媒体处理
│   ├── metrics/                 # 指标收集
│   ├── permission/              # 权限系统
│   ├── pii/                     # PII 检测/处理
│   ├── prompt/                  # Prompt 管理
│   ├── routing/                 # 会话键解析
│   ├── runtimes/                # 能力账本
│   ├── scheduler/               # 作业调度
│   ├── search/                  # 搜索供应商
│   ├── secrets/                 # 密钥管理
│   ├── security/                # 安全工具
│   ├── session/                 # 会话服务
│   ├── skill/                   # 技能系统
│   ├── supervisor/              # 执行监督
│   ├── tasks/                   # 任务管理
│   ├── teams/                   # 团队协作
│   ├── tool_output/             # 工具输出处理
│   ├── utils/                   # 工具函数
│   ├── vision/                  # 视觉处理
│   └── wizard/                  # 向导流程
├── desktop/
│   ├── shared/                  # DesktopCapability trait + IPC
│   ├── macos/                   # macOS 原生实现
│   ├── linux/                   # Linux 原生实现
│   └── windows/                 # Windows 原生实现
├── shared/
│   ├── protocol/                # 共享协议类型
│   ├── logging/                 # 日志基础设施
│   ├── client/                  # 共享客户端工具
│   └── ui_logic/                # 共享 UI 逻辑
├── interfaces/
│   ├── cli/                     # CLI 客户端
│   ├── tui/                     # TUI 客户端
│   └── webchat/                 # Web 聊天界面
├── docs/
│   ├── reference/               # 架构与系统文档
│   └── superpowers/             # 设计规格与运行报告
├── justfile                     # 构建流水线
└── Cargo.toml                   # 工作区根
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


