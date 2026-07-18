# Project Workspaces — 设计文档

**日期**: 2026-06-16
**目标 (来自 /goal)**: Aleph 从 server 模式（agent 工作目录固定 `~/.aleph/workspaces/{agent_id}`）演进为桌面 App，可像 Claude Code 那样自由选择本机任意项目文件夹作为工作空间；在该项目目录下使用项目 `CLAUDE.md` / `.claude` / `.aleph` 文件夹（skill、instruction 等）；Panel 提供切换按钮；未指定时回退默认 workspace。

---

## 0. 范围裁定（Core Slice）

经调研，Aleph 已存在大量基础设施。本 slice 只补"未连"的三条主链，**不**做 plugin / MCP / memory 的项目级隔离（留作后续 slice）。

**已存在、本 slice 直接复用（不重写）：**

| 能力 | 现状 | 位置 |
|------|------|------|
| 项目目录作为 CWD | ✅ `chat.send(project_root)` → `RunRequest.workspace_override` → effective CWD → 注入 `bash`/`code_exec` | `gateway/handlers/agent.rs`, `execution_engine/run_loop.rs:226-243` |
| 项目级 skill 发现 | ✅ `get_all_skills_dirs(Some(workspace))` 已以 effective workspace 为根，向上走到 git root 扫 `.aleph/skills`、`.claude/skills` | `utils/paths.rs:267`, `run_loop.rs:1339` |
| Panel 文件夹切换按钮 | ✅ `ProjectMenu`（DirectoryBrowser + 最近项目 + `set_active_project`） | `views/chat/project_menu.rs` |
| 服务端文件夹浏览 | ✅ `fs.*` RPC + `DirectoryBrowser`（对桌面/LAN 均正确，故意不用 Tauri 本地 picker） | `components/directory_browser.rs` |
| 默认回退 | ✅ `project_root = None` → `agent.workspace()` = `~/.aleph/workspaces/{id}` | `run_loop.rs:226-243` |
| active_project_root 内存级保活 | ✅ 存入 `ChatSnapshot`（按 tab） | `views/chat/state.rs:801,821,842` |

**本 slice 要补的三条链：**

1. **G1 — 信任门 loopback 放行**：让桌面本地 Panel 能真正设置 project_root。
2. **G2 — 项目级指令文件进 prompt**：项目 `CLAUDE.md` / `AGENTS.md` / `.claude` / `.aleph` 被读取并注入 system prompt。
3. **G3 — per-session project_root 持久化**：选定的项目目录写入会话元数据，重载/换客户端不丢；选会话时恢复。

---

## G1. 信任门：loopback 放行

### 现状（问题）
`gateway/handlers/agent.rs:254-282`：设置 `project_root` 是 config-tier 能力。chat-tier（远程 Panel 配对在 "chat"，或外部 channel 标 "guest"）被拒：

```rust
let role = current_caller_role();
let is_config_tier = !matches!(role.as_deref(), Some(r) if r != "operator");
if !is_config_tier { return Err("... requires config-tier authorization ..."); }
```

桌面 App 的本地 Panel 若被标为 chat-tier，则**无法选项目目录** —— 与目标冲突。

### 设计（用户裁定：Local-loopback allowed）
保留"信任边界 = 网络边界"。新增 per-connection loopback 信号，门改为：

```rust
let allow = is_config_tier || caller_is_loopback();
```

- **`caller_is_loopback()`**：新增 task-local，在 gateway dispatch 循环里与 `CALLER_ROLE` 同点设置，值取自 WS 连接 peer 地址是否 loopback（`127.0.0.0/8`、`::1`、`localhost`）。复用 `gateway/origin_policy.rs` 的 `is_loopback_host` / rate_limiter 的 `is_loopback` 判定逻辑（抽到一个共享小函数，避免重复）。
- 远程 LAN 客户端（peer 非 loopback 且 chat-tier）→ 仍拒，trust model 不变。
- 绑定 `0.0.0.0` 但本机自己连（peer = loopback）→ 放行（本机操作者意图明确）。

