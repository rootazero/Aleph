# 把 Claude Code 的 Mod 系统移植到 Aleph

> 状态：可行性分析 + 落地方案
> 日期：2026-10-14
> 分析版本：Claude Code `2.1.296`（本地 `/Users/zouguojun/.local/share/claude/versions/2.1.296`，230MB Mach-O arm64，Bun 编译产物），Aleph `crates/` 当前 main

## 0. 一句话结论

**可以移植，而且 Aleph 已经完成了 ~70-80% 的基础工作**（manifest 解析、marketplace、registry、hooks 引擎、4 种 PluginKind、trust 策略）。剩下的关键缺口是 **Mod 子系统本身——即"在宿主进程内执行 JS 的 hooks module"**——以及 LSP / monitors / output-styles / workflows / bin PATH 等几个组件落地。整体方向是**逐步缩小差距，让 CC 第三方插件能直接装到 Aleph 上跑**，而不是把 Aleph 改造成 CC。

---

## 1. 术语澄清：CC 的 "mod" 到底是什么

很多中文资料把 Claude Code 的 plugin 叫 "mod"，**官方原义里 mod ⊊ plugin**。`docs.claude.com/en/docs/claude-code/plugins.md` 明确说：

> A **hooks module**: hooks written as JavaScript functions, which can also draw panes and add commands. **A plugin that has one is called a mod**. Some mods are built into Claude Code.

也就是说：

| 概念 | 含义 |
|---|---|
| **Plugin** | 装在 `~/.claude/plugins/...` 下的整个目录，可以挂载 skills / commands / agents / hooks / MCP / LSP / output-styles / workflows / themes / monitors / bin / settings |
| **Mod** | 一种特殊的 plugin，**额外带一个 `hooks/register.js`（hooks module）**——一段运行在 Claude Code 进程内的 JS，可以响应事件、调 API、画 UI pane、注册 slash command |

同时 `mods/overview.md` 又澄清了一个常见的混淆点：

> Claude Code's existing [settings hooks] also run on events, as a shell command, HTTP request, or prompt you configure in a settings file. A mod's handlers are functions that run inside Claude Code instead. ... on these pages, "hook" means a mod's handler, and the settings-file kind is a "settings hook".

所以 CC 同时有两套 hook：
1. **Settings hook**（`settings.json` / `hooks/hooks.json`）——shell 命令、HTTP、prompt
2. **Mod hook**（`hooks/register.js`）——进程内 JS 函数，**额外的** API 能力

`Mod` 的核心能力清单（`/docs/en/plugins/mods/overview`）：
- 响应事件：`onPreToolUse` / `onPostToolUse` / `onPromptSubmit` / `onTurnBegin` / `onSessionStart` / ...
- 调 API：`drawPane` / `addCommand` / `createTimer` / 调用工具 / 调用模型 / 读文件
- 绘制 UI：pane、按钮、文本框、状态
- 注册 slash command
- 工具调用规则（policy mods）
- 测试：可以无 session 跑自动化测试
- 管理：`allowManagedModsOnly` / `disableAllHooks` 控制开关

CC 自己内置的几个 mod：`cc-plugin-agents-md`、`cc-plugin-diff`、`cc-plugin-plugin-authoring`、`cc-plugin-sec-default`。

---

## 2. 证据：从二进制里挖出的实现细节

CC 2.1.296 是 230MB 的 Mach-O arm64（Bun 编译），`strings` 抓出以下 mod/plugin 相关内部标识符：

### 2.1 文件级约定
- `.claude-plugin/plugin.json`（主 manifest，JSON，camelCase）
- `.claude-plugin/marketplace.json`（marketplace 目录清单）
- `.claude-plugin/marketplace.toml`（TOML 变体，CC 也认）
- `.claude-plugin/types/claude-code/index.d.ts`（自带 TS 类型）
- `installed_plugins.json`（已装清单，存在 `~/.claude/plugins/`）
- `.claude-plugin-link` / `.claude-plugin-temp-`（临时/链接目录前缀）
- 错误信息模板（"won't load. To load this copy, give it a different 'name' in .claude-plugin/plugin.json"）—— 强 name 唯一性约束

### 2.2 一个 plugin 可以装哪些组件（自动发现用）

