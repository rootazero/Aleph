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

[[aleph.services]]
name = "metrics-collector"
start_handler = "startCollector"
stop_handler = "stopCollector"
auto_start = true                           # 默认 true：插件加载后自动启动

# 插件声明的工具（handler = WASM 导出函数名）
[[aleph.tools]]
name = "memory_stats"
description = "Report memory statistics"
handler = "memory_stats"
parameters = { type = "object", properties = { window = { type = "string" } } }

# 注入 agent 上下文的提示词文件
[aleph.prompt]
file = "SYSTEM.md"
scope = "system"                            # "system" | "user"

# 用户配置的 JSON Schema + 表单提示
[aleph.config_schema]
type = "object"
properties = { api_key = { type = "string" } }

[aleph.config_ui_hints.api_key]
label = "API Key"
sensitive = true
```

> **`[aleph]` 是超集的全部落点。** `tools` / `hooks` / `commands` / `prompt` /
> `config_schema` / `config_ui_hints` / `memory` 在 2026-08-19 之前只存在于**已废弃**的
> `aleph.plugin.toml`，而 `parse_cc_plugin_toml_content` 把它们硬编码成 `None` —— 于是
> adapter 里 `if let Some(ref tools) = manifest.tools_v2` 那两条分支不可达，
> `validation.rs` 的 config-schema 检查与 UI-hint 报告全部空转，
> `loader.rs::register_memory_extension_if_declared` 从不触发。内置的
> `plugins/memory-analytics` 从 WASM 导出 `memory_stats` / `memory_timeline`
> 却带着 `.claude-plugin/plugin.toml`，整个工具面因此不可达。
> Claude Code 会忽略未知顶层键，所以带超集的 manifest 在两个宿主里都能加载。

> **⚠️ `[[aleph.channels]]` / `[[aleph.providers]]` 从来不存在。** 这两段曾写在这里，
> 而 manifest 类型上**根本没有这两个字段**（`AlephExtensionsToml` 只有 runtime / entry /
> permissions / capabilities / services + 上面这几项），声明它们只会被静默忽略。
> 新增 channel 或 provider 目前只能改 core（`src/gateway/channel*` / `src/providers/`）。

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

## 插件状态（`plugins.list` 的 `status`）

| status | 含义 | 补救 |
|--------|------|------|
| `loaded` | 活跃，capability 对模型可见（`PluginStatus::Loaded`） | — |
| `pending` | 已 mount，但某个声明的依赖尚未到达终态（`PluginStatus::Pending { waiting_on }`：MCP manager 未接上 → `mcp:manager`；server 未完成 `initialize` → `mcp:<server_id>`）。**不因超时变 `error`**（判据 §8：「还没准备好」≠「失败了」） | `status_detail` 列出 `waiting on …`；`aleph doctor` 的 `extension/plugins-activated` 逐个点名 |
| `disabled` | operator 关掉了（`plugins.toml`；`PluginStatus::Disabled`）。`origin: claude_cache` 详见 Runtime 模型后的 ClaudeCache 段落（默认 disabled，模型不能 `plugin_manage enable`，只能禁用）| `al plugin enable <name>` 或 Panel 插件开关（同一个 `plugins.enable` RPC，按 registry 解析 id，任何 origin 都认）；离线 `aleph-server plugin enable <name>` 同样认得 Claude Code 装的 id。`claude_cache` 行**模型不能启用**（`plugin_manage` 拒绝并给出上面的人类命令），只能禁用 |
| `error` | manifest 解析失败，或 `mount` 某一步失败——`PluginStatus::Error("<step>: <reason>")`，由 `lifecycle.rs::write_failed_row` 这一处写入；已注册的部分已 dispose（**全有或全无**）。声明的 MCP server 启动失败（`mcp:<server_id>: <reason>`，插件仍在 mount 状态）| `status_detail` 给出原因 |
| `blocked` | owner trust policy 拒绝了它（`PluginStatus::Blocked(reason)`） | `plugin_manage(action='trust', name=…)` |

> **2026-08-16 之前只有前两个是真的。** `Overridden` / `Error` 是**零生产者**的枚举变体：
> 重名插件在 `load_all` 里被 `continue` 静默丢弃，manifest 解析失败只有一句 `debug!`，
> 两者都**不进 registry** ⇒ 在每一个面上「装了但坏了」与「从来没装过」逐字节相同，
> 而 operator 手里没有任何可修的东西。owner trust 拒绝同理（`skipped_by_trust` 计数器的
> doc 声称它「Surfaced in `extensions.stat`」，实际零消费者）。
>
> 现在 `error` / `blocked` 有了 registry 行 + `status_detail`。**`overridden` 从未有过生产者**：
> registry 按 id 键控，输家（被遮蔽的副本）拿不到自己的行，`load_all` → `discover_and_mount`
> （`src/extension/lifecycle.rs`，前者调用后者，不是被它取代）只在赢家的 registry 上记一条
> `shadowed` 诊断（`PluginDiagnostic`，不是 registry 行）——这一节曾写「现在三者都有 registry 行」，
> 那句是假的；变体已于 2026-09 删除，新增的 `pending` 走的是第三条路：既不进 `errors`
> 计数也不是诊断，而是 `PluginRecord.status` 本身。状态词表的单一源是
> `aleph_protocol::plugins::PluginRuntimeStatus`。
>
> **`blocked` 直到 2026-08-19 仍然产生不出来**——不是因为它没有 registry 行，而是
> 因为整个 owner trust policy 零生产者（见下文「策略住在哪」）。现在它由
> `[trust] enforce` 产生，且 `qa/plugins/run.sh trust` 真机断言它与「不存在」不同：
> 被拒的插件**必须留下带 id 的行**，否则 operator 手里没有可以 vouch 的东西。
> `activation_gate` 认得的**终态集合从枚举派生**（守卫 G6），不是手写清单；`Pending` 计入 `is_active()`。

## Runtime 模型

| `[aleph] runtime` | PluginKind | 加载方式 | 适用场景 |
|--------------------|-----------|---------|---------|
| 不填 / `"static"` | Static | 纯 Markdown，无 runtime | Skills/agents/commands only |
| `"mcp"` | Mcp | 读取 `.mcp.json`，通过 MCP 协议 | Node.js、Python 等 |
| `"wasm"` | Wasm | Extism 沙箱直接加载 | 高性能安全插件 |

Runtime 与 **origin** 是两个轴。origin（`PluginOrigin`）多了一个值：`ClaudeCache`——`~/.claude/plugins/`
里 Claude Code 已装的插件（读 `installed_plugins.json`，解析到 `cache/<marketplace>/<plugin>/<version>/`）。
**只读发现**：Aleph 永不写 `~/.claude/`，不读 `settings.json`；这一 origin 的插件**默认 disabled**
（`plugins.toml` 里缺席即 disabled，只对这个 origin 如此），`plugin_manage list` 带 origin 露出，
一个动词启用，启用态只写 `plugins.toml`。`installed_plugins.json` 形状变了 → 整个来源跳过 + 一条 warn，
其它来源不受影响。

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

**实现：** 命名空间是**按面各自解析**的，没有统一的 `ComponentId` 类型——
此前本文档点名的 `src/extension/component_id.rs` 从未存在。真实锚点：
工具走 `ExtensionManager::resolve_active_plugin_tool`（接受短名或 `plugin_id:name`），
skills/commands 走 `SkillRegistration` 的 `skill_type` 分流，
MCP server id 由 `mcp_config.rs` 组成 `plugin:<id>/<server>`。

---

## Marketplace 系统

### 命令

```bash
# Marketplace 管理
al plugin marketplace list                      # 列出注册项（含内置 aleph-official）
al plugin marketplace browse [query]            # 列出内容（--marketplace 收窄）
al plugin marketplace add HKUDS/CLI-Anything    # 添加 GitHub marketplace
al plugin marketplace add /local/path           # 添加本地 marketplace
al plugin marketplace update [name]             # 同步缓存
al plugin marketplace remove <name>             # 移除

