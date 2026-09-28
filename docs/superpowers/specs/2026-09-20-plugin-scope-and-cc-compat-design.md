# 插件宿主作用域化 + Claude Code 兼容补齐 + MCP server 面 — 设计 (2026-09-20)

> **Status**: 设计已经用户逐节批准（§1–§7 口头批准于 2026-09-20），待用户审阅本文件后进 writing-plans。
> **Branch**: `worktree-plugin-scope-round`（从 main `3ddc1f2e7` 分叉；`baseRef = head`）。
> **Evidence**: 四份只读扫描报告在 [`2026-09-20-plugin-scope-and-cc-compat-evidence/`](2026-09-20-plugin-scope-and-cc-compat-evidence/)：
> `scan-dsh-cordis.md`（DeepSeek Harness / Cordis 机制 + dsh 作为 MCP 宿主）· `scan-pi.md`（pi 扩展系统 + pi 作为 MCP 宿主 / CLI 调用方）· `scan-aleph-plugins.md`（Aleph 现状清单、生命周期、CC 兼容真伪、OpenClaw 足迹、既有裁定、断线清单）· `scan-cc-plugin-format.md`（Claude Code 插件格式真源 + 70 项兼容清单）。
> 本文引用的每个 `file:line` 都以 `3ddc1f2e7` 为准；数字带谓词（判据 §18）。

---

## 0. 用户裁定（本轮的边界，按时间顺序）

| # | 问题 | 裁定 |
|---|---|---|
| U1 | 「吸收 Cordis」做到哪一层 | **效果作用域 + 可撤销注册**。不动 harness 循环；不把「一切皆插件」推到内核 |
| U2 | 外部贴文（"Aleph 是 OSINT 枢纽 / gRPC+SSE 双通道 / pi 只靠 bash"） | **只作参考**。设计严格按探索出的真实代码；贴文的 OSINT 前提与 gRPC 手段均被否 |
| U3 | 三条主线 | **A 主线**：作用域化插件宿主 + CC 兼容补齐 + 砍 OpenClaw + 连线清死代码；**B 支线**：Aleph 暴露 MCP server 面（不是 gRPC）；**pi**：不在 Aleph 内跑 pi 的 TS 扩展，pi 经 MCP 面 / CLI 接入 |
| U4 | 空间可组合级别 | **项目级**，所有能力种类共用一个谓词；不做 session/agent 级子作用域 |
| U5 | CC `agents/*.md` 正文 | **接上**（推翻 FL §3.10 "刻意仍不做"） |
| U6 | `~/.claude/` 集成 | **只读发现 `~/.claude/plugins/` 已装插件**；不读 `settings.json`；启用态由 Aleph 自己的 `plugins.toml` 决定 |
| U7 | MCP 面暴露范围 | **配置驱动白名单，默认一组无副作用能力** |
| U8 | 核心机制方案 | **C：效果归 scope、视图归派生** |

---

## 1. 背景：为什么是现在、为什么是这个形状

### 1.1 三轮「不引 fiber」裁定与它的证伪

2026-08-15（dsh 对照）、08-16（插件系统深化）、08-19（生态兼容）三轮都裁定「对照 Cordis 但架构不移植」。代码侧的表述在 `src/extension/projection.rs:14-24`：

> "Aleph deliberately does **not** adopt a fiber runtime (R10) … the equivalent guarantee here is cheaper: **one function derives the whole set from the registry, and every path that can change plugin activation calls it.**"

守卫 `publishing_plugin_projections_has_exactly_one_author`（`projection.rs:162`）证明的是「改激活态的路径都调了那一个函数」。它**证不出那个函数盖住了所有面**——派生只盖 skill dirs / sub-agents / tool index 三个面，三个月里在派生之外漏了四处（`scan-aleph-plugins.md` §2.5、§2.7）：

| 漏点 | 位置 | 形状 |
|---|---|---|
| `[memory]` 扩展 disable 后仍挂着 | `memory/extensions/registry.rs:95-114` 只有 `register`/`register_mcp`，**没有 unregister** | 闸只有一个方向（判据 §14） |
| 插件 slash 命令只在 boot 注册一次 | `agent_init/tool_catalog_init.rs:216-247` 是 `register_skills` 的唯一调用点 | 两端完整中间没线（§7） |
| MCP 插件 disable→enable **不重挂** server | `plugin_ops.rs:586-641` disable 走 `unload_runtime_plugin`，enable 什么都不做；只有 `reload()`（`mod.rs:828`）调 `sync_mcp_plugin_servers` | 闸的两个方向不对称（§14） |
| `reload_plugin(id)` 是 `reload()` 的窄孪生 | `mod.rs:1303-1341` 只刷 tool index，跳过 hooks / projections / MCP / services | 孪生（§16） |

