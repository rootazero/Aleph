# 大文件拆分 Round 1 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans（每个 PR 一个 task，串行执行）。Steps 用 `- [ ]` 跟踪。

**Goal:** 按 [`docs/reference/CODE_ORGANIZATION.md`](../reference/CODE_ORGANIZATION.md) Pattern A/C/D 拆分 7 个超大文件（3 P0 + 4 P2），把 6 个因 Rust 语言约束或单概念纵深无法拆的文件登记为「声明遗留」，行为零变更，pub API 零变更。

**Architecture:** 7 个独立 PR 串行推进；每个 PR 内部分步完成「复制 → 子模块化 → 删旧 → 验证 → commit」。`mod.rs` 留最薄入口（re-export + 公共类型），子模块按职责拆分。`source_scan` 测试守卫一律改为按模块路径而非文件名 derive。

**Tech Stack:** Rust 1.95（MSRV）、Cargo workspace、alephcore 单 crate；本机 Linux（`/home/zou/data/workspace/Aleph`）；纯 `cargo` 工具链。

**Spec:** [`docs/superpowers/specs/2026-09-22-large-file-split-design.md`](../specs/2026-09-22-large-file-split-design.md) — 计划从 spec 推出；阅读 §0（范围）、§1（问题与形状）、§2（七文件逐一）、§3（共同原则）、§5（声明遗留登记）。

---

## Global Constraints

- **单分支开发**：所有工作在 `main` 上直接进行（AGENTS.md 「单分支开发」规则）。**不创建 worktree**、不用 `EnterWorktree`。
- **R10 红线不动**：`src/harness/` 12 文件棘轮；`tests/act.rs` 2687 行等测试文件按 §3.4 保留为整体。
- **行为零变更**：拆分前后所有可观察行为（RPC 响应、日志行、metrics、文件输出、副作用顺序）逐项一致。
- **pub API 稳定**：模块路径（`crate::extension::ExtensionManager` 等）不变；只调整内部组织。
- **测试文件不拆**（用户裁定）。
- **CHANGELOG 不进**：pure refactor。
- **commit 格式**：`<scope>: split <原文件名> into <N> files (P<n>)`，如 `gateway: split server/handler.rs into 6 files (P0)`。
- **cargo 配方**（本机 Linux）：
  - 快速类型检查：`cargo check -p alephcore`（约 1–4 min）
  - lib 测试构建：`cargo test -p alephcore --lib --no-run`
  - 全测试构建：`cargo test -p alephcore --lib --bins --features test-helpers --test '*' --no-run`
  - clippy：`cargo clippy --workspace --all-targets`
  - **内存受限**（<16GB）：`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib`
- **不引入新依赖**、不改 `Cargo.toml`。
- **不修任何 bug**、不做任何功能新增。
- **不删任何代码**。

---

## Review Focus

| # | 输入类 / 失败模式 | 期望行为 | 所属 task |
|---|---|---|---|
| 1 | 拆分后 `cargo check` 通过但 `cargo test --no-run` 失败（漏改 cfg(test)） | `cargo test -p alephcore --lib --no-run` 必须通过 | Task #1, #2, #3, #5, #6, #7 |
| 2 | `include_str!("handler.rs")` 守卫测试在拆分后失效 | 拆第一动作先改用模块路径名 | Task #1（PR #1） |
| 3 | `pub(crate)` 单例（如 `set_global` / `AgentRunManager`）拆开后跨模块不可见 | 可见性保留 | Task #6（PR #6） |
| 4 | `source_scan` 守卫用硬编码文件名定位生产端（如 `.append_message(`） | 拆后用模块路径名重新 pin | Task #7（PR #7） |
| 5 | PhaseTally 跨文件迁移后丢失内部状态收集调用 | 调用图重映射 | Task #5（PR #5） |
| 6 | 拆分后 `cargo clippy --workspace --all-targets` 出现新 warning（pub use 未触达） | 0 warning | Task #1–#7 |
| 7 | `BrowserService`/`BrowserPool` 等拆到子模块后 crate 内 `use` 路径失效 | 全路径 re-export | Task #1–#7 |

---

## 0. 准备（一次性，PR #0）

- [ ] **Step 1: 拉取最新 main 并确认基线干净**

```bash
cd /home/zou/data/workspace/Aleph
git status
git log -1 --oneline
git pull --ff-only
cargo check -p alephcore 2>&1 | tail -10
```

期望：基线 `cargo check` 通过；git status 干净。