```
.claude-plugin/                     # 根目录
├── plugin.json                     # manifest（可选；无 manifest 时按目录名识别）
├── marketplace.json                # marketplace 入口（marketplace 自身）
├── commands/                       # slash command（Markdown）
├── agents/                         # subagent 定义
├── skills/                         # skills，标准 layout 是 <name>/SKILL.md
├── hooks/hooks.json                # settings-hook 配置
├── .mcp.json                       # MCP servers
├── .lsp.json                       # LSP servers
├── output-styles/                  # output style Markdown
├── workflows/                      # workflow .js
├── themes/                         # theme JSON
├── monitors/monitors.json          # 监控定义
├── bin/                            # 暴露到 PATH 的可执行文件
└── settings.json                   # per-plugin 临时覆盖 agent/subagent 配置
```

### 2.3 Plugin manifest（`plugin.json`）的完整 schema（官方 reference 拼出来的）

```jsonc
{
  "name": "deploy-tools",                  // 必填，plugin id
  "displayName": "Deploy Tools",           // 可选
  "version": "1.2.0",                      // semver
  "description": "...",
  "author": {                              // string 或 object
    "name": "...", "email": "...", "url": "..."
  },
  "homepage": "...",
  "repository": "https://...",            // string 或 object
  "license": "MIT",
  "keywords": ["deployment"],
  "defaultEnabled": true,                  // 未在 enabledPlugins 显式设时是否默认开
  "dependencies": ["secrets-vault"],       // 字符串/对象 数组
  "metadata": { "catalogId": "cat-123" },

  // 组件字段（每个都接受 path / path 数组 / inlined object——`component_source` 就是处理这个灵活性的）
  "skills": ["./extra-skills/"],
  "commands": {
    "status": { "source": "./commands/status.md", "description": "..." }
  },
  "agents": ["./agents/reviewer.md"],
  "hooks": "./config/extra-hooks.json",    // settings-hook 配置
  "mcpServers": {
    "deploy-api": { "command": "node", "args": ["${CLAUDE_PLUGIN_ROOT}/server.js"] }
  },
  "lspServers": "./.lsp.json",
  "outputStyles": "./styles/",
  "experimental": {                        // 不稳定字段
    "themes": "./themes/",
    "monitors": "./config/monitors.json"
  },

  "userConfig": {                          // 用户级配置，类型化
    "api_token": { "type": "string", "title": "API token",
                   "description": "...", "sensitive": true }
  }
}
```

### 2.4 userConfig 字段类型

| 字段 | 必填 | 说明 |
|---|---|---|
| `type` | ✅ | `string` / `number` / `boolean` / `directory` / `file` |
| `title` | ✅ | `/config` 对话框标签 |
| `description` | ✅ | 帮助文字 |
| `required` |  | 是否允许空值 |
| `default` |  | 默认值（str/num/bool/字符串数组） |
| `options` |  | string 类型的枚举（CC ≥ 2.1.271） |
| `multiple` |  | string 类型允许多选 |
| `sensitive` |  | 标记后存进 secure storage，不进 settings.json |
| `min` / `max` |  | number 的范围 |

### 2.5 内置 mod 行为（从二进制里抓的实现痕迹）

- `functionHooks/hooks-worker/hooks-worker.js` —— 独立 worker 跑 hooks module
- 事件类型：`hook_blocking_error` / `hook_cancelled` / `hook_error_during_execution` / `hook_non_blocking_error` / `hook_success` / `hook_system_message` / `hook_additional_context` / `hook_stopped_continuation` / `hook_deferred_tool`
- HookEvent 变体：`PreToolUse` / `PostToolUse` / `PostToolUseFailure` / `SessionStart` / `SessionEnd` / `UserPromptSubmit` / `Stop` / `SubagentStart` / `SubagentStop` ...
- 在 assistant turn 上加 `attributionPlugin` / `attributionSkill` / `attributionAgent` / `attributionMcpServer` / `attributionMcpTool` 字段，便于审计
- CLI 表面：`/plugin validate .claude-plugin/plugin.json` / `/plugin tag` / `/plugin marketplace`（带 git tag + force 语义）
- 安全策略：mod 跑在 user 权限下；`headersHelper` 必须 `strict: false` 并内联 manifest；组织可以 `allowManagedModsOnly` 锁死

---

## 3. Aleph 现状盘点（看代码不是看 README）

### 3.1 已经存在的（`/Volumes/TBU4/Workspace/Aleph/src/extension/...`）