这是**列举法**（判据 §5）：派生函数列举了它认得的三个面，下一个新面还会漏。dsh 的做法（`scan-dsh-cordis.md` §1）不是运行时魔法，只是一条所有权规则：**每个进运行时的注册返回一个 disposer，由注册方持有，卸载即逆序执行**。这条规则可以在 Rust 里以显式签名表达（`register() -> Disposer`），不需要 Proxy 上下文、DI 容器、级联重启。

因此本轮**收窄**（不是推翻）那三轮裁定：采所有权规则；DI 容器 / Proxy 上下文 / 级联重启 / HMR / 依赖声明与版本闸**仍不采**。`projection.rs:14-24` 与 `HARNESS_PHILOSOPHY.md §8 第五课` 必须同笔改写，否则它们成为说谎的那一份（判据 §1）。

### 1.2 OpenClaw 已经基本不存在

grep `openclaw|clawhub|claw` 在 `src/` 命中 286 行 / 156 文件，但**只服务 OpenClaw 格式的代码 ≈ 50 行**：`src/tools/markdown_skill/spec.rs:47-124` 的 `OpenClawMetadata` / `OpenClawInstallSpec` 两个 serde DTO，**零读者**。其余是 parity 注释、`ClawTeam`、`clawshell`，以及把 OpenClaw 当 **ACP agent** 托管的预设（R3 定位，保留）。没有 OpenClaw 插件适配器，没有 ClawHub 目录客户端，`ARCHITECTURE.md:261` 列的 `src/clawhub/` 是幽灵目录。「放弃支持 OpenClaw」是一个小 CUT + 文档改标签。

### 1.3 CC 兼容的真缺口在执行端

manifest / skills / marketplace / hooks 配置解析都是真的（`scan-aleph-plugins.md` §3）。缺的是：exit code 2 不算 block（CC 最常见的 hook 写法在 Aleph 静默放行）、`hookSpecificOutput.updatedInput` 不解析、`commands/*.md` 正文从不到达模型、agent 正文按裁定丢弃、`~/.claude/plugins/` 不扫描、23 事件 vs CC 现行 32 事件。

### 1.4 pi 与 dsh 都是 MCP 宿主，而 Aleph 没有 server 面

`src/mcp/`（19k 行）和 `src/acp/` 都是**客户端**。dsh 一行 `cordis.yml` 就能挂一个 `stdio | streamable-http` 的 MCP server（`scan-dsh-cordis.md` §7）；pi 核心没有 MCP 客户端，靠 `pi-mcp-adapter`（用户本机已装）挂 http server（`scan-pi.md` §9）。给 Aleph 加一个 Streamable HTTP 的 MCP server 面，dsh / pi / Claude Code 都能零专属代码挂载——贴文追求的「绝对纯粹性」用一个标准协议买到，不需要 gRPC/Tonic + 第二个事件平面。

---

## 2. 差距摘要（全表见 evidence；此处只留决定动作的行）

| 维度 | Aleph 现状 | 动作 |
|---|---|---|
| 注册可撤销 | 派生盖 3 面，4 处漏 | `EffectScope` + `Disposer`（§3.1） |
| 可见性作用域 | 发现 = 所有项目并集；只有 hooks 有 `project_scope_allows`（`executor.rs:362`，调用点 `:918`） | `ScopeKey` + `visible_to` 五张脸共用（§3.4） |
| 依赖满足即激活 | registry 记每个插件结局，无 boot 断言 | `Pending{waiting_on}` + activation gate（§3.5） |
| Hook 协议 | 23 事件全接线；exit-2 / `updatedInput` / `transcript_path` / `permission_mode` 缺 | §3.6 #1–#6 |
| commands | 解析→boot 注册 slash；正文不到模型；`SkillTemplate` 零消费者 | §3.6 #7 |
| agents | 正文丢弃 | §3.6 #8 |
| marketplace | 只认文档的 `source.type`，磁盘真形状是 `source.source` | §3.6 #9 |
| `~/.claude/plugins/` | 不扫描 | §3.6 #10 |
| MCP server 面 | 无 | §3.7 |
| Node/JS 运行时 | 文档写有、目录不存在；`packages/plugin-sdk` 零消费者 | CUT（§3.9） |
| 零客户端 RPC | 62 注册 / 20 零客户端 | CUT（§3.9） |

