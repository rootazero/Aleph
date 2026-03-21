# 层级式斜杠命令设计

> 日期: 2026-03-21
> 状态: Draft

## 背景

当前斜杠命令使用扁平命名（`/session_new`、`/sessions_list`、`/agent_create`），存在以下问题：

- 命名不一致：单数/复数混用（`session_new` vs `sessions_list`）
- 与 CLI 命名不对齐：CLI 用 `aleph session new`，斜杠用 `/session_new`
- 命令多了难以发现：107 个命令平铺，用户需要记忆每个命令全名
- 无引导能力：用户不知道有哪些子命令可用

## 目标

将下划线连接的扁平命令改为空格分隔的层级命令（`/session new`），支持两种输入模式：
1. **快捷模式**：`/session new my-topic` — 一步到位
2. **引导模式**：`/session` 回车 → 显示子命令列表 → 选择 → 输入参数

三端（TUI/Bot/WebChat）用户体验一致，底层适配各端能力差异。

## 设计

### 1. 命令数据模型

#### 命令树结构

```
/session                    ← namespace（无动作，触发引导）
  ├── new [topic]           ← action（可选参数）
  ├── list                  ← action（无参数）
  ├── delete <id>           ← action（必填参数）
  ├── rename <topic>        ← action（必填参数）
  └── send <session> <msg>  ← action（必填参数）

/agent                      ← namespace
  ├── create <name>
  ├── list
  └── delete <name>

/plugin                     ← namespace
  ├── list
  ├── install <name>
  ├── uninstall <name>
  └── marketplace           ← 二级 namespace（三级）
      ├── search <query>
      ├── install <name>
      └── list

/skill                      ← namespace
  ├── list
  └── read <name>

/memory                     ← namespace
  └── browse

/image                      ← namespace
  └── generate <prompt>

/speech                     ← namespace
  └── generate <text>

/cron                       ← namespace
  └── manage

/vault                      ← namespace
  └── store

/search <query>             ← 独立一级 action
/webfetch <url>             ← 独立一级 action
/switch <agent>             ← 独立一级 action
/groupchat                  ← 独立一级 action
/snapshot                   ← 独立一级 action
```

最多支持三级层级（如 `/plugin marketplace install`）。

#### 命名规则

- Namespace 统一使用**单数**（`/session`，不是 `/sessions`）
- 独立命令无 namespace，直接作为一级命令
- 有逻辑分组的命令用 namespace，独立功能的直接一级
- Namespace 无默认动作 — 单独输入触发引导模式

#### 命令全名对应关系

| 内部 ID | 斜杠命令 | CLI 命令 |
|---------|---------|---------|
| `session.new` | `/session new` | `aleph session new` |
| `session.list` | `/session list` | `aleph session list` |
| `session.delete` | `/session delete` | `aleph session delete` |
| `session.rename` | `/session rename` | `aleph session rename` |
| `session.send` | `/session send` | `aleph session send` |
| `agent.create` | `/agent create` | `aleph agent create` |
| `agent.list` | `/agent list` | `aleph agent list` |
| `agent.delete` | `/agent delete` | `aleph agent delete` |
| `image.generate` | `/image generate` | `aleph image generate` |
| `speech.generate` | `/speech generate` | `aleph speech generate` |
| `skill.list` | `/skill list` | `aleph skill list` |
| `skill.read` | `/skill read` | `aleph skill read` |
| `memory.browse` | `/memory browse` | `aleph memory browse` |
| `cron.manage` | `/cron manage` | `aleph cron manage` |
| `vault.store` | `/vault store` | `aleph vault store` |
| `search` | `/search` | `aleph search` |
| `webfetch` | `/webfetch` | `aleph webfetch` |
| `switch` | `/switch` | `aleph switch` |
| `groupchat` | `/groupchat` | `aleph groupchat` |
| `snapshot` | `/snapshot` | `aleph snapshot` |

内部 ID 用点分隔（`session.new`），斜杠命令用空格分隔（`/session new`），CLI 用空格分隔（`aleph session new`）。

### 2. UnifiedTool 扩展

#### 与现有字段的关系

当前 `UnifiedTool` 已有 `subtools: Vec<String>` 和 `has_subtools: bool`，用于 MCP/Skill 的动态子工具发现（UI 展示用，不参与解析）。新字段的关系：

- `has_subtools` → 废弃，由 `is_namespace` 替代
- `subtools` → 废弃，由 `children` 替代
- 迁移时统一替换，不保留旧字段

#### Namespace 节点策略

Namespace 节点（如 `/session` 本身）**不作为 UnifiedTool 存入 ToolRegistry**。层级结构在查询时从工具名的点分隔推导：

- 注册工具 `session.new`、`session.list`、`session.delete` → 自动推导出 `session` namespace
- `commands.list` RPC 在返回时聚合为树形结构
- `command.execute` 解析时从已注册工具名前缀匹配 namespace

这样避免了 namespace 节点污染 LLM 的工具列表（LLM 只看到可执行的工具，不看到 namespace 占位符）。

#### 字段变更