# 插件安装
al plugin install <plugin-name>                 # 从 marketplace 安装
al plugin install <git-url>                     # 直接 URL 安装
al plugin list                                  # 列出已安装
al plugin update [name] [--force] [--scope ...] # 升级已装插件（省略 name 升级全部）
al plugin uninstall <name>                      # 卸载
al plugin enable/disable <name>                 # 启用/禁用（耐久，见下）
```

> **`enable` / `disable` 的耐久载体是 `<data_dir>/plugins.toml`**（`src/extension/plugin_state.rs`），
> 不是插件目录里的 `.disabled` 标记文件。
>
> 2026-08-16 之前那个标记有**四个写者、零个读者**——`discovery::scanner` 的
> `has_plugin_manifest` 与 `scan_plugin_parent` 从不看它——所以 `al plugin disable X`
> 打印成功、改变的东西活不过这个进程。handler 自己的 doc 逐字写着
> "preventing the plugin from being discovered and loaded on next scan"，那句话是假的。
>
> 改用 config 文档而不是「把标记读起来」的理由有两条：标记住在插件目录里，
> 而 `plugin update` 的原子换装与 `uninstall` 都会删掉那棵树（禁用会在升级后复活）；
> bundled 插件可以来自只读目录，根本写不进去。形状照抄孪生子系统 `SkillsConfig`
> （`<data_dir>/skills.toml`）——同一个问题在隔壁已经有答案时，另起一个不同的答案就是让两者漂移。
>
> **旧标记会被一次性迁移**：开机 `load_all` 见到 `.disabled` 就把 `enabled = false`
> 写进 `plugins.toml` 并删除标记（保住用户此前的意图，同时收敛到单一源）。
> Claude Code 缓存（`~/.claude/plugins/cache/…`，`PluginOrigin::ClaudeCache`）里的目录**除外**：
> 那棵树只读，从不迁移、从不写。
>
> 被禁用的插件**仍然注册进 registry 作为一行 `disabled` 记录（带 manifest 计数，但没有 capability 行）**——
> 只有被 `mount` 过的插件才有 capability 行（`lifecycle.rs`，effect `registry_row`）；
> 运行时重新启用走 `set_plugin_enabled(id, true)` → `mount`，由它注册 capability，
> 所以状态位背后不会是空的。下游的 `status.is_active()` 过滤保留（无害，P2 的 `visible_to` 还要用这些行）。

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

## Service 的标识符：`id` 与 `name` 是同一个字段（2026-08-20）

`[[aleph.services]]` 的标识符在 `ServiceSection` 上叫 `name`，而 **`id` 是被接受的
输入拼法**（`#[serde(alias = "id")]`）。

这不是风格宽容，是一次事故的修复：Aleph 自己随包发的 `diagnostics` 与 `voice-call`
写的是 `id`，而 `name` 是**必填**字段——缺一个必填字段对 serde 不是「少一个字段」，
是**整份文档解析失败**。于是这两个（**每一个声明了 service 的随包插件**）根本装不
上，错误还是一句看起来像 TOML 语法错的 `TOML parse error at line 13`，而那份 TOML
完全合法。它长期看不见，是因为更上游的一个缺陷让 install-by-name 从来走不到 validate
（内置 marketplace 对所有查找不可读，见 §3.10）。

**选放宽输入而不是改那两份 manifest**：插件住在独立仓、独立发布节奏，一个只认 `name`
的解析器同时也让每一个已按 `id` 发布的第三方插件变砖。词汇仍然只有一个所有者——这个
字段——并且 `plugin.toml` **从不被 Aleph 写回**（唯一被序列化的 manifest 是
`plugins.toml`，操作者自己的文档），所以作者选的拼法不会在他不知情时被改写。

两条守卫在 `src/extension/validation.rs`：`a_service_may_spell_its_identifier_id_or_name`
断言两种拼法都**把值送到**（只放宽类型却丢掉所接受的东西，是把响亮的拒绝换成静默的
半加载，比原缺陷更糟），且没有标识符时仍然报错；
`every_bundled_plugin_passes_the_installers_own_validation` 让**每一个随包插件都必须
过 Aleph 自己的安装校验**——`plugins/` 是 submodule，写它的和解析它的是两个作者，而
在此之前从没有人拿第二个作者跑过第一个作者的产物。

---

## Scope 管理：发现路径 与 可见性是两件事

**发现路径**（谁被扫到，优先级高→低）：

| Scope | 路径 | `ScopeKey` |
|-------|------|-----------|
| `agent-level` | `~/.aleph/agents/<id>/plugins/` | `Global` |
| `local` | `<project>/.aleph/plugins.local/` | `Project(root)` |
| `project` | `<project>/.aleph/plugins/`、`<project>/.claude/` | `Project(root)` |
| `user` | `~/.aleph/plugins/installed/` | `Global` |
| `claude-cache` | `~/.claude/plugins/cache/…`（只读） | `Global` |
| `bundled` | 编译期嵌入 | `Global` |

**可见性**（谁在哪个请求里看得见，2026-09-20 起）：每条 registry 行在发现时带一个 `ScopeKey`；
每张脸在**请求构建时**调同一个谓词 `visible_to(key, ctx)`（`src/extension/visibility.rs`，前身是只服务
hooks 的 `scope.rs::project_scope_allows`）：`Global` 永远可见；`Project(p)` 只对
`ctx.project_root == Some(p)` 的会话可见。五张脸共用：tool index · skills 索引 · agents 解析 · slash 列表 ·
MCP tool bridge（按拥有 server 的插件的 key 过滤；server 进程本身仍是全局的）。hooks 的
`project_scope_allows` 改为调它。**`VisibilityCtx.project_root` 只有一份推导**——hooks 今天那一份
（`executor.rs:918` 上游）抽出来共用，不新造第二个「当前项目」。

⚠️ **行为变更**：**无 project 的会话只见 `Global`**（fail-closed；此前是全部可见）。一个在 `~` 里起的
会话再也看不到某个项目的 `.claude/` 插件——这是有意的。

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
al plugin marketplace add HKUDS/CLI-Anything

# 步骤 2: 安装插件
al plugin install cli-anything