---

## 3. 设计

### 3.1 时间可组合：`EffectScope` 与 `Disposer`

**位置**：新模块 `src/extension/effects/{mod.rs, scope.rs, disposer.rs}`。

```rust
/// One reversible side effect a plugin made on the running process.
/// Dispose is async because MCP server removal and service stop are.
pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// Everything one mounted plugin put into the runtime, in registration order.
pub struct EffectScope {
    plugin_id: PluginId,
    disposers: Vec<(&'static str /* step label */, Disposer)>,
}

impl EffectScope {
    pub fn effect(&mut self, step: &'static str, d: Disposer);
    /// Runs every disposer in reverse order. A failing disposer is logged
    /// with its step label and does NOT stop the rest (P7). Consumes self:
    /// a scope cannot be half-disposed.
    pub async fn dispose(self) -> DisposeReport;
}
```

**C 方案的那一句规则**：

- **效果**＝有逆操作的运行时资源。每个产生效果的注册函数返回 `#[must_use] Disposer`，调用方（lifecycle）把它放进该插件的 `EffectScope`。本轮纳入的效果种类：
  1. `PluginRegistry` 行（`CapabilityApi::register_capability` ↔ unregister）
  2. WASM 模块（`runtime/wasm/loader.rs` load ↔ unload）
  3. MCP transient server（`registrar/mcp_registrar.rs` → `McpManagerHandle::add_transient_server` ↔ remove）
  4. 插件 service（`service_manager.rs` start ↔ stop）
  5. memory extension（`memory/extensions/registry.rs` register ↔ **新增** `unregister(plugin_id)`）
  6. ToolCatalog 里的 slash 条目（`register_skills` ↔ **新增** unregister）
- **视图**＝可从 registry 重算的东西：tool index 快照（`mod.rs:1210-1249`）、`PLUGIN_SKILL_DIRS`、`PLUGIN_SUBAGENTS`（`projection.rs:118`）、`HookExecutor`（`sync_hooks_from_registry` `mod.rs:1128`，今天就是整个重建）。保留单函数派生；**触发点收敛为 `lifecycle.rs` 里的一处**。
- 分辨法：**它有逆操作吗？** 有 → 效果；没有但能从 registry 重算 → 视图。两者都不是 → 它不该由插件写入运行时。

`Disposer` 的执行顺序＝注册的逆序；registry 行是第一个注册、最后一个撤销的效果，所以视图重算时它已不在。

### 3.2 生命周期四原语

**位置**：新 `src/extension/lifecycle.rs`（从 1,884 行的 `src/extension/mod.rs` 抽出；P2 单文件 <800 行）。

| 原语 | 语义 |
|---|---|
| `mount(id)` | 解析 manifest → owner-trust / enabled 门（现有 `plugin_trust.rs` / `plugin_state.rs`）→ 新建 `EffectScope` → 逐个注册效果 → 任一步失败即 `dispose` 已注册部分（**全有或全无**，同 dsh 两阶段 `scan-dsh-cordis.md` §7.4）→ 写终态 |
| `unmount(id)` | 从 `ExtensionManager.scopes: HashMap<PluginId, EffectScope>` 取出 → `dispose` → 写终态（`Disabled` / 移除） |
| `reload_plugin(id)` | `unmount` + `mount`。今天的窄孪生（`mod.rs:1303`）删除 |
| `reload()` | 对每个已发现插件 `unmount` + `mount`；`stop_orphaned_services` 变成 dispose 的自然结果 |

`set_plugin_enabled(true/false)`（`plugin_ops.rs:586`）直接映射到 `mount` / `unmount`；watcher（`watcher.rs`）与 `plugin.reload` / `hooks.reload` RPC 同样只调这四个原语。

**每次迁移之后，且只在这里**：`republish_plugin_projections()` + `sync_hooks_from_registry()`（视图）+ 向 MCP 面广播 `notifications/tools/list_changed`（§3.7）。

### 3.3 终态、`Pending` 与 activation gate（dsh #2）

`PluginStatus`（`types/plugins.rs:181`）：
- 新增 `Pending { waiting_on: Vec<String> }`——插件已 mount 但某个效果尚未到达终态（例：MCP transient server 未完成 `initialize`；运行时未 provision）。`waiting_on` 从声明的依赖派生，不是布尔。
- `Overridden` **删除**（零生产者，`mod.rs:556-580` 只给赢家记 `shadowed` 诊断）；`PLUGIN_SYSTEM.md:149-160` 的假声明同笔改。
- `Pending` **不因超时变 `Failed`**（判据 §8「还没准备好」≠「失败了」）。