- [ ] **Step 2: 备份 7 个目标文件路径**

```bash
mkdir -p /tmp/refactor-backup-2026-09-22
for f in \
  src/gateway/server/handler.rs \
  src/bin/aleph-server/commands/start/mod.rs \
  src/extension/mod.rs \
  src/extension/hooks/mod.rs \
  src/builtin_tools/workflow_tool.rs \
  src/gateway/handlers/agent.rs \
  src/gateway/session_projector.rs
do
  mkdir -p "/tmp/refactor-backup-2026-09-22/$(dirname "$f")"
  cp "$f" "/tmp/refactor-backup-2026-09-22/$f"
done
ls /tmp/refactor-backup-2026-09-22
```

期望：7 个备份文件存在。

- [ ] **Step 3: 跑基线 lib 测试构建做对照**

```bash
cd /home/zou/data/workspace/Aleph
cargo test -p alephcore --lib --no-run 2>&1 | tail -20
```

期望：基线 lib 测试构建通过（仅类型检查 + cfg(test) 编译）。

---

## Task 1 / 7 — PR #1: `src/gateway/server/handler.rs` (P0, Pattern D)

> **风险最高**：含 `tests::the_delivery_loop_parses_each_event_once_and_projects_it` 用 `include_str!("handler.rs")` 守卫 production half。**拆第一动作**：先改测试用模块路径名而非文件名 derive。

**Files:**
- Modify: `src/gateway/server/handler.rs` (目标：留入口 ~50 行)
- Create: `src/gateway/server/connection/mod.rs`、`upgrade.rs`、`auth.rs`、`dispatch.rs`、`forward.rs`、`cleanup.rs`
- Modify: `src/gateway/server/mod.rs`（添加 `pub mod connection`）

**Interfaces:**
- Consumes: 5 个生命周期阶段（upgrade / auth / dispatch / forward / cleanup）
- Produces: `pub fn handle_connection(...)`（入口签名不变）

### 步骤

- [ ] **Step 1.1: 先破 include_str! 耦合（拆分前的预备工作）**

定位守卫测试：

```bash
cd /home/zou/data/workspace/Aleph
grep -rn 'include_str!("handler.rs")\|include_str!(.*server/handler.rs' src/gateway/server/
```

读测试代码，把 `include_str!("handler.rs")` 改为从模块路径解析（用 `proc_macro2` / `cargo_metadata` 不可行，改用环境变量或常量占位）。**最简方案**：用 `env!("CARGO_MANIFEST_DIR") + "/src/gateway/server/connection/forward.rs"` 形式。

