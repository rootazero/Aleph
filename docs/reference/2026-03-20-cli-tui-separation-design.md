# CLI/TUI 严格分离设计

> 日期: 2026-03-20
> 状态: Draft

## 背景

当前 CLI 和 TUI 代码存在职责混淆：

- `src/bin/aleph/cli.rs` — Gateway 服务端自带 761 行 CLI 定义，与 `apps/cli/` 撞名
- `apps/cli/` — 客户端 CLI + 内嵌 TUI（27+ 命令 + ratatui 交互模式）
- `apps/cli/src/tui/` — CLI crate 内嵌的 TUI 代码
- `apps/tui/` — 独立 TUI crate，与上一条功能重叠
- TUI 的 `SlashCommand` enum（17 个硬编码命令）与 Gateway 的 `CommandParser`/`ToolRegistry` 完全脱节

此外，`src/cli/` 中存在一个隐藏的客户端层（`GatewayClient`、`OutputFormat`、config/cron/channels 等命令 handler），被 `src/bin/aleph/` 的命令直接调用。这意味着**服务端 crate 内部包含了客户端库**，是最严重的架构问题。

目标：严格区分 CLI 和 TUI，消除命名混淆和职责重叠。

## 核心原则

- **CLI** = 纯 Unix 命令行工具，不走 Gateway（Local 命令）或走 Gateway RPC（RPC 命令），输出到 stdout
- **TUI** = 交互式终端对话窗口（如 Claude Code），通过 Gateway 通信，支持斜杠命令，默认绑定 main agent
- **TUI 与 Bot/WebChat 等价** — 在 Gateway 眼中是同类 interface，协议层完全一致（JSON-RPC 2.0 over WebSocket）
- **命令双注册** — 同一命令可同时注册 CLI 和 TUI/Bot/WebChat，各端自行渲染

## 设计

### 1. 物理结构

CLI/TUI/WebChat 是 Aleph 的**核心通信通道**（用户与 AI 对话的 interface），不是"应用"。`apps/` 保留给平台特定的桌面扩展能力。

```
src/bin/aleph-server/
└── main.rs              # 极简：解析启动参数 → 启动 Gateway server
                         # 二进制名: aleph-server（不对用户暴露，由 CLI 的 daemon 命令管理）

shared/
├── protocol/            # aleph-protocol — JSON-RPC 协议类型
├── client/              # aleph-client — 共享客户端库
│   └── src/
│       ├── lib.rs
│       ├── connection.rs      # AlephClient — 持久 WebSocket 连接（TUI/交互模式）
│       ├── gateway_client.rs  # GatewayClient — 无状态一次性 RPC（CLI 管理命令）
│       ├── config.rs          # ~/.aleph/config 加载
│       ├── error.rs           # 客户端错误类型
│       └── output.rs          # 输出格式化（table/json/plain）
└── ui_logic/            # 共享 UI 逻辑

interfaces/
├── cli/                 # aleph-cli crate — 纯 CLI，bin: aleph
│   └── src/
│       ├── main.rs         # clap 解析 → 子命令分发
│       ├── output.rs       # stdout 格式化
│       └── commands/       # 各子命令实现
│           ├── chat.rs     # `aleph chat` → 调用 aleph_tui::run()
│           ├── daemon.rs   # `aleph daemon start/stop/status`（Local）
│           ├── config.rs   # `aleph config get/set/reload`（RPC）+ `config edit`（Local）
│           ├── plugins.rs  # `aleph plugins list/install/...`（RPC）
│           ├── session.rs  # `aleph session new/list/...`（RPC）
│           ├── model.rs    # `aleph model list/set`（RPC）
│           ├── ...
│           └── completion.rs  # shell 补全生成（Local）
│
├── tui/                 # aleph-tui crate — 纯交互终端
│   └── src/
│       ├── lib.rs          # pub fn run(agent: &str, ...) 入口
│       ├── app.rs          # AppState
│       ├── event.rs        # 键盘/终端事件
│       ├── render.rs       # ratatui 布局渲染
│       ├── theme.rs        # 颜色/样式
│       ├── markdown.rs     # Markdown 终端渲染
│       └── widgets/        # chat_area, input_area, status_bar...
│
└── webchat/             # Leptos WASM 面板（从 apps/panel/ 迁入）

apps/                    # 平台桌面扩展能力（手脚）
├── macos/               # macOS 原生扩展（Swift/AppKit）
├── tauri/               # Linux/Windows 桌面壳（Tauri，将被逐步替代）
├── linux/               # 未来：Rust + D-Bus/Wayland
└── windows/             # 未来：Rust + windows-rs
```