**activation gate**（新 `src/extension/activation_gate.rs` + `src/diagnostics/checks/` 一项 `extension/plugins-activated`）：boot 的 `load_all` 结束后列出所有非终态插件及其 `waiting_on`。daemon 形态：日志一行 + doctor 项；QA / 测试 profile：fatal。这是判据 §7「两端完整中间没线」的直接探测器。

### 3.4 空间可组合：`ScopeKey` 与 `visible_to`

**位置**：现有 `src/extension/scope.rs`（135 行，今天只服务 hooks 的 `project_scope_allows`）改名 `visibility.rs`，升为共用谓词。

```rust
pub enum ScopeKey { Global, Project(CanonicalRoot) }
pub struct VisibilityCtx { pub project_root: Option<CanonicalRoot> }
pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool
```

- 每条 registry 行带 `ScopeKey`，从发现时的 scope（`discovery/scanner.rs` 的 `ClaudeGlobal` / `AlephGlobal` / `Project` / Bundled / marketplace / `ClaudeCache`）派生：`Project(root)` 只来自 `<project>/.claude/` 与 `<project>/.aleph/plugins{,.local}`，其余全是 `Global`。
- **五张脸**（判据 §9）都在请求构建时调 `visible_to`：① tool index（`gateway/execution_engine/tool_refresh.rs:27 active_plugin_tools_for_agent`）② skills 索引（`build_skills_prompt_xml`）③ agents 解析（`AgentRegistry::resolve` 对 `PLUGIN_SUBAGENTS`）④ slash 列表 ⑤ MCP tool bridge（按拥有该 server 的插件的 key 过滤；server 进程本身仍是全局的）。hooks 的 `project_scope_allows` 改为调同一谓词。
- `VisibilityCtx.project_root` 的推导**复用 hooks 今天那一份**（`executor.rs:918` 调用点上游），不新造第二个「当前项目」推导（判据 §12）。
- **行为变更**：无 project 的会话只见 `Global`（Cordis「根只见根」；fail-closed）。今天是全部可见。写进 FL 与 PLUGIN_SYSTEM.md。

### 3.5 守卫（每条都要做一次变异证伪；先看红名单再看条数）

| # | 守卫 | 变红条件 |
|---|---|---|
| G1 | census：`src/extension/registrar/`、`service_manager.rs`、`runtime/wasm/loader.rs`、`memory/extensions/` 里产生运行时副作用的 `pub fn` 返回 `Disposer`（源码级测试，同 `census` 家族） | 新增一个不返回 `Disposer` 的注册函数 |
| G2 | 往返：夹具插件覆盖 §3.1 全部六种效果，`mount` → 六面快照 → `unmount` → 快照 == mount 前 | 注释掉任一 disposer |
| G3 | 现有 `publishing_plugin_projections_has_exactly_one_author` 改为钉住 `lifecycle.rs` 里的那个调用点 | 在别处再调一次 `republish` |
| G4 | 事件别名 census：每个 CC 别名指向的 `HookEvent` 在 `src/extension/` 之外有生产者（扩现有 `HookEvent::` 普查） | 加一个无生产者的别名 |
| G5 | `[mcp_server].expose` ⊆ 工具目录（启动时校验 + 测试） | expose 里写一个不存在的工具名 |
| G6 | `PluginStatus` 终态 census：`activation_gate` 认得的终态集合从枚举派生，不是手写清单 | 加一个枚举变体不更新 gate |

### 3.6 Claude Code 兼容补齐（执行端）

原则：只补有生产者的东西（判据 §7），每项能指出接线的那一行。`scan-cc-plugin-format.md` 末尾的 **70 项清单**进计划作验收表，每项标 IMPLEMENTED / CONNECT / DEVIATION（见 §10）。

