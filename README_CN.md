# Aleph (ℵ)

> 你的个人 AI 助手 —— **原生桌面体验，远程随时可达**。

[![Rust](https://img.shields.io/badge/Rust-1.92%2B-b7410e)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)]()

[English](README.md)

<p align="center">
  <img src="docs/images/aleph-desktop.png" alt="Aleph 桌面 App（macOS）" width="900">
</p>

---

## Aleph 是什么？

Aleph 是一款运行在**你自己设备上**的自托管个人 AI 助手。它以**原生桌面 App** 的形式发布，支持 macOS、Windows、Linux —— 下载 `.dmg` / `.msi` / `.deb`，双击安装，一分钟之内就有一个会话式、多模态的 AI 助手常驻在你的系统托盘里。

桌面 App 内置 `aleph-server`（你的私有 AI 大脑）。因为大脑一直在线，Aleph 也能从你已经在用的渠道找到你 —— Telegram、Discord、Slack、WhatsApp、iMessage、Matrix、IRC、邮件，以及十几种其他通道 —— 让你在手机、手表、任何浏览器上都能继续和它对话。

**一颗大脑。多个入口。你的数据，你的设备。**

---

## 为什么是桌面 App？

Aleph 最初是一个 headless server。我们围绕真实的桌面 App 重新构建它，是因为这才是个人 AI 应该待的地方 —— 紧挨着你的工作，而不是藏在一个 URL 后面。

### 🖥️ 一键安装，长期常驻

下载 → 双击 → 完成。安装包自带 `aleph-server` 守护进程，自动注册开机自启，常驻系统托盘。不需要端口转发，不需要 `docker compose`，不需要打开终端 —— 非开发者也能用。

### 💬 真正的对话体验

精致的聊天面板（Leptos + WASM）：流式响应、代码块、文件拖拽、图片预览、语音输入、内联工具调用、审批弹窗。这是你期待的 Claude / ChatGPT 桌面 App 应有的体验 —— 但你对话的是**你自己的**助手，用着**你自己的**记忆和**你自己的**工具。

### ⚙️ 可视化配置，告别 YAML

选模型、粘 API Key、开关通道、安装技能、管理记忆笔记 —— 全部在 App 内的设置面板里完成。配置文件依然存在供高级玩家使用，但你不必碰它们。

### 🖱️ 真正原生

- **系统托盘** —— 永远在那，不打扰
- **全局唤起热键** —— `⌘ ⇧ Space`（可配置），任何场景秒呼出聊天
- **系统通知** —— 任务结果、Daemon 提醒、审批请求通过原生通知推送
- **`aleph://` 深链接** —— 从浏览器、其他 App 或快捷指令直接发起任务
- **自动更新** —— 已签名的更新静默下载，你想重启时再重启
- **原生输入 / 截屏 / 剪贴板** —— 经你明确授权后，Aleph 能读屏、能输入

### 🔒 数据全在本地

所有对话、记忆笔记、向量、凭据都存在 `~/.aleph/`。Aleph 只与你配置的 LLM 提供商（或本地 Ollama）通讯。**没有任何数据流经第三方云**。

---

## 远程通道 —— 助手跟你走

桌面 App 是大本营，但 Aleph 也向外延伸。通过统一的 **Gateway**，同一颗大脑可以处理来自以下渠道的消息：

| 类别 | 通道 |
|------|------|
| **聊天** | Telegram · Discord · Slack · WhatsApp · iMessage · 微信 · QQ · 飞书 · Matrix · IRC · LINE · Mattermost · MS Teams · XMPP · Signal · Nostr |
| **异步** | Email · Webhook |
| **高级** | Web Chat（浏览器）· CLI · TUI · MCP · A2A · ACP |

在设置面板里配置一次通道，你就能从手机、团队的 Slack、咖啡厅的浏览器找到它 —— 使用同样的记忆、同样的技能、同样的身份。

> **R5 设计原则 —「AI 主动到达」**：Aleph 不强迫你切换 App，而是出现在你已有的工作通道里。

---

## 核心亮点

### 🧠 认知记忆，不只是 RAG
- **笔记层** —— Markdown 记忆 + Obsidian 风格 `[[wikilink]]` 知识图谱
- **混合检索** —— 向量 ANN（sqlite-vec）+ FTS5 全文 + wikilink 图谱遍历，支持多跳推理
- **自我学习** —— 自动从笔记模式中提炼技能
- **梦境守护进程** —— 后台压缩，把记忆合成为更高层的概念

### 🤖 薄 Harness Agent 循环
基于 **LLM 主权原则**：极简的 `Think → Act` 循环（约 1500 行），把意图理解、工具选择、完成度判断、安全评估**全部**交给模型。模型越强，Aleph 越强 —— 无需修改循环代码。

### 🔌 处处可插拔
- **30+ 内置工具**（文件系统、Shell、浏览器、视觉、OCR、记忆 ...）
- **MCP** 客户端，接入外部工具服务器
- **技能**（Python / Shell 脚本）+ **WASM / Node.js 扩展**
- **多供应商 LLM**：Claude · GPT · Gemini · DeepSeek · Ollama · Moonshot · Kimi · 通义千问

### 🛡️ 默认沙箱
每个工具都跑在 OS 原生沙箱里（Seatbelt / Landlock / AppContainer），带能力账本、默认拒绝的网络/文件策略，对高风险操作显式请求用户授权。

---

## 架构（一核多端）

```
                ┌─────────────────────────────────────────┐
                │      🖥  原生桌面 App (Tauri)            │
                │  聊天面板 · 托盘 · 热键 · 系统通知       │
                └──────────────────┬──────────────────────┘
                                   │  JSON-RPC（本地）
┌────────────┐  远程    ┌──────────▼──────────┐ ┌────────────────┐
│  浏览器     │────────▶│      Gateway         │◀│ Telegram bot   │
│ (Web Chat) │  WS     │  （认证 · 会话 ·     │ │ Discord · Slack│
└────────────┘         │   通道注册）          │ │ WhatsApp · …   │
                       └───────────┬──────────┘ └────────────────┘
                                   │
                       ┌───────────▼──────────┐
                       │   Orchestrator       │
                       │   → Harness          │
                       │      (Think→Act)     │
                       │   → Thinker (LLM)    │
                       └─┬─────┬─────┬─────┬──┘
                         ▼     ▼     ▼     ▼
                     Session Tools Memory Sandbox
                                   │
                            ┌──────┴──────┐
                            │  ~/.aleph/  │
                            │  SQLite +   │
                            │  sqlite-vec │
                            └─────────────┘
```

完整设计：[docs/reference/ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) · [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md)

---

## 安装

从 [最新 release](https://github.com/rootazero/Aleph/releases/latest) 下载你所在平台的安装包：

| 平台 | 安装包 |
|------|--------|
| macOS | `.dmg`（Apple Silicon + Intel） |
| Windows | `.msi` |
| Linux | `.deb` · `.AppImage` |

App 已内置 `aleph-server`。首次启动会拉起守护进程、注册开机自启，并常驻系统托盘。点托盘图标或按下全局热键即可呼出聊天面板。

> 需要 Node.js / Python 运行时的技能：**设置 → Runtime** 一次性引导安装。

### 数据目录

所有数据存放在 `~/.aleph/`：

```
~/.aleph/
├── aleph.toml       # 主配置（通道、供应商 ...）
├── data/            # SQLite + sqlite-vec（记忆、会话、向量）
├── logs/            # 服务日志
├── skills/          # 已安装技能
├── plugins/         # 扩展
└── workspaces/      # 每会话沙箱目录
```

---

## 从源码构建

前置条件：Rust 1.92+、[`just`](https://github.com/casey/just)、Node.js、`wasm-bindgen`、Swift 工具链（仅 macOS）。

```bash
git clone https://github.com/rootazero/Aleph.git
cd Aleph
just shell-dev       # 以开发模式启动桌面 App（自动构建 WASM）
```

| 命令 | 说明 |
|------|------|
| `just shell-dev` | 以开发模式启动桌面 App |
| `just shell-build` | 构建已签名的桌面安装包（`.dmg` / `.msi` / `.deb`） |
| `just dev` | 仅启动 `aleph-server`（headless，调试模式） |
| `just build` | Release 构建（WASM + Server） |
| `just test-all` | 跑全部测试（core + desktop + proptest） |
| `just clippy` | Lint |
| `just verify-build` | CI 三平台构建验证（不打 tag、不发布） |
| `just release YY.M.D` | 打 tag 并触发 GitHub workflow 发布 |

### Headless / 仅服务模式

不想要桌面 GUI？你依然可以直接运行 `aleph-server` —— 它就是 App 内置的同一个二进制。适合放在 VPS 上，仅通过 Web Chat 和通道 Bot 使用。

```bash
cargo run --bin aleph-server start
```

---

## 项目结构

```
Aleph/
├── src/                 # Rust 核心（alephcore crate）
│   ├── gateway/         # JSON-RPC 控制平面 + 通道接口
│   ├── orchestrator/    # AgentDef 解析 + Harness 调度
│   ├── harness/         # Think→Act 循环（薄，约 1500 行）
│   ├── thinker/         # LLM 交互层
│   ├── memory/          # SQLite + sqlite-vec（笔记、向量、FTS）
│   ├── builtin_tools/   # 30+ 内置工具
│   ├── sandbox/         # OS 原生隔离
│   ├── providers/       # 多协议 LLM 客户端
│   ├── mcp/             # MCP 客户端
│   ├── extension/       # WASM + Node.js 插件系统
│   └── ...              # session · approval · scheduler · daemon · ...
├── desktop/
│   ├── shell/           # Tauri 桌面 App（托盘、热键、通知 ...）
│   ├── shared/          # DesktopCapability trait + IPC 协议
│   ├── macos/ + bridge/ # macOS 原生（AppKit、Vision、Swift bridge）
│   ├── windows/         # Windows 原生（Win32）
│   └── linux/           # Linux 原生（Wayland/X11）
├── interfaces/
│   ├── webchat/         # Leptos + WASM Panel UI（桌面与浏览器共用）
│   ├── cli/             # CLI 客户端
│   └── tui/             # TUI 客户端
├── plugins/             # 内置插件 crate
├── docs/reference/      # 架构与系统文档
└── justfile             # 构建流水线
```

---

## 文档

| 文档 | 链接 |
|------|------|
| 架构 | [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) |
| Harness 哲学 | [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) |
| Agent 系统 | [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) |
| Gateway 协议 | [GATEWAY.md](docs/reference/GATEWAY.md) |
| 工具系统 | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| 记忆系统 | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| 沙箱 | [SANDBOX.md](docs/reference/SANDBOX.md) |
| 安全 | [SECURITY.md](docs/reference/SECURITY.md) |
| 桌面 Shell | [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) |
| 桌面 Bridge | [DESKTOP_BRIDGE.md](docs/reference/DESKTOP_BRIDGE.md) |
| 多 Agent 系统 | [MULTI_AGENT_SYSTEM.md](docs/reference/MULTI_AGENT_SYSTEM.md) |
| 扩展系统 | [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |

---

## 贡献

在 `main` 分支上进行单分支开发。提交格式：`<scope>: <description>`（英文）。
示例：`gateway: add WebSocket server foundation`

开发环境中重启 Aleph 前：

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
```

多进程并存 → HMAC 失败 → **vault 数据丢失**。基于 flock 的单例机制让这种情况极少出现，但安全提醒依然有效。

完整流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。

---

## 许可证

MIT。详见 [LICENSE](LICENSE)。