```rust
pub struct UnifiedTool {
    // 现有字段保留
    pub id: String,             // "builtin:session.new"
    pub name: String,           // "session.new"（点分隔全名）或 "search"（独立命令）
    pub description: String,
    pub source: ToolSource,

    // 新增字段
    pub param_hint: Option<String>, // 参数提示，如 "[topic]"、"<name>"

    // 废弃字段（由层级推导替代）
    // pub subtools: Vec<String>,   // 移除
    // pub has_subtools: bool,      // 移除
}
```

`namespace`、`is_namespace`、`children` 不是 `UnifiedTool` 字段，而是在 `commands.list` 响应中**按需计算**的视图层数据。

#### 参数传递

命令解析后，`args` 作为原始字符串传递给工具执行层。工具内部负责解析参数结构（现有行为不变）。对于 LLM 发起的工具调用，仍走 JSON 结构化参数（`parameters_schema`），不受斜杠命令改造影响。

### 3. 命令解析

#### 完整输入解析

```
输入: "/session new my-topic"
  1. 去掉 "/" → "session new my-topic"
  2. 取第一个词 "session" → 查找匹配
     - 精确匹配独立命令？否
     - 匹配 namespace？是
  3. 取第二个词 "new" → 查找 session 的 children
     - 匹配子命令？是 → session.new
  4. 剩余文本 "my-topic" → 作为参数
  5. 返回: { namespace: "session", action: "new", args: "my-topic", resolved: true }
```

#### Namespace 引导触发

```
输入: "/session"
  1. "session" 是 namespace，无后续词
  2. 返回: { namespace: "session", action: null, resolved: false,
             needs_interaction: true, children: [...] }
  → 客户端收到 needs_interaction → 展示子命令列表
```

#### 独立命令解析

```
输入: "/search weather"
  1. "search" → 精确匹配独立命令
  2. 剩余 "weather" → 参数
  3. 返回: { namespace: null, action: "search", args: "weather", resolved: true }
```

#### 三级命令解析

```
输入: "/plugin marketplace install my-plugin"
  1. "plugin" → namespace
  2. "marketplace" → plugin 的子命令 → 也是 namespace
  3. "install" → marketplace 的子命令 → action
  4. "my-plugin" → 参数
  5. 返回: { namespace: "plugin.marketplace", action: "install", args: "my-plugin" }
```

#### 二级 namespace 引导

```
输入: "/plugin marketplace"
  1. "plugin" → namespace
  2. "marketplace" → plugin 的子命令 → 也是 namespace，无后续词
  3. 返回: { needs_interaction: true, namespace: "plugin.marketplace", children: [...] }
```

#### 错误处理

所有解析失败统一返回 `resolved: false` + `error` 消息：

```
输入: "/sesion new"  （namespace 拼写错误）
  1. "sesion" → 不匹配任何 namespace 或独立命令
  2. 返回: { resolved: false, error: "Unknown command: /sesion new" }

输入: "/session nw"  （子命令拼写错误）
  1. "session" → namespace
  2. "nw" → 不匹配 session 的任何子命令
  3. 返回: { resolved: false, error: "Unknown subcommand: nw",
             needs_interaction: true, namespace: "session", children: [...] }
     → 客户端显示错误 + 子命令列表（帮用户找到正确的）

输入: "/nonexistent"  （完全无法识别）
  返回: { resolved: false, error: "Unknown command: /nonexistent" }
```

不做模糊匹配/拼写建议 — 引导模式本身就是发现机制（YAGNI）。

### 4. RPC 接口变化

#### `command.execute` 增强

请求不变：`{ "input": "session new my-topic" }`

响应 — 完整命令（直接可执行）：
```json
{
  "resolved": true,
  "command": {
    "namespace": "session",
    "action": "new",
    "args": "my-topic",
    "internal_id": "session.new",
    "source_type": "builtin"
  }
}
```

响应 — 只输入 namespace（需要引导）：
```json
{
  "resolved": false,
  "needs_interaction": true,
  "namespace": "session",
  "children": [
    { "name": "new", "hint": "Start new session", "param_hint": "[topic]" },
    { "name": "list", "hint": "List all sessions", "param_hint": null },
    { "name": "delete", "hint": "Delete a session", "param_hint": "<id>" },
    { "name": "rename", "hint": "Rename session topic", "param_hint": "<topic>" },
    { "name": "send", "hint": "Send to another session", "param_hint": "<session> <msg>" }
  ]
}
```

#### `commands.list` 增强

返回树形结构：
```json
[
  {
    "name": "session",
    "is_namespace": true,
    "hint": "Session management",
    "children": [
      { "name": "new", "hint": "Start new session", "param_hint": "[topic]" },
      { "name": "list", "hint": "List all sessions" },
      { "name": "delete", "hint": "Delete a session", "param_hint": "<id>" },
      { "name": "rename", "hint": "Rename topic", "param_hint": "<topic>" }
    ]
  },
  {
    "name": "search",
    "is_namespace": false,
    "hint": "Web search",
    "param_hint": "<query>"
  },
  {
    "name": "switch",
    "is_namespace": false,
    "hint": "Switch agent",
    "param_hint": "<agent>"
  }
]
```

