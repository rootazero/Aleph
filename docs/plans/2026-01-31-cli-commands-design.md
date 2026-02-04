# CLI Commands Design

> Date: 2026-01-31
> Status: Approved
> Goal: 实现核心 CLI 命令：gateway call, config, channels, cron

## Overview

扩展现有 `aleph_gateway.rs` binary，添加四组新命令：
1. **gateway call** — 通用 RPC 调用
2. **config** — 配置管理
3. **channels** — 渠道管理
4. **cron** — 定时任务管理

## Command Structure

```
aleph
├── start                    # 启动 Gateway (已有)
├── stop                     # 停止 Gateway (已有)
├── status                   # Gateway 状态 (已有)
├── pairing                  # 设备配对 (已有)
├── devices                  # 设备管理 (已有)
├── plugins                  # 插件管理 (已有)
│
├── gateway                  # 【新增】Gateway RPC 工具
│   └── call <method>        # 通用 RPC 调用
│       --params <json>
│       --url <ws://...>
│       --timeout <ms>
│
├── config                   # 【新增】配置管理
│   ├── get [path]           # 获取配置 (全部或指定路径)
│   ├── set <path> <value>   # 设置配置值
│   ├── edit                 # 打开编辑器
│   ├── validate             # 验证配置
│   ├── reload               # 热重载
│   └── schema               # 输出 JSON Schema
│
├── channels                 # 【新增】渠道管理
│   ├── list                 # 列出所有渠道
│   └── status [name]        # 渠道状态
│
└── cron                     # 【新增】定时任务
    ├── list                 # 列出任务
    ├── status               # Cron 服务状态
    └── run <job-id>         # 手动触发任务
```

## Architecture

```
core/src/bin/aleph_gateway.rs     # 主入口 (修改)
    │
    ├── Commands enum              # Clap subcommands
    │   ├── Start, Stop, Status    # 已有
    │   ├── Pairing, Devices       # 已有
    │   ├── Plugins                # 已有
    │   ├── Gateway { Call }       # 新增
    │   ├── Config { Get, Set, Edit, Validate, Reload, Schema }
    │   ├── Channels { List, Status }
    │   └── Cron { List, Status, Run }
    │
    └── 执行逻辑
        │
        ▼
core/src/cli/                      # 【新增】CLI 模块
    ├── mod.rs                     # 模块导出
    ├── client.rs                  # Gateway RPC 客户端
    ├── output.rs                  # 输出格式化 (JSON/Table)
    ├── gateway.rs                 # gateway call 实现
    ├── config.rs                  # config 命令实现
    ├── channels.rs                # channels 命令实现
    └── cron.rs                    # cron 命令实现
```

## RPC Client

```rust
// core/src/cli/client.rs

pub struct GatewayClient {
    url: String,
    timeout: Duration,
}

impl GatewayClient {
    pub fn new(url: &str) -> Self { ... }

    /// Connect and send RPC request
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<T, CliError> { ... }
}
```

## Output Format

```rust
// core/src/cli/output.rs

pub enum OutputFormat {
    Table,  // Human-readable table
    Json,   // Machine-readable JSON
}

pub fn print_output<T: Serialize>(data: &T, format: OutputFormat) { ... }
```

All commands support `--json` flag for machine-readable output.

## Command Details

### gateway call

```bash
# Usage
aleph gateway call <method> [--params <json>] [--url <ws://...>] [--timeout <ms>]

# Examples
aleph gateway call health
aleph gateway call config.get --params '{"path": "general"}'
aleph gateway call agent.run --params '{"message": "Hello"}' --timeout 30000
```

### config commands

```bash
# Get all config
aleph config get [--json]

# Get specific path
aleph config get general.language
aleph config get providers.openai

# Set value
aleph config set general.language "zh-Hans"
aleph config set providers.openai.model "gpt-4o"

# Edit in $EDITOR
aleph config edit

# Validate
aleph config validate

# Hot reload
aleph config reload

# Output JSON Schema
aleph config schema [--output <file>]
```

**RPC Mapping:**

| Command | RPC Method |
|---------|------------|
| `config get` | `config.get` |
| `config set` | `config.patch` |
| `config validate` | `config.validate` |
| `config reload` | `config.reload` |
| `config schema` | `config.schema` |

### channels commands

```bash
# List all channels
aleph channels list [--json]
# Output: name, type, status, connected_at

# Channel status
aleph channels status telegram
```

**RPC Mapping:**

| Command | RPC Method |
|---------|------------|
| `channels list` | `channels.list` |
| `channels status` | `channels.status` |

### cron commands

```bash
# List cron jobs
aleph cron list [--json]
# Output: id, schedule, description, last_run, next_run

# Cron service status
aleph cron status

# Trigger job manually
aleph cron run <job-id>
```

**RPC Mapping:**

| Command | RPC Method |
|---------|------------|
| `cron list` | `cron.list` |
| `cron status` | `cron.status` |
| `cron run` | `cron.run` |

## File Changes

```
core/src/
├── cli/                           # New module
│   ├── mod.rs
│   ├── client.rs                  # GatewayClient
│   ├── output.rs                  # OutputFormat, print helpers
│   ├── error.rs                   # CliError type
│   ├── gateway.rs                 # gateway call impl
│   ├── config.rs                  # config commands impl
│   ├── channels.rs                # channels commands impl
│   └── cron.rs                    # cron commands impl
├── bin/
│   └── aleph_gateway.rs          # Modify: add new subcommands
└── lib.rs                         # Export cli module
```

## Implementation Order

1. **Phase 1**: Create cli module structure + GatewayClient
2. **Phase 2**: Implement `gateway call` command
3. **Phase 3**: Implement `config` commands
4. **Phase 4**: Implement `channels` commands
5. **Phase 5**: Implement `cron` commands
6. **Phase 6**: Integration tests + documentation

## Success Criteria

- [ ] `aleph gateway call health` returns Gateway health
- [ ] `aleph config get` shows full configuration
- [ ] `aleph config set` modifies config via RPC
- [ ] `aleph config schema` outputs JSON Schema
- [ ] `aleph channels list` shows channel status
- [ ] `aleph cron list` shows scheduled jobs
- [ ] All commands support `--json` output
- [ ] Error handling with clear messages