### 影响面
- `gateway/caller_identity.rs`（或同模块）：加 `CALLER_IS_LOOPBACK` task-local + `set/current` 函数，镜像 `CALLER_ROLE`。
- gateway dispatch 处（设置 `CALLER_ROLE` 的同一位置）：从连接 peer addr 计算 loopback 并 set。
- `agent.rs:254-282`：门条件加 `|| caller_is_loopback()`；错误信息更新。
- 非 gateway 运行（cron/internal，无 peer）→ `caller_is_loopback()` 返回 `false`，但这些路径 `current_caller_role()` 为 None → `is_config_tier == true` → 本就放行，不受影响。

---

## G2. 项目级指令文件进 prompt（CLAUDE.md / AGENTS.md / .claude / .aleph）

### 现状（问题）
`IdentityFiles::load(identity_dir, …)` 只从**全局** agent 身份目录 `~/.aleph/agents/{id}/` 读 `SOUL.md/IDENTITY.md/AGENTS.md/TOOLS.md/HEARTBEAT.md`（`harness_bridge.rs:1029`）。项目目录里的 `CLAUDE.md` / `.claude/CLAUDE.md` / `AGENTS.md` **从不进 prompt** —— 这是与"在任何项目目录下使用项目 claude.md"差距最大的一点。

### 设计
新增**项目指令层**，与 agent 身份文件正交（agent 身份是"我是谁"，项目指令是"这个项目要怎么干"）。

**新模块** `src/thinker/project_instructions.rs`：
- `ProjectInstructions::load(project_root: &Path) -> ProjectInstructions`
- 仅当 effective workspace 是**用户选定的 project_root**（即 `workspace_override.is_some()`）时加载；默认 agent workspace 不触发（避免给默认 workspace 注入空层）。
- 发现顺序（镜像 `get_all_skills_dirs`：从 project_root 向上走到 git root），按 Claude Code 约定 **越靠近 project_root 优先级越高，但全部拼接注入**（不是互相覆盖，是叠加）：
  - 每层目录读：`CLAUDE.md`、`.claude/CLAUDE.md`、`AGENTS.md`、`.aleph/AGENTS.md`
  - `.claude/rules/*.md`（可选，二期；本 slice 先只读顶层 4 个文件，避免范围膨胀）
- 复用 `truncate_with_head_tail` 做预算控制：单文件上限 20KB，本层总上限 32KB（防止巨型 monorepo 根 CLAUDE.md 撑爆上下文）。
- `.claude` 与 `.aleph` 二者同名取其一即可（`.aleph` 优先，向后兼容 `.claude`）。

**注入点**：
- `harness_bridge.rs`：加载 agent IdentityFiles 之后，若 `workspace_override` 存在，再 `ProjectInstructions::load(project_root)`，作为**独立 prompt 层**附加，渲染在 agent 身份/soul 之后、对话之前。
- 渲染 header 明确来源："# Project Instructions"，每个文件块标注相对路径（如 `<!-- from ./CLAUDE.md -->`），便于模型理解层级与可调试。
- 走现有 `LayerInput` / `prompt_builder` 机制新增一层（最小侵入），不改 harness 笨循环（R10 合规：脚手架而非认知）。

### 为什么不复用 IdentityFiles
`IDENTITY_FILE_NAMES` 是 agent 全局身份语义，且 `resolve_path` 只查单目录不向上走。项目指令需要"向上走到 git root + CLAUDE.md 文件名 + 叠加语义"，与身份文件不同；硬塞会污染身份语义并破坏现有测试。新模块更内聚（P2 高内聚）。

---

## G3. per-session project_root 持久化

### 现状（问题）
`active_project_root` 只在前端 `ChatSnapshot`（内存、按 tab）保活；重载 Panel / 换客户端 / 重启 daemon 即丢。历史记忆确认 `set_project_root` 此前是 cosmetic metadata。

