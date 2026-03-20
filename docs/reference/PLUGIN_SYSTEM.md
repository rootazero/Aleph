# Plugin System — Claude Code 兼容架构

> Aleph 插件系统完全兼容 Claude Code 插件格式，支持 Marketplace 安装、命名空间、Scope 管理。

---

## 概述

Aleph 插件系统实现了 **单向兼容 + 超集** 策略：
- **任何 Claude Code 插件**（skills、agents、commands、hooks、MCP servers）**无需修改即可在 Aleph 中安装和运行**
- Aleph 独有能力（WASM runtime、channels、providers、services）通过 `[aleph]` 扩展字段承载
- 格式原则：**写 TOML，读 TOML+JSON**

**核心文件位置：** `core/src/extension/`

---

## 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                       Plugin System                                  │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐   │
│  │  Marketplace  │    │   Manifest   │    │     Discovery        │   │
│  │              │    │   Parsers    │    │                      │   │
│  │ • add/remove │    │              │    │ • Scope-ordered scan │   │
│  │ • update     │    │ • CC TOML   │    │ • Auto-discover      │   │
│  │ • search     │    │ • CC JSON   │    │ • Shadow resolution  │   │
│  │ • install    │    │ • Legacy    │    │                      │   │
│  └──────────────┘    └──────────────┘    └──────────────────────┘   │
│                                                                      │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐   │
│  │   Registry    │    │  Plugin      │    │     Runtime          │   │
│  │              │    │  Loader     │    │                      │   │
│  │ • Namespaced │    │              │    │ • MCP (default)      │   │
│  │ • Dual-key   │    │ • MCP config│    │ • WASM (Extism)      │   │
│  │ • ComponentId│    │ • WASM load │    │ • Static (Markdown)  │   │
│  └──────────────┘    └──────────────┘    └──────────────────────┘   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Manifest 格式

### 优先发现顺序

1. `.claude-plugin/plugin.toml` — **首选**（Aleph 原生 + CC 兼容超集）
2. `.claude-plugin/plugin.json` — CC 兼容（只读）
3. `aleph.plugin.toml` — **已废弃**（加载时打印 deprecation warning）
4. `aleph.plugin.json` — **已废弃**
5. `package.json` with `aleph` field — **已废弃**
6. 无 manifest — **自动发现模式**（扫描 `skills/`、`agents/`、`commands/`、`hooks/`、`.mcp.json`）

### plugin.toml 超集 Schema

```toml
# .claude-plugin/plugin.toml — Aleph 推荐格式
name = "my-plugin"                          # 必填，用作 ID
version = "1.0.0"
description = "Plugin description"
repository = "https://github.com/..."
license = "MIT"
keywords = ["keyword1"]

# 组件路径（补充默认位置，不替代）
commands = "./commands/"
agents = "./agents/"
skills = "./skills/"
hooks = "./hooks/hooks.json"
mcp-servers = "./.mcp.json"

[author]
name = "Author Name"
email = "author@example.com"

# === Aleph 扩展字段（Claude Code 会忽略）===
[aleph]
runtime = "mcp"                             # "mcp" | "wasm" | "static"
entry = "target/wasm32-wasi/release/x.wasm" # 仅 WASM

[aleph.permissions]
network = true
filesystem = "read"                         # true | "read" | "write" | false
shell = false

[[aleph.channels]]
id = "telegram"
label = "Telegram"

[[aleph.providers]]
id = "custom-llm"
name = "Custom LLM"

[[aleph.services]]
name = "metrics-collector"
start_handler = "startCollector"
stop_handler = "stopCollector"
```

### 与 Claude Code plugin.json 的对应关系

| plugin.json (camelCase) | plugin.toml (kebab-case) | 说明 |
|------------------------|-------------------------|------|
| `name` | `name` | 插件 ID |
| `version` | `version` | 语义版本 |
| `skills` | `skills` | Skills 目录路径 |
| `agents` | `agents` | Agents 目录路径 |
| `commands` | `commands` | Commands 目录路径 |
| `hooks` | `hooks` | Hooks 配置路径 |
| `mcpServers` | `mcp-servers` | MCP 服务配置路径 |
| — | `[aleph]` | Aleph 独有扩展 |

---

## Runtime 模型

| `[aleph] runtime` | PluginKind | 加载方式 | 适用场景 |
|--------------------|-----------|---------|---------|
| 不填 / `"static"` | Static | 纯 Markdown，无 runtime | Skills/agents/commands only |
| `"mcp"` | Mcp | 读取 `.mcp.json`，通过 MCP 协议 | Node.js、Python 等 |
| `"wasm"` | Wasm | Extism 沙箱直接加载 | 高性能安全插件 |

---

## 命名空间