| # | 项 | 做法 | 落点 |
|---|---|---|---|
| 1 | **exit code 2 = block** | JSON 决策与 exit-code 决策在**同一个函数**里派生（判据 §12）：Interceptor 型事件 exit 2 → `blocked { reason: stderr }` 喂回模型；其他非零 → 非阻塞警告；Observer 型事件只记日志 | `hooks/executor.rs:697-713`、`:941-952` |
| 2 | `hookSpecificOutput.updatedInput` | 与 `update_input:` 行前缀走同一条路 | `hooks/json_output.rs`、`hooks/mod.rs:399-401` |
| 3 | stdin payload 补 `transcript_path`、`permission_mode`（映射自会话执行档） | **P0 真机抓包**定 `tool_result` / `tool_response` 名字：本机装一个把 stdin dump 到文件的 PostToolUse hook，跑一次 Claude Code。不猜 | `hooks/executor.rs:100-150` |
| 4 | 32 事件对齐 | 逐个三分：Aleph 有同名时刻 → alias；Aleph 有时刻无事件 → 加生产者；Aleph 无此时刻（如 worktree 创建）→ 文档写「无此时刻」。**不新增零生产者事件**（G4） | `types/hooks.rs:39-121` |
| 5 | matcher 里的 CC 工具名（`Bash` / `Edit` / `Write` / `mcp__srv__tool`） | 一张 CC→Aleph 名字别名表，单一推导，只在 matcher 匹配处使用 | hooks matcher |
| 6 | 默认超时 CC 600s vs Aleph 上限 300s | **保留 300s**（hook 在工具派发内运行，本就受 tool budget 约束）→ DEVIATION | `MAX_HOOK_TIMEOUT_SECS` |
| 7 | **`commands/*.md` 正文注入** | `/cmd args` → `SkillRegistration{Command}` → 读正文 → `SkillTemplate`（CONNECT 零消费者的 `template.rs`）展开 `$ARGUMENTS` / `$1..$N` / `${N:-d}`、`@file`（经沙箱读）、`` !`cmd` ``（经与 command hook 相同的 shell 同意闸 `hooks/consent.rs`；未同意留占位 + 警告）→ 作为本轮用户内容注入。frontmatter：`argument-hint` 进列表；`allowed-tools` 作本轮静态 retain（与 `src/tools/scoped/` 三道 retain 同层，R10 例外同类）；`model` 走请求级模型 pin；`disable-model-invocation` 只留人类入口。slash 条目随 lifecycle 注册/撤销（§3.1 效果 6）。**CUT** `plugins.executeCommand` + `execute_plugin_command`（`handlers/runtime.rs:82-116`，把 markdown 当 WASM 导出名，零客户端） | `slash_command.rs:249-275`、`template.rs` |
| 8 | **agents 正文** | `AgentDef.system_prompt: Option<String>`；插件 agent（`mod.rs:254-294 plugin_agent_to_def`）与磁盘 `agents/*.md` 同一条路；`permissionMode` 能 1:1 映射执行档就映射否则忽略并记录；`color` 忽略 | `src/agents/` |
| 9 | marketplace 判别键 | `source.source` 与 `source.type` 都接受 | `marketplace/manifest.rs` |
| 10 | **只读发现 `~/.claude/plugins/`** | 读 `installed_plugins.json` → `cache/<marketplace>/<plugin>/<ver>/` 作插件目录；新 `PluginOrigin::ClaudeCache`；**默认 disabled**，`plugin_manage list` 带 origin 露出，一个动词启用；启用态只写 `plugins.toml`；**永不写 `~/.claude/`**；不读 `settings.json` | `discovery/scanner.rs:214-224` |
| 11 | `allowed-tools` 双语义 | command → 本轮限制；skill → 预授权（跳审批）不限制。两条路各一测 | — |
| 12 | skills 的 CC 专属字段（`when_to_use` / `context: fork` / `hooks` / `paths` / `disallowed-tools`） | 宽容解析不报错；除已有 `disable-model-invocation` / `user-invocable` 外不兑现 → DEVIATION 明列 | `skill/manifest.rs` |

### 3.7 MCP server 面（B 支线）

**位置**：`src/gateway/mcp_face/`——一张接口脸（R4：纯 I/O，把 `tools/call` 翻成 Aleph 的工具调用路径）。wire 类型复用 `src/mcp/{jsonrpc,protocol,types}.rs`，**不复制**（判据 §1；CLAUDE.md 禁用清单加一行）。