| 现状 | 路径 | 行数 | 备注 |
|---|---|---|---|
| **CC plugin.json 解析器** | `src/extension/manifest/cc_plugin_json.rs` | 795 | 完整支持 CC 所有 manifest 字段，包括 `name` / `version` / `description` / `author` / `repository` / `keywords` / `skills` / `commands` / `agents` / `hooks` / `mcpServers` |
| **CC plugin.toml 解析器** | `src/extension/manifest/cc_plugin_toml.rs` | 857 | CC 还支持 TOML 变体，Aleph 也实现了 |
| **Aleph 自己的 TOML 解析器** | `src/extension/manifest/adapters/aleph_toml.rs` | – | 4 个 adapter 之一 |
| **Codex / Cursor adapter** | `src/extension/manifest/adapters/{codex,cursor,auto_discover}.rs` | – | Aleph 把 manifest 抽象成了 trait |
| **ManifestAdapter trait** | `src/extension/manifest/adapter.rs` | 628 | 每个 adapter 处理一种格式，统一通过 `detect + parse` 入口 |
| **ComponentSource** | `src/extension/manifest/component_source.rs` | 226 | **关键**：处理 CC 的灵活性（path string / 数组 / inlined object）；专门有"inline 必须用真实 parser 解析"的设计纪律 |
| **Aleph superset `[aleph]`** | `src/extension/manifest/cc_plugin_json.rs:189-230` | – | 在 CC plugin.json 里塞 `[aleph]` 段，可声明 `runtime` / `services` / `permissions`——CC 看不到，Aleph 启用 |
| **PluginKind 枚举** | `src/extension/types/plugins.rs:156` | – | 3 种：`Wasm` / `Mcp` / `Static` |
| **PluginRegistry** | `src/extension/registry/plugin_registry/mod.rs` | – | tools / hooks / services / skills / agents / commands 统一注册；命令也走 skills 路径（`SkillType::Command` 标签） |
| **Hook 引擎** | `src/extension/types/hooks.rs` | – | **24 种 HookEvent**（超过 CC！）：`BeforeAgentStart` / `BeforeToolCall` / `AfterToolCall` / `AfterToolCallFailure` / `ToolResultPersist` / `MessageReceived` / `MessageSending` / `MessageSent` / `SessionStart` / `SessionEnd` / `BeforeCompaction` / `AfterCompaction` / `PreApiRequest` / `PostApiRequest` / `GatewayStart` / `GatewayStop` / `Notification` / `PermissionRequest` / `PermissionDenied` / `UserPromptSubmit` / `SubagentStart` / `SubagentStop` / `Stop`，`HookExecutor` 执行，matcher 区分 `ToolName` / `SessionSource` / `Ignored` |
| **Plugin trust 策略** | `src/extension/plugin_trust.rs` | – | owner trust 验证，参考 openclaw |
| **MCP plugin 运行时** | `src/extension/mcp_plugin_tests.rs` + `McpManagerConfig` | – | JSON-RPC 2.0 over stdio，正是当前 `aleph-official` 7 个 plugin 用的 |
| **Auto-discover** | `src/extension/manifest/mod.rs:280` | – | 5 个组件目录已被识别：`skills/` + `SKILL.md` / `agents/*.md` / `commands/*.md` / `hooks/hooks.json` / `.mcp.json` / `.lsp.json`（LSP 是空 stub） |
| **Manifest 缓存** | `src/extension/manifest/manifest_cache.rs` | 350 | 性能优化 + 信任旁路 |
| **集成测试** | `tests/plugin_lifecycle_roundtrip.rs` / `tests/plugin_visibility.rs` | – | 端到端覆盖 |

### 3.2 当前 Aleph plugin 的实际形态（看 `~/.aleph/plugins/cache/aleph-official/`）

每个 plugin 是：
- `package.json`（npm 风格，`main: src/index.js`，`@aleph/<name>` scope，`private: true`）
- `.claude-plugin/plugin.toml`（TOML 而非 JSON，camelCase 不一致）
- `.claude-plugin/marketplace.toml`（marketplace 根目录）
- `src/index.js`（Node.js 实现，JSON-RPC 2.0 over stdio，遵循 `plugin.call` / `ping` 协议）
- `README.md`

TOML manifest 例子（`phone-control/.claude-plugin/plugin.toml`）：
```toml
name = "phone-control"
version = "0.1.0"
description = "Android phone control via ADB..."

[aleph]
runtime = "mcp"

[aleph.permissions]
shell = true
filesystem = "read"
```

`[aleph.services]` 可以声明 start/stop handler（如 `metrics-collector`），`[aleph.config_schema]` 可以声明用户配置 schema——比 CC 的 `userConfig` 还更结构化。

### 3.3 缺什么（按优先级排）