### 5. 三端交互实现

#### 统一交互协议

三端的差异只在渲染层，Gateway 侧逻辑完全一致：

```
1. 客户端发送 command.execute { input: "session" }
2. Gateway 返回 { needs_interaction: true, children: [...] }
3. 客户端用各自方式展示子命令
4. 用户选择后，客户端组装完整命令发送
```

#### TUI（ratatui 终端）

**快捷模式**：直接输入 `/session new my-topic`，回车发送。

**引导模式**：
```
用户输入: /session [回车]
  → 命令面板弹出，显示子命令列表：
    ┌─────────────────────────────┐
    │ /session                    │
    ├─────────────────────────────┤
    │ ▸ new [topic]      新建会话 │
    │   list             列表    │
    │   delete <id>      删除    │
    │   rename <topic>   重命名  │
    │   send <sess> <msg> 发送   │
    └─────────────────────────────┘
  → 键盘上下选择 "new"，回车
  → 输入栏变为: /session new |（光标等待参数）
  → 输入 "my-topic"，回车发送
```

**命令面板补全**：输入 `/ses` 时自动过滤显示匹配的命令和 namespace。

#### Bot（Telegram）

**原生 / 菜单**：只注册顶级命令：
```
/session   - Session management
/agent     - Agent management
/plugin    - Plugin management
/search    - Web search
/switch    - Switch agent
/cron      - Scheduled tasks
```

**引导模式**（用户点击 `/session`）：
```
Bot 收到 "/session"
  → Gateway 返回 needs_interaction + children
  → Bot 发送 inline keyboard：
    Session management:
    [new] [list] [delete] [rename] [send]
  → 用户点击 [new]
  → Bot 回复："请输入话题名称（可选，直接发送 . 跳过）："
  → 用户输入 "my-topic"
  → Bot 组装并发送 "/session new my-topic"
```

**快捷模式**：用户直接输入 `/session new my-topic`，正常解析执行。

#### WebChat（Leptos WASM）

**快捷模式**：输入框输入 `/session new my-topic`，回车。

**引导模式**：
```
输入 "/session" 回车
  → 输入框上方弹出子命令选择面板（可点击按钮）
  → 点击 "new"
  → 输入框自动填充 "/session new "，光标就位
  → 输入参数，回车发送
```

### 6. 实施策略

**Step 1 — Gateway 层级解析**（核心改动）
- 扩展 `UnifiedTool`：添加 `namespace`、`is_namespace`、`children`、`param_hint`
- 重写命令注册：从扁平名改为层级注册
- 重写 `CommandParser`：支持空格分隔的层级解析
- 更新 `command.execute`：返回 `needs_interaction` + children
- 更新 `commands.list`：返回树形结构
- 内部 builtin tool 执行映射：`session.new` → 调用 `SessionNewTool`

**Step 2 — 三端交互适配**
- TUI：命令面板支持层级浏览和子命令选择
- Bot（Telegram）：namespace 触发 inline keyboard 交互
- WebChat：子命令选择面板

## 不做的事

- 不改 builtin tool 的 Rust 代码（`SessionNewTool` 等内部实现），只改注册名和解析层
- 不做重交互参数表单（轻交互：选完子命令后同行输入参数）
- 不做命令权限过滤（所有端看到相同命令树）
- MCP/Skill/Plugin 动态命令暂不层级化（它们已有自己的 namespace 机制如 `/mcp_server:tool`）

## 向后兼容

### 下划线别名（永久保留）

解析器同时支持下划线和空格分隔，作为永久别名而非过渡期：
- `/session_new` 和 `/session new` 解析为同一工具 `session.new`
- 实现方式：解析时将下划线替换为点号尝试匹配（`session_new` → `session.new`）
- 零额外存储成本，仅在解析器中增加一条 fallback 路径

### LLM 工具调用

LLM 发起的工具调用使用结构化 JSON（`tool_use` / `function_call`），不走斜杠命令解析。工具注册名从 `session_new` 改为 `session.new` 后：
- System prompt 中的工具列表同步更新（自动生成，无手动维护）
- LLM 输出 `session.new` 作为 tool name → 执行层直接匹配
- 旧格式 `session_new` 在执行层做 fallback 映射（下划线 → 点号）

### Telegram BotCommand

Telegram 的 `setMyCommands` 只接受 `[a-z0-9_]` 格式。注册时将层级命令转为下划线格式：
- `session.new` → 注册为 `/session_new` 到 Telegram
- 但只注册顶级 namespace（`/session`、`/agent`）到原生菜单
- 用户点击后通过 inline keyboard 选子命令

## 风险

| 风险 | 缓解措施 |
|------|---------|
| 旧客户端仍发送 `/session_new` | 解析器同时支持下划线和空格分隔（过渡期），下划线格式映射到对应层级命令 |
| Telegram / 菜单只能显示顶级命令 | 正是设计意图 — 顶级命令 + inline keyboard 引导 |
| 三级命令交互链太长 | 快捷模式一步到位；引导模式最多 3 步选择 |
| 命令重命名导致用户习惯断裂 | `/session_new` 等旧名在过渡期自动映射到 `/session new` |
