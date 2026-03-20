# CLI/TUI 严格分离设计

> 日期: 2026-03-20
> 状态: Draft

## 背景

当前 CLI 和 TUI 代码存在职责混淆：

- `core/src/bin/aleph/cli.rs` — Gateway 服务端自带 761 行 CLI 定义，与 `apps/cli/` 撞名
- `apps/cli/` — 客户端 CLI + 内嵌 TUI（27+ 命令 + ratatui 交互模式）
- `apps/cli/src/tui/` — CLI crate 内嵌的 TUI 代码
- `apps/tui/` — 独立 TUI crate，与上一条功能重叠
- TUI 的 `SlashCommand` enum（17 个硬编码命令）与 Gateway 的 `CommandParser`/`ToolRegistry` 完全脱节

目标：严格区分 CLI 和 TUI，消除命名混淆和职责重叠。

## 核心原则

- **CLI** = 纯 Unix 命令行工具，不走 Gateway（Local 命令）或走 Gateway RPC（RPC 命令），输出到 stdout
- **TUI** = 交互式终端对话窗口（如 Claude Code），通过 Gateway 通信，支持斜杠命令，默认绑定 main agent
- **TUI 与 Bot/WebChat 等价** — 在 Gateway 眼中是同类 interface，协议层完全一致（JSON-RPC 2.0 over WebSocket）
- **命令双注册** — 同一命令可同时注册 CLI 和 TUI/Bot/WebChat，各端自行渲染

## 设计

### 1. 物理结构

```
core/src/bin/aleph/
└── main.rs              # 极简：解析启动参数 → 启动 Gateway server（无 cli.rs）

apps/
├── client/              # aleph-client crate — 共享客户端库
│   └── src/
│       ├── lib.rs
│       ├── connection.rs   # WebSocket JSON-RPC 2.0 连接管理
│       ├── config.rs       # ~/.aleph/config 加载
│       └── error.rs        # 客户端错误类型
│
├── cli/                 # aleph-cli crate — 纯 CLI，bin: aleph
│   └── src/
│       ├── main.rs         # clap 解析 → 子命令分发
│       ├── output.rs       # stdout 格式化（table/json/plain）
│       └── commands/       # 各子命令实现
│           ├── chat.rs     # `aleph chat` → 调用 aleph_tui::run()
│           ├── daemon.rs   # `aleph daemon start/stop/status`（Local）
│           ├── config.rs   # `aleph config get/set`（Local）
│           ├── plugins.rs  # `aleph plugins list/install/...`（RPC）
│           ├── session.rs  # `aleph session new/list/...`（RPC）
│           ├── model.rs    # `aleph model list/set`（RPC）
│           ├── ...
│           └── completion.rs  # shell 补全生成（Local）
│
└── tui/                 # aleph-tui crate — 纯交互终端
    └── src/
        ├── lib.rs          # pub fn run(agent: &str, ...) 入口
        ├── app.rs          # AppState
        ├── event.rs        # 键盘/终端事件
        ├── render.rs       # ratatui 布局渲染
        ├── theme.rs        # 颜色/样式
        ├── markdown.rs     # Markdown 终端渲染
        └── widgets/        # chat_area, input_area, status_bar...
```

### 2. 依赖关系

```
apps/cli  ──→  apps/tui     (仅 chat 子命令调用 aleph_tui::run())
apps/cli  ──→  apps/client  ──→  shared/protocol
apps/tui  ──→  apps/client  ──→  shared/protocol
```

- `apps/client` 不依赖 core — 纯协议客户端
- `apps/tui` 不依赖 `apps/cli`

### 3. 二进制入口

单一二进制名 `aleph`：

- `aleph chat [--agent <name>]` — 启动 TUI（默认绑定 main agent）
- `aleph <command> [args]` — 执行 CLI 命令
- 无子命令时显示 help

### 4. 命令分类