| # | 缺口 | 重要度 | CC 位置 | 当前 Aleph 状态 |
|---|---|---|---|---|
| **1** | **Mod 子系统（in-process JS hooks module）** | 🔴 核心 | `hooks/register.js` + `hooks/hooks.json` 引用 | ❌ 无 JS runtime；hooks 全是 shell/HTTP/command 类型 |
| 2 | **LSP servers 解析器** | 🟡 高 | `.lsp.json` | ❌ adapter 列表里写 `(.".lsp.json", None)` 占位 |
| 3 | **Monitors 解析器** | 🟡 中 | `monitors/monitors.json` | ❌ 未实现 |
| 4 | **Output styles** | 🟡 中 | `output-styles/*.md` | ❌ 未实现 |
| 5 | **Workflows** | 🟢 低 | `workflows/*.js` | ❌ 未实现 |
| 6 | **bin/ on PATH** | 🟢 低 | `bin/*` | ❌ 未实现 |
| 7 | **`per-plugin settings.json`** | 🟢 低 | 临时覆盖 agent 配置 | ❌ 未实现 |
| 8 | **Attribution 字段**（turn 上记 `attributionPlugin/Skill/Agent/McpServer/McpTool`） | 🟡 中 | assistant turn JSON | ❌ 没有 |
| 9 | **Mod `drawPane` UI API** | 🔴 核心 | in-process JS API | ❌ 整套缺；Aleph 是 Rust + WASM 渲染，不是 TUI+in-process JS |
| 10 | **`addCommand` programmatic** | 🟡 中 | 模组注册 slash command | 部分有（plugin 注册 SkillType::Command）但缺 programmatic API |
| 11 | **Mod 测试运行器（无 session 跑）** | 🟢 低 | `/docs/en/plugins/mods/test` | ❌ 无 |
| 12 | **`installed_plugins.json` 类似机制** | 🟡 中 | 第三方 CC plugin 列表 | 部分有（`mcp_config.json` / `hub_catalog.db`）但不是按 plugin id 维度 |
| 13 | **WASM plugin 加载器实现** | 🟡 中 | –（CC 用 JS） | `PluginKind::Wasm` 已枚举但未见 Wasmtime/wasmer 依赖导入确认；`grep "v8\|deno\|boa"` 在 Cargo.toml 里全无 |

### 3.4 Aleph 的特殊优势（CC 反而没这些）

| 能力 | 来源 | 含义 |
|---|---|---|
| **WASM plugin kind** | `PluginKind::Wasm` | 一旦实现 mod，可以走 WASM 沙箱，比 CC 的 in-process JS 更安全 |
| **services 长生命周期** | `[aleph.services]` | plugin 可以有后台服务（`metrics-collector`），CC 没有等价物 |
| **config_schema 完整结构** | `[aleph.config_schema.<key>]` | 比 CC 的 `userConfig` 支持更细的类型（`array`、`object` 等） |
| **24 种 HookEvent** | 超出 CC 的覆盖 | 已经在 `mod.rs:230` 的 `ALL` 数组固化，比 CC 现有事件更多 |
| **`allowManagedModsOnly` 等价物** | `plugin_trust.rs` 已有 owner trust | 组织策略层比 CC 早 |
| **Codex / Cursor 兼容** | `adapters/{codex,cursor}.rs` | 不止 CC，还能装 Cursor / Codex plugin |

---

## 4. 移植方案

### 4.1 设计原则

> 任务不是"复刻 CC"，而是"让 CC 的第三方 plugin 能直接装到 Aleph 上跑，并且 Aleph 自己有比 CC 更强的 mod 能力"。

具体三条原则：
1. **读 CC 原生格式，不发明新格式**。CC 第三方 plugin 的 `plugin.json` / `commands/*.md` / `agents/*.md` / `skills/<name>/SKILL.md` / `hooks/hooks.json` / `.mcp.json` 全部直接认。**禁止要求作者把 manifest 改成 Aleph TOML**。
2. **Aleph 能力放在 `[aleph]` superset 段**。这个约定已经存在于 `cc_plugin_json.rs:31`："The `[aleph]` section, if present, enables Aleph-specific features while remaining invisible to Claude Code." 沿用。
3. **Mod 选 WASM 而非 in-process JS**。原因：(a) Aleph 已有 `PluginKind::Wasm` 占位；(b) CC 的 in-process JS 实际上是 Bun's JS engine（从 `/$bunfs/root/...` 路径看出来），不是 V8；(c) WASM 沙箱更严格，符合 Aleph 的 trust policy。JS 作者可以走 `js2wasm`（quickjs → wasm 工具链），对作者体验损失很小。

### 4.2 架构总图