| 项 | 决定 | 依据 |
|---|---|---|
| 传输 | 只做 **Streamable HTTP**，挂网关已有 HTTP 监听的 `/mcp`（POST JSON-RPC + 可选 SSE 通知流 + `Mcp-Session-Id`） | stdio 会让宿主 spawn 第二个 `aleph-server` 撞单例 flock（`scan-dsh-cordis.md` §7.6）；legacy SSE dsh 不支持 |
| 协议版本 | 协商：客户端提出的版本在支持集内则接受，否则回最新；支持集至少含 `2025-03-26`（pi-mcp-adapter 默认）与当前最新 | `scan-pi.md` §9.1、`scan-dsh-cordis.md` §7.2 |
| 能力 | 只宣告 `tools: { listChanged: true }`；短 `instructions` | dsh 只消费 tools；adapter 永不自动注入 instructions |
| 不做 | prompts / resources / sampling / elicitation / roots / tasks | YAGNI |
| 认证 | 复用网关 `connect` 那一份：loopback 免凭据；远程 `Authorization: Bearer <token>`（dsh / adapter 都是静态 `headers`） | 一层信任模型，不造第二套 |
| 暴露 | `[mcp_server] enabled / expose = [...]`；默认集合＝无副作用工具，**名单在计划阶段从工具目录按谓词挑出并逐个点名**（本文不空口列名，判据 §17）；`bash` / 文件写 / 浏览器须显式加入 | U7 |
| 执行 | 每个 MCP session ↔ 一个 Aleph 会话；principal `McpClient { client_name }`（来自 `clientInfo.name`）；调用走**同一条** scoped dispatch（审批门 + spend ledger）；需要人审批：有 operator UI 在线走现有审批流，否则 **deny → `isError: true`** 且原因里说明从哪开（fail-closed 不是 fail-dead，判据 §14） | — |
| 名字 | Aleph 工具名已是 `[a-z0-9_]`，满足 dsh 64 字符规则；计划阶段验证无超长名 | `scan-dsh-cordis.md` §7.3 |
| 通知 | lifecycle 迁移 / `expose` 配置变更 → `notifications/tools/list_changed` | §3.2 |

### 3.8 `packages/pi-aleph/`

顶替要 CUT 的 `packages/plugin-sdk/`：一个**无 JS** 的 pi 包（`scan-pi.md` §9.2 配方）——`package.json#pi = { skills, mcp }`，`mcp.json` 指向 Aleph 的 `/mcp`，`skills/aleph/SKILL.md` 教模型 Aleph 有什么、怎么用（Agent Skills 可移植字段）。文档同时给 Claude Code `.mcp.json`（`type: http`）与 dsh `cordis.yml` 一行。

### 3.9 CUT 清单（熵减）

| 类别 | 项 | 动作 |
|---|---|---|
| OpenClaw 代码 | `markdown_skill/spec.rs:47-124` 两个 DTO + `:1-4,36,44`、`mod.rs:4`、`loader.rs:166`、`tool_adapter.rs:143` 的「OpenClaw 兼容」注释 + `executor.rs:690` 夹具 + `hub/catalog_client.rs:326,345` 夹具值 + `tests/fixtures/markdown_skills/echo-basic/SKILL.md:3,11` 措辞 + 3 个 `*_matches_openclaw*` 测试名（只改名） | CUT / 改名 |
| OpenClaw 文档 | `ARCHITECTURE.md:261` 幽灵行；`ALEPH_HUB.md §7` 去 OpenClaw 框架**保留其中裁定**（install-policy / telemetry / promotions 有意不做）；`PLUGIN_SYSTEM.md` `SKILL_MODEL_TAXONOMY.md` 的 "(openclaw parity)" 标签改 Aleph 自述；`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` 移 `docs/archive/` | CUT / 改标签 / 归档 |
| OpenClaw **保留** | ACP 预设 `config/types/acp.rs:420-429` + `member_add.rs:102`；`ClawTeam`；`clawshell`；`settings_sidebar.rs:287-291 clawhub_tab_is_removed` 守卫 | 不动 |
| 零客户端 RPC | `plugin.*` 单数 6 个（`handlers/mod.rs:377-389`）+ 说反的注释 `:364`；`plugins.{load,unload,executeCommand}`（`:371-374`）；`command.execute` 桩（`:346-352`）+ `handlers/commands.rs::handle_execute`；`mcp.*` 11 个（`bin/…/handlers/mcp.rs:32-45`）。census 只查了单行字面量：**删前逐个用多行 grep 复核** Panel / TUI / CLI / qa；有工具面等价物的删 RPC 面 | CUT（复核后） |
| 幻影 Node 运行时 | `EXTENSION_SYSTEM.md:143-218` 整节；`packages/plugin-sdk/`；`examples/plugins/media-video` `kind="nodejs"` → `runtime="mcp"`，改不了就删 | CUT / 替换 |
| 兄弟仓 | `plugins/media-office/src/index.js:264 onPostToolUse`（JS hook 无人能调）在 Aleph-plugins submodule | 本仓不动；写成兄弟仓 follow-up（带 path:line） |
| 枚举 / 孪生 | `PluginStatus::Overridden`；`reload_plugin` 窄孪生 | CUT（§3.2、§3.3） |
| 说谎的注释 | `projection.rs:14-24`；`HARNESS_PHILOSOPHY.md §8 第五课` | 改写为 C 的规则 + 收窄记录 |