| 类型 | 特征 | 示例 | 注册位置 |
|------|------|------|----------|
| **Local** | 不需要 Gateway 在线 | `daemon start/stop`, `config get/set`, `plugin init/validate/pack`, `completion` | 仅 CLI |
| **RPC** | 需要 Gateway 在线 | `plugins list`, `session new`, `model set`, `memory search`, `health` | CLI + TUI/Bot/WebChat |

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

- **`commands.list`** — 查询可用命令列表，接受 `interface` 参数，从 `ToolRegistry` 聚合所有 builtin、MCP、skill、plugin 命令，按 interface 能力过滤
- **`command.execute`** — 执行斜杠命令，复用现有 `CommandParser.parse_async()` + 快速路径

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

### 10. CLI 子命令完整结构

```
aleph
├── chat [--agent <name>]          # → 启动 TUI
├── ask <message> [--agent <name>] # → 单条消息，stdout 输出
│
├── daemon start [--port N]        # Local
├── daemon stop                    # Local
├── daemon status                  # Local
│
├── session new [topic]            # RPC
├── session list                   # RPC
├── session delete <id>            # RPC
│
├── model list                     # RPC
├── model set <name>               # RPC
│
├── plugins list                   # RPC
├── plugins install <name>         # RPC
├── plugins uninstall <name>       # RPC
│
├── plugin init                    # Local (dev tools)
├── plugin validate                # Local (dev tools)
├── plugin pack                    # Local (dev tools)
│
├── memory search <query>          # RPC
├── memory clear                   # RPC
│
├── config get <key>               # Local
├── config set <key> <value>       # Local
│
├── health                         # RPC
├── info                           # RPC
├── completion <shell>             # Local
└── gateway call <method> [params] # RPC (escape hatch)
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

从 `core/src/bin/aleph/cli.rs` 迁出：

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
- `apps/cli/` 和 `apps/tui/` 改为依赖 `apps/client/`
- 验证：现有功能不受影响

### Phase 2: Gateway 瘦身 + CLI 命令迁移

- `core/src/bin/aleph/cli.rs` 中的命令迁移到 `apps/cli/commands/`
- `core/src/bin/aleph/main.rs` 精简为纯启动逻辑
- 命令命名统一
- 验证：`aleph start` 能启动，所有管理命令从 `apps/cli/` 可用

### Phase 3: TUI 重构

- 删除 `apps/cli/src/tui/` 内嵌代码
- `apps/tui/` 接入 Gateway 命令系统（删除本地 `SlashCommand` enum）
- Gateway 新增 `commands.list` 和 `command.execute` RPC 方法
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
| `core/src/bin/aleph/cli.rs` 有些命令直接操作内部数据结构，迁移到 RPC 需要新增 RPC 方法 | Phase 2 逐个评估，缺什么 RPC 补什么 |
| `apps/cli/` 现有 27+ 命令可能与迁入的命令冲突 | 命名统一时合并重复，不保留两套 |
| TUI 接入 Gateway 命令后延迟增加（本地 → RPC） | 命令执行本身是轻量 RPC，延迟可忽略；`/clear` 等纯 UI 操作保持本地 |

## 最终架构总览

```
                    ┌─────────────────────────────────┐
                    │       Aleph Gateway Server       │
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
         │  TUI   │ │  Bot   │ │WebChat │   ← 等价 interface
         │(ratatui│ │(Telegram│ │(Leptos │      全部走 Gateway RPC
         │ 终端)  │ │ Slack) │ │ WASM)  │
         └────┬───┘ └────────┘ └────────┘
              │
    aleph chat ← CLI 子命令入口
              │
    ┌─────────┴────────────────────────┐
    │           aleph (CLI)             │
    │  daemon start/stop  ← Local      │
    │  config get/set     ← Local      │
    │  plugin init/pack   ← Local      │
    │  session/model/...  ← RPC        │
    │  aleph chat         ← 启动 TUI   │
    └──────────────────────────────────┘
```