```
                    Aleph host (Rust)
                            │
                            ▼
            ┌──────────────────────────────┐
            │   PluginRegistry (已有)      │   统一注册 tools / hooks /
            └──────────────┬───────────────┘   services / skills / agents
                           │
       ┌──────────┬────────┴────────┬─────────────┐
       ▼          ▼        ▼        ▼             ▼
   ManifestAdapters  HookExecutor  Services  Mod Runtime (新)  PermissionGate
       │          │                       │
       │          │                       │
   ┌───┴───┐ ┌────┴─────┐                 │
   │ aleph │ │ cc_json  │                 │   ┌──────────────────────┐
   │ _toml │ │ cc_toml  │                 │   │  PluginKind::Wasm    │
   │ codex │ │ cursor   │                 │   │  ┌──────────────┐    │
   │ auto  │ └──────────┘                 │   │  │ wasmtime host│    │
   └───────┘                              │   │  │ + JS-in-WASM │    │
                                          │   │  │   (quickjs)  │    │
                                          │   │  └──────────────┘    │
                                          │   └──────────────────────┘
                                          │
                                  PluginTrustPolicy (已有)
```

### 4.3 分阶段实施

#### 阶段 0：兼容性基线（已有，只需要测试加固）
> 工期 0 人天——全部已实现

- [x] `.claude-plugin/plugin.json` 解析（`cc_plugin_json.rs`）
- [x] `.claude-plugin/plugin.toml` 解析（`cc_plugin_toml.rs`）
- [x] `[aleph]` superset
- [x] `commands/*.md` / `agents/*.md` / `skills/<name>/SKILL.md` auto-discover
- [x] `.mcp.json` MCP servers
- [x] `hooks/hooks.json` settings-hook（shell / HTTP）
- [x] 24 种 HookEvent 覆盖

**这一阶段只缺：把这套兼容性跑过实际 CC 第三方 plugin 的回归测试套件**。建议新增 `tests/fixtures/cc-plugins/<name>/` 目录，复制 5-10 个真实 CC marketplace 第三方 plugin 做端到端测试。`scan-cc-plugin-format.md §5` 提到的 "fail loud" 教训要继续守。

#### 阶段 1：补全 6 个静态组件解析器
> 工期 ~1-2 周（单个 PR + 测试）

每个组件都是 `ComponentSource` → 解析 → `CapabilityDeclaration` 的标准套路，沿用 `mcp_servers` 已有的实现：

1. **LSP servers**（`.lsp.json`）—— 新增 `lsp_servers.rs` 解析器；在 `auto_discover/mod.rs:282` 把 `(.".lsp.json", None)` 改成 `Some(解析器)`
2. **Monitors**（`monitors/monitors.json`）—— `monitors` 数组，注册为 `CapabilityDeclaration::Monitor`
3. **Output styles**（`output-styles/*.md`）—— 解析 frontmatter，注册到 `output_style` 槽
4. **Workflows**（`workflows/*.js`）—— 静态解析（看 import / export），动态执行留给阶段 3
5. **bin/ on PATH** —— 加载时把 plugin 的 `bin/` 临时加到受限 `PATH`，受 `shell = true` permission gate
6. **per-plugin settings.json** —— 在 `PluginRecord` 加 `settings_overrides: serde_json::Value`，加载时合并到 `AgentConfig`

每个都要：
- 写 `parse_xxx_content` 函数（参考 `parse_hooks_content`）
- 写 capability declaration
- 加单元测试 + 端到端 fixture 测试
- 更新 `PluginKind::ALL` 和 capability 总数统计

**验收**：拿 5 个真实 CC plugin（含官方 `commit-commands`、`security-guidelines` 等）跑 fixture 测试，全部 `PluginStatus::Loaded` 0 错误。

#### 阶段 2：Mod 运行时（WASM 沙箱 + JS 互操作）
> 工期 ~6-8 周（核心 PR）

这是最重的一步。设计如下：

**2.1 WASM 运行时选型**
- 加 `wasmtime = "x"` 依赖（CC mod 不需要，但 Aleph 选 WASM 是更稳的）
- 加 `quickjs-wasm-rs` 或 `rquickjs` + quickjs → wasm 编译链；或更简单：让作者交 WASM，文档说明如何用 `wasm-pack` 编译 JS
- **决策点**：要不要原生支持 `.js` 文件？建议**先不支持**——给作者一个 `js2wasm.sh` 工具，但宿主只吃 `.wasm`。理由：(a) V8 集成是 100MB+ 依赖；(b) CC 的 Bun 内嵌也是 100MB+；(c) `wasmtime` + `quickjs` 的 wasm 镜像总共 < 5MB