# 验证
al plugin list
# → cli-anything    -    enabled    Build powerful, stateful CLI interfaces...
```

**支持的组件类型：**

| CC 组件 | Aleph 支持 | 说明 |
|---------|-----------|------|
| `skills/*/SKILL.md` | ✅ 完全支持 | 通过 SkillSystem 加载 |
| `agents/*.md` | ✅ 完全支持 | frontmatter → `AgentDef`，**正文 → `AgentDef.system_prompt`**（2026-09-20 起；此前正文丢弃）。`permissionMode` **解析但不应用**（见下方 DEVIATION 3）；`color` 忽略 |
| `commands/*.md` | ✅ 完全支持 | slash 条目随 mount/unmount 注册/撤销；`/cmd args` 时**正文经 `SkillTemplate` 展开后注入本轮**（`$ARGUMENTS` / `$1..$N` / `${N:-d}` / `@file` 经沙箱读 / `` !`cmd` `` 经 shell 同意闸）——展开后的正文**瞬时投递**，转录里持久化的是原始 `/cmd args`（U-c）。`argument-hint` 进列表；`allowed-tools` 作本轮静态 retain；`model` 走请求级 pin；`disable-model-invocation` 只留人类入口 |
| `hooks/hooks.json` `timeout` | ⚠️ 偏离 | Claude Code 默认 600 s；Aleph 默认 **300 s** 且上限 **300 s**（`MAX_HOOK_TIMEOUT_SECS`，`src/extension/hooks/mod.rs`）——hook 在工具派发内运行，本就受 180 s tool budget 约束，更长的值会被钳到 300 并记一条 warn。写 `timeout: 600` 不报错，只是拿不到 600。 |
| `hooks/hooks.json` (command type) | ✅ 支持 | Shell 命令型 hook。**继承守护进程的整份环境**（与 CC 一致：CC 的 hook 继承 CC 的环境，守护进程环境里的 provider key、channel bot token 因此对 hook 可见），而插件命令的 `` !`cmd` `` 不继承（见上一行） |
| `.mcp.json` (MCP servers) | ✅ 支持 | 通过 MCP client 启动 |
| `.claude-plugin/plugin.json` | ✅ 完全支持 | CC JSON parser |
| `marketplace.json` | ✅ 完全支持 | Marketplace 系统 |
| `outputStyles` | ⏳ 延后 | 解析但不执行 |
| `.lsp.json` | ⏳ 延后 | 解析但不执行 |

**DEVIATION（有意与 Claude Code 不同；验收表 `scan-cc-plugin-format.md` 70 项里标 DEVIATION 的就是这几条）：**

1. **hook `timeout` 默认 300 s、上限 300 s**（CC 600 s）——见上表那一行；理由：hook 跑在工具派发内，受 tool budget 约束。
2. **skills 的 CC 专属字段**：`skills/*/SKILL.md` Claude-Code-only fields (`when_to_use`, `argument-hint`, `arguments`, `disallowed-tools`, `model`, `effort`, `context: fork`, `agent`, `background`, `hooks`, `paths`, `shell`) parse without error and are NOT honoured, except `disable-model-invocation`, `user-invocable`, `allowed-tools` (pre-grant) and `when_to_use` (read). A skill relying on `context: fork` runs inline; one relying on skill-scoped `hooks` gets none.
3. **agent `permissionMode` 不应用**：sub-agent 跑在父的 `ScopedToolService` 上，没有自己的执行档；值被解析并以 `debug!` 记下它本会映射到的档，偏离可见于日志而不是静默。
4. **不读 `~/.claude/settings.json`**：启用态由 `plugins.toml` 决定（U6）。
5. **用户级 hooks 文件在 `~/.aleph/hooks.json`**，不是 `~/.claude/settings.json` 的 `hooks` 键。

**CONNECT（与 CC 对齐，2026-09-20 接线；不是偏离）：**

- **exit code 2 = block**：JSON 决策与 exit-code 决策在同一个函数里派生；Interceptor 型事件 exit 2 → `blocked { reason: stderr }`（stderr 空也 block，通用原因）；其它非零 → 非阻塞警告；Observer 型只记日志。`hookSpecificOutput.updatedInput` 与 `update_input:` 前缀走同一条路；`permissionDecision: "block"` 亦读作 Block。
- **`hook_event_name` = hook 注册时用的那个拼法**（U-b）：注册为 `PreToolUse` 的 hook 收到 `"hook_event_name":"PreToolUse"`，注册为 `before_tool_call` 的收到 `before_tool_call`——注册行上一个字段、一份推导，Aleph 原生脚本不变；别名表 `CC_TOOL_ALIASES` 只在 matcher 派发时把 CC 名（`Bash` / `Edit` / `Write` / `mcp__srv__tool`）翻成 Aleph 名。
- **`permission_mode`** 从会话执行档映射，`ExecTier::Auto → "auto"`（CC 六值枚举里的真值）。
- **`allowed-tools` 双语义**：command → 本轮限制；skill → 预授权跳审批、不限制。
- **无 project 的会话只见 `Global` 插件**（行为变更，见 Scope 管理；CC 没有对应概念，不列为偏离）。

---

## 环境变量

插件内容（skill/agent/hook/MCP 配置）中可用：

| 变量 | 值 | 说明 |
|------|-----|------|
| `${CLAUDE_PLUGIN_ROOT}` | 插件安装目录绝对路径 | CC 兼容 |
| `${ALEPH_PLUGIN_ROOT}` | 同上 | Aleph 别名 |
| `${CLAUDE_PLUGIN_DATA}` | `~/.aleph/plugins/data/{id}/` | 持久数据目录 |
| `${ALEPH_PLUGIN_DATA}` | 同上 | Aleph 别名 |

四个变量在**同一个点**展开：`mcp_config.rs::substitute_vars`，路径单一源
`extension::plugin_data_dir`。**shell 源码例外**——hook 的 `command` 与命令正文里的
`` !`cmd` ``：解析插件时（`AdapterRegistry::parse_dir`）只展开 skill / 命令 / agent 正文里的
散文，这两处原样保留（哪段是 `` !`cmd` `` 由 `template::inline_commands` 这一个识别器决定——
命令正文（`SkillTemplate::render`）与 skill 正文（`skill_read` 的预处理器 `skill::preprocess`）
两张脸都用它，都在原文上找、从不重扫自己插进去的文字）；unix 上这些变量——以及 skill 的
`${ALEPH_SKILL_DIR}`——只写进子进程的环境、由 `sh` 当数据展开（目录名里的 `$(…)` / `"` / 空格
不会被当作源码解析；单引号 `'${CLAUDE_PLUGIN_ROOT}'` 因此保持字面量）；Windows 的 `cmd` 不会展开
`${…}`，在起进程时替换——hook 与 inline 命令共用 `hooks::plugin_shell_line` 这一个推导（skill 的
`${ALEPH_SKILL_DIR}` 在同一个构造器 `template::inline_shell_command` 里紧随其后替换）。