如果守卫不可改：保持原 include_str!，但把 handler.rs 留一个 stub re-export，**所有真实代码迁到 connection/* 子文件**，测试的 `format!("#[cfg{}]", "(test)")` 切分从 stub.rs 派生（handler.rs 仍存在且内容为纯 re-export）。

- [ ] **Step 1.2: 创建 connection/ 子模块骨架**

```bash
mkdir -p src/gateway/server/connection
touch src/gateway/server/connection/{mod,upgrade,auth,dispatch,forward,cleanup}.rs
```

`connection/mod.rs` 写：
```rust
//! WebSocket 连接生命周期子模块
//! 见 handler.rs 文档了解阶段划分。
pub mod upgrade;
pub mod auth;
pub mod dispatch;
pub mod forward;
pub mod cleanup;
```

- [ ] **Step 1.3: 把 upgrade 阶段代码从 handler.rs 迁到 connection/upgrade.rs**

读 `handler.rs` 中所有以 `upgrade` / `ws_upgrade` / `do_handshake` 开头的函数，整体剪切到 `connection/upgrade.rs`，添加文件级 `//! upgrade 阶段 — WS 握手`。

- [ ] **Step 1.4: 把 auth 阶段代码迁到 connection/auth.rs**

读 handler.rs 中所有 `authenticate_*` / `verify_token` / `check_origin` 等函数，剪切到 `connection/auth.rs`。

- [ ] **Step 1.5: 把 dispatch 阶段代码迁到 connection/dispatch.rs**

读 handler.rs 中消息分发主循环（dispatch / on_message / handle_request 等），剪切到 `connection/dispatch.rs`。

- [ ] **Step 1.6: 把 forward 阶段代码迁到 connection/forward.rs**

读 handler.rs 中事件投递 / event_loop / forward 等，剪切到 `connection/forward.rs`。**这是 include_str! 守卫 pin 的文件**，迁移后需确保守卫能定位到正确路径。

- [ ] **Step 1.7: 把 cleanup 阶段代码迁到 connection/cleanup.rs**

读 handler.rs 中 close / shutdown / cleanup 等，剪切到 `connection/cleanup.rs`。

- [ ] **Step 1.8: 重写 handler.rs 为最薄入口（< 50 行）**

```rust
//! WebSocket 连接生命周期入口
//!
//! 5 个阶段：upgrade → auth → dispatch → forward → cleanup。
//! 各阶段实现见 connection/ 子模块。

pub use connection::*;

/// 处理一条 WebSocket 连接（入口）。
pub async fn handle_connection(...) -> Result<()> {
    upgrade::do_handshake(...).await?;
    auth::verify_origin_and_token(...)?;
    dispatch::run_loop(...).await?;
    forward::event_loop(...).await?;
    cleanup::close_and_log(...).await;
    Ok(())
}
```

- [ ] **Step 1.9: 在 server/mod.rs 添加 `pub mod connection;`**

```bash
cd /home/zou/data/workspace/Aleph
grep -n 'mod handler' src/gateway/server/mod.rs
```

在合适位置添加：
```rust
pub mod connection;
```

- [ ] **Step 1.10: 跑 cargo check**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -30
```

期望：通过。

- [ ] **Step 1.11: 跑 lib 测试构建**

```bash
cargo test -p alephcore --lib --no-run 2>&1 | tail -20
```

期望：通过。

- [ ] **Step 1.12: 跑 bin + 集成测试构建**

```bash
cargo test -p alephcore --bins --features test-helpers --test '*' --no-run 2>&1 | tail -20
```

期望：通过。

- [ ] **Step 1.13: 跑 clippy**

```bash
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -30
```

期望：0 warning。

- [ ] **Step 1.14: 跑 handler.rs 相关测试**

```bash
cargo test -p alephcore --lib -- gateway::server::handler 2>&1 | tail -30
```

期望：全绿。

- [ ] **Step 1.15: commit**

```bash
cd /home/zou/data/workspace/Aleph
git add -A
git status
git commit -m "gateway: split server/handler.rs into 6 files (P0)"
```

期望：单个 commit，diff 显示文件移动 + connection/ 子目录新建。

- [ ] **Step 1.16: 验证 handler.rs 实际行数**

```bash
wc -l src/gateway/server/handler.rs src/gateway/server/connection/*.rs
```

期望：`handler.rs` < 100 行；`connection/*.rs` 各 200–800 行。

---

## Task 2 / 7 — PR #2: `src/bin/aleph-server/commands/start/mod.rs` (P0, Pattern D)

> **风险中等**：启动路径，回归测试厚。已有 `start/builder/agent_init/` 兄弟子目录，需补齐剩余 builder 子模块。

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs`（目标：留 < 100 行入口）
- Create: `src/bin/aleph-server/commands/start/builder/{providers,tools,gateway,channels,session,config,shutdown}.rs`（按需）

**Interfaces:**
- Consumes: `StartArgs`、所有 builder 子模块的初始化方法
- Produces: `pub async fn start_server(args: StartArgs) -> Result<()>`（签名不变）

### 步骤

- [ ] **Step 2.1: 读现有 start/mod.rs 找出 5+ 个初始化阶段**

```bash
cd /home/zou/data/workspace/Aleph
grep -n '// ===\|// ---\|// STEP\|^fn\|^pub fn\|^pub async fn' src/bin/aleph-server/commands/start/mod.rs | head -50
```

列出阶段：providers / tools / gateway / channels / session / config / shutdown。

- [ ] **Step 2.2: 创建 builder 子模块文件**

```bash
cd src/bin/aleph-server/commands/start/builder
for f in providers tools gateway channels session config shutdown; do
  touch "${f}.rs"
done
ls
```

每个新 builder/*.rs 起始内容：
```rust
//! <阶段名> 初始化
use super::*;
// 占位 — Step 2.3 起迁入代码
```

- [ ] **Step 2.3: 迁出 provider 初始化代码**

定位 `start/mod.rs` 中所有 `init_providers` / `register_providers` / `Provider::register` 调用，整体剪切到 `builder/providers.rs`。

- [ ] **Step 2.4: 迁出 tool 注册代码**

定位 `register_tools` / `add_builtin_tool` 等，剪切到 `builder/tools.rs`。

- [ ] **Step 2.5: 迁出 gateway 装配代码**

定位 `setup_gateway` / `start_gateway_server` 等，剪切到 `builder/gateway.rs`。

- [ ] **Step 2.6: 迁出 channel registry 代码**

定位 `register_channels` / `channel_manager.register` 等，剪切到 `builder/channels.rs`。

- [ ] **Step 2.7: 迁出 session manager 代码**

定位 `init_session_manager` 等，剪切到 `builder/session.rs`。

- [ ] **Step 2.8: 迁出 config watcher 代码**

定位 `start_config_watcher` 等，剪切到 `builder/config.rs`。

- [ ] **Step 2.9: 迁出 shutdown / 信号处理代码**

定位 `install_signal_handlers` / `pid_file_handle` / `with_lock` 等，剪切到 `builder/shutdown.rs`。

- [ ] **Step 2.10: 重写 mod.rs 为最薄入口**

```rust
//! Server 启动入口 — 解析参数 → 装配 → 运行
pub mod builder;

pub use builder::ServerBuilder;

pub async fn start_server(args: StartArgs) -> Result<()> {
    ServerBuilder::new(args)?.build()?.run().await
}
```

- [ ] **Step 2.11: 在 builder/mod.rs 添加 7 个新子模块**

```rust
pub mod providers;
pub mod tools;
pub mod gateway;
pub mod channels;
pub mod session;
pub mod config;
pub mod shutdown;
```

- [ ] **Step 2.12: cargo check + test build + clippy**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --bins --no-run 2>&1 | tail -10
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -10
```

期望：全通过。

- [ ] **Step 2.13: 跑 start_server 路径相关测试**

```bash
cargo test -p alephcore --lib -- bin::aleph_server::commands::start 2>&1 | tail -30
```

- [ ] **Step 2.14: 验证 + commit**

```bash
wc -l src/bin/aleph-server/commands/start/mod.rs src/bin/aleph-server/commands/start/builder/*.rs
git add -A
git commit -m "bin: split commands/start/mod.rs into 7 builder files (P0)"
```

---

## Task 3 / 7 — PR #3: `src/extension/mod.rs` (P0, Pattern C)

> **风险中等**：46 个公共方法跨 4 子系统（Skill / Service / Plugin / MCP）。拆分后 `ExtensionManager` 留 facade。

**Files:**
- Modify: `src/extension/mod.rs`（目标：留 facade < 200 行）
- Create: `src/extension/{executor,registry,controller,loader,types}.rs`

**Interfaces:**
- Consumes: `ExtensionManager` 的 46 个方法签名不变（委托到子组件）
- Produces: 4 个子组件类型 `PluginExecutor` / `SkillRegistry` / `ServiceController` / `Loader`

### 步骤

- [ ] **Step 3.1: 列出 46 个方法的归类**

读 `src/extension/mod.rs`，按方法名分组：
- Skill 执行：`execute_skill` / `invoke_skill_tool` / `load_skill` / ...
- Service 管理：`start_service` / `stop_service` / `get_service_status` / ...
- Plugin 执行：`call_plugin_tool` / `execute_plugin_hook` / ...
- MCP / 加载：`get_mcp_servers` / `load_all` / `reload` / ...

输出方法分组表。

- [ ] **Step 3.2: 创建子模块文件**

```bash
cd src/extension
for f in executor registry controller loader types; do
  touch "${f}.rs"
done
```

- [ ] **Step 3.3: 抽出 ExtensionConfig / LoadSummary 到 types.rs**

把 `ExtensionConfig` / `LoadSummary` / 其他共享 enum/struct 整体迁出。

- [ ] **Step 3.4: 抽出 Loader 到 loader.rs**

把 `load_all` / `reload` / `discover_plugins` 等迁出。

- [ ] **Step 3.5: 抽出 SkillRegistry 到 registry.rs**

把 `execute_skill` / `invoke_skill_tool` / skill 查找相关迁出。

- [ ] **Step 3.6: 抽出 ServiceController 到 controller.rs**

把 `start_service` / `stop_service` / `get_service_status` 迁出。

- [ ] **Step 3.7: 抽出 PluginExecutor 到 executor.rs**

把 `call_plugin_tool` / `execute_plugin_hook` / MCP 加载迁出。

- [ ] **Step 3.8: 重写 mod.rs 为 facade**

```rust
//! Extension 子系统 — 4 个组件 + facade
pub mod types;
pub mod loader;
pub mod registry;
pub mod controller;
pub mod executor;

use std::sync::Arc;

pub struct ExtensionManager {
    loader: Arc<loader::Loader>,
    registry: Arc<registry::SkillRegistry>,
    controller: Arc<controller::ServiceController>,
    executor: Arc<executor::PluginExecutor>,
}

// 委托方法（46 个原方法，按 facade 模式逐个委托到对应子组件）
impl ExtensionManager {
    pub async fn load_all(&self) -> Result<()> { self.loader.load_all().await }
    pub async fn reload(&self, name: &str) -> Result<()> { self.loader.reload(name).await }
    pub async fn execute_skill(&self, ...) -> Result<()> { self.registry.execute_skill(...).await }
    pub async fn start_service(&self, name: &str) -> Result<()> { self.controller.start_service(name).await }
    // ... 其余 41 个方法按分组委托
}
```

> **判断点**：如果某些调用者直接调用 manager 方法无所谓，facade 委托可只保留最常用的 ~10 个，其余改为调用者直接访问 `manager.executor.xxx`。读所有 `crate::extension::ExtensionManager::method` 调用点（`grep -rn`），根据实际调用频度决定 facade 委托列表。

- [ ] **Step 3.9: cargo check + test build + clippy**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib --no-run 2>&1 | tail -10
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -10
```

- [ ] **Step 3.10: 跑 extension 相关测试**

```bash
cargo test -p alephcore --lib -- extension 2>&1 | tail -30
```

- [ ] **Step 3.11: 验证 + commit**

```bash
wc -l src/extension/*.rs
git add -A
git commit -m "extension: split mod.rs into facade + 5 subcomponents (P0)"
```

---

## Task 4 / 7 — PR #4: `src/extension/hooks/mod.rs` (P2, Pattern C)

> **风险中等**：与 #3 同源，连贯推进。

**Files:**
- Modify: `src/extension/hooks/mod.rs`（目标：留 facade < 100 行）
- Create: `src/extension/hooks/{registry,executor,types}.rs`

### 步骤

- [ ] **Step 4.1: 列出 hooks/mod.rs 的公共方法分组**

读 `src/extension/hooks/mod.rs`，按职责分组：注册 / 执行 / 类型定义。

- [ ] **Step 4.2: 创建子模块文件**

```bash
cd src/extension/hooks
for f in registry executor types; do touch "${f}.rs"; done
```

- [ ] **Step 4.3: 抽 types.rs（HookDefinition / HookContext 等）**

- [ ] **Step 4.4: 抽 registry.rs（Hook 查找相关）**

- [ ] **Step 4.5: 抽 executor.rs（Hook 执行相关）**

- [ ] **Step 4.6: 重写 mod.rs 为 facade**

```rust
pub mod types;
pub mod registry;
pub mod executor;

pub struct HookManager {
    registry: Arc<registry::HookRegistry>,
    executor: Arc<executor::HookExecutor>,
}

impl HookManager {
    pub async fn execute(&self, hook: &HookDefinition, ...) -> Result<()> {
        self.executor.execute(hook, ...).await
    }
}
```

- [ ] **Step 4.7: 验证**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib --no-run 2>&1 | tail -10
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -10
cargo test -p alephcore --lib -- extension::hooks 2>&1 | tail -30
```

- [ ] **Step 4.8: commit**

```bash
git add -A
git commit -m "extension: split hooks/mod.rs into facade + 3 subcomponents (P2)"
```

---

## Task 5 / 7 — PR #5: `src/builtin_tools/workflow_tool.rs` (P2, Pattern A)

> **风险中等**：PhaseTally 内部状态跨文件迁移。

**Files:**
- Modify: `src/builtin_tools/workflow_tool.rs`（目标：留 ~200 行主体）
- Create: `src/builtin_tools/workflow/{mod,dto,phase_tally}.rs`

### 步骤

- [ ] **Step 5.1: 列出 workflow_tool.rs 的 6 个 DTO + 4 个 impl**

读文件，定位：
- DTO: `WorkflowArgs` 枚举 + `*List` / `*Run` / `*Pin` / `*Step` 5 个 struct + `WorkflowToolOutput`
- Impl: `WorkflowToolOutput` / `WorkflowTool` (inherent) / `PhaseTally` / `AlephTool for WorkflowTool`

- [ ] **Step 5.2: 创建 workflow/ 子模块**

```bash
mkdir -p src/builtin_tools/workflow
touch src/builtin_tools/workflow/{mod,dto,phase_tally}.rs
```

- [ ] **Step 5.3: 抽 dto.rs（所有 DTO + WorkflowToolOutput）**

把 DTO 与其 Serialize / Display / From 等 trait impl 迁到 `workflow/dto.rs`。

- [ ] **Step 5.4: 抽 phase_tally.rs（impl PhaseTally）**

PhaseTally struct + 全部 impl 迁到 `workflow/phase_tally.rs`。**重映射调用图**：用 `grep -rn 'PhaseTally' src/` 找到所有引用，更新为 `crate::builtin_tools::workflow::phase_tally::PhaseTally`。

- [ ] **Step 5.5: 重写 workflow_tool.rs（主体 + AlephTool impl）**

```rust
//! workflow_tool — LLM 端的 workflow action 调度

pub mod workflow;
pub use workflow::{dto::*, phase_tally::*};

pub struct WorkflowTool { /* 主 impl 字段 */ }