### 设计（镜像 set_topic 既有模式，不加 SQL 列）
**后端**：
- `sessions.set_project_root` RPC：`{ session_key, project_root: Option<String> }`，在 `gateway/handlers/session/db_handlers/modify.rs` 新增 handler，镜像 `handle_set_topic_db`，写入 `identity_meta.custom["project_root"]`（与现有 `topic` 同款 JSON metadata 模式，无需改表/结构体/行映射）。
- `SessionInfo`（`db_handlers/types.rs`）加 `project_root: Option<String>`；`query.rs` 映射时从 `identity_meta.custom.get("project_root")` 读出（与 `topic` 同处）。
- 注册到 session handler 分发（`session/mod.rs`），re-export 两处（参照 `set_pinned` 先例）。

**持久化时机（as-built）**：选项目会 `clear_session`（1:1 绑定），选时尚无 session_key，故**前端 persist 不可靠**。改为**服务端首消息 stamp**：`execution_engine/execute.rs` 在 `is_first_message` 且 `workspace_override` 存在时 `tokio::spawn` 调 `sm.set_project_root(key, Some(root))`（镜像 source-channel/topic stamping，best-effort）。项目↔会话绑定固定于会话创建，后续切换起新会话重新 stamp。

**前端**：
- `api/sessions.rs`：加 `set_project_root(state, session_key, Option<&str>)`（显式 set/clear 的 affordance，主持久化走服务端 stamp）。
- `SessionEntry`（chat_sidebar）：加 `#[serde(default)] project_root: Option<String>`。
- **选会话恢复**：`on_select_session` 按 key 查 `sessions` 信号取 `project_root` → 直接 set `chat.active_project_root`/`active_project_name`（不走 `set_active_project`，避免 clear 掉正要加载的会话），使工作目录跨重载稳定。`None` 回退默认 workspace。

### 默认回退
`project_root` 为空/未设 → 会话不带，`workspace_override = None` → 默认 workspace。语义不变。

---

## 数据流（端到端）

```
用户点 ProjectMenu → DirectoryBrowser(fs.* 浏览服务端) → 选定 /path
  → chat.set_active_project(Some(/path))            [内存 + tab snapshot]
  → sessions.set_project_root(session_key, /path)   [G3 持久化到 identity_meta.custom]
下一次发消息:
  chat.send(..., project_root=/path)
  → agent.run handler: 信任门 [G1 loopback 放行] → 校验绝对/存在/目录
  → RunRequest.workspace_override = Some(/path)
  → effective_workspace = /path
     ├─ 工具 CWD 注入 (已存在)
     ├─ get_all_skills_dirs(Some(/path)) 项目级 skill (已存在)
     └─ ProjectInstructions::load(/path) 项目 CLAUDE.md/AGENTS.md → prompt 新层 [G2]
重载 Panel / 选回该会话:
  sessions.list 返回 project_root → on_select_session 恢复 active_project_root [G3]
```

---

## 测试计划

- **G1**：单元测试 `caller_is_loopback` 判定（127.0.0.1 / ::1 / localhost / LAN IP）；门逻辑测试（config-tier 放行 / chat-tier+loopback 放行 / chat-tier+LAN 拒）。
- **G2**：`ProjectInstructions::load` 单元测试 —— 顶层 CLAUDE.md、`.claude/CLAUDE.md`、向上走到 git root 叠加、预算截断、project_root 无文件返回空、`.aleph` 优先于 `.claude`。
- **G3**：`set_project_root` 往返（set → list 读回 → clear）；`SessionInfo.project_root` 序列化默认值。
- **构建门**：`cargo check -p alephcore --bin aleph-server`；`cargo build -p aleph-panel --target wasm32-unknown-unknown`（前端）。
- **人工 E2E（留用户）**：桌面 App 选真实项目目录 → 验证项目 skill 可见 + 项目 CLAUDE.md 进 prompt（行为可观察）+ 重载后目录保持。

---

## Slice 2a — 指令扩展（2026-06-16，已批准）

承 Core slice。用户裁定先做 4 簇里的 **A. 指令扩展**（`.claude/rules/*.md`、`CLAUDE.local.md`、`@import`）。全部落在 `src/thinker/project_instructions.rs` 一个模块内，纯 prompt 层、零 harness 改动、唯一 consumer 是 `harness_bridge`（签名不变）。