**skill 的 inline shell（`allow-inline-shell: true`，2026-09-28 起）与命令的走同一道闸**
（`extension::inline_shell`）：只在操作者的调用、且这一轮的工具闸不拒 `bash` 时跑
（`inline_shell_refusal`，工具面由派发咽喉 `ScopedToolService` 每次调用发布为
`TURN_INLINE_SHELL`；没经过咽喉的调用——`tools.invoke`、直接调用——一律不跑）；且原文必须在
同意清单里被批准，按 `(owner, skill, 原文)` 记在事件 `SkillRead` 下（插件 skill 的 owner 是插件
id + 插件的可见性键；其它 skill——**用户自己写的也算**——owner 是 `user`、按所在 skills 目录
键控），批准绑定 skill 目录，经 `aleph-server hooks list` / `hooks test` 审——模型能写 skill
（`skill_manage`、往项目 `.aleph/skills` 里 `file_write`），所以 skill 的来路不是批准；而批准本身
（`<config>/shell-hooks-allowlist.json`）模型的文件工具写不到（`file_ops::get_denied_paths`）。否则
原位留占位 `[!`cmd` not run: <原因>]`，与命令面同一个字面量。子进程在 skill 目录里跑，
`CLAUDE_PROJECT_DIR` 是这一轮的目录，插件 skill 另有插件的路径变量与插件设置（去掉所有 secret，
与命令面同一个调用）；超时 30 s、输出 64 KiB，与命令面相同。

**脚本词：绑定或拒绝，没有第三种答案**（与命令面同一个谓词 `ShellHookConsent::unbindable_script_word`）。
同意清单把批准绑定到命令所跑脚本的**内容**：skill 的命令在 skill 目录里跑，所以相对路径
（`sh scripts/x.sh`）或经 `$ALEPH_SKILL_DIR` 写的脚本会被绑定，改了就要重审；经**其它任何变量**写的
脚本词（`${CLAUDE_PLUGIN_ROOT}/x.sh`、`$CLAUDE_PROJECT_DIR/…`、`$HOME/…`）同意清单找不到文件，这样
的命令**既不入清单也不跑**，占位写明是哪个词——skill 应把脚本放进自己的目录。（命令面的对应规则是
拒绝相对路径：它在会话目录而非插件根里跑。）**给 skill 作者**：被绑定的是命令里第一个像路径的词所指的
**文件**，数据文件也算——`cat "$ALEPH_SKILL_DIR/state.json"` 会把批准绑到 `state.json` 的内容上，这个
文件一改，批准就回到 pending；会变的数据别作为第一个路径词出现在 inline 命令里。
**迁移**：此前跑过 inline shell 的 skill，升级后先显示占位，直到被批准一次。
同意清单因此记的是插件写下的原文（`${CLAUDE_PLUGIN_ROOT}/…`），不是展开后的路径（2026-09-27 起；
此前用展开文本记下的批准失效、回到 pending 一次）。安装路径若会改动正文里的 `` !`cmd` ``
（路径里带反引号），正文整段不展开并记一条 warn。拼写清单单一源：`hooks::{PLUGIN_ROOT_VARIABLES, PLUGIN_DATA_VARIABLES}`。数据目录在插件**首次引用它**时创建（无条件创建会给每个
装好的插件留一个空目录）。

> **`_DATA` 那一对在 2026-08-16 之前没有任何生产者。** `mcp_config.rs` 上有一句注释说
> 它们「lives in the higher-level `McpManagerConfig::env` substitution path」——那条路径
> 全仓不存在，于是用了这个变量的插件收到的是字面量 `${ALEPH_PLUGIN_DATA}` 字符串。
> 更难发现的是：**那句注释是全仓唯一提到这个名字的地方**，所以按名字 grep 找断线，
> 找到的正是这个 bug 自己的不在场证明。配套还有一条测试**断言变量不该被展开**，
> 把缺陷钉成了契约。

---

## 关键代码文件

### Manifest 解析
| 文件 | 职责 |
|------|------|
| `manifest/cc_plugin_toml.rs` | 解析 `.claude-plugin/plugin.toml` |
| `manifest/cc_plugin_json.rs` | 解析 `.claude-plugin/plugin.json` |
| `manifest/adapters/auto_discover.rs` | 无 manifest 时自动发现组件 |
| `manifest/mod.rs` | 统一入口，优先级调度 |
| `manifest/types.rs` | `PluginManifest`、`AlephExtensions`、`AlephRuntime` |
| `manifest/declared_sections.rs` | **三个方言共用的**「声明的 section → capability」翻译（`[[tools]]`/`[[hooks]]`/`[[commands]]`/`[[services]]`/`[prompt]`） |
| `manifest/component_source.rs` | 组件字段的 path / 数组 / 内联三形态，以及内联分支的真消费者 |
| `plugin_vars.rs` | 四个 `${*_PLUGIN_ROOT}`/`${*_PLUGIN_DATA}` 的**唯一**展开器 + 配置→环境变量的唯一翻译 |

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
| `projection.rs` | **唯一**的进程级投影咽喉（skill dirs / subagents / 工具索引），源码级 census 守 |
| `plugin_state.rs` | `<data_dir>/plugins.toml` — 耐久启用态（`.disabled` 标记的替代者）|
| `scope.rs` | Scope 路径解析 |
| `mcp_config.rs` | 读取 `.mcp.json`，环境变量替换 |
| `loader.rs` | 运行时加载（MCP/WASM/Static）|
| `types/plugins.rs` | `PluginKind`、`PluginScope`、`PluginRecord` |

### CLI
| 文件 | 职责 |
|------|------|
| `interfaces/cli/src/commands/cli_args.rs` | `PluginAction`/`MarketplaceAction` 定义 |
| `interfaces/cli/src/commands/plugins_cmd.rs` | 走 Gateway 的生命周期子命令 |
| `interfaces/cli/src/commands/plugin_cmd.rs` | 本地开发工具（init/validate/pack/doctor）|
| `src/bin/aleph-server/commands/plugins.rs` | `aleph-server` 内建的本地 handler |
| **`shared/protocol/src/plugins.rs`** | **wire 契约单一源**——每个 `plugin.*` 形状 |

### Gateway
| 文件 | 职责 |
|------|------|
| `gateway/handlers/plugins/handlers.rs` | RPC handlers（`plugin.*` + `plugin.marketplace.*`） |
| `gateway/handlers/mod.rs` | RPC 方法注册 |

---

## Gateway RPC 方法

| 方法 | 说明 |
|------|------|
| `plugins.list` | 列出已安装插件 |
| `plugin.install` / `plugins.install` | 安装插件（URL） |
| `plugin.uninstall` / `plugins.uninstall` | 卸载插件 |
| `plugin.update` | 升级已装插件（原子换装 + 版本比对，`force` 强制）|
| `plugins.enable` | 启用插件 |
| `plugins.disable` | 禁用插件 |
| `plugin.marketplace.list` | 列出 marketplace **注册项**（name/source/type）——不是内容 |
| `plugin.marketplace.browse` | 列出 marketplace **内容**（可选 `marketplace` / `query` 子串，匹配名称与描述）|
| `plugin.marketplace.add` | 添加 marketplace |
| `plugin.marketplace.update` | 更新缓存 |
| `plugin.marketplace.remove` | 移除 marketplace |
| `plugin.marketplace.install` | 从 marketplace 安装 |

`plugins.*`（复数）是每个客户端实际调用的命名空间；`plugin.*`（单数）只剩有客户端的四个动词（`install` /
`uninstall` / `update` / `reload`）与 `plugin.marketplace.*`。2026-09-20 之前单数还注册着 `list` /
`installFromZip` / `enable` / `disable` / `config.get` / `config.set` 六个零客户端动词，且这里的注释把
"谁是遗留"说反了；插件配置的唯一面现在是 `plugin_manage(config_get / config_set)`。