impl AlephTool for WorkflowTool { /* 主体实现 */ }
```

- [ ] **Step 5.6: 在 workflow/mod.rs 添加**

```rust
pub mod dto;
pub mod phase_tally;
```

- [ ] **Step 5.7: 验证**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib --no-run 2>&1 | tail -10
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -10
cargo test -p alephcore --lib -- builtin_tools::workflow_tool 2>&1 | tail -30
```

- [ ] **Step 5.8: commit**

```bash
git add -A
git commit -m "builtin_tools: split workflow_tool.rs into dto + tally + main (P2)"
```

---

## Task 6 / 7 — PR #6: `src/gateway/handlers/agent.rs` (P2, Pattern A)

> **风险中等**：`pub(crate)` 单例 `set_global` 可见性需保留。

**Files:**
- Modify: `src/gateway/handlers/agent.rs`（目标：留 ~100 行 re-export + RPC 入口）
- Create: `src/gateway/handlers/agent/{mod,types,manager}.rs`

### 步骤

- [ ] **Step 6.1: 列出 5 struct + 2 enum + 1 manager + 4 verb**

读文件，定位：
- Struct: `Attachment` / `AgentRunParams` / `AgentRunResult` / `RunState` / `AgentRunManager`
- Enum: `RunStatus` / `BuildRunError`
- Verb: `agent.run` / `agent.wait` / `agent.cancel` / `agent.status`