### 2. 依赖关系

```
interfaces/cli  ──→  interfaces/tui     (仅 chat 子命令调用 aleph_tui::run())
interfaces/cli  ──→  shared/client      ──→  shared/protocol
interfaces/tui  ──→  shared/client      ──→  shared/protocol
interfaces/webchat ──→ shared/client    ──→  shared/protocol
```

- `shared/client` 不依赖 core — 纯协议客户端，基于 `aleph-protocol` crate
- `interfaces/tui` 不依赖 `interfaces/cli`
- `src/cli/`（现有的隐藏客户端层）已合并到 `shared/client/`，待删除

### 3. 二进制入口

两个二进制，用户只接触 `aleph`：

- **`aleph`**（来自 `apps/cli/`）— 用户唯一入口
  - `aleph chat [--agent <name>]` — 启动 TUI
  - `aleph <command> [args]` — 执行 CLI 命令
  - 无子命令时显示 help
- **`aleph-server`**（来自 `src/bin/aleph-server/`）— Gateway 服务进程
  - 用户不直接运行，由 `aleph daemon start` 作为子进程或 daemon 启动
  - 仅接受启动参数（端口、数据目录等），无子命令

### 4. 命令分类

| 类型 | 特征 | 示例 | 注册位置 |
|------|------|------|----------|
| **Local** | 不需要 Gateway 在线 | `daemon start/stop/status`, `plugin init/validate/pack`, `completion` | 仅 CLI |
| **RPC** | 需要 Gateway 在线 | `plugins list`, `session new`, `model set`, `memory search`, `health`, `config get/set/reload` | CLI + TUI/Bot/WebChat |

> **注意**：`config get/set` 归类为 RPC 而非 Local。虽然配置文件在本地磁盘，但 config 操作需要运行时校验、热重载通知，走 RPC 保证一致性。`config edit`（打开编辑器）除外，属于 Local。

### 5. 统一命令命名

CLI 命令和斜杠命令使用同一命名空间：

```
CLI:  aleph <group> <action> [args]
TUI:  /<group> <action> [args]
Bot:  /<group> <action> [args]
```

| CLI | 斜杠命令 | RPC 方法 |
|-----|---------|----------|
| `aleph session new` | `/session new` | `session.create` |
| `aleph session list` | `/session list` | `session.list` |
| `aleph plugins list` | `/plugins list` | `plugins.list` |
| `aleph plugins install x` | `/plugins install x` | `plugins.install` |
| `aleph model set gpt-4` | `/model set gpt-4` | `model.set` |
| `aleph model list` | `/model list` | `models.list` |
| `aleph memory search x` | `/memory search x` | `memory.search` |
| `aleph health` | `/health` | `system.health` |

### 6. TUI 接入 Gateway 命令系统

**删除** TUI 本地的 `SlashCommand` enum 和 `parse()` 函数。

TUI 启动时通过 RPC 获取可用命令列表：

```
TUI 启动
  → client.call("commands.list", { interface: "tui" })
  ← 返回 [{ name: "session new", hint: "Create new session", ... }, ...]
  → 用于命令面板（command palette）自动补全
```

用户输入 `/xxx` 时：

```
用户输入 "/session new my-topic"
  → client.call("command.execute", { input: "session new my-topic" })
  ← Gateway 走现有 CommandParser → 快速路径执行 → 返回结果
  → TUI 渲染结果到对话流
```