⚠️ **两个命名空间的能力集并不相等**：`callTool` / `list` / `installFromZip` / `enable` / `disable` **只**在
复数上（后四个的单数注册——连同 `config.get` / `config.set` 一起——于 2026-09-20 本轮 CUT：零客户端，插件
配置的唯一面现在是 `plugin_manage` 工具），`update` / `reload` / `marketplace.*` **只**在单数上
（`executeCommand` / `load` / `unload` 于 2026-09-20 CUT——零客户端，且 `load`/`unload` 绕过 registry
直接对 WASM loader 寻址，与 mount/unmount 生命周期相悖）。

**这一段曾经描述的缺陷已经修完，分两轮**：Panel 的设置页此前只说复数命名空间，于是
装插件走的是仅支持 git URL 的 `handle_install`，一个 marketplace 名字被当成 git URL
克隆并失败（2026-08-19 第二轮改为调 `plugin.install`，服务端自己分类）；而找到那个
名字则要等本轮的 `plugin.marketplace.browse`。现 Panel 调 `plugin.install` /
`plugin.marketplace.browse` / `plugin.marketplace.update` / `plugin.marketplace.install`。
**2026-08-20 起 `add` / `remove` 也有 Panel 面了**（设置 → 插件 → Marketplaces）。此前
它们唯一的客户端是 `interfaces/cli`，而 release workflow 从不构建那个二进制，所以桌面
App 用户要进 .app bundle 翻出内嵌的 `aleph-server` 才能加一个第三方市场。同日第二轮
再加工具面（见下）。三个面因此都在，且**都调同一个 `classify`**。

⚠️ **`add` 不抓取，三个面各自 compose `add` + `update`**：一个注册了却空着的目录看起来
是坏的，而这三个面上没有任何东西提示第二次调用会填满它。刻意不折进 handler——`add`
对每个客户端保持一个意思。

### source 分类只有一个答案（2026-08-20）

`add` 的两个面各写过一份启发式，在**四种输入上分歧**：`C:\dir\mk`（RPC 判 github、
名字 `c:\dir\mk`；子命令判 local、名字 `mk`）· `./foo/bar`（RPC local ✓；子命令
github ✗）· `myrepo` 裸名（RPC github；子命令 local）· `/abs/My Dir`（名字大小写两个
答案）。外加**两边都错的一条**：`~/foo` 双方都判 github，而 `local_source::expand_tilde`
正是为这个形式写的——一个只有 resolver 支持、没有任何生产者产得出来的分支。

**修法不是第三个启发式，分类器早就存在**：`github_source::is_valid_owner_repo` 就是
决定 github 那条路能不能走通的那个函数（严格两段 `[A-Za-z0-9_.-]`，`.`/`..` 不算）。
问它，「判成 github」和「clone 得下来」就是同一个答案，而不是两个要保持同步的猜测。

单一源 `marketplace/source_spec.rs::classify(source, explicit_name)`，返回
`MarketplaceSpec { name, source, source_type }`。**名字校验上移到 add 边界**——旧 RPC
存得进 `c:\dir\mk`，要到 sync 才失败。**GitHub URL 归一化不是加功能，是不制造回归**：
`is_valid_owner_repo` 拒绝 URL，判 Local 之后错误会变成「Local marketplace path does
not exist: https://…」，比它取代的那句更误导；所以四种 URL 拼法折叠成它们指代的
`owner/repo`，而 deep link（`/tree/main/sub`）**刻意不猜**——猜出来会 fetch 一个没人
要的东西。

顺带收敛掉三族重复（本轮都碰到了，抄第六份才是错的）：**(a)** 「这个名字会被 join 到
一个受管目录上」的安全谓词有**五份**（`sync_github_marketplace` / `removal_refusal` /
`resolve_cache_dir` / `install_plugin_from_cache` / `update_plugin_from_cache`），两份
措辞短到不说明规则；现 `marketplace/names.rs::reject_unsafe_segment`。**(b)** 「怎么把
一条已存的注册读回来」有**四份**；现 `configs_from_entries` + `MarketplaceManager::
from_config()`，后者顺带让工具面不必依赖一个网关 handler。**(c)** `"github"` / `"local"`
两个 token 有**六个**手写点；现 `MarketplaceSourceType::{as_config_str, from_config_str}`。

⚠️ **`list` 与 `browse` 是两个问题**：前者答「注册了哪些 marketplace」，后者答「某个
marketplace 里有什么」。拿 `list` 去找插件名的调用者会一个都找不到，然后得出「这个
marketplace 是空的」——那正是 `browse` 存在的理由。`browse` **不联网**（只读已抓下来
的缓存），拉取是 `update` 这个显式动作；读不出的 marketplace 逐条出现在
`problems` 里并说明该跑哪条命令，因为空列表同时意味着「没匹配」「没同步」「不是
marketplace 仓」「manifest 坏了」，只有第一种值得继续敲字。每一行还带
`installable` + `unavailable_reason`，来源是 `PluginSearchResult::installable_path`
——**install 自己执行的那个谓词**，不是渲染端对 source 枚举的第二次解读。

**marketplace 条目的 `source` 对象也只有一个读者（2026-09-20）**：判别键在磁盘上拼作 `source.source`
（`claude-plugins-official` 当天的 310 条里 258 条对象形全是这个拼法，0 条 `type`），在官方文档里拼作
`source.type`。两种拼法都进 `MarketplacePluginSource::external_kind()`（`marketplace/types.rs`）——先读
`source` 再读 `type`——而不是各建一个 serde 字段：对象形本来就不逐字段建模（五个无消费者的 struct，R10），
拒绝消息引用的那个词才是唯一要读的东西。

---

## 模型面：`plugin_manage` 工具

R8 要求每个可配置操作都有对话面。插件曾是唯一没有工具面的扩展类型
（skills 有 `skill_manage`、hooks 有 `hooks_manage`、Hub 有六个 `hub_*`）。

`plugin_manage` 的动作：`list` / `show` / `enable` / `disable` / `reload` /
`config_get` / `config_set` / `trust_status` / `trust` / `untrust` /
`trust_enforce` / `marketplace_list` / `marketplace_browse` / `marketplace_add` /
`marketplace_remove` / `marketplace_update`。

**marketplace 动作（2026-08-20 用户裁决加入，推翻 2026-08-19 的「刻意不加」）**：
注册一个目录**不是**安装一个插件——它记录目录住在哪，同步时 `git clone` 一堆
manifest，在人类安装之前没有任何东西从它里面执行。这是这五个动作与下面那条 install
边界能同时成立的全部理由，而 DESCRIPTION 必须把两件事都说出来，否则「不能 install」
读起来就是自相矛盾。

⚠️ **原来那条反对意见没有被一起推翻**：给模型一张它不能作用的目录，是「一张附带动作
邀请的清单，它列的每一行都必须真的能被那个动作作用」的反面。`MarketplacePluginRow`
的 `installable` 位主语是 **Panel 的 Install 按钮**，挂在一个没有 install 动词的工具上
就正是那个假邀请。所以 browse **不原样透传那一行**：工具侧投影把它改名为
`operator_can_install`（同一个 `marketplace_row` 推导，只是说清主语），守卫
`a_browse_row_names_the_actor_who_can_install` 钉住它。`unavailable_reason` 保留——
「Aleph 根本装不了这一条」正是操作者需要听到的。