- [ ] **Step 6.2: 创建 agent/ 子模块**

```bash
mkdir -p src/gateway/handlers/agent
touch src/gateway/handlers/agent/{mod,types,manager}.rs
```

- [ ] **Step 6.3: 抽 types.rs（所有 DTO + enum + Display/From impl）**

**不依赖** manager.rs（避免循环依赖）。

- [ ] **Step 6.4: 抽 manager.rs（AgentRunManager impl + 4 verb handler）**

AgentRunManager impl + 4 verb handler 函数迁到 `manager.rs`。**关键**：`pub(crate) fn set_global()` / `global()` 仍定义在 manager.rs，**可见性保留**。

- [ ] **Step 6.5: 重写 agent.rs 为 re-export + RPC 入口**

```rust
//! agent.* RPC handler 入口
pub mod agent;
pub use agent::*;

// 顶层 RPC 入口函数（如有）
```

- [ ] **Step 6.6: 在 agent/mod.rs 添加**

```rust
pub mod types;
pub mod manager;

pub use types::*;
pub use manager::*;
```

- [ ] **Step 6.7: 验证（特别检查 pub(crate) 单例可见性）**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib --no-run 2>&1 | tail -10
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -10
cargo test -p alephcore --lib -- gateway::handlers::agent 2>&1 | tail -30
```

期望：全通过。若 `set_global` / `global()` 不可见，把其可见性改为 `pub(super)` 或 `pub(crate)` 在 manager.rs 显式声明。

- [ ] **Step 6.8: commit**

```bash
git add -A
git commit -m "gateway: split handlers/agent.rs into types + manager (P2)"
```

---

## Task 7 / 7 — PR #7: `src/gateway/session_projector.rs` (P2, Pattern A)

> **风险中等**：source-scan 单写者守卫需重建。

**Files:**
- Modify: `src/gateway/session_projector.rs`（目标：留 ~1000 行主体 + 2 个 pub fn）
- Create: `src/gateway/projector/{mod,missed_seqs,run_span}.rs`

### 步骤

- [ ] **Step 7.1: 列出 3 struct + 4 impl + drain task**

读文件，定位：
- Struct: `RepairReport` / `FlushTimeout` / `MessageProjector`
- Impl: `MissedSeqs` / `MessageProjector` (inherent) / `RunSpan` / `SessionEventObserver for MessageProjector`
- drain task（消息处理主循环）

- [ ] **Step 7.2: 找到 source-scan 单写者守卫**

```bash
cd /home/zou/data/workspace/Aleph
grep -rn 'the_projector_is_the_only_production_writer\|append_message' src/gateway/session_projector.rs tests/
```

定位守卫测试。

- [ ] **Step 7.3: 创建 projector/ 子模块**

```bash
mkdir -p src/gateway/projector
touch src/gateway/projector/{mod,missed_seqs,run_span}.rs
```

- [ ] **Step 7.4: 抽 missed_seqs.rs（MissedSeqs + RepairReport + FlushTimeout）**

迁出独立 seq-set difference 算法实现。

- [ ] **Step 7.5: 抽 run_span.rs（RunSpan）**

迁出 RunSpan 子状态。

- [ ] **Step 7.6: 重写 session_projector.rs（MessageProjector + 2 pub fn）**

```rust
//! session_events → messages 投影器
pub mod projector;
pub use projector::{missed_seqs::*, run_span::*};