所有插件组件使用 `plugin-name:component-name` 格式：

```
/cli-anything:list           # 插件命令
/cli-anything:refine         # 插件命令
/diagnostics:system_health   # 插件工具（MCP）
/memory-search               # 内置 skill（无前缀）
```

- 同名冲突：内置优先，插件按注册顺序（first-come wins for short name）
- 跨 marketplace 同名：`name@marketplace` 区分

**实现：** `ComponentId` struct（`core/src/extension/component_id.rs`）

---

## Marketplace 系统

### 命令

```bash
# Marketplace 管理
aleph plugin marketplace list                      # 列出（含内置 aleph-official）
aleph plugin marketplace add HKUDS/CLI-Anything    # 添加 GitHub marketplace
aleph plugin marketplace add /local/path           # 添加本地 marketplace
aleph plugin marketplace update [name]             # 同步缓存
aleph plugin marketplace remove <name>             # 移除

# 插件安装
aleph plugin install <plugin-name>                 # 从 marketplace 安装
aleph plugin install <git-url>                     # 直接 URL 安装
aleph plugin list                                  # 列出已安装
aleph plugin uninstall <name>                      # 卸载
aleph plugin enable/disable <name>                 # 启用/禁用
```

### 目录结构

```
~/.aleph/plugins/
├── cache/                          # Marketplace 缓存
│   ├── aleph-official/             # 内置官方 marketplace（git repo）
│   │   ├── .claude-plugin/
│   │   │   └── marketplace.toml
│   │   └── plugins/
│   │       ├── diagnostics/
│   │       ├── diff-viewer/
│   │       └── ...
│   └── cli-anything/               # 第三方 marketplace
│       ├── .claude-plugin/
│       │   └── marketplace.json    # CC 标准格式
│       └── cli-anything-plugin/
└── installed/                      # 已安装的插件
    ├── diagnostics/
    │   ├── .claude-plugin/
    │   │   └── plugin.toml
    │   ├── src/
    │   └── package.json
    ├── cli-anything/               # 第三方 CC 插件
    │   ├── .claude-plugin/
    │   │   └── plugin.json         # CC 原生格式
    │   └── commands/
    └── ...
```

### marketplace.toml 格式

```toml
name = "aleph-official"

[owner]
name = "Rootazero"
url = "https://github.com/rootazero"

[metadata]
description = "Aleph official plugin marketplace"
version = "1.0.0"
plugin-root = "./plugins"

[[plugins]]
name = "diagnostics"
source = "./plugins/diagnostics"
description = "System health monitoring"
version = "0.1.0"
```

也读取 `marketplace.json`（Claude Code 标准格式）。

### 内置 Marketplace

`aleph-official` 指向 `rootazero/Aleph-plugins`，始终可用，无需手动添加。

---

## Scope 管理

| Scope | 路径 | 用途 |
|-------|------|------|
| `user` | `~/.aleph/plugins/installed/` | 个人全局（默认） |
| `project` | `<project>/.aleph/plugins/` | 团队共享，入 VCS |
| `local` | `<project>/.aleph/plugins.local/` | 个人项目级，gitignore |
| `agent-level` | `~/.aleph/agents/<id>/plugins/` | Agent 专属（Aleph 独有） |

**优先级（高→低）：** `agent-level` > `local` > `project` > `user` > `bundled`

```bash
aleph plugin install <name> --scope user      # 默认
aleph plugin install <name> --scope project   # 团队共享
aleph plugin install <name> --scope local     # 个人项目
```

---

## 安装第三方 Claude Code 插件

完全兼容，零修改安装：

```bash
# 步骤 1: 添加 marketplace（GitHub repo）
aleph plugin marketplace add HKUDS/CLI-Anything

# 步骤 2: 安装插件
aleph plugin install cli-anything

# 验证
aleph plugin list
# → cli-anything    -    enabled    Build powerful, stateful CLI interfaces...
```

**支持的组件类型：**

| CC 组件 | Aleph 支持 | 说明 |
|---------|-----------|------|
| `skills/*/SKILL.md` | ✅ 完全支持 | 通过 SkillSystem 加载 |
| `agents/*.md` | ✅ 完全支持 | 自动发现 |
| `commands/*.md` | ✅ 完全支持 | 注册到 dispatch registry，命名空间化 |
| `hooks/hooks.json` (command type) | ✅ 支持 | Shell 命令型 hook |
| `.mcp.json` (MCP servers) | ✅ 支持 | 通过 MCP client 启动 |
| `.claude-plugin/plugin.json` | ✅ 完全支持 | CC JSON parser |
| `marketplace.json` | ✅ 完全支持 | Marketplace 系统 |
| `outputStyles` | ⏳ 延后 | 解析但不执行 |
| `.lsp.json` | ⏳ 延后 | 解析但不执行 |

