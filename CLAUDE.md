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

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## 🎯 Project Vision: Rust 版 Moltbot

**Aether** 正在进化为 **Rust 版 Moltbot** — 一个功能强大的个人 AI 助手系统。

> **参考实现**: [Moltbot](https://github.com/moltbot/moltbot) - TypeScript 版本，74.9k+ stars
> **我们的目标**: 用 Rust 重写核心，保持相同的架构设计和功能完整性

### 核心定位

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

## 🚀 目标功能清单

### Phase 1: Gateway 控制面 (核心)

- [ ] **WebSocket Server** - 本地控制面 `ws://127.0.0.1:18789`
- [ ] **Session Management** - 会话生命周期、隔离、路由
- [ ] **Event Distribution** - 事件分发和订阅
- [ ] **Config Reload** - 热配置重载
- [ ] **Auth & Security** - 认证、DM pairing、allowlist

### Phase 2: Multi-Channel 消息接入

- [ ] **Channel Abstraction** - 统一的渠道抽象层
- [ ] **iMessage** - macOS 原生 iMessage 接入
- [ ] **Telegram** - Telegram Bot API
- [ ] **Discord** - Discord Bot
- [ ] **Slack** - Slack App
- [ ] **WhatsApp** - WhatsApp Web (Baileys)
- [ ] **WebChat** - 内置 Web 聊天界面

### Phase 3: Agent Runtime

- [ ] **RPC Mode** - Agent 以 RPC 模式运行
- [ ] **Tool Streaming** - 流式工具调用
- [ ] **Block Streaming** - 流式内容块
- [ ] **Thinking Levels** - off/minimal/low/medium/high/xhigh
- [ ] **Model Failover** - 多模型故障转移

### Phase 4: Tools & Automation

- [ ] **Browser Control** - 托管 Chrome/Chromium，CDP 控制
- [ ] **Canvas (A2UI)** - Agent 驱动的可视化工作区
- [ ] **Cron Jobs** - 定时任务
- [ ] **Webhooks** - 外部触发器
- [ ] **Sessions Tools** - Agent 间通信：sessions_list、sessions_send

### Phase 5: Nodes & Apps

- [ ] **macOS App** - Menu Bar 控制、Voice Wake、Push-to-Talk
- [ ] **iOS Node** - Canvas、Voice、Camera，Bonjour 配对
- [ ] **Android Node** - Canvas、Camera、Screen Recording
- [ ] **Node Protocol** - 统一的节点通信协议

### Phase 6: Voice & Media

- [ ] **Voice Wake** - 语音唤醒 (ElevenLabs)
- [ ] **Talk Mode** - 持续对话模式
- [ ] **Media Pipeline** - 图片/音频/视频处理
- [ ] **Transcription** - 语音转文字

---

## 📁 目标项目结构

```
aether/
├── core/                           # Rust Core
│   └── src/
│       ├── gateway/                # WebSocket 控制面
│       │   ├── server.rs           # WS 服务器
│       │   ├── protocol.rs         # 协议定义
│       │   ├── auth.rs             # 认证
│       │   ├── routing.rs          # 路由
│       │   └── hooks.rs            # 钩子系统
│       ├── channels/               # 消息渠道
│       │   ├── mod.rs              # 渠道抽象
│       │   ├── telegram.rs         # Telegram
│       │   ├── discord.rs          # Discord
│       │   ├── slack.rs            # Slack
│       │   ├── imessage.rs         # iMessage
│       │   └── webchat.rs          # WebChat
│       ├── agents/                 # Agent Runtime
│       │   ├── runtime.rs          # RPC 运行时
│       │   ├── session.rs          # 会话管理
│       │   ├── tools.rs            # 工具执行
│       │   └── streaming.rs        # 流式处理
│       ├── tools/                  # 内置工具
│       │   ├── browser.rs          # 浏览器控制
│       │   ├── canvas.rs           # Canvas/A2UI
│       │   ├── cron.rs             # 定时任务
│       │   └── sessions.rs         # Session 工具
│       ├── nodes/                  # 设备节点
│       │   ├── protocol.rs         # 节点协议
│       │   ├── registry.rs         # 节点注册
│       │   └── commands.rs         # 节点命令
│       ├── media/                  # 媒体处理
│       │   ├── pipeline.rs         # 处理管道
│       │   ├── transcription.rs    # 转录
│       │   └── storage.rs          # 存储
│       └── config/                 # 配置系统
│           ├── loader.rs           # 配置加载
│           └── hot_reload.rs       # 热重载
├── platforms/
│   ├── macos/                      # macOS App
│   │   └── Sources/
│   │       ├── Gateway/            # Gateway 客户端
│   │       ├── MenuBar/            # Menu Bar 控制
│   │       ├── VoiceWake/          # 语音唤醒
│   │       └── Canvas/             # Canvas 渲染
│   ├── ios/                        # iOS Node
│   └── android/                    # Android Node
├── extensions/                     # 扩展插件
│   ├── msteams/                    # Microsoft Teams
│   ├── matrix/                     # Matrix
│   └── signal/                     # Signal
├── ui/                             # Web UI
│   ├── webchat/                    # WebChat 界面
│   └── control/                    # Control UI
└── docs/                           # 文档
```

---

## ⚙️ Technical Stack

| Layer | Technology |
|-------|------------|
| **Gateway Core** | Rust + tokio + tokio-tungstenite (WebSocket) |
| **Agent Runtime** | Rust + async/await + streaming |
| **FFI Bridge** | UniFFI 0.31+ (保留用于 Data Plane) |
| **macOS App** | Swift + SwiftUI + AppKit + URLSessionWebSocketTask |
| **iOS/Android** | Swift/Kotlin + Native UI |
| **Web UI** | React + TypeScript + Vite |
| **Browser Control** | Chrome DevTools Protocol (CDP) |
| **Database** | SQLite (sessions, config) |

---

## 🔄 架构演进: UniFFI → WebSocket Gateway

### 为什么转向 Gateway 模式

将 Aether 从 UniFFI 的 FFI 调用转向基于 WebSocket 的 **"网关模式"** 是一个激进但具备高度前瞻性的架构演进。这种模式在 Raycast、VS Code Remote 等高性能多端协作工具中都有类似应用。

### 优势 (Pros)

| 优势 | 说明 |
|------|------|
| **原生流式输出** | WS 天然支持全双工通信，LLM Stream 响应可直接封装成多个 JSON frame 推送，彻底解决 UniFFI 阻塞同步调用的瓶颈 |
| **通信层解耦** | Rust Core 成为独立 Service，UI 变成纯粹 Client。可用任何语言编写 UI，不再处理复杂的内存所有权跨界传递（FFI 的噩梦） |
| **统一事件总线** | 轻松实现"单处操作，多处更新"。CLI 删除记录，macOS UI 可通过监听 WS 事件实时同步 |
| **多端协作** | 同一 Gateway 可同时服务 macOS App、iOS Node、Web UI、CLI 等多个客户端 |

### 挑战 (Cons)

| 挑战 | 说明 | 应对策略 |
|------|------|----------|
| **生命周期管理** | Core Service 何时启动？UI 挂起时 WS 断开，Core 状态如何保持？ | In-process Server 模式 |
| **安全性风险** | 本地 18789 端口暴露核心 API，恶意本地网页可能尝试 CSWSH 攻击 | Token 认证 + Origin 校验 |
| **序列化开销** | 频繁大数据量传输（如知识库索引）在 JSON/WS 下性能不如 FFI 直接内存共享 | 混合模式：Control Plane WS + Data Plane FFI |

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

---

## 📡 通信协议设计

### JSON-RPC 2.0 风格

定义严谨的信令结构，参考 JSON-RPC 2.0：

```json
{
  "id": "uuid-123",
  "type": "request",
  "method": "chat/stream",
  "params": {
    "message": "Hello",
    "thinking": "high"
  }
}
```

```json
{
  "id": "uuid-123",
  "type": "response",
  "result": {
    "content": "...",
    "done": false
  }
}
```

```json
{
  "type": "event",
  "event": "session/updated",
  "data": {
    "sessionId": "...",
    "status": "active"
  }
}
```

### 消息类型

| Type | 方向 | 用途 |
|------|------|------|
| `request` | Client → Gateway | 客户端请求 |
| `response` | Gateway → Client | 请求响应 |
| `event` | Gateway → Client | 服务端推送事件 |
| `stream` | Gateway → Client | 流式数据帧 |

### 二进制数据处理

对于文件上传、图片生成等场景：
- 支持 **MessagePack** 序列化（可选）
- WS 支持 **Binary Frame**，避免 Base64 体积膨胀
- 大文件走独立的文件传输通道

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

## 🚀 进程模式设计

### In-Process Server (推荐初期方案)

Rust Core 随 UI 启动而启动，作为 UI 进程的一个子线程运行：

```
┌─────────────────────────────────────────┐
│            macOS App Process            │
│  ┌─────────────────────────────────┐   │
│  │         Swift UI Layer          │   │
│  └──────────────┬──────────────────┘   │
│                 │ WS (localhost)        │
│  ┌──────────────▼──────────────────┐   │
│  │     Rust Core (In-Process)      │   │
│  │  ┌──────────────────────────┐   │   │
│  │  │   Gateway (port 18789)   │   │   │
│  │  └──────────────────────────┘   │   │
│  └─────────────────────────────────┘   │
└─────────────────────────────────────────┘
```

**优势**：
- 生命周期管理最简单
- 无需单独的 Daemon 进程
- 资源随 App 释放

### 独立 Daemon (后期演进)

当需要支持"App 关闭后继续运行"时，演进为独立 Daemon：

```
┌─────────────────┐     ┌─────────────────┐
│   macOS App     │     │   iOS Node      │
└────────┬────────┘     └────────┬────────┘
         │ WS                    │ WS
         └───────────┬───────────┘
                     ▼
         ┌───────────────────────┐
         │   Aether Daemon       │
         │   (launchd/systemd)   │
         └───────────────────────┘
```

---

## 🔄 错误处理与重连

### 自动重连机制

WS 报错会导致连接断开，必须实现自动重连：

```
Exponential Backoff 策略:
  第 1 次重试: 1s
  第 2 次重试: 2s
  第 3 次重试: 4s
  第 4 次重试: 8s
  最大间隔:    30s
```

### 状态恢复

重连后需要恢复状态：
1. 重新订阅事件
2. 同步丢失的状态变更
3. 恢复进行中的流式响应（如果可能）

---

## 📦 Schema 共享

使用 Rust 定义 DTO，自动生成多端类型定义：

```
┌─────────────────────────────────────────────────────────────┐
│                    Schema 共享流程                           │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  core/src/protocol/types.rs  (Rust 定义)                    │
│            │                                                │
│            ▼                                                │
│  ┌─────────────────────────────────────────┐               │
│  │         Schema Generator                 │               │
│  │   (ts-rs / schemars / 自定义脚本)        │               │
│  └─────────────────────────────────────────┘               │
│            │                                                │
│     ┌──────┼──────┐                                        │
│     ▼      ▼      ▼                                        │
│  Swift   TypeScript  Kotlin                                │
│  Types    Types      Types                                 │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

**工具选择**:
- **TypeScript**: `ts-rs` crate 或 `schemars` → JSON Schema → `json-schema-to-typescript`
- **Swift**: 自定义脚本或 `Codable` 兼容的 JSON Schema 生成
- **Kotlin**: `kotlinx.serialization` 兼容的 Schema

---

## 🔧 Quick Commands

```bash
# Rust Core (Gateway + Agent Runtime)
cd core && cargo build && cargo test

# 启动 Gateway
cd core && cargo run --bin aether-gateway -- --port 18789

# macOS App
cd platforms/macos && xcodegen generate && open Aether.xcodeproj

# Web UI
cd ui && pnpm install && pnpm dev

# CLI
aether gateway run --port 18789
aether agent --message "Hello" --thinking high
aether channels status
```

---

## 📋 Configuration

最小配置 `~/.aether/config.json`:

```json5
{
  "agent": {
    "model": "anthropic/claude-opus-4-5"
  },
  "gateway": {
    "port": 18789,
    "bind": "loopback"
  }
}
```

完整配置参考 Moltbot: [Configuration](https://docs.molt.bot/gateway/configuration)

---

## 🔒 Security Model

### DM Pairing (默认)

- 未知发送者收到配对码
- 通过 `aether pairing approve <channel> <code>` 批准
- 批准后加入本地 allowlist

### Sandbox Mode

- **Main session**: 完全工具访问
- **Non-main sessions**: Docker 沙箱隔离（可选）
- 工具白名单/黑名单控制

---

## 🎯 Development Priorities

### 当前聚焦: Gateway 控制面

1. **WebSocket Server** - 基础 WS 服务器
2. **Session Protocol** - 会话协议定义
3. **Event System** - 事件分发机制
4. **Config System** - 配置加载和热重载

### 迁移策略

| 现有模块 | 目标模块 | 状态 |
|----------|----------|------|
| `agent_loop` | `agents/runtime` | 重构 |
| `dispatcher` | `gateway/routing` | 重构 |
| `ffi/processing` | `gateway/protocol` | 重构 |
| `components` | `agents/components` | 保留 |
| `extension` | `extensions/` | 扩展 |

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
- **当前聚焦**: Gateway 控制面基础建设