---

## 4. 错误处理

- `mount` 任一步失败 → dispose 已注册部分 → `Failed { step, reason }`；不 panic；锁 `unwrap_or_else(|e| e.into_inner())`。
- `dispose` 单条失败 → 记日志（带 step 标签）继续；scope 被消费，registry 行最后删——不存在「半卸载」。
- `Pending` 只由依赖上报改变，不因超时变 `Failed`；doctor 露出。
- MCP 面：工具执行错误 → `isError` 结果（不是协议错误）；远程无凭据 → 401；未知 `Mcp-Session-Id` → 404；不在 `expose` → tool not found；协议版本不支持 → 回最新版本由客户端决定。
- hook exit 2 且 stderr 空 → 仍 block，通用原因。
- `~/.claude/plugins/` 解析失败（`installed_plugins.json` 形状变了）→ 该来源整体跳过 + 一条 warn，不影响其他来源（P7 外部数据不信任）。

---

## 5. 测试

**单测**：disposer 逆序 / async / 单条失败继续；`visible_to` 真值表（Global×有/无项目、Project×同/异/无项目）；hook 决策派生矩阵（exit 0/2/其他 × 有/无 JSON × Observer/Interceptor）；`SkillTemplate` 展开（`$ARGUMENTS` / `$N` / `${N:-d}` / `@file` / `!cmd` 同意与未同意）；marketplace 两种判别键；`ClaudeCache` 解析（夹具取自本机真实 `installed_plugins.json` 形状）；`AgentDef.system_prompt` 映射（插件与磁盘同路）；`Pending` 派生；MCP 面握手三个版本、`expose` 过滤、`isError`、401/404。

**守卫**：§3.5 G1–G6，每条做变异证伪并记录红名单。

**集成**（`tests/`）：mock MCP server 插件 enable → disable → enable，transient server 与 memory extension 随之出现 / 消失 / 再出现；`/cmd` 正文到达模型（断言转录）；`ClaudeCache` 默认 disabled；无项目会话看不见 Project 插件。

**最小可信验证集六条 + `just clippy`**；Panel 若改（插件列表露 origin / `Pending`）跑 `cargo test -p aleph-panel --lib` + `just wasm`。

---

## 6. 真机 QA

起 `aleph-server` 必须给假 API key（否则 `tools.invoke` 是占位符）；**跑的必须是本 worktree 编出的二进制**（worktree 有自己的 `target/`，不设 `CARGO_TARGET_DIR`）。

| 装置 | 阶段 | 证明什么 |
|---|---|---|
| `qa/plugins/run.sh` 新增 | `scope` | MCP 插件 enable→disable→enable，`tools.catalog` 每次都变 |
| | `command` | `/cmd` 后正文出现在模型收到的消息里 |
| | `exit2` | 一个 `echo reason >&2; exit 2` 的 PreToolUse hook 阻断工具调用且原因回到模型 |
| | `cc-cache` | 夹具 `~/.claude/plugins/` 被发现、默认 disabled、启用后工具可见 |
| | `visibility` | Project 插件对无项目会话不可见，对该项目会话可见 |
| `qa/mcp_face/run.sh` 新建 | `handshake` | 三个协议版本各握手成功 |
| | `tools` | `tools/list` ⊆ `expose`；调用一个只读工具得到文本结果 |
| | `auth` | 远程无 bearer → 401 |
| | `list_changed` | toggle 插件 → 客户端收到通知 |
| | `deny` | 非 expose 工具 → not found；需审批的工具无 operator → `isError` |

---

## 7. 文档更新（判据 §1：改代码的同笔改文档）

