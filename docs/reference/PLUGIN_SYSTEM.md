# Plugin System — Claude Code 兼容架构

> Aleph 插件系统完全兼容 Claude Code 插件格式，支持 Marketplace 安装、命名空间、Scope 管理。

---

## 概述

Aleph 插件系统实现了 **单向兼容 + 超集** 策略：
- **任何 Claude Code 插件**（skills、agents、commands、hooks、MCP servers）**无需修改即可在 Aleph 中安装和运行**
- Aleph 独有能力（WASM runtime、channels、providers、services）通过 `[aleph]` 扩展字段承载
- 格式原则：**写 TOML，读 TOML+JSON**

**核心文件位置：** `src/extension/`

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
background = true                           # [[aleph.services]] 需要此权限

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
auto_start = true                           # 默认 true：插件加载后自动启动
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

**实现：** `ComponentId` struct（`src/extension/component_id.rs`）

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
aleph plugin update [name] [--force] [--scope ...] # 升级已装插件（省略 name 升级全部）
aleph plugin uninstall <name>                      # 卸载
aleph plugin enable/disable <name>                 # 启用/禁用
```

> **`plugin update` 语义**：以 marketplace 缓存为准，原子换装已安装插件目录（暂存→备份旧→换入新→删备份，失败回滚，绝不损坏现有安装）。仅当版本发生变化时才换装——两端均为 semver 时不降级，CalVer / git SHA / `local` 等非 semver 版本以"不相等即变更"判定（对齐 codex `IfVersionChanged`）；`--force` 强制重装。插件持久化数据目录 `~/.aleph/plugins/data/<id>/` 位于安装树之外，不受换装影响。

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
| `plugin.update` | 升级已装插件（原子换装 + 版本比对，`force` 强制）|
| `plugin.enable` / `plugins.enable` | 启用插件 |
| `plugin.disable` / `plugins.disable` | 禁用插件 |
| `plugin.marketplace.list` | 列出 marketplace |
| `plugin.marketplace.add` | 添加 marketplace |
| `plugin.marketplace.update` | 更新缓存 |
| `plugin.marketplace.remove` | 移除 marketplace |
| `plugin.marketplace.install` | 从 marketplace 安装 |

`plugin.*`（单数）是 CC 兼容方法名，`plugins.*`（复数）保留作为向后兼容别名。

---

## MCP Runtime Wiring（已完成）

MCP 插件的 `.mcp.json` server 现已作为 **transient（仅运行时，不落盘）** server 注册到运行中的 `McpManager`，工具经现有 tool bridge 自动注册。

- **transient 通道**：`McpManagerHandle::add_transient_server` / `remove_transient_server`（`src/mcp/manager/`）。与 `add_server` 不同，它只 `start_server_internal`，**不** upsert/持久化到用户 MCP 配置文件——插件 server 由插件生命周期管理，绝不污染用户配置。`server_id` 形如 `plugin:<id>/<name>`。
- **注册编排**：`ExtensionManager::sync_mcp_plugin_servers()` 把所有启用的 MCP-kind 插件的 server 交给 manager（幂等；无 handle 时 no-op）。boot 时在 tool bridge spawn 之后由 `set_mcp_handle` + 后台任务触发；`reload()` 末尾再次调用以覆盖热重载安装。
- **卸载清理**：`unload_runtime_plugin` 在卸载前捕获 server id，卸载后 `remove_transient_server` 拆除，避免残留进程/工具。
- `list_servers` 同时列出 transient client（不止 config），使 `mcp.list` 与 tool bridge 的 lag-recovery `resync_all` 都能感知插件 server。

### 远程 MCP transport

`.mcp.json` 现支持 HTTP/SSE 远程 transport，格式与 CC 兼容：

```json
{
  "mcpServers": {
    "remote-srv": {
      "type": "remote",
      "url": "https://mcp.example.com/api",
      "headers": { "Authorization": "Bearer ${ALEPH_PLUGIN_ROOT}/token" }
    },
    "events": {
      "type": "remote",
      "url": "https://events.example.com/sse",
      "transport": "sse"
    }
  }
}
```

`type` 默认是 `stdio`，所以现有插件无需修改。`McpServerConfig` 现在是 enum：
`Stdio { command, args, env } | Remote { url, headers, oauth?, timeout_ms? }`，由
`PluginLoader::load_mcp_plugin` 直接路由到对应的 `McpTransportType`。`McpJsonServerEntry`
解析器对缺失字段（stdio 无 `command` / remote 无 `url` / 未知 `type`）做 hard error，
不让 spawn 进入半配置状态。

### 内联 MCP 工具自动发现

`McpScope::provision` 在 spawn 完所有 inline MCP server 后，立刻调用每个
`InlineMcpHandle.process.list_tools()` 并把返回的工具转换为
`ToolRegistration`（name 命名空间化为 `<server>:<tool>`，`plugin_id = "inline:<server>"`）。
子 agent 的工具表面现在能看到 inline server 的工具，而不只是 referenced global tools。
失败 list 的 inline server 不会破坏其他 server——降级 log。

### WASM Tool Discovery
当前状态：WASM 插件可加载，但工具未自动注册。
需要：从 WASM 模块导出函数列表中发现并注册 tools。

### Aleph-plugins 仓库
当前状态：目录结构已迁移到 CC 兼容格式（`.claude-plugin/plugin.toml`），Node.js 插件标记为 `runtime = "mcp"` 但 `src/index.js` 仍是旧 IPC 格式。
需要：将每个 Node.js 插件的入口文件改为 MCP Server SDK 实现。

---

## WASM Credential Injection（host-side 落地）

WASM plugin 的 `http.credentials: Vec<CredentialBinding>` 字段声明 host-pattern +
secret 名 + 注入策略（Bearer/Basic/Header/Query/UrlPath）。host 端的
`host_functions::try_http_fetch` 现在在 egress 前实际调用
`credential_injector::inject_credential`，通过 `WasmCapabilityKernel` 持有的
`SecretResolver` 解析 secret 名。Plugin guest 永远不接触明文 secret 值——这是
**live property**，不再是 goal。

`SecretResolver` trait + `InMemorySecretResolver` / `DenyAllSecretResolver` 实现在
`src/extension/runtime/wasm/secret_resolver.rs`。生产部署可通过自定义 resolver
对接 Aleph vault（替换默认的 deny-all）。已修复 `WasmCapabilityKernel` 的 header
注释中 "NOT yet done" 的过时措辞。

---

## Manifest 解析缓存（openclaw parity）

`manifest_cache::ManifestCache`（`src/extension/manifest/manifest_cache.rs`）—
LRU（512 条），key = `(canonical path, size, mtime, ctime, dev, ino)`。Boot 时
`parse_manifest_from_dir_cached_global(dir)` 自动咨询/填充；热重载期间任何
in-place 编辑都会改变 key tuple，cache 自然 miss。openclaw 也有相同模式
（`plugin-cache-primitives.createPluginCacheKey`），但 Aleph 版本借助 Rust
类型系统多加了 `dev`/`ino` 字段以对抗硬链接替换。

---

## Lazy Activation Planner —— ❌ 已删除（2026-08-07）

**不存在懒激活。所有 enabled 插件在 boot 时一次性加载。** `[plugin.activation]`
块**不被任何 adapter 读取**，写了等于没写。

曾经有过一个 openclaw `activation-planner.ts` 的 Rust 移植
（`src/extension/activation.rs` 的 `ActivationPlanner` / `ActivationHints` /
`ActivationTrigger` / `ActivationPlan` / `CapabilityKind` / `tier_kinds`，约 600 行），
本轮按 R10 YAGNI 整体删除。删的理由比「planner 没有生产调用者」更深一层：
`PluginManifest.activation` **从来没有非 `None` 过**——三个 manifest adapter
（`cc_plugin_json` / `cc_plugin_toml` / `toml_types`）在各自的构造点全部硬编码
`activation: None`，其中 `cc_plugin_json` 甚至把这个块反序列化进自己的 DTO 之后
再丢掉。所以写了 `activation` 块的插件作者既没拿到懒加载，也没拿到任何诊断。

**重连不是补一个调用点**：懒激活需要一条「按 trigger 重入」的加载路径，而
`load_plugins` 是 boot 时对插件目录的一次性遍历——那条路径得先造出来。要复活
请从 openclaw 的 `activation-planner.ts` 和
`git log --follow src/extension/plugin_trust.rs` 起步，不要从被删的 Rust 起步——
它从未对着真实 registry 跑过。

存活下来的是同文件里的 `OwnerTrustPolicy`（见下节），文件已随之更名为
`src/extension/plugin_trust.rs`。

---

## Owner Trust Policy（P3.5 — openclaw parity）

锚点 `src/extension/plugin_trust.rs`（该文件曾名 `activation.rs`，
activation planner 删除后按内容更名）。

Aleph 暴露 `OwnerTrustPolicy::permissive()` (默认) 和
`OwnerTrustPolicy::restrictive(allowlist)`。restrictive 模式下，`Bundled` 和
`Config` origin 的插件始终可加载；`Workspace` 和 `Global` origin 的插件必须在
allowlist 中。`ExtensionManager::set_owner_trust_policy(policy)` 切换策略；
`current_owner_trust_policy()` 暴露给 operator。`LoadSummary.skipped_by_trust`
记录被策略跳过的 plugin 数，让 operator 看到"装了但没启用"的 plugin。

这对应 openclaw 的 `passesManifestOwnerBasePolicy` + bundled 短路。

---

## 设计文档

- **Spec**: `docs/superpowers/specs/2026-03-20-plugin-system-claude-code-compat-design.md`
- **P0+P1 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p0-p1.md`
- **P2 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p2-marketplace.md`
- **P3 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p3-scope.md`
- **P4 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p4-runtime.md`