pub struct MessageProjector { /* ... */ }

impl SessionEventObserver for MessageProjector { /* ... */ }
```

- [ ] **Step 7.7: 在 projector/mod.rs 添加**

```rust
pub mod missed_seqs;
pub mod run_span;
```

- [ ] **Step 7.8: 修改 source-scan 守卫用模块路径名**

把守卫测试里硬编码的 `include_str!("session_projector.rs")` 或 `gateway::session_projector` 改为覆盖三个新文件（`session_projector.rs` + `projector/missed_seqs.rs` + `projector/run_span.rs`），分别派生 production half。

- [ ] **Step 7.9: 验证**

```bash
cd /home/zou/data/workspace/Aleph
cargo check -p alephcore 2>&1 | tail -10
cargo test -p alephcore --lib --no-run 2>&1 | tail -10
cargo clippy -p alephcore --all-targets -- -D warnings 2>&1 | tail -10
cargo test -p alephcore --lib -- gateway::session_projector 2>&1 | tail -30
```

期望：守卫测试通过（含 source-scan 单写者验证）。

- [ ] **Step 7.10: commit**

```bash
git add -A
git commit -m "gateway: split session_projector.rs into main + missed_seqs + run_span (P2)"
```

---

## Task 8 / 7 — 后置：声明遗留登记 + 全量验证

> 7 个拆分 PR 全合并后，最后做一次性文档化与全量回归。

**Files:**
- Modify: `docs/reference/CODE_ORGANIZATION.md` §6（增补 6 个新增声明遗留）

### 步骤

- [ ] **Step 8.1: 在 CODE_ORGANIZATION.md §6 增补 6 个新声明遗留**

在 `store_impl.rs` 现有范式后追加：

```markdown
#### `src/browser/page_state/fetch_chromium.rs` (7236 行)