整段跑在 `spawn_blocking` 里：`git clone`、配置读写、目录删除都是阻塞的，而这里是
agent 循环的执行器。

**它结构上不能装也不能卸**——装插件就是在机主的机器上运行第三方代码，那一步留给人
和 consent-gated 的 `hub_install_run`。这是 `hooks_manage` 的先例：随便报，永远不批。

---

## MCP Runtime Wiring（已完成）

MCP 插件的 `.mcp.json` server 现已作为 **transient（仅运行时，不落盘）** server 注册到运行中的 `McpManager`，工具经现有 tool bridge 自动注册。

- **transient 通道**：`McpManagerHandle::add_transient_server` / `remove_transient_server`（`src/mcp/manager/`）。与 `add_server` 不同，它只 `start_server_internal`，**不** upsert/持久化到用户 MCP 配置文件——插件 server 由插件生命周期管理，绝不污染用户配置。`server_id` 形如 `plugin:<id>/<name>`。
- **注册编排（2026-09-20 起走 lifecycle）**：`mount(id)` 对 MCP-kind 插件调 `add_transient_server`，返回的 `Disposer` 记入该插件的 `EffectScope`（step `"mcp_server"`）；`unmount(id)` 逆序 dispose 即 `remove_transient_server`。此前 `set_plugin_enabled(true)` 什么都不做、只有 `reload()` 调 `sync_mcp_plugin_servers`（判据 §14 闸的两个方向不对称）——那条路已删。
- **卸载清理**：不再有单独的「捕获 server id 再拆」逻辑——server id 住在 disposer 闭包里，dispose 就是拆。
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

`type` 默认是 `stdio`，所以现有插件无需修改。**唯一的读者**是
`mcp_config::parse_declared_servers`（P4.15 / D-1）：六个 manifest adapter 都经它把
`.mcp.json` / manifest 指定的路径 / 内联 `mcpServers` 对象解析成 `McpManagerConfig`，
作为 `CapabilityDeclaration::McpServer` 挂在 adapter 输出上——`plugins.list` 的
`mcp_servers_count` 数的是这张表，`mount` 的 `mcp_server` step spawn 的也是这张表
（只在 spawn 时叠上运营者的插件配置 env）——两边共用一道闸 `PluginKind::starts_mcp_servers`
（只有 `mcp` kind 为真）：显式 `wasm` / `static` runtime 或无 runtime 概念的格式（Codex / Cursor /
auto-discover）声明了 server 时，行上计数为 0，`status_detail` 写明声明了几个、为何不启动。
manifest 缓存的 key 覆盖 kind 的每个输入（含插件根的 `.mcp.json` 是否存在）。`McpJsonServerEntry` 解析器对缺失字段
（stdio 无 `command` / remote 无 `url` / 未知 `type`）做 hard error，不让 spawn 进入半配置状态。

**stdio `command` 的归属规则**（The one reader of a stdio command）：裸名（`node` / `npx` / `uvx`）
走 `PATH`；未用 `${CLAUDE_PLUGIN_ROOT}` / `${ALEPH_PLUGIN_ROOT}` 书写的绝对路径原样接受；
相对路径或以这两个变量书写的路径必须存在且 canonicalize 后落在插件根内，并被改写为该绝对路径
（spawn 没有自己的 cwd）。越界或无法解析 ⇒ 整个插件解析失败，行上的错误写明 server 名与原因，
不计数也不 spawn。参数不经 shell（argv 形式 spawn）。

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
当前状态：目录结构已迁移到 CC 兼容格式（`.claude-plugin/plugin.toml`），Node.js 插件标记为 `runtime = "mcp"` 但 `src/index.js` 仍是旧 IPC 格式——那个格式（`method === "plugin.call"`）从来没有宿主，例：`plugins/media-office/src/index.js:349`；其 `:264 onPostToolUse` 是无人能调的 JS hook handler。本仓 2026-09-20 删掉了同形状的 `examples/plugins/media-video`。
需要：将每个 Node.js 插件的入口文件改为 MCP Server SDK 实现（兄弟仓 Aleph-plugins 的 follow-up）。

---

## WASM Credential Injection（host-side 落地）

WASM plugin 的 `http.credentials: Vec<CredentialBinding>` 字段声明 host-pattern +
secret 名 + 注入策略（Bearer/Basic/Header/Query/UrlPath）。host 端的
`host_functions::try_http_fetch` 现在在 egress 前实际调用
`credential_injector::inject_credential`，通过 `WasmCapabilityKernel` 持有的
`SecretResolver` 解析 secret 名。Plugin guest 永远不接触明文 secret 值——这是
**live property**，不再是 goal。

`SecretResolver` trait 与三个实现在
`src/extension/runtime/wasm/secret_resolver.rs`。**运行中的 daemon 装的是
`VaultBackedSecretResolver`**（2026-08-19 接线）；`DenyAllSecretResolver` 是没有
vault 时的回落——测试二进制从不安装那个进程级 vault，所以它们不必特意选择就保持
拒绝。

⚠️ **在此之前这一整节描述的是一个不可达的特性**：`load_plugin` 转发的是 `None`，
而 `load_plugin_with_resolver(_, Some(..))` 全仓零调用点，所以每个
`[capabilities.http.credentials]` 绑定都把该插件的 `http_fetch` 变成必然的
`secret not found`。

**闸与线必须同批落地**。`WasmCapabilityKernel::resolve_secret` 一道
`check_secret_pattern` 都不过，而 `try_http_fetch` 把 `binding.secret_name` 从
manifest 原样喂给它 —— 真 resolver 一装，`[capabilities.http.credentials]` 就是
一条点名任意 vault key、绕过 `[capabilities.secrets] allowed_patterns` 的路。闸
现在住在 `resolve_secret` **里面**而不是它的调用者上（第三个调用者不必知道它存在
就继承它），未声明 `[capabilities.secrets]` 时 fail-closed。

### `{{secret:NAME}}` 与两种设置形态

`plugins.toml` 是明文，所以 `config_ui_hints` 标 `sensitive` 的值以
`{{secret:NAME}}` 引用形式存放。**解析只发生在运行时那条边上**
（`extension/plugin_secrets.rs`）：

| 消费者 | 形态 | 为什么 |
|--------|------|--------|
| loader 快照 → WASM guest / MCP 子进程 | 已解析 | 插件代码要真值 |
| hook 子进程环境 | 已解析 | 同上 |
| `plugin_manage(config_get / show)` | **存储形态** | 这段文字进模型上下文 |

在 `plugin_settings` 里解析是一行，代价是把每个配置好的密钥灌进转录和设置页 ——
所以运行时形态是另一个函数（`plugin_settings_for_runtime`），名字说明它站在哪一边。
守卫是整目录规则（`builtin_tools/` 与 `gateway/handlers/` 不得读解析形态）。
解析不了的引用**丢弃这个键**而不是透传占位符：读不到设置的插件走自己的默认值，
而拿到 `{{secret:...}}` 的插件会把那个字面量当凭据发给远端。

