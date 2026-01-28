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

> *"这是人类历史上第一次，把一个机器的灵魂装进一个壳子。"*
> — 攻壳机动队 / Ghost in the Shell

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## 🔮 核心哲学：从知识到躯壳

Aether 不只是工具，而是一个理念的具象化 —— **让 AI 拥有干涉世界的能力**。

### 五层涌现架构

```
散落的积木 → 分类堆放 → 堆叠整齐 → 功能模块 → 多态智能体
   ↓            ↓           ↓          ↓           ↓
经验之海    领域分类    原子技能    即插即用    随需而变
(Know)     (Classify)  (Know-how)  (Compose)   (Embody)
```

| 层级 | 名称 | 本质转变 |
|------|------|----------|
| **L1** | 经验之海 (Sea of Knowledge) | 互联网、代码、历史、常识 — AI 的预训练养料 |
| **L2** | 领域分类 (Domain Classification) | 医学、法律、编程、物理 — 知识有了学科边界 |
| **L3** | 原子技能 (Atomic Skills) | **Know-what → Know-how** — 从拥有知识到拥有能力 |
| **L4** | 功能模块 (Functional Modules) | 技能封装，即插即用 — AI 可以组合能力达成目标 |
| **L5** | 多态智能体 (Polymorphic Agents) | **灵魂获得躯壳** — 随需变身，干涉物理/数字世界 |

### Aether 的三重定位

- **通向 AGI 的一条可能通路** — 当智能获得行动能力
- **杀鸡用的牛刀** — 过度设计是刻意的，因为 AI 能做一切
- **AI 海啸上的小船** — 巨浪有多高，它就能浮起多高，带着掌舵人走向浪潮之巅

### Ghost 美学

| 原则 | 实现 |
|------|------|
| **Invisible First** | 无 Dock 图标、无常驻窗口，只有后台进程 + 菜单栏 |
| **De-GUI** | 光标处涌现、任务后消融的临时 UI |
| **Frictionless** | AI 来到你身边，而不是你去找 AI |
| **Native-First** | 100% 原生代码 (Rust + Swift)，零 Webview |
| **Polymorphic** | 一个灵魂，无限形态 — 高达、战车、房屋、火箭、游乐场 |

---

## 🎯 Project Vision: Rust 版 Moltbot

**Aether** 正在进化为 **Rust 版 Moltbot** — 一个功能强大的个人 AI 助手系统。