**2.2 Mod manifest 检测**
- 改 `cc_plugin_json.rs:139`：在 `[aleph.runtime]` 加 `"mod"` 这个新 kind
- 改 `PluginKind` 枚举：加 `Mod` 变体（或复用 `Wasm`，因为 mod 实质就是 wasm 进程内扩展）
- 一个 plugin 是 mod 的判定：目录下存在 `hooks/register.js` **或** manifest 里 `hooks` 指向一个 hooks module 类型

**2.3 Mod API surface**

按 `docs/en/plugins/mods/api` 的 API 列表，移植到 WASM import 命名空间：

| CC Mod API | Aleph WASM import | 实现位置（建议） |
|---|---|---|
| `drawPane(...)` | `aleph_ui::draw_pane` | `src/extension/mod_runtime/ui.rs` |
| `addCommand(...)` | `aleph_commands::add` | `src/extension/mod_runtime/commands.rs` |
| `createTimer(...)` | `aleph_timers::create` | `src/extension/mod_runtime/timers.rs` |
| `callTool(name, args)` | `aleph_tools::call` | `src/extension/mod_runtime/tools.rs` |
| `callModel(...)` | `aleph_model::call` | `src/extension/mod_runtime/model.rs` |
| `readFile / writeFile` | `aleph_fs::{read,write}` | `src/extension/mod_runtime/fs.rs` |
| 事件回调 | `aleph_events::{on_before_tool_call,on_after_tool_call,...}` | `src/extension/mod_runtime/events.rs` |
| 测试模式 | `aleph_test::run` | `src/extension/mod_runtime/test.rs` |

每个 import 必须：
- 受 `plugin_trust.rs` 的 owner policy 限制
- 走 `PermissionGate`（已经有的 `permissions` 字段：`shell` / `filesystem` / `network`）
- 加 timeout 防止死循环
- 加 `attribution` 标注（这正是 CC 的 `attributionPlugin` 字段的来源）

**2.4 Mod 加载器**
- 新增 `src/extension/mod_runtime/loader.rs`
- 流程：manifest 解析 → trust check → wasm 验证 → 沙箱实例化 → 注册 imports → 调用 `register()` entry point → 注册事件回调
- 失败回退：任何一步失败，plugin 降级为 `PluginStatus::Error`，但其他 component（skills / commands / MCP）照常加载（CC 同理：`disableAllHooks` 是局部降级）

**2.5 重写 `hooks/hooks.json` 解读**
- 当前 Aleph 的 `hooks/hooks.json` 是 settings-hook 风格
- CC mod 的 `hooks/hooks.json` 是声明式引用：`{ "hooks": [{ "event": "PreToolUse", "module": "./register.js" }] }`——是 mod 自身的注册清单，不是 shell command 列表
- **必须分开类型**：在 `component_source.rs` 解析时识别"指向 register.js"vs"指向 shell command"，分流到 settings-hook 引擎或 mod 引擎

**2.6 attribution**
- 在 `tools/runtime.rs` 的 assistant turn 序列化路径上，加 5 个可选字段：`attribution_plugin` / `attribution_skill` / `attribution_agent` / `attribution_mcp_server` / `attribution_mcp_tool`
- Mod 调 `callTool` 时填 `attribution_plugin`；MCP tool 调用时填 `attribution_mcp_server`；等等
- 跟 CC 完全对齐

**2.7 `drawPane` UI**
- 这是最难的一块——CC 的 pane 是 TUI 内嵌，Aleph 是 Web 渲染
- 短期方案：mod 用 `aleph_ui::draw_pane` 注册一个 JSON 结构（title + body + buttons），Aleph WebUI 渲染成 modal
- 中期方案：自定义 React 组件
- **决策点**：是否要做 mod 的 webview 沙箱（iframe + postMessage）？建议**不做**——让 mod 输出结构化 JSON，由 Aleph 自家 UI 渲染，安全 + 跨平台

**验收**：
- 把 CC 官方 `cc-plugin-diff`（一个真实的 mod）源码用 `js2wasm.sh` 编译到 `.wasm`，能装到 Aleph 上跑
- 触发 `/diff` slash command 调出 Aleph 的 diff pane
- 写一个全新 mod：监听 `BeforeToolCall`，数 tool 调用次数，在 TUI 顶部显示 "tool calls: N"——必须能跑

#### 阶段 3：registry / packaging / 安全
> 工期 ~3-4 周