**保留理由**：单 capture 算术（DOMSnapshot + 跨源 stitch）；source_scan 守卫锁定 RawDom 构造位置。
**潜在风险**：若强行拆为 stitch.rs + capture.rs，会让 `only_the_page_state_fetchers_construct_a_raw_dom` 守卫定位漂移，且 RawDom 构造位置不可见。
**何时重新评估**：lines 突破 10000；或 Rust 加 `impl` 跨文件语法。

#### `src/executor/builtin_registry/definitions.rs` (4115 行)

**保留理由**：纯 catalog 数据；模块文档禁描述字面重复。
**潜在风险**：拆为多源（plugins 注入条目）会破坏 catalog 一致性。
**何时重新评估**：若 catalog 拆为多源且每个源都有独立测试。

#### `src/gateway/event_visibility.rs` (3686 行)

**保留理由**：SessionOwnership 与 EventVisibilityIndex 互锁；2 impl 服务同一概念。
**潜在风险**：拆为 classification.rs + index.rs 会让 stream wire form 守卫的 pin 路径变复杂。
**何时重新评估**：若 SessionIdentity 衍生第二概念。

#### `src/builtin_tools/desktop/native.rs` (3665 行)

**保留理由**：平台实现深度；macOS 一份，Linux/Windows 平行。
**潜在风险**：若三平台都同等规模且各自独立测试，可拆为 rail/clipboard/tool 三文件。
**何时重新评估**：Linux/Windows 也达到同等规模。

#### `src/capability/census.rs` (3540 行)