### A1. `CLAUDE.local.md`（gitignore 的本地覆盖）
加进 `CANDIDATES`，排在 `CLAUDE.md` 之后（local 覆盖读在 base 之后 = 权重更高）。`.claude/CLAUDE.local.md` 同理。每层目录最终顺序：
`CLAUDE.md → CLAUDE.local.md → .claude/CLAUDE.md → .claude/CLAUDE.local.md → AGENTS.md → .aleph/AGENTS.md`。

### A2. `.claude/rules/*.md` + `.aleph/rules/*.md`
每层目录 glob `rules/` 子目录下的 `*.md`，按文件名字典序（确定性排序），每个规则文件成独立 `ExtraPromptFile`，渲染在该层主指令文件之后，共享同一 `TOTAL_MAX_CHARS` 总预算。新增私有 `glob_rule_files(dir) -> Vec<PathBuf>`。

### A3. `@import` 内联（用户裁定：限工作区子树）
对每个加载文件的内容做后处理 `expand_imports(content, file_dir, git_root, &mut visited, depth)`：
- 逐行扫描，跟踪 ```` ``` ```` 围栏代码块状态（块内 `@` 不展开，Claude Code parity）；行内 backtick 包裹的 `@` 跳过；`user@domain` 这类被 `@` 前是字母数字判定排除。
- 命中 `@<path>`：相对路径按"导入文件所在目录"解析，canonical 化。
- **安全边界（用户裁定）**：canonical 路径必须在 `git_root` 子树内；越界则不内联，替换为 `<!-- import skipped (outside workspace): {path} -->` 标记（防恶意克隆 repo 把 `~/.ssh`、`/etc/passwd` 外泄进 prompt）。无 git_root 时以 `workspace` 自身为边界根。
- 递归内联，最大深度 `MAX_IMPORT_DEPTH = 5`；`visited: HashSet<PathBuf>` 防环（已访问的导入替换为 `<!-- import skipped (cycle): {path} -->`）。
- 读不到的文件替换为 `<!-- import not found: {path} -->`，不报错。
- 预算：**先展开 `@import` 再**套 `PER_FILE_MAX_CHARS` / `TOTAL_MAX_CHARS` 截断。

### 架构
`load_project_instructions` 拆为：(a) 发现文件列表（candidates + rules glob），(b) 对每个文件 `expand_imports` 内联，(c) 套预算去重。新增私有 `expand_imports` + `glob_rule_files`。现有 6 个测试行为不变（无 @import/rules/local 的项目走原路径）。

### 测试（约 8-10 新单测）
CLAUDE.local.md 渲染顺序；rules glob 字典序 + 共享预算；@import 相对解析、递归内联、深度上限、环检测、子树越界跳过（含 `~`/绝对路径）、围栏代码块内不触发、行内 backtick 不触发、import not found 标记。

### 验证策略
自包含模块。收尾跑**一次**聚焦 `cargo test -p alephcore project_instructions::`，不跑全 suite（尊重 cargo 负担）。

### 红线合规
R7/R9/R10 纯 prompt 层不进 harness 笨循环；P2 全在一模块高内聚；P7 import 子树边界 + 深度/环/预算三重上限。

---

## 不在本 slice（后续簇 B/C/D）

- **B. 能力发现**：项目级 plugin 自动发现（现 `discover_plugins_with_extra` 需显式注册）、`.aleph/mcp_config.json` 合并。
- **C. 记忆隔离**：per-project memory 命名空间。
- **D. 配置分层**：项目级 settings.json 合并层级。

这些与 Core slice + Slice 2a 解耦，可独立加而不回改已落的链。

---

## 红线合规自检

- **R1/R6**：文件夹浏览走 `fs.*` 服务端 RPC，非客户端本地 picker —— 一核多端正确语义。
- **R7/R9/R10**：信任门是确定性安全硬过滤（允许）；项目指令是 prompt 层（智慧在 prompt），不进 harness 笨循环。
- **P2 高内聚**：项目指令独立模块，不污染 agent 身份语义。
- **P7 防御性**：project_root 入口仍校验绝对/存在/目录；loopback 放行不削弱 LAN 边界。