1. **installed_plugins.json 等价物**——`hub_catalog.db` 已有；新增 `installed_plugins_v1.json` 记录每个第三方 plugin 的来源 marketplace + commit SHA + install time
2. **`/plugin` TUI 界面**——CC 的 `/plugin` 命令有 4 个 tab：Discover / Installed / Marketplaces / Settings。复用 Aleph 现有 TUI 框架加这几个 tab
3. **`/plugin validate`** —— 复用 `cc_plugin_json.rs` 现有解析逻辑做离线校验
4. **`/plugin marketplace add <git-url>`**——从 git clone marketplace，记录到 `marketplaces.json`
5. **`/plugin install <name>@<marketplace>`**——按 marketplace manifest 拉取并 install
6. **`/plugin tag`** / **`/plugin publish`**——发布到自己 marketplace（git tag + force 语义跟 CC 对齐）
7. **Managed policy** —— 跟 CC 的 `allowManagedModsOnly` / `disableAllHooks` 对齐：在 `plugin_trust.rs` 基础上加企业级管理接口
8. **mod 测试运行器**——`aleph mod test <path>`，无 session 跑 mod 的单元测试（参考 `/docs/en/plugins/mods/test`）

#### 阶段 4：生态与文档
> 工期 ~2-3 周

1. `js2wasm.sh` 工具脚本——给 JS 作者一个零成本迁移路径：`./js2wasm.sh hooks/register.js` → `hooks/register.wasm`
2. `aleph plugin new <name>` 脚手架——生成符合 CC 规范的 plugin 骨架，让作者写完能同时在 CC 和 Aleph 跑
3. 写完整的 mod 移植指南 `docs/port-a-cc-mod.md`（本文件的精简版）
4. 迁移 7 个现有 Aleph 官方 plugin 中的 1-2 个到新 mod 风格，做示范

---

## 5. 兼容性矩阵

CC plugin 装到 Aleph 上的实际行为：

| CC 组件 | 装到 Aleph | 备注 |
|---|---|---|
| `plugin.json` manifest | ✅ 已认 | `[aleph]` 段可选启用 Aleph 特性 |
| `commands/*.md` | ✅ 已认 | 走 `SkillType::Command` 槽 |
| `agents/*.md` | ✅ 已认 | 注册到 `PluginRegistry::agents` |
| `skills/<name>/SKILL.md` | ✅ 已认 | 跟 Aleph `agents/*.md` 共存，**注意命名不冲突** |
| `hooks/hooks.json` (settings) | ✅ 已认 | 走 `HookExecutor` + shell/HTTP |
| `hooks/hooks.json` (mod) | 🟡 阶段 2 | 解析后转 mod loader |
| `.mcp.json` | ✅ 已认 | JSON-RPC over stdio |
| `.lsp.json` | 🟡 阶段 1.1 | adapter 占位，需要补解析器 |
| `output-styles/` | 🟡 阶段 1.3 | 需新增 |
| `workflows/` | 🟡 阶段 1.4 | 静态解析即可，动态执行阶段 2 |
| `monitors/` | 🟡 阶段 1.2 | 需新增 |
| `bin/` | 🟡 阶段 1.5 | 需新增 |
| `settings.json` per-plugin | 🟡 阶段 1.6 | 需新增 |
| `userConfig` | ✅ 已认 | 比 Aleph `[aleph.config_schema]` 字段少；做映射或扩 schema |
| `dependencies` | 🟡 阶段 3.1 | 现有 Aleph 没依赖图 |
| `experimental.themes` | ❌ 暂缓 | Aleph 已有独立 theme 系统，不强求互通 |
| **Mod（in-process JS hooks module）** | 🔴 **阶段 2 核心** | 整块缺，要做 WASM 沙箱 |
| `drawPane` UI | 🔴 **阶段 2.7** | 走 JSON 结构化输出，由 Aleph UI 渲染 |
| `addCommand` programmatic | 🟡 阶段 2.3 | 已有 `SkillType::Command`，加 programmatic import |
| Attribution 字段 | 🟡 阶段 2.6 | turn JSON 上加 5 个可选字段 |

---

## 6. 风险与未决问题

### 6.1 高风险

1. **WASM ↔ JS 互操作的学习成本**。CC mod 作者习惯写 JS，让他们写 Rust WASM 或 quickjs-wasm 是有摩擦的。**缓解**：阶段 4 投入做 `js2wasm.sh` + 写一份"如果你是 CC mod 作者"的迁移指南。
2. **`drawPane` UI 抽象**。CC mod pane 是 TUI 原生元素，Aleph 是 Web 渲染。**不强行 1:1 还原**——给 mod 一个 JSON 输出契约，Aleph 决定怎么渲染。**风险**：高级 mod（带交互的表单）会受限于 Aleph 支持的 JSON schema。
3. **WASM sandbox 性能**。CC mod 是进程内 JS，调用开销 ~µs。WASM 调用虽然也快，但跨边界有 100ns 量级开销。**风险**：高频事件（`BeforeToolCall` 每次都跑）会有可观开销。**缓解**：批量事件 + async import。