---

## Manifest 解析缓存

`manifest_cache::ManifestCache`（`src/extension/manifest/manifest_cache.rs`）—
LRU（512 条），key = `(canonical path, size, mtime, ctime, dev, ino)`。Boot 时
`parse_manifest_from_dir_cached_global(dir)` 自动咨询/填充；热重载期间任何
in-place 编辑都会改变 key tuple，cache 自然 miss。key 里带 `dev`/`ino` 是为了对抗硬链接替换（`canonicalize` 关不掉硬链接——附录 E.3）。

---

## Lazy Activation Planner —— ❌ 已删除（2026-08-07）

**不存在懒激活。所有 enabled 插件在 boot 时一次性加载。** `[plugin.activation]`
块**不被任何 adapter 读取**，写了等于没写。

曾经有过一个从参考实现的 activation planner 移植来的 Rust 版
（`src/extension/activation.rs` 的 `ActivationPlanner` / `ActivationHints` /
`ActivationTrigger` / `ActivationPlan` / `CapabilityKind` / `tier_kinds`，约 600 行），
本轮按 R10 YAGNI 整体删除。删的理由比「planner 没有生产调用者」更深一层：
`PluginManifest.activation` **从来没有非 `None` 过**——三个 manifest adapter
（`cc_plugin_json` / `cc_plugin_toml` / `toml_types`）在各自的构造点全部硬编码
`activation: None`，其中 `cc_plugin_json` 甚至把这个块反序列化进自己的 DTO 之后
再丢掉。所以写了 `activation` 块的插件作者既没拿到懒加载，也没拿到任何诊断。

**重连不是补一个调用点**：懒激活需要一条「按 trigger 重入」的加载路径，而
`load_plugins` 是 boot 时对插件目录的一次性遍历——那条路径得先造出来。要复活
请从参考实现的 activation planner 和
`git log --follow src/extension/plugin_trust.rs` 起步，不要从被删的 Rust 起步——
它从未对着真实 registry 跑过。

存活下来的是同文件里的 `OwnerTrustPolicy`（见下节），文件已随之更名为
`src/extension/plugin_trust.rs`。

---

## Owner Trust Policy（P3.5）

锚点 `src/extension/plugin_trust.rs`（该文件曾名 `activation.rs`，
activation planner 删除后按内容更名）。

Aleph 暴露 `OwnerTrustPolicy::permissive()` (默认) 和
`OwnerTrustPolicy::restrictive(allowlist)`。restrictive 模式下，`Bundled` 和
`Config` origin 的插件始终可加载；`Workspace`、`Global` 和 `ClaudeCache` origin 的插件必须在
allowlist 中。`LoadSummary.skipped_by_trust` 记录被策略跳过的 plugin 数，让
operator 看到"装了但没启用"的 plugin。

Bundled / Config origin 短路、其余按 allowlist——这是 Aleph 自己的规则，不再标注出处。

### 策略住在哪、谁能拨（2026-08-19 接线）

⚠️ **在此之前这整节描述的是一个零生产者的策略**：唯一的生产构造点传的是
`ExtensionConfig::default()`，所以每个安装都是 permissive，`PluginStatus::Blocked`
不可能产生。

耐久答案在 **`plugins.toml`**，不是新造的 `[extensions]` 段：

```toml
[trust]
enforce = true          # 默认 false = legacy「全都加载」

[entries.my-plugin]
trusted = true          # 这一条才让它在 enforce 下通过
enabled = true          # 与 trusted 正交，见下
```

选这里而不是 `config.toml`：它已经是插件子系统的耐久操作者文档，manager 在构建
策略**前两条语句**就加载了它，而同一决定的「按插件」那一半本来就是它的一个 entry。
`ExtensionConfig.owner_trust` 与它的 DTO 已 **CUT**——"这个插件能不能加载"有两个源
时，输的会是操作者手动编辑的那个。

**`trusted` 与 `enabled` 刻意分开**：停用是「现在不要」，信任是「这段代码允许运行」。
合并等于重新启用一个插件时静默重新授予信任。

拨它的是 `plugin_manage` 的四个 action（R8）：`trust_status` / `trust` / `untrust` /
`trust_enforce`。**这些都是 LOAD 闸**——它们不停止已经在跑的插件，`disable` 才是；
工具的回复逐字说明这一点，因为两个动词的名字本身分不出来。

**`Bundled` / `Config` 至今没有生产者，这是有意的**。随包插件被解压进 marketplace
cache（`plugins/cache/aleph-official/<id>`），scanner 只从 plugin parent 下降一层
而那条路径是两层，所以它们不是「被豁免」而是**根本没被发现**；它们只有被安装进
`plugins/installed/` 才可加载，那时就是货真价实的 `Global`。所以 enforce 的含义没有
星号：每个插件都需要一次显式 vouch，被拒的那些按 id 列出来，好让人有东西可 vouch。

`PluginOrigin` 本身在同一轮才有了真生产者：六个 manifest adapter 全部在构造点硬
编码 `Global`，`collect_plugin_dirs` 又把 scanner 的 `DiscoverySource` 丢了。现在
它带着走，`PluginOrigin::classify` 是唯一推导。

真机覆盖：`qa/plugins/run.sh trust`（三次重启——LOAD 闸只有在下一次加载时可观测）。

### 为什么技能目录没有对应的闸（2026-08-19 裁决）

这道闸治理的是插件目录。一个自然的追问是：手工塞进 `~/.aleph/skills/` 的目录，该不该
同样需要背书？**裁决是不该**，理由不是「风险小」，是**两者拦的不是同一件事**：

- 插件闸拦的是**能力注册**——hook 不经模型选择就触发、MCP 起子进程、WASM 被加载。
- 技能注册不了任何东西。它是模型**可能会读**的一段文字，而它能引发的一切动作仍要
  重新过工具权限层、执行档位与沙箱。

还有一条结构性理由：`~/.aleph/skills/` 有一个**设计内的模型写者**（`skill_manage`，
R8/R9 的自我改进条款）。给它加 allowlist，要么自动放行模型自己的写入（那就什么都
没拦住，因为「有东西写了一个目录」正是要拦的那件事），要么打断那个循环。而按上文
`[trust] enforce` 自己的论证，默认还必须是关的——于是交付的是一个旋钮，不是一道控制。

**真正存在的不对称在别处，且已修**：`skill/guard.rs` 的威胁扫描挂在**安装**路径
（`skill_manage` 两处 + markdown 上传），手工放进目录的技能绕过它；而 `skill_read`
把正文**不加围栏**交给模型——这是对的，技能就是指令，加围栏会废掉这个机制——只是
模型无从知道这段字是随包发的、是某天出现在技能目录里的、还是跟着刚 clone 的仓库
进来的。出处一直被算着（`skill::guess_source`），只用于覆盖优先级。

现 `ReadSkillOutput.provenance`：`bundled` / `user` / `workspace` / `plugin:<id>`，
由 `SkillSource::provenance` 的**穷尽匹配**派生，所以新增变体必须在编译期回答这一问。
**只上 `skill_read`，不上 `<available_skills>` 索引**——索引对 ~90 个条目每请求付费，
而这个答案要紧的时刻正是正文即将被照做的那一刻（R9 第二把尺）。**只陈述不劝诫**：
出处是模型推不出的运行时事实，「该多怀疑谁」是教强模型怎么想。