**少量 TUI-only 命令**保留在本地（不走 RPC）：

- `/clear` — 清屏（纯 UI 操作）
- `/quit` — 退出（纯 UI 操作）
- `/verbose` — 切换调试显示（纯 UI 操作）

### 7. Gateway 侧新增 RPC 方法

**`commands.list`** — 查询可用命令列表

请求：`{ "interface": "tui" }`

响应：
```json
[
  {
    "name": "session new",
    "hint": "Create a new session",
    "source_type": "builtin",
    "arguments_schema": { "type": "object", "properties": { "topic": { "type": "string" } } }
  },
  {
    "name": "plugins list",
    "hint": "List installed plugins",
    "source_type": "builtin",
    "arguments_schema": null
  }
]
```

从 `ToolRegistry` 聚合所有 builtin、MCP、skill、plugin 命令，按 interface 能力过滤。

**`command.execute`** — 执行斜杠命令

请求：`{ "input": "session new my-topic", "session_id": "..." }`

复用现有 `CommandParser.parse_async()` + 快速路径。这是 TUI/Bot 的**唯一命令入口** — 客户端发送原始文本，Gateway 负责解析和路由。客户端不需要知道底层 RPC 方法名（如 `session.create`），只需要 `command.execute`。

CLI 不使用 `command.execute`，而是直接调用具体的 RPC 方法（因为 clap 已经完成了参数解析）。

### 8. Interface 能力声明

扩展现有 `ChannelCapabilities`：

```rust
pub struct CommandCapabilities {
    pub max_commands: Option<usize>,           // Telegram: Some(100), TUI/WebChat: None
    pub supported_sources: Vec<ToolSourceType>, // Telegram: [Builtin], WebChat: [All]
}
```

### 9. TUI 生命周期与 Agent 绑定

```
aleph chat [--agent <name>]
  ├─ 1. 加载 config（apps/client）
  ├─ 2. 连接 Gateway WebSocket
  ├─ 3. 绑定 agent（默认 "main"）
  │     → client.call("session.create", { agent_id: "main" })
  │     ← 返回 session_id
  ├─ 4. 获取命令列表
  │     → client.call("commands.list", { interface: "tui" })
  ├─ 5. 进入主循环（ratatui 事件循环）
  │     ├─ 用户消息 → client.call("chat.send", { session_id, text })
  │     ├─ 斜杠命令 → client.call("command.execute", { input })
  │     ├─ Gateway 事件 → 流式渲染到对话区
  │     └─ 本地命令（/clear, /quit）→ 本地处理
  └─ 6. 退出 → 断开 WebSocket
```

Agent 1:1 绑定：

- TUI 启动时指定 agent（默认 main），整个生命周期内不变
- 不允许运行时切换 agent（与 Bot 行为一致）
- 使用其他 agent → 启动新 TUI 实例：`aleph chat --agent research`
- Session 操作只能看到当前绑定 agent 的 session
- `/session new [topic]` 在 TUI 内：创建新 session（同 agent 下），TUI 切换到新 session，清空对话区

### 10. CLI 子命令完整结构