- **FEATURE_LOCATOR.md**：§3.10 新一轮条目（作用域化、CC 补齐、ClaudeCache、CUT 清单）；新增 §5.x「MCP server 面」；附录 D 全文（派生法的列举盲区、投影裁定收窄、OpenClaw 真实大小 ≈50 行）；附录 E.3 / E.9 触发器——**不加新形状名**（都是 §5 / §7 / §14 / §16 的实例）。
- **CLAUDE.md**：禁用清单加「第二个 MCP server 实现（唯一真源 `src/gateway/mcp_face/`，wire 类型来自 `src/mcp/`）」；子系统路由加 `src/gateway/mcp_face/` 行 + QA 指针；`src/extension/` 行的判据指针更新。
- **PLUGIN_SYSTEM.md**：生命周期节重写为四原语；`ClaudeCache`；commands / agents 正文；`Overridden` 假声明删；"(openclaw parity)" 改标签。
- **EXTENSION_SYSTEM.md**：effects / visibility 两节；删 Node.js 运行时节（`:143-218`）；Node 插件的正解＝MCP stdio server。
- **GATEWAY.md**：加「MCP 面」一节（不新建平行 reference 文件）。
- **ALEPH_HUB.md** / **ARCHITECTURE.md** / **SKILL_MODEL_TAXONOMY.md** / **HARNESS_PHILOSOPHY.md**：§3.9 所列改法。
- **`src/extension/projection.rs:14-24`** 注释改写。
- **`qa/README.md`**：两个装置的阶段清单与「在证明什么」。

---

## 8. 非目标 / 刻意不做 / DECIDE

**刻意不做（延续既有裁定）**：DI 容器、Proxy 上下文、级联重启、HMR、插件依赖声明与 host API 版本闸（2026-08-19 用户裁定继续 defer）、版本化插件缓存 + orphan 墓碑、lazy activation（2026-08-07 CUT，勿复活）、session / agent 级子作用域（U4）、在 Aleph 内跑 pi 的 TS 扩展（U3）、读 `~/.claude/settings.json`（U6）、MCP 面的 prompts / resources / sampling / elicitation、gRPC。

**DECIDE（写进本 spec，本轮不做）**：
1. `AlephSkillSpec` Phase-2 解散（`src/tools/markdown_skill/` 2,448 行，`#[deprecated(since="26.5.20")]`，`SKILL_MODEL_TAXONOMY.md` 写的截止日期逾期 3.5 月）——本轮只 CUT 其中的 OpenClaw DTO，并把截止日期改成真话；解散另开一轮。
2. `aleph-cli` 输出面按 pi 的 bash 契约整改（stdin 关闭、无 TTY、合流、尾截断 2000 行 / 50 KiB）——前提是先决定 `interfaces/cli` 是否发货（`plugin_manage.rs:36-38`：release 工作流不构建它）。
3. `PostToolUse` payload 的结果字段名（`tool_result` / `tool_response`）——P0 抓包后定，不在此猜。

---

## 9. 执行编排与验收

- **一棵 worktree** `worktree-plugin-scope-round`；**同一时刻只一个 agent 碰树**；submodule 已 `update --init`（`include_dir!` 缺目录直接编译失败）。并行只用于只读 reviewer。
- **阶段**：P0 hook stdin 真机抓包 → P1 effects + lifecycle + registrar `Disposer` + 两个 unregister + G1–G3 → P2 visibility 五张脸 → P3 activation gate + `Pending` + G6 → P4 CC 兼容 12 项 + G4 → P5 CUT 清单 → P6 MCP 面 + `pi-aleph` + G5 → P7 QA 装置 + 六条验证集 → P8 文档 → P9 合并 main（用户批准后）。
- **每个任务**：实现子代理（Opus，TDD，交付物增量落盘）→ 审查子代理（spec 符合度 + 代码质量）→ 主 agent 验收（跑守卫、做变异、读 diff 摘要）。
- **提交**：`<scope>: <description>`，英文；单分支开发，最后合 main。
- **完成定义**：§5 全部测试绿；§3.5 六条守卫各有一次记录在案的变异红；§6 十个真机阶段 PASS（跑在本 worktree 二进制上）；§7 文档全部落地；`src/harness/` diff 为 0 行；`cargo test -p alephcore --bins` 的 census 绿。

---

## 10. 验收表约定

`scan-cc-plugin-format.md` 末尾 70 项每项在计划里标注：
- **IMPLEMENTED**：指出代码 + 测试；
- **CONNECT**：本轮接线，指出任务号；
- **DEVIATION**：明写偏离与理由（目前已知：默认超时 300s 而非 600s；skills 的 CC 专属字段不兑现；`~/.claude/settings.json` 不读；hooks 用户级文件在 `~/.aleph/hooks.json`）。

同一张表也覆盖 dsh 宿主一行配置与 pi-mcp-adapter 一行配置能否挂载 Aleph（§3.7 的两个真机阶段）。