---

## 生命周期四原语（2026-09-20，`src/extension/lifecycle.rs`）

| 原语 | 语义 |
|---|---|
| `mount(id: &str) -> Result<PluginStatus, MountError>` | 解析 manifest → owner-trust / enabled 门（`plugins.toml`；`ClaudeCache` origin 按 origin 判默认 disabled）→ 新建 `EffectScope` → **按固定顺序**注册六种效果 → 任一步失败即 `dispose` 已注册部分（**全有或全无**）→ `write_failed_row` 写 `Error("<step>: <reason>")`；成功写 `Loaded`（MCP-kind 先 `Pending { waiting_on }`，server 完成 `initialize` 后由 `watch_server_starts` 改 `Loaded`） |
| `unmount(id: &str) -> Result<DisposeReport, UnmountError>` | 从 `scopes`（`Mutex<HashMap<String, EffectScope>>`）取出该插件的 `EffectScope` → `dispose`（逆序；单条失败记日志带 step 标签、**不停**）→ 写 `Disabled` / 移除行 |
| `reload_plugin(id)` | `unmount` + `mount`。2026-09-20 前的窄孪生（只刷 tool index、跳过 hooks / projections / MCP / services）已删 |
| `reload()` | 对每个已发现插件 `unmount` + `mount`；`stop_orphaned_services` 变成 dispose 的自然结果 |

六种效果与它们的逆（注册顺序即下表顺序；dispose 逆序，所以 registry 行最后撤——视图重算时它已不在）：

| step 标签 | 注册 ↔ 逆 |
|---|---|
| `registry_row` | `PluginRegistry::register_plugin` ↔ `unregister_plugin` |
| `wasm_module` | `PluginLoader` load ↔ unload |
| `mcp_server` | `McpManagerHandle::add_transient_server` ↔ `remove_transient_server` |
| `service` | `service_manager` start ↔ stop |
| `memory_extension` | `MemoryExtensionRegistry::register*` ↔ `unregister(plugin_id)`（**2026-09-20 新增**——此前 disable 后 `[memory]` 扩展仍挂着） |
| `slash_command` | ToolCatalog `register_skills` ↔ `unregister_skills(&[String])`（**新增**；disposer 持有它注册的那些 id——此前只在 boot 注册一次） |

**规则只有一句**：它有逆操作吗？有 → **效果**，注册函数返回 `#[must_use] Disposer`，由 lifecycle 放进该插件的
`EffectScope`；没有但能从 registry 重算 → **视图**（tool index 快照、`PLUGIN_SKILL_DIRS`、`PLUGIN_SUBAGENTS`、
`HookExecutor`），由下一节那一个函数派生；两者都不是 → 它不该由插件写入运行时。

**每次迁移之后，且只在 `after_transition()` 这一处（每个公共原语跑一次）**：`republish_plugin_projections()` +
`sync_hooks_from_registry()` + `if let Some(face) = try_mcp_face() { face.notify_tools_list_changed() }`。
迁移在既有的 `load_guard` 上串行。`set_plugin_enabled(true/false)`、watcher、`plugin.reload` / `hooks.reload` RPC
都只调这四个原语——没有第五条改激活态的路。插件 id 就是 `String`（无 newtype）。

守卫：G1 census（`registrar/` `service_ops.rs` `src/extension/loader.rs` `memory/extensions/` 里产生运行时副作用的
`pub fn` 必须返回 `Disposer`）· G2 往返（夹具插件覆盖六种效果，`mount` → 六面快照 → `unmount` → 快照 == mount 前）·
G3（`publishing_plugin_projections_has_exactly_one_author` 改钉 `lifecycle.rs` 里的那个调用点）。

## 进程级投影的单一咽喉（`projection.rs`）

一个插件不只活在 `PluginRegistry` 里。加载它会把它**发布**到四个活得比任何单次调用都久的面：

| 投影面 | 谁读它 |
|--------|--------|
| `utils::paths::PLUGIN_SKILL_DIRS` | `get_all_skills_dirs` → `skill_read` / `skill_list` 的搜索集；`SkillSystem::scan_roots` → 每次扫描/usage sidecar 遍历的插件那一半（**插件技能目录的唯一来源**——`init` 只收 base 目录，D-5，2026-09-22）；`skill::guess_source` → 技能的归属插件 id（随目录一起发布的**注册表 id**，不是目录名——D-4，2026-09-22） |
| `agents::PLUGIN_SUBAGENTS` | `AgentRegistry::resolve`（委派）+ harness 的 `<available_agents>` |
| `SkillSystem` | 模型的 `<available_skills>` 索引（base 目录 merge 进 `skill_dirs`；插件目录不进——从 `PLUGIN_SKILL_DIRS` 现读，禁用即离开索引） |
| `ExtensionManager::active_plugin_tools` | 工具名索引 |

这些是 **effect 不是返回值**——之后的任何一次调用都不会提醒你它们还装在那儿。2026-08-16 的答案是
「一个函数从 registry 派生整套投影，每一条能改变插件激活状态的路径都调它」，并刻意不引入 fiber 运行时。
**那个答案只对了一半**（2026-09-20）：它证明的是「改激活态的路径都调了那一个函数」，证不出「那个函数
盖住了所有面」——派生列举了三个面，三个月里在派生之外漏了四处（memory extension 无 unregister、slash
只在 boot 注册、MCP disable→enable 不重挂、`reload_plugin` 窄孪生）。这是列举法（判据 §5）。
现在**效果归 `EffectScope`、视图归派生**（上一节）：派生函数仍然只写一遍谓词，但它只负责**可重算**的
东西，且**唯一的触发点是 `lifecycle.rs::after_transition`**。DI 容器 / Proxy 上下文 / 级联重启仍不采。

它替换掉的缺陷：此前这份推导有**两个作者**，且**谓词不一致**——

| | skill dirs | sub-agents |
|---|---|---|
| `load_all` | `list_plugins()`（**任何状态**）| `list_agents()`（**不过滤**）|
| `set_plugin_enabled` | `list_active_plugins()` | 按 `status.is_active()` 过滤 |

于是一次开机（或任何 `reload()`，文件监视器会触发）会把**被禁用、被遮蔽、加载失败**的
插件的 skills 与 sub-agents 一并发布出去——模型读得到它们的 SKILL.md、委派得到它们的
agent——而运行时切换用的是正确的谓词。两条路径、相反的答案，跑在每次启动上的是错的那条。

谓词现在只写一遍。守卫 `projection.rs::tests::publishing_plugin_projections_has_exactly_one_author`
是**源码级** census（运行时分不出「第二个作者」和「第一个跑了两次」），
在别处出现 `publish_plugin_*` 调用时按文件行号红。

## 设计文档

- **Spec**: `docs/superpowers/specs/2026-03-20-plugin-system-claude-code-compat-design.md`
- **P0+P1 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p0-p1.md`
- **P2 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p2-marketplace.md`
- **P3 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p3-scope.md`
- **P4 Plan**: `docs/superpowers/plans/2026-03-20-plugin-cc-compat-p4-runtime.md`