```
aleph
├── chat [--agent <name>]          # → 启动 TUI
├── ask <message> [--agent <name>] # → 单条消息，stdout 输出
│
│ # ── Daemon 管理 (Local) ──
├── daemon start [--port N]        # Local: 启动 aleph-server 进程
├── daemon stop                    # Local: 停止 daemon
├── daemon status                  # Local: 查看 daemon 状态
│
│ # ── Session (RPC) ──
├── session new [topic]            # RPC
├── session list                   # RPC
├── session switch <id>            # RPC
├── session delete <id>            # RPC
│
│ # ── Model (RPC) ──
├── model list                     # RPC
├── model set <name>               # RPC
│
│ # ── Provider (RPC) ──
├── provider list                  # RPC
├── provider add <type>            # RPC
├── provider remove <id>           # RPC
│
│ # ── Plugins (RPC) ──
├── plugins list                   # RPC
├── plugins install <name>         # RPC
├── plugins uninstall <name>       # RPC
│
│ # ── Plugin Dev (Local) ──
├── plugin init                    # Local
├── plugin validate                # Local
├── plugin pack                    # Local
│
│ # ── Skill (RPC) ──
├── skill list                     # RPC
├── skill install <name>           # RPC
│
│ # ── Memory (RPC) ──
├── memory search <query>          # RPC
├── memory clear                   # RPC
│
│ # ── Config (RPC + Local) ──
├── config get <key>               # RPC
├── config set <key> <value>       # RPC
├── config reload                  # RPC
├── config edit                    # Local: 打开编辑器
│
│ # ── 系统管理 (RPC) ──
├── health                         # RPC
├── info                           # RPC
├── tools [filter]                 # RPC: 列出可用工具
├── vault get/set/delete           # RPC: 密钥管理
├── identity show/set              # RPC: 身份管理
├── workspace list/switch          # RPC: 工作区管理
├── logs [--level]                 # RPC: 日志管理
├── channels                       # RPC: 频道状态
├── cron list/add/remove           # RPC: 定时任务
├── devices list/remove            # RPC: 设备管理
│
│ # ── 工具 (Local) ──
├── completion <shell>             # Local: shell 补全
└── gateway call <method> [params] # RPC: 万能 escape hatch
```

统一 `--format` flag 支持 `table`（默认）、`json`、`plain`，方便脚本化使用。

### 11. RPC 命令统一执行模式

所有 RPC 命令在 CLI 中遵循同一模式：

```rust
// apps/cli/src/commands/session.rs
pub async fn handle(client: &AlephClient, args: &SessionArgs) -> CliResult {
    match args.action {
        SessionAction::New { topic } => {
            let result = client.call("session.create", json!({ "topic": topic })).await?;
            output::print_result(&result, args.format)?;
        }
        SessionAction::List => {
            let result = client.call("session.list", json!({})).await?;
            output::print_table(&result, &["id", "topic", "created_at"])?;
        }
        // ...
    }
    Ok(())
}
```

### 12. Gateway 命令迁移清单

从 `src/bin/aleph/cli.rs` 迁出：

| 命令 | 类型 | 迁移目标 |
|------|------|---------|
| `start` | Local | `apps/cli/commands/daemon.rs` |
| `stop` | Local | `apps/cli/commands/daemon.rs` |
| `status` | Local | `apps/cli/commands/daemon.rs` |
| `plugins *` | RPC | `apps/cli/commands/plugins.rs` |
| `plugin *` | RPC + Local | `apps/cli/commands/plugin.rs` |
| `config *` | Local | `apps/cli/commands/config.rs` |
| `channels *` | RPC | `apps/cli/commands/channels.rs` |
| `cron *` | RPC | `apps/cli/commands/cron.rs` |
| `audit *` | RPC | `apps/cli/commands/audit.rs` |
| `secret *` | RPC | `apps/cli/commands/vault.rs` |
| `gateway call` | RPC | `apps/cli/commands/gateway.rs` |
| `pairing/devices` | RPC | `apps/cli/commands/devices.rs` |

## 实施阶段

### Phase 1: 提取 `apps/client/` crate

- 从 `apps/cli/src/` 中提取 `client.rs`、`config.rs`、`error.rs` 到 `apps/client/`
- 将 `src/cli/` 中的 `GatewayClient`、`OutputFormat` 等通用客户端代码合并到 `apps/client/`
- 删除 `src/cli/`（服务端 crate 不应包含客户端库）
- `apps/cli/` 和 `apps/tui/` 改为依赖 `apps/client/`
- 验证：现有功能不受影响

### Phase 2: Gateway 瘦身 + CLI 命令迁移

- Gateway 二进制从 `src/bin/aleph/` 重命名为 `src/bin/aleph-server/`
- `cli.rs` 中的命令迁移到 `apps/cli/commands/`
- `aleph-server/main.rs` 精简为纯启动逻辑（仅接受启动参数）
- `apps/cli/` 的 `daemon start` 命令负责启动 `aleph-server` 子进程
- Gateway 新增 `commands.list` 和 `command.execute` RPC 方法（为 Phase 3 做准备）
- 命令命名统一
- 验证：`aleph daemon start` 能启动服务器，所有管理命令从 `apps/cli/` 可用

