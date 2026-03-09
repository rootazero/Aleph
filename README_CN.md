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

## 为什么选择 Aleph？— 与 OpenClaw 的对比

Aleph 最初受 [OpenClaw](https://github.com/AIChatClaw/OpenClaw) 启发，但在架构和能力上已大幅分化。以下是基于代码的真实对比。

### 总览

| | OpenClaw | Aleph |
|---|---|---|
| **语言** | TypeScript (Node.js ≥22) | Rust (tokio 异步运行时) |
| **二进制大小** | ~200MB+ (node_modules) | 单个静态二进制 (~50MB) |
| **内存占用** | ~150-300MB (V8 堆) | ~20-50MB (无 GC) |
| **并发模型** | 单线程事件循环 | 多线程异步 (tokio) |
| **类型安全** | TypeScript (运行时可能异常) | Rust (编译时保证，无 null/undefined) |

### 架构：大脑-四肢分离 vs 单体

OpenClaw 在单个 Node.js 进程中运行所有组件 — Gateway、Agent、工具和桌面控制共享一个运行时。如果某个工具崩溃或内存泄漏，整个助手都会挂掉。

Aleph 强制执行严格的**大脑-四肢分离**（架构红线 R1）。Rust Core（大脑）通过类型化 IPC 协议与 Desktop Bridge、UI 外壳和外部工具通信。每个层可以独立崩溃而不影响核心。这在编译时通过 Rust 的 trait 系统强制执行 — 平台特定代码在物理上无法被导入到 `core/src`。

### 安全：纵深防御 vs 基于信任

OpenClaw 采用**单用户信任模型** — 一旦认证，操作者拥有完全访问权限。执行审批可用但可选。沙箱模式使用 Docker 容器隔离。

Aleph 实现了**编译时强制的分层安全**：

- **三层执行安全** — `AllowlistEntry`（预审批模式匹配）→ `RiskAssessment`（基于 SAFE/DANGER/BLOCKED 分类的模式危险评分）→ `ApprovalManager`（通过 Unix socket IPC 的异步用户确认）
- **10 类行为审批系统** — 每种操作类型（BrowserNavigate、DesktopClick、ShellExec、FileWrite 等）都经过 `ConfigApprovalPolicy` 的 blocklist → allowlist → defaults → ask 链
- **密钥遮蔽** — `SecretMasker` 在日志和工具输出中脱敏
- **沙箱配置** — macOS 沙箱配置文件用于工具执行，超越 Docker
- **锁安全** — 项目级规范：所有 mutex 访问使用 poison recovery（`.unwrap_or_else(|e| e.into_inner())`）
- **UTF-8 安全** — 字符串切片使用 `char_indices()` / `.get(..n)`，禁止 `&s[..n]`

### Agent 智能：POE 架构 vs 简单循环

OpenClaw 的 Agent 循环由 `@mariozechner/pi-agent-core`（第三方库）驱动，Agent 运行直到产生响应或达到 token 上限。

Aleph 实现了 **POE（Principle-Operation-Evaluation）架构** — 一个自我纠正的 Agent 循环，包含三个阶段：

1. **Principle（原则）** — 执行前，`SuccessManifest` 定义成功标准：`ValidationRule`（硬约束：文件存在、命令通过）+ `SoftMetric`（加权质量评分）
2. **Operation（操作）** — Agent 在预算跟踪下执行（`PoeBudget` 监控 token、尝试次数和基于熵的卡死检测，状态：Improving/Stable/Degrading/Stuck/Exhausted）
3. **Evaluation（评估）** — 双阶段验证：`HardValidator`（确定性检查）+ `SemanticValidator`（LLM 质量评估）。评估失败时循环自我纠正并切换策略

这意味着 Aleph 不是"试到完成为止" — 它预先定义成功标准，监控自身进度，检测何时卡住，并能自主切换策略。

### 多智能体：群体智能 vs 配置路由

OpenClaw 通过配置驱动路由支持多 Agent — 在配置中定义 Agent，每个有独立工作区，`sessions_send` 工具在它们之间传递消息。功能可用但静态。

Aleph 拥有**三套多智能体系统**：

1. **A2A 协议** — 完整的 HTTP Agent 间通信，包含 server/client 适配器、SSE 流式传输、任务存储、智能路由、Agent 卡片发现和分级认证
2. **群体智能** — `SwarmCoordinator` 编排多个 Agent，`AgentMessageBus`（事件总线）、`SemanticAggregator`（压缩 N 个 Agent 的洞察）、`CollectiveMemory`（共享团队记忆）、`RuleEngine`（事件过滤）
3. **SharedArena** — 多 Agent 协作工作区，基于 slot 的事件、settlement 协议和 `ArenaManager` 持久存储

### 记忆：认知架构 vs 扁平存储

OpenClaw 使用 LanceDB 进行向量搜索，配合批量 embedding 和压缩。适用于基本 RAG 检索。

Aleph 的记忆系统是**认知架构**，包含 50+ 模块：

- **分层存储** — `MemoryTier`：Ephemeral → Short-term → Long-term → Archive，自动衰减（`DecayScheduler`）
- **事实类型** — `FactType`：Fact、Hypothesis、Pattern、Policy、Config、Observation、Artifact
- **记忆层次** — `MemoryLayer`：Operational（工作）、Tactical（近期）、Strategic（长期）
- **专用存储** — `MemoryStore`（事实）、`SessionStore`（对话）、`GraphStore`（实体关系）、`DreamStore`（每日洞察）、`CompressionStore`（摘要）
- **做梦** — `CompressionDaemon` 在后台运行记忆整合，从积累的经验中生成洞察（类似人类睡眠时的记忆巩固）
- **自适应检索** — `AdaptiveRetrievalGate` 决定何时搜索记忆，`Reranker` 重排结果，`ValueEstimator` 通过 LLM 评估重要性
- **涟漪效应** — `RippleEffect` 在相关事实间传播记忆影响

### 自我学习：经验结晶化 vs 无

OpenClaw 没有自学习机制。会话事实被存储但从不分析模式。

Aleph 实现了**技能进化流水线**：

```
EvolutionTracker → SolidificationDetector → SkillGenerator → GitCommitter
```

1. `EvolutionTracker` 将每次执行记录到 SQLite
2. `SolidificationDetector` 识别超过成功阈值的反复出现的模式
3. `SkillGenerator` 从固化的模式中创建新技能（SKILL.md）
4. `GitCommitter` 自动提交新技能到仓库

安全门控（`SafetyLevel`：Benign → Caution → Warning → Danger → Blocked）和用户审批工作流防止生成不安全的技能。

### MCP：一等协议 vs 桥接

OpenClaw 通过 `mcporter` 支持 MCP — 一个将 MCP 服务器桥接为 OpenClaw 工具的 skill。这增加了延迟并限制了功能覆盖。

Aleph 实现了**一等 MCP 支持**，包含三种传输层：

- `StdioTransport` — 本地子进程通信
- `HttpTransport` — 远程 HTTP 服务器
- `SseTransport` — HTTP + Server-Sent Events 流式传输

以及：`McpResourceManager`（资源发现）、`McpPromptManager`（提示模板）、`OAuthStorage`（token 持久化）和 `SamplingCallback`（LLM 采样支持）。

### 意图检测：统一 LLM 驱动流水线 vs 简单路由

OpenClaw 基于会话配置和 Agent 分配来路由消息。

Aleph 使用**统一的 LLM 驱动流水线**（`UnifiedIntentClassifier`），将核心分类交给 AI：

| 层级 | 名称 | 功能 |
|------|------|------|
| Abort | `AbortDetector` | 多语言停止词精确匹配（11 种语言：中/英/日/韩/俄/德/法/西/葡/阿/印）。快速，无需 LLM |
| L0 | 斜杠命令 | 内置命令（`/screenshot`、`/ocr`、`/search` 等）+ 运行时注册指令（`/think`、`/model`、`/notools`） |
| L1 | `StructuralDetector` | 文件路径（Unix/Windows）、URL、上下文信号（选中文件、剪贴板）的模式匹配。无语言依赖 |
| L2 | `KeywordIndex` | **可选的**加权关键词匹配，支持 CJK 分词。作为快速路径保留，不作为核心依赖 |
| L3 | `AiBinaryClassifier` | **核心决策者** — LLM 二分类（执行 vs 对话），3 秒超时，置信度阈值过滤 |
| L4 | 默认兜底 | 所有层都弃权时，回退到执行（置信度 0.5）或对话 |

早退优化：第一个匹配的层即返回。流水线输出为统一的 `IntentResult` 枚举（DirectTool / Execute / Converse / Abort），携带检测层和置信度元数据。可选的 `ConfidenceCalibrator` 基于历史和上下文进行后处理调优。

### 弹性：状态恢复 vs 无

OpenClaw 没有崩溃恢复机制。进程死亡后上下文丢失。

Aleph 包含：

- `ShadowReplayEngine` — 崩溃后重放执行轨迹
- `RecoveryManager` — 协调恢复决策（Resume、Rollback、Replay、Fail）
- `ResourceGovernor` + `QuotaManager` — 每个 Agent 的资源配额（token、任务、内存）
- `RecursiveSentry` — 防止 Agent 循环中的无限递归
- `GracefulShutdown` — 带状态持久化的优雅终止
- `StateDatabase`（SQLite）— 持久化事件/任务/追踪/会话

### 性能：编译型 vs 解释型

| 指标 | OpenClaw (Node.js) | Aleph (Rust) |
|------|-------------------|--------------|
| 启动时间 | ~2-3 秒（V8 预热） | ~100 毫秒 |
| 工具分发 | JS 事件循环（单线程） | tokio 多线程异步 |
| 并发测试 | Vitest（单元测试） | Loom（穷举状态探索） |
| 属性测试 | 无 | Proptest（每个测试 1024+ 用例） |
| 内存安全 | GC 管理（可能泄漏） | 所有权系统（编译时保证） |

### OpenClaw 做得更好的地方

公平地说：

- **更容易扩展** — TypeScript 插件 vs Rust 编译
- **更大的技能库** — 52 个内置技能 + ClawHub 注册表
- **更成熟的通道** — WhatsApp（Baileys）、Signal、Zalo 经过生产验证
- **更简单的安装** — `npm install` vs Rust 工具链
- **移动节点** — iOS/Android 伴侣应用支持摄像头/位置/Canvas

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