> **参考实现**: [Moltbot](https://github.com/moltbot/moltbot) - TypeScript 版本，74.9k+ stars
> **我们的目标**: 用 Rust 重写核心，保持相同的架构设计和功能完整性

### 技术定位

**Aether 是一个自托管的个人 AI 助手**，你可以在自己的设备上运行。它通过统一的 Gateway 控制面连接多个消息渠道（WhatsApp、Telegram、Slack、Discord、iMessage 等），同时支持 macOS/iOS/Android 原生应用、语音交互、Canvas 可视化工作区。

**如果你想要一个本地化、快速、永远在线的个人助手，这就是它。**

---

## 🏗️ Moltbot 架构参考

### 核心架构图

```
WhatsApp / Telegram / Slack / Discord / Signal / iMessage / WebChat / ...
               │
               ▼
┌───────────────────────────────────────┐
│              Gateway                  │
│          (控制面 Control Plane)        │
│       ws://127.0.0.1:18789            │
│                                       │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ │
│  │ Routing │ │Sessions │ │  Tools  │ │
│  └─────────┘ └─────────┘ └─────────┘ │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ │
│  │  Cron   │ │Webhooks │ │ Events  │ │
│  └─────────┘ └─────────┘ └─────────┘ │
└──────────────┬────────────────────────┘
               │
               ├─── Agent Runtime (RPC mode)
               ├─── CLI (aether ...)
               ├─── WebChat UI
               ├─── macOS App (Menu Bar)
               └─── iOS / Android Nodes
```

### 关键子系统

| 子系统 | 描述 | Moltbot 参考 |
|--------|------|-------------|
| **Gateway** | WebSocket 控制面，统一管理 sessions、channels、tools、events | `src/gateway/` |
| **Channels** | 多消息渠道接入：WhatsApp、Telegram、Slack、Discord、Signal、iMessage 等 | `src/channels/`, `src/telegram/`, `src/discord/` |
| **Agent Runtime** | Pi agent 核心，RPC 模式，流式工具调用和 block streaming | `src/agents/` |
| **Sessions** | 会话管理：main session、group isolation、activation modes | `src/sessions/` |
| **Tools** | 第一等工具支持：browser、canvas、nodes、cron、sessions | `src/browser/`, `src/cron/` |
| **Nodes** | 设备节点：macOS/iOS/Android 执行节点，Canvas、Camera、Voice | `src/node-host/` |
| **Media Pipeline** | 媒体处理：图片/音频/视频，转录钩子，大小限制 | `src/media/` |

---

## 🚀 实现进度

### Phase 1: Gateway 控制面 (核心)

- [x] **WebSocket Server** - JSON-RPC 2.0 over WS `ws://127.0.0.1:18789`
- [x] **Session Management** - SQLite 持久化、compaction、auto-reset
- [x] **Event Distribution** - Topic-based EventBus 事件分发和订阅
- [x] **Config Reload** - 文件监听 + 热配置重载
- [x] **Auth & Security** - Bearer token 认证、设备配对、allowlist

### Phase 2: Multi-Channel 消息接入

- [x] **Channel Abstraction** - 统一的渠道 trait + feature-gated 实现
- [x] **iMessage** - macOS 原生 iMessage 接入 (macOS-only)
- [x] **Telegram** - Telegram Bot API (feature: telegram)
- [x] **Discord** - Discord Bot (feature: discord)
- [ ] **Slack** - Slack App
- [ ] **WhatsApp** - WhatsApp Web (Baileys-style)
- [x] **WebChat** - 内置 Web 聊天界面 + 静态文件服务
- [x] **CLI** - 命令行测试 channel

### Phase 3: Agent Runtime

- [x] **RPC Mode** - ExecutionEngine 桥接 Gateway 到 agent_loop
- [x] **Tool Streaming** - 流式工具调用
- [ ] **Block Streaming** - 流式内容块 (`<think>` streaming)
- [x] **Thinking Levels** - off/minimal/low/medium/high/xhigh + provider fallback
- [x] **Model Failover** - 多模型故障转移策略

### Phase 4: Tools & Automation

- [x] **Browser Control** - Chrome DevTools Protocol (CDP)
- [ ] **Canvas (A2UI)** - Agent 驱动的可视化工作区
- [x] **Cron Jobs** - 定时任务调度 (Croner)
- [ ] **Webhooks** - 外部触发器
- [ ] **Sessions Tools** - Agent 间通信：sessions_list、sessions_send

### Phase 5: Nodes & Apps

- [x] **macOS App** - Menu Bar 控制、完整 SwiftUI 界面
- [ ] **iOS Node** - Canvas、Voice、Camera，Bonjour 配对
- [ ] **Android Node** - Canvas、Camera、Screen Recording
- [ ] **Node Protocol** - 统一的节点通信协议

### Phase 6: Voice & Media

- [ ] **Voice Wake** - 语音唤醒 (ElevenLabs)
- [ ] **Talk Mode** - 持续对话模式
- [ ] **Media Pipeline** - 图片/音频/视频处理
- [ ] **Transcription** - 语音转文字

---

## 🔌 Gateway RPC 方法体系

Gateway 通过 HandlerRegistry 注册 RPC 方法，按域分组：

| 域 | 方法前缀 | 说明 | 状态 |
|----|---------|------|------|
| **health** | `health.*` | 健康检查、ping | ✅ |
| **auth** | `auth.*` | 设备配对、token 颁发 | ✅ |
| **agent** | `agent.*` | Agent 执行、状态查询 | ✅ |
| **session** | `session.*` | 会话管理、历史查询 | ✅ |
| **channel** | `channel.*` | 渠道状态、配置 | ✅ |
| **events** | `events.*` | 事件订阅/退订 | ✅ |
| **config** | `config.*` | 配置获取/patch/apply | ✅ |
| **browser** | `browser.*` | CDP 浏览器控制 | ✅ |
| **chat** | `chat.*` | 消息发送、中断、历史 | 🔲 |
| **cron** | `cron.*` | 定时任务管理 | ✅ |
| **nodes** | `nodes.*` | 设备节点管理 | 🔲 |
| **models** | `models.*` | 模型列表、目录 | 🔲 |
| **sessions** | `sessions.*` | 跨 session 通信工具 | 🔲 |
| **voicewake** | `voicewake.*` | 语音唤醒控制 | 🔲 |

### RPC 方法注册模式 (Rust)

`core/src/gateway/handlers/` 中实现：

- Async handler trait: `Fn(JsonRpcRequest) -> Future<Output = JsonRpcResponse>`
- 按域自动路由：`method: "agent.run"` → `handlers/agent.rs`
- 角色权限：operator (CLI/UI) 或 node (设备)
- 幂等键：side-effect 方法支持 idempotency key

---

## 🤝 WebSocket 连接握手

### 连接流程

客户端连接后，**第一帧必须是 connect 请求**，否则 Gateway 断开连接：

**Client → Gateway:**
```json
{
  "type": "req",
  "id": "uuid-xxx",
  "method": "connect",
  "params": {
    "minProtocol": 1,
    "maxProtocol": 1,
    "client": { "id": "cli", "version": "0.1.0", "platform": "macos" },
    "role": "operator",
    "scopes": ["operator.read", "operator.write"],
    "device": {
      "id": "device_fingerprint",
      "publicKey": "...",
      "signature": "...",
      "nonce": "challenge_nonce"
    },
    "auth": { "token": "bearer_token" }
  }
}
```

**Gateway → Client:**
```json
{
  "type": "res",
  "id": "uuid-xxx",
  "ok": true,
  "payload": {
    "type": "hello-ok",
    "protocol": 1,
    "auth": { "deviceToken": "...", "role": "operator" }
  }
}
```

### 客户端角色

| 角色 | 说明 | 权限 |
|------|------|------|
| `operator` | CLI、macOS App、Web UI | 完全控制权 |
| `node` | iOS/Android 设备节点 | 受限执行权限 |

### 消息类型

| Type | 方向 | 用途 |
|------|------|------|
| `req` | Client → Gateway | 客户端请求 |
| `res` | Gateway → Client | 请求响应 |
| `event` | Gateway → Client | 服务端推送事件 |
| `stream` | Gateway → Client | 流式数据帧 |

---

## 🔑 Session Key 层级

借鉴 Moltbot 的 session key 设计，Aether 实现了 6 种 session key 变体（`core/src/routing/session_key.rs`）：

| 变体 | 格式 | 用途 |
|------|------|------|
| **Main** | `agent:main:main` | 跨渠道共享主 session |
| **DirectMessage** | `agent:main:telegram:dm:user123` | DM 对话，支持 DmScope |
| **Group** | `agent:main:discord:group:guild-id` | 群组/频道会话 |
| **Task** | `agent:main:cron:daily-summary` | 定时任务、webhook |
| **Subagent** | `subagent:agent:main:translator` | 子 agent 委托 |
| **Ephemeral** | `agent:main:ephemeral:uuid` | 单轮临时会话 |

### DM Scope 策略

| Scope | 说明 |
|-------|------|
| `Main` | 所有 DM 共享主 session |
| `PerPeer` | 每用户隔离（跨渠道）**[默认]** |
| `PerChannelPeer` | 每渠道每用户隔离 |

---

## 🧩 Plugin & Extension 系统

### 架构

借鉴 Moltbot 的插件架构，Aether 支持：

- **Channels as Plugins** — 每个消息渠道是独立插件（feature-gated crate）
- **Hook System** — 事件驱动的钩子：Gmail 监听、Slack 集成、Webhook 触发
- **Extension Registry** — 动态发现和加载插件
- **Schema Registration** — 插件可注册自己的配置 schema + UI hints

### 目标插件列表

**Core Channels** (feature-gated):
- `telegram`, `discord`, `slack`, `imessage`, `webchat`

**Extension Channels** (独立 crate):
- `signal`, `whatsapp`, `matrix`, `msteams`, `google-chat`

**Hooks**:
- Gmail → 邮件监听和自动回复
- Slack → Slack 事件集成
- Webhook → HTTP 触发器
- Custom → 用户自定义钩子

---

## 🤖 多 Agent 编排

### Agent 实例隔离

每个 AgentInstance (`core/src/gateway/agent_instance.rs`) 拥有：
- 独立工作区目录
- 独立 session 存储
- 独立配置（model、thinking、identity）
- 独立状态机（Idle → Running → Paused → Error → Stopping）

### Sub-Agent 委托

主 agent 可通过 `sessions_send` 工具委托子 agent：
- TaskTool 调用子 agent 执行特定任务
- McpSubAgent 连接 MCP 服务
- SkillSubAgent 调用预定义技能
- Session key 自动嵌套：`subagent:agent:main:translator`

### Tool Policy

- **Main session**: 完全工具访问
- **Non-main sessions**: 工具白名单
- **Sudo approval**: 危险操作需要用户确认
- **Sandbox**: 可选 Docker 隔离

---

## 📁 项目结构

```
aether/
├── core/                           # Rust Core (aethecore crate)
│   └── src/
│       ├── gateway/                # WebSocket 控制面 (23 files)
│       │   ├── server.rs           # WS 服务器 + GatewayConfig
│       │   ├── protocol.rs         # JSON-RPC 2.0 类型
│       │   ├── router.rs           # AgentRouter
│       │   ├── session_manager.rs  # SQLite session 持久化
│       │   ├── agent_instance.rs   # Agent 实例隔离
│       │   ├── execution_engine.rs # 桥接 agent_loop
│       │   ├── event_bus.rs        # 事件分发
│       │   ├── hot_reload.rs       # 配置热重载
│       │   └── handlers/           # RPC 方法处理器
│       │       ├── auth.rs         # 设备配对、token
│       │       ├── agent.rs        # Agent 执行
│       │       ├── session.rs      # Session 管理
│       │       ├── channel.rs      # 渠道状态
│       │       ├── events.rs       # 事件订阅
│       │       ├── config.rs       # 配置管理
│       │       └── browser.rs      # CDP 控制
│       ├── routing/                # Session 路由 (NEW)
│       │   └── session_key.rs      # 6 种 SessionKey 变体
│       ├── channels/               # 消息渠道 (feature-gated)
│       │   ├── mod.rs              # Channel trait
│       │   ├── telegram.rs         # Telegram Bot API
│       │   ├── discord.rs          # Discord Bot
│       │   ├── imessage.rs         # iMessage (macOS)
│       │   └── webchat.rs          # WebChat
│       ├── agent_loop/             # 核心 Observe-Think-Act 循环 (15 files)
│       ├── agents/                 # Agent 系统 (12 files)
│       │   ├── thinking.rs         # Thinking levels
│       │   ├── thinking_adapter.rs # Provider 适配
│       │   ├── rig/                # Rig-core agent
│       │   └── sub_agents/         # 子 agent 委托
│       ├── providers/              # AI 提供商 (17 files)
│       │   ├── claude.rs           # Anthropic Claude
│       │   ├── openai/             # OpenAI GPT
│       │   ├── gemini.rs           # Google Gemini
│       │   ├── ollama.rs           # Local Ollama
│       │   └── failover.rs         # 多模型故障转移
│       ├── browser/                # Chrome CDP (4 files)
│       ├── cron/                   # 定时任务
│       ├── memory/                 # sqlite-vec 向量记忆
│       ├── mcp/                    # MCP 协议集成
│       ├── extension/              # 扩展系统
│       ├── config/                 # 配置系统 + hot reload
│       └── lib.rs                  # 100+ public modules
├── platforms/
│   ├── macos/                      # macOS App (Swift/SwiftUI)
│   │   ├── Aether/                 # Sources (45 dirs)
│   │   │   ├── Gateway/            # WS 客户端
│   │   │   ├── Components/         # UI 组件
│   │   │   ├── DesignSystem/       # macOS 26 设计
│   │   │   └── Settings/           # 设置页面
│   │   └── project.yml             # XcodeGen 配置
│   ├── tauri/                      # Cross-platform Tauri App
│   │   ├── src-tauri/              # Rust backend
│   │   └── src/                    # React frontend
│   └── archived/                   # 归档平台
├── docs/
│   └── plans/                      # 设计文档
├── Cargo.toml                      # Workspace root
├── CLAUDE.md                       # 本文档
└── README.md                       # 用户文档
```

---

## ⚙️ Technical Stack

| Layer | Technology |
|-------|------------|
| **Gateway Core** | Rust + tokio + tokio-tungstenite + axum |
| **Agent Runtime** | Rust + async/await + rig-core |
| **Session Storage** | SQLite + sqlite-vec (向量搜索) |
| **FFI Bridge** | UniFFI 0.31+ (保留用于 Data Plane) |
| **macOS App** | Swift + SwiftUI + AppKit |
| **Cross-platform** | Tauri + React + TypeScript |
| **Browser Control** | Chrome DevTools Protocol (CDP) |
| **Schema** | schemars + serde + JSON Schema |

---

## 📋 Configuration

### 配置文件

`~/.aether/config.json` (JSON5 格式，支持注释和尾逗号):

```json5
{
  // Agent 配置
  agents: {
    defaults: {
      workspace: "~/aether-workspace",
      model: "anthropic/claude-opus-4-5",
      thinking: "medium",
    },
    list: [{
      id: "main",
      identity: "You are Aether, a helpful personal AI assistant.",
      groupChat: { requireMention: true },
    }],
  },

  // Gateway 配置
  gateway: {
    port: 18789,
    bind: "loopback",
  },

  // 渠道配置
  channels: {
    telegram: {
      token: "BOT_TOKEN",
      allowFrom: ["+1234567890"],
      groups: { "*": { requireMention: true } },
    },
  },

  // Session 策略
  session: {
    dmScope: "per-peer",
    autoResetHour: 4,
    expiryDays: 30,
  },
}
```

### 配置热更新

- `config.get` — 获取当前配置 + hash
- `config.patch` — JSON Merge Patch 部分更新
- `config.apply` — 完整替换 + 重启
- **Strict Validation** — 未知 key 拒绝启动，`aether doctor` 诊断修复
- **Schema-driven UI** — 配置 schema 导出为 JSON Schema，Control UI 自动渲染表单

完整配置参考 Moltbot: [Configuration](https://docs.molt.bot/gateway/configuration)

---

## 💻 CLI 命令体系

```bash
# Gateway 管理
aether gateway run [--port 18789] [--bind loopback]
aether gateway status
aether gateway call <method> --params '<json>'

# Agent 交互
aether agent --message "Hello" [--thinking high] [--session main]
aether agent abort [--session <key>]

# 渠道管理
aether channels status
aether channels login <channel>          # QR for WhatsApp, token for Telegram

# 配置
aether config get
aether config set <key> <value>
aether config edit                       # 打开编辑器

# 设备配对
aether pairing approve <channel> <code>
aether pairing list
aether pairing revoke <device-id>

# 定时任务
aether cron list
aether cron run <job-id>

# 诊断
aether doctor [--fix]
aether logs [--follow]
aether health
```

---

## 🔐 Gateway 安全设计

### Token 认证机制

**不要让 WS 裸奔。** Core 启动时必须实施认证：

```
┌─────────────────────────────────────────────────────────────┐
│                    Token 认证流程                            │
├─────────────────────────────────────────────────────────────┤
│  1. Core 启动时生成随机 App-Token                            │
│  2. Token 存储在本地安全目录（macOS Keychain / Linux secret）│
│  3. UI 启动后读取 Token 作为 WS 连接的初始校验               │
│  4. WS 握手时验证 Token，拒绝未授权连接                      │
└─────────────────────────────────────────────────────────────┘
```

### 双向验证

借鉴 Moltbot 的设备身份认证：
- **设备配对**: 新设备首次连接需要配对码确认
- **Session Token**: 配对成功后颁发长期 Session Token
- **Origin 校验**: 拒绝非预期来源的 WS 连接请求

### Security Model

- **DM Pairing (默认)**: 未知发送者收到配对码，通过 `aether pairing approve` 批准
- **Main session**: 完全工具访问
- **Non-main sessions**: Docker 沙箱隔离（可选）
- **Tool Policy**: 工具白名单/黑名单控制

---

## 🔀 混合模式 (Hybrid Approach)

完全放弃 UniFFI 会在某些场景下损失效率。采用 **Control Plane + Data Plane** 分离：

```
┌─────────────────────────────────────────────────────────────┐
│                     混合架构                                 │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────┐      ┌─────────────────┐              │
│  │  Control Plane  │      │   Data Plane    │              │
│  │   (WebSocket)   │      │    (UniFFI)     │              │
│  ├─────────────────┤      ├─────────────────┤              │
│  │ • 异步指令       │      │ • 大量数据读取   │              │
│  │ • 流式输出       │      │ • 搜索索引查询   │              │
│  │ • 状态广播       │      │ • 文件系统访问   │              │
│  │ • 事件订阅       │      │ • 内存共享数据   │              │
│  └─────────────────┘      └─────────────────┘              │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

| 通道 | 用途 | 技术 |
|------|------|------|
| **Control Plane** | 异步指令、流式输出、状态广播、事件订阅 | WebSocket |
| **Data Plane** | 大量数据读取、搜索索引、只读高性能访问 | UniFFI (保留) |

---

## 🔧 Quick Commands

```bash
# Rust Core (Gateway + Agent Runtime)
cd core && cargo build && cargo test

# 启动 Gateway (feature-gated)
cd core && cargo run --features gateway --bin aether-gateway -- --port 18789

# macOS App
cd platforms/macos && xcodegen generate && open Aether.xcodeproj

# Tauri App
cd platforms/tauri && pnpm install && pnpm tauri dev

# 特定 feature 编译
cargo build -p aethecore --features "gateway,telegram,discord,cron,browser"
```

---

## 📚 Reference

### Moltbot 文档

- [架构概览](https://docs.molt.bot/concepts/architecture)
- [Gateway 配置](https://docs.molt.bot/gateway/configuration)
- [Channels](https://docs.molt.bot/channels)
- [Tools](https://docs.molt.bot/tools)
- [Nodes](https://docs.molt.bot/nodes)
- [Security](https://docs.molt.bot/gateway/security)

### Moltbot 源码

- Gateway: `/Users/zouguojun/Workspace/moltbot/src/gateway/`
- Agents: `/Users/zouguojun/Workspace/moltbot/src/agents/`
- Channels: `/Users/zouguojun/Workspace/moltbot/src/channels/`
- Config: `/Users/zouguojun/Workspace/moltbot/src/config/`
- Routing: `/Users/zouguojun/Workspace/moltbot/src/routing/`

---

## 🛠️ Development

### Branch Strategy

**单分支开发模式**：所有开发工作直接在 main 分支进行。

### Git Commit

English commit messages. Format: `<scope>: <description>`

Example: `gateway: add WebSocket server foundation`

### Language

- Reply in Chinese
- Code comments in English
- Documentation in both

### Skills

Use skills from: `~/.claude/skills/build-macos-apps`

---

## 📝 Session

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.

### Key Context

- **项目定位**: Rust 版 Moltbot
- **核心架构**: Gateway 控制面 + Multi-Channel + Agent Runtime + Nodes
- **参考实现**: `/Users/zouguojun/Workspace/moltbot/`
- **当前聚焦**: Session Key 实现完成，下一步 Agent 间通信 (Part B)