### Phase 3: TUI 重构

- 删除 `apps/cli/src/tui/` 内嵌代码
- `apps/tui/` 接入 Gateway 命令系统（删除本地 `SlashCommand` enum）
- `apps/cli/` 的 `chat` 子命令调用 `aleph_tui::run()`
- 验证：`aleph chat` 启动 TUI，斜杠命令走 Gateway

### Phase 4: 命令能力声明与 Interface 对齐

- 扩展 `ChannelCapabilities` 增加 `CommandCapabilities`
- 各 interface 声明自己的命令能力
- `commands.list` RPC 按 interface 能力过滤
- Telegram 自动注册命令到 `setMyCommands`

## 不做的事情

- 不重写 TUI 的 UI 层（ratatui 渲染、widgets、主题保持不变）
- 不改 Gateway 核心架构（CommandParser + ToolRegistry + 快速路径不变）
- 不改 Bot/WebChat（已正常工作，本次只对齐 TUI 和 CLI）
- 不引入新的命令框架（复用 clap + ToolRegistry）

## 风险

| 风险 | 缓解措施 |
|------|---------|
| `src/bin/aleph/cli.rs` 有些命令直接操作内部数据结构，迁移到 RPC 需要新增 RPC 方法 | Phase 2 逐个评估，缺什么 RPC 补什么 |
| `apps/cli/` 现有 27+ 命令可能与迁入的命令冲突 | 命名统一时合并重复，不保留两套 |
| TUI 接入 Gateway 命令后延迟增加（本地 → RPC） | 命令执行本身是轻量 RPC，延迟可忽略；`/clear` 等纯 UI 操作保持本地 |
| `secret`、`devices`、`plugins enable/disable` 直接操作本地状态，无对应 RPC | Phase 2 逐个补充 RPC 方法：`vault.*`、`devices.*`、`plugins.enable/disable` |
| TUI 版本与 Gateway 版本不匹配（`commands.list` 不存在） | TUI 启动时检测，若 `commands.list` 失败则降级为硬编码命令列表 + 提示升级 |

## 最终架构总览

```
                    ┌─────────────────────────────────┐
                    │    Aleph Gateway (aleph-server)   │
                    │  ┌───────────┐ ┌──────────────┐ │
                    │  │ToolRegistry│ │CommandParser │ │
                    │  │(all tools) │ │(slash→intent)│ │
                    │  └─────┬─────┘ └──────┬───────┘ │
                    │        └───────┬───────┘         │
                    │          Agent Loop               │
                    │    commands.list / command.execute │
                    └──────────┬──────────────────┬────┘
                         WS JSON-RPC 2.0          │
              ┌──────────┬──────────┬─────────────┘
              ↓          ↓          ↓
         ┌────────┐ ┌────────┐ ┌────────┐
         │  TUI   │ │  Bot   │ │WebChat │   ← interfaces/ (核心通信通道)
         │(ratatui│ │(Telegram│ │(Leptos │      全部走 Gateway RPC
         │ 终端)  │ │ Slack) │ │ WASM)  │
         └────┬───┘ └────────┘ └────────┘
              │
    aleph chat ← CLI 子命令入口
              │
    ┌─────────┴────────────────────────┐
    │     aleph (CLI) ← interfaces/cli  │
    │  daemon start/stop  ← Local      │
    │  config get/set     ← RPC        │
    │  plugin init/pack   ← Local      │
    │  session/model/...  ← RPC        │
    │  aleph chat         ← 启动 TUI   │
    └──────────────────────────────────┘

    apps/ = 平台桌面扩展（手脚）
    ┌──────────┬──────────┬──────────┐
    │  macos/  │  linux/  │ windows/ │
    │ (Swift)  │ (future) │ (future) │
    └──────────┴──────────┴──────────┘
```