---

## 环境变量

插件内容（skill/agent/hook/MCP 配置）中可用：

| 变量 | 值 | 说明 |
|------|-----|------|
| `${CLAUDE_PLUGIN_ROOT}` | 插件安装目录绝对路径 | CC 兼容 |
| `${ALEPH_PLUGIN_ROOT}` | 同上 | Aleph 别名 |
| `${CLAUDE_PLUGIN_DATA}` | `~/.aleph/plugins/data/{id}/` | 持久数据目录 |
| `${ALEPH_PLUGIN_DATA}` | 同上 | Aleph 别名 |

---

## 关键代码文件

### Manifest 解析
| 文件 | 职责 |
|------|------|
| `manifest/cc_plugin_toml.rs` | 解析 `.claude-plugin/plugin.toml` |
| `manifest/cc_plugin_json.rs` | 解析 `.claude-plugin/plugin.json` |
| `manifest/auto_discover.rs` | 无 manifest 时自动发现组件 |
| `manifest/mod.rs` | 统一入口，优先级调度 |
| `manifest/types.rs` | `PluginManifest`、`AlephExtensions`、`AlephRuntime` |

### Marketplace
| 文件 | 职责 |
|------|------|
| `marketplace/mod.rs` | `MarketplaceManager` 编排 |
| `marketplace/types.rs` | `MarketplaceManifest`、`MarketplaceConfig` |
| `marketplace/manifest.rs` | 解析 `marketplace.toml` / `.json` |
| `marketplace/github_source.rs` | GitHub git clone/pull |
| `marketplace/local_source.rs` | 本地路径解析 |
| `marketplace/installer.rs` | 复制插件到安装目录 |

### 其他
| 文件 | 职责 |
|------|------|
| `component_id.rs` | `ComponentId` 命名空间标识 |
| `scope.rs` | Scope 路径解析 |
| `mcp_config.rs` | 读取 `.mcp.json`，环境变量替换 |
| `plugin_loader.rs` | 运行时加载（MCP/WASM/Static） |
| `types/plugins.rs` | `PluginKind`、`PluginScope`、`PluginRecord` |

### CLI
| 文件 | 职责 |
|------|------|
| `bin/aleph/cli.rs` | `Plugin`/`PluginAction`/`MarketplaceAction` 定义 |
| `bin/aleph/commands/plugins.rs` | 所有 handler（本地执行，不走 Gateway） |

### Gateway
| 文件 | 职责 |
|------|------|
| `gateway/handlers/plugins/handlers.rs` | RPC handlers（`plugin.*` + `plugin.marketplace.*`） |
| `gateway/handlers/mod.rs` | RPC 方法注册 |

---

## Gateway RPC 方法

| 方法 | 说明 |
|------|------|
| `plugin.list` / `plugins.list` | 列出已安装插件 |
| `plugin.install` / `plugins.install` | 安装插件（URL） |
| `plugin.uninstall` / `plugins.uninstall` | 卸载插件 |
| `plugin.enable` / `plugins.enable` | 启用插件 |
| `plugin.disable` / `plugins.disable` | 禁用插件 |
| `plugin.marketplace.list` | 列出 marketplace |
| `plugin.marketplace.add` | 添加 marketplace |
| `plugin.marketplace.update` | 更新缓存 |
| `plugin.marketplace.remove` | 移除 marketplace |
| `plugin.marketplace.install` | 从 marketplace 安装 |

`plugin.*`（单数）是 CC 兼容方法名，`plugins.*`（复数）保留作为向后兼容别名。

---

## 后续工作

### MCP Runtime Wiring（P4 完善）
当前状态：PluginLoader 读取 `.mcp.json` 并存储配置，但实际 MCP server 启动未 wired 到 `McpManagerHandle`。
需要：在 Gateway 启动时，将 MCP 插件的 server 配置传递给 MCP manager 启动。

### WASM Tool Discovery
当前状态：WASM 插件可加载，但工具未自动注册。
需要：从 WASM 模块导出函数列表中发现并注册 tools。

### Aleph-plugins 仓库
当前状态：目录结构已迁移到 CC 兼容格式（`.claude-plugin/plugin.toml`），Node.js 插件标记为 `runtime = "mcp"` 但 `src/index.js` 仍是旧 IPC 格式。
需要：将每个 Node.js 插件的入口文件改为 MCP Server SDK 实现。

---

## 设计文档

- **Spec**: `docs/superpowers/specs/2026-03-20-plugin-system-claude-code-compat-design.md`
- **P0+P1 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p0-p1.md`
- **P2 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p2-marketplace.md`
- **P3 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p3-scope.md`
- **P4 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p4-runtime.md`