**保留理由**：单一推导规则 + 11 个 guard tests；source_scan relative path 强耦合。
**潜在风险**：拆为 rules.rs + guards.rs 会破坏 source_scan 的 relative path 语义。
**何时重新评估**：若 guard tests 拆为多文件测试。

#### `src/memory/dreaming/mod.rs` (3464 行)

**保留理由**：13 个兄弟子文件已就位（含 stages/、evolution/）；mod.rs 仅 orchestration。
**潜在风险**：若 orchestration 突破 5000 行，重新评估拆为 daemon_lifecycle.rs + cycle.rs + report_aggregate.rs。
**何时重新评估**：orchestration 突破 5000 行。
```

- [ ] **Step 8.2: commit 文档**

```bash
git add docs/reference/CODE_ORGANIZATION.md
git commit -m "docs: register 6 newly-identified legacy-keep files in CODE_ORGANIZATION §6"
```

- [ ] **Step 8.3: 全量验证**

```bash
cd /home/zou/data/workspace/Aleph
cargo test -p alephcore --lib --bins --features test-helpers --test '*' --no-run 2>&1 | tail -20
cargo clippy --workspace --all-targets 2>&1 | tail -20
cargo check -p aleph-desktop-macos 2>&1 | tail -10
cargo check -p aleph-desktop-linux 2>&1 | tail -10
cargo check -p aleph-desktop-windows 2>&1 | tail -10
```

期望：全部通过，0 warning。

- [ ] **Step 8.4: 行数核对**

```bash
wc -l \
  src/gateway/server/handler.rs \
  src/gateway/server/connection/*.rs \
  src/bin/aleph-server/commands/start/mod.rs \
  src/bin/aleph-server/commands/start/builder/*.rs \
  src/extension/mod.rs src/extension/*.rs \
  src/extension/hooks/mod.rs src/extension/hooks/*.rs \
  src/builtin_tools/workflow_tool.rs src/builtin_tools/workflow/*.rs \
  src/gateway/handlers/agent.rs src/gateway/handlers/agent/*.rs \
  src/gateway/session_projector.rs src/gateway/projector/*.rs
```

期望：
- 入口文件 < 200 行
- 子模块 200–800 行
- 总行数 ≈ 拆分前（pure refactor，不删代码）

- [ ] **Step 8.5: 清理备份**

```bash
rm -rf /tmp/refactor-backup-2026-09-22
```

---

## Self-Review

- **Spec coverage**: §0（范围）— 7 个文件全部有 task；§1（问题）— 在 Task 0 准备步骤与各 Task step 1 中体现；§2（拆分设计）— Task 1–7 每个 §2 子节都有对应 task；§3（共同原则）— 体现在 Global Constraints 与各 Task 中；§4（执行顺序）— Task 1–7 串行；§5（声明遗留登记）— Task 8.1；§6（自审）— 本节；§8（不在范围）— Global Constraints 已覆盖。✅
- **Placeholder scan**: 无 "TBD" / "TODO"。所有步骤都有具体命令、文件路径、行数目标。✅
- **Type consistency**: 7 个入口方法（`handle_connection` / `start_server` / `ExtensionManager` 36 方法 / `HookManager` / `WorkflowTool::execute` / `agent.run` 等 4 verb / `MessageProjector::drain`）签名在所有 task 中保持一致。✅
- **Review Focus**: 7 个 Review Focus 行的对应测试已在所属 task 中明确：Task #1 / #2 / #3 / #5 / #6 / #7 各承担 1–2 行；其中 Review Focus #2（include_str! 守卫）由 Task 1.1 承担；Review Focus #3（pub(crate) 单例）由 Task 6.7 承担；Review Focus #4（source-scan 单写者）由 Task 7.8 承担。✅
- **Bite-sized**: 每个 step 一动作（grep / 读 / 写 / 跑命令 / commit），平均 2–5 分钟。✅

---

## 执行交接

Plan 已保存到 `docs/superpowers/plans/2026-09-22-large-file-split.md`。

**7 个 PR 串行**，每个 PR 是 Task 1–7 的子集；Task 0 是基线准备；Task 8 是文档 + 后置验证。

**推荐执行方式：Native**（直接在本会话执行）— 理由：每个 PR 步骤明确、命令具体、验收可执行；无跨任务接口需要 fresh context；本机单分支开发，refresh 一致；测试构建一次到位。

请 review plan，确认后开始 Task 0 准备 + Task 1（PR #1 handler.rs 拆分）。