### 6.2 中风险

4. **CC 第三方 plugin 用到了 Aleph 没有的事件**（如 `PermissionRequest` 是 Aleph 已有，OK；但如果 CC 加了新事件要跟）。**缓解**：HookEvent 枚举加新变体是廉价操作（24→25→26 都不难），保持观察 CC changelog。
5. **Bun 路径 `/ $bunfs/root/...`**——CC mod 的 `register.js` 写 import 用的是 Bun 的虚拟文件系统。**不是 Aleph 问题**，但写迁移指南时要提醒。
6. **CC marketplace 的 `headersHelper` + `strict: false`**（line 220 of cc_plugin_json.rs 提到）——CC 有种内联 manifest 模式要求 `strict: false`。**Aleph 现有解析器是否处理？** 需查 `cc_plugin_json.rs:220` 周围的代码确认，必要时补单元测试。

### 6.3 低风险 / 已知

7. Aleph 自己已发布的 7 个 plugin 用的是**TOML 格式和 JSON-RPC over stdio**。**不要破坏它们的体验**。新 mod 格式和它们并行存在，由 `[aleph.runtime]` 区分。
8. 用户的 dotfiles 仓库（`~/.dotfiles`）跟踪了部分 Aleph 配置（`config.toml`、`mcp_config.json`），但 plugin 目录 `~/.aleph/plugins/` 走 `info/exclude` 排除。**没 dotfile 同步问题**。
9. 现有 Aleph 的 `PluginKind::Wasm` 实际加载器是否完成？`grep "wasmtime\|wasmer"` 看不到。**阶段 0 先做现状核查**，别假设。

---

## 7. 推荐下一步

按 ROI 排：

1. **本周**：阶段 1 的 LSP / monitors / output-styles 三个解析器（每个 1-2 天，加测试 1 天）。这是低垂果实，能让 ~30% 的 CC marketplace plugin 装上。
2. **下周**：写 `tests/fixtures/cc-plugins/` 回归套件，复制 5-10 个真实 CC 第三方 plugin 跑端到端，**找出当前 `cc_plugin_json.rs` / `cc_plugin_toml.rs` 实际漏掉的边界**。
3. **第三周起**：阶段 2 的 mod 运行时——**先做 PoC**：选 CC 官方 `cc-plugin-diff`（一个真实内置 mod，源码公开），用 `quickjs-wasm-rs` 编译到 WASM 跑通"调出 diff pane"这个最小路径。PoC 通过再排全量。
4. **第八周后**：阶段 3 packaging + 阶段 4 生态。

---

## 8. 关键文件索引（按出现顺序）

| 章节 | 文件 |
|---|---|
| 2.1 | Claude Code 2.1.296 二进制 `strings` 输出（`/Users/zouguojun/.local/share/claude/versions/2.1.296`） |
| 2.2-2.5 | `https://docs.claude.com/en/docs/claude-code/plugins.md` 和子页 mods/overview、manifest-reference、components、api、events、interface、create、test |
| 3.1 | `/Volumes/TBU4/Workspace/Aleph/src/extension/manifest/cc_plugin_json.rs` (795 行)<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/manifest/cc_plugin_toml.rs` (857 行)<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/manifest/adapter.rs` (628 行)<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/manifest/component_source.rs` (226 行)<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/types/hooks.rs`<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/types/plugins.rs` (PluginKind)<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/registry/plugin_registry/mod.rs`<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/plugin_trust.rs`<br>`/Volumes/TBU4/Workspace/Aleph/src/extension/manifest/mod.rs:280` (auto-discover) |
| 3.2 | `/Users/zouguojun/.aleph/plugins/cache/aleph-official/{diagnostics,llm-task,phone-control,media-office,voice-call, ...}/`<br>`/Volumes/TBU4/Workspace/Aleph-plugins/` |

---

## 9. 一句话总结

Aleph 缺的不是 mod 系统的"想法"，是 **WASM mod runtime 这一块骨头**。其他 80% 的基础设施（manifest 解析、registry、hook 引擎、trust policy、Codex/Cursor 兼容）已经在了。阶段 1 拿两周收 6 个解析器，阶段 2 拿六到八周啃 mod 运行时 + drawPane，阶段 3 拿三到四周补 packaging。**总共 3-4 个月可以让 Aleph 装上 90% 的 CC marketplace plugin，并拥有 CC 没有的 WASM 沙箱 + services + 24-event hook 优势。**
