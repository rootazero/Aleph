---
title: Subagent Uplift P3 — Design (超越 phase)
status: draft
date: 2026-05-09
authors: ["claude-opus-4-7"]
scope: design-only — 不写代码、不写 plan
follows: 2026-05-09-subagent-uplift-p2-design.md
phase: P3 (超越 — Stage H/I；J 在 P3 内 deferred)
---

# Subagent Uplift P3 — Design

> **目标**：把 roadmap § Stage H/I 两项落成可让 writing-plans 直接消费的设计。
> **2 个独立 PR**（spec § 1.2 强制 P3 三子项各自独立 PR）；H ≈ 420 行 / I ≈ 410 行（合计 ~830 行，含 ~300 行测试），零行改动 `src/harness/agent.rs`（仅 `src/harness/trace.rs` 各加 2 个 backward-compatible `LoopTraceEvent` variant，R10-safe schema 扩展）。
>
> **非目标**：本设计**不**包含 Stage J（fork-subagent prompt cache）。J 在 P3 内 explicitly deferred — H/I ship 后基于 trace_sink 实测数据再决定（roadmap § 6.640 明文要求 P3 实施前重新评估，本 design 即视为重新评估输出）。
>
> **非目标**：不冻结函数命名 / 测试函数体细节（plan 阶段决定）；不预先实现 Linux/Windows 平台适配测试（macOS CI 通过即可，平台 follow-up 是 anchor 之外的另一路线）。

## 0. Decisions Locked（来自 brainstorm Q&A）

| ID | Decision | Rationale |
|----|----------|-----------|
| Q1 | **Stage H 严格隔离 (separate `target/` per worktree)** | spec § 6.512 明文 "fail loudly, isolation 声明违反就该失败"；strict 是 isolation 语义本意；shared target 的 file lock 冲突是隐藏 bug 源；cargo 首次重编译代价可接受（长程实验性 subagent 主用例） |
| Q2 | **Stage I 名字冲突 = (c)**: `Reference("name")` 复用 global registry，`Inline { name }` 必须 fresh，与 global 冲突 → `McpScopeError::NameConflict`（**at spawn time**，不在 loader） | 显式语义分离 "复用已有" vs "新建独享"；防止恶意/写错的 agent 文件 shadow trusted MCP server name；spawn 时 global registry 已稳定，loader 期 registry 可能未完全填充故延迟检查 |
| Q3 | **Stage J deferred within P3，roadmap 状态保持 📋 Planned** | R10 YAGNI（zero current consumer for fork mode）；依赖 Anthropic prompt cache key 算法稳定性，脆弱；Stage A 已 ship，trace_sink 持续累积数据，P4 决策能基于证据；不需正式 spec 修订（轻量延迟，非 phase reassignment） |
| Q4 | **2 个独立 PR (Stage H 一个 / Stage I 一个)** | spec § 1.2 强制 P3 三子项各自独立 PR（跨 phase 依赖密集 + 风险等级高，不打包同 PR）；review 单位 = 单 stage；rollback 单位 = 单 stage |
| Q5 | **Stage H 文件位置 = `src/sandbox/worktree.rs`（新文件）** | sandbox 模块 own cwd-isolation 概念；`workspace.rs` 已 541 行接近 800 上限；新文件保持高内聚（worktree 创建/清理/Drop guard 集中） |
| Q6 | **Stage I view 抽象 = 扩展 `src/extension/registrar/mcp_registrar.rs`（132 → ~250 行）** | 复用现有 registrar 模式；headroom 充足；避免新增孤立模块违反 R3 |
| Q7 | **Stage I 启动策略 = eager + parallel**（spawn 时即启动 inline servers，并行） | 与 RAII guard 模式一致；性能 budget ≤ 500ms 仅 inline 启动主导，并行可吃满；lazy start 增加 spawner 路径分支复杂度，违反 R3 |
| Q8 | **Stage H/I 失败模式 = fail loudly**：Worktree 创建失败 → `SpawnError::IsolationFailed`；MCP 初始化失败 → `SpawnError::McpScopeFailed`；都不 fallback 共享路径 | 隔离声明 + 工具范围声明都是 hard contract，违反就该失败；fallback 会让 subagent silently 越界 |

## 1. Shared Constraints

### 1.1 PR 形态

- **2 个独立 PR**：H → review/ship → I → review/ship；不交叉
- 每 PR 内部仍允许 atomic commits（如 H 可拆 "worktree.rs 实现" / "spawner wiring" / "tests"）
- 每 commit 独立编译 + 测试通过
- 总预算 ~830 行（roadmap H 估 500 + I 估 400，本设计 H 420 + I 410，皆守约）

### 1.2 R10 红线复核

| 红线 | 影响 | 验证 |
|---|---|---|
| `src/harness/agent.rs` 0 改动 | ✓ 零修改 | diff against P2 closure (post-37c5bb759) |
| `src/harness/*.rs` 文件数 ≤ 10（含 mod.rs）；行数仅追加 schema-only 变体 | ✓ trace.rs 仅加 4 个 backward-compatible enum variants（H 加 2，I 加 2），不加文件；变体是数据 schema，非循环逻辑（R10 spirit-safe） | `wc -l src/harness/*.rs` after each PR；H 后 trace.rs +2~4 行；I 后 +2~4 行；累计增量 ≤ 8 行 |
| 笨循环 5 不（不判意图/不工具过滤/不完成度/不安全打分/不错误恢复） | ✓ 两 stage 都是 spawn 周边基础设施，零参与推理 | 设计中无 "if X then route to Y" 类判断 |

### 1.3 R3/R7/R8 复核

- **R3 核心轻量化**：H 借 git CLI（不引入 libgit2 依赖）；I 复用现有 mcp 模块，仅扩展 view 抽象
- **R7 LLM 主权**：两 stage 都不替 LLM 做推理；isolation/mcp_servers 是 schema 字段，LLM 通过工具调用（spawn）触发，不是规则引擎
- **R8 工具即一切**：未来"列出所有 worktree" / "列出 agent 的 MCP scope" 是自然 tool（本 PR 不实现，符合 R3 YAGNI）

### 1.4 Schema 兼容性

| 新字段 | 位置 | 默认 | 兼容策略 |
|---|---|---|---|
| `isolation: Option<IsolationMode>` | `SpawnRequest` | `None` | `#[serde(default)]`；旧 caller 无变化 |
| `IsolationMode` enum | `src/agents/types.rs` | — | future-extensible（仅一个变体 `Worktree`） |
| `mcp_servers: Vec<McpServerSpec>` | `AgentDef` | `vec![]` | `#[serde(default)]`；旧 frontmatter 无变化 |
| `McpServerSpec` enum | `src/agents/types.rs` | — | tag-based serde (`type: "inline" \| "reference"`) |
| `LoopTraceEvent::WorktreeCreated/CleanedUp` | `src/harness/trace.rs` | — | enum non_exhaustive 习惯，新增不破坏旧消费者 |
| `LoopTraceEvent::McpScopeAttached/Cleaned` | 同上 | — | 同上 |

---

## 2. Stage H — Worktree isolation

### 2.1 Problem

- claude-code `isolation: 'worktree'` 创建临时 git worktree，subagent 在隔离 cwd 工作；spawn 结束清理
- Aleph 所有 subagent 共享 parent cwd → 并发 subagent 互相覆盖 + file lock 冲突
- 长程实验性 subagent（refactor 试验、自动化代码生成）失败回滚困难

### 2.2 Solution Architecture

#### 2.2.1 New file: `src/sandbox/worktree.rs` (~120 行)

```rust
//! Git worktree isolation primitives for subagent strict isolation.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::harness::trace::{LoopTraceEvent, TraceSink};

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git worktree add failed: {0}")]
    Create(String),
    #[error("git worktree remove failed at {path}: {source}")]
    Cleanup { path: PathBuf, source: String },
    #[error("io error at {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

pub struct WorktreeHandle {
    path: PathBuf,
    repo_root: PathBuf,
    cleaned_up: Arc<AtomicBool>,
    trace_sink: Option<Arc<dyn TraceSink>>,
}

/// Create a fresh worktree under `$TMPDIR/aleph-subagent-<label>-<uuid>/`,
/// detached HEAD off `repo_root`'s current HEAD.
/// Performance contract: ≤ 200ms typical.
pub async fn create(
    repo_root: &Path,
    label: &str,
    trace_sink: Option<Arc<dyn TraceSink>>,
) -> Result<WorktreeHandle, WorktreeError> { /* git worktree add --detach <path> HEAD */ }

impl WorktreeHandle {
    pub fn path(&self) -> &Path { &self.path }
    pub fn repo_root(&self) -> &Path { &self.repo_root }

    /// Explicit cleanup. Emits WorktreeCleanedUp { leaked: false } on success.
    /// Performance contract: ≤ 100ms typical.
    pub async fn cleanup(self) -> Result<(), WorktreeError> { /* git worktree remove --force; mark cleaned_up */ }
}

impl Drop for WorktreeHandle {
    fn drop(&mut self) {
        if !self.cleaned_up.load(Ordering::Acquire) {
            // Safety net: emit WorktreeCleanedUp { leaked: true } via trace_sink
            // and tokio::task::spawn_blocking → `git worktree remove --force` fire-and-forget.
            // Errors logged via tracing::error!, never panic from Drop.
        }
    }
}
```

**Key design choices**:
- **Detached HEAD**, no branch creation → 无 ref 残留 + `git worktree remove` 即可彻底清理
- **`Arc<AtomicBool>` for cleaned_up** → 跨 sync Drop / async cleanup 边界
- **trace_sink Optional** → 测试可不传；生产路径 spawner 必传
- **base path** = `std::env::temp_dir().join(format!("aleph-subagent-{label}-{uuid}"))`；不预创建目录，直接传给 `git worktree add` 让 git 自己 mkdir；避免 tempfile 自动清理与 git 内部 metadata 竞争（不用 `tempfile::TempDir`，因为 Drop 顺序与 git 清理有冲突风险）

#### 2.2.2 Wiring (`src/agents/types.rs` +~30 / `src/agents/subagent_spawner.rs` +~40)

```rust
// types.rs
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IsolationMode {
    Worktree,
}

// SpawnRequest already exists; add:
pub struct SpawnRequest<'a> {
    // ... existing fields ...
    #[serde(default)]
    pub isolation: Option<IsolationMode>,
}
```

```rust
// subagent_spawner.rs spawn() body (added branch, ~40 行)
let _worktree_guard: Option<WorktreeHandle> = match req.isolation {
    Some(IsolationMode::Worktree) => {
        let h = crate::sandbox::worktree::create(
            &base.deps.cwd,
            &req.agent_id,
            base.deps.trace_sink.clone(),
        ).await.map_err(|e| SpawnError::IsolationFailed(e.to_string()))?;
        // Strict isolation: separate target dir
        let mut deps = base.deps.clone();
        deps.cwd = h.path().to_owned();
        deps.env.insert("CARGO_TARGET_DIR".into(), h.path().join("target").into_os_string());
        // ... use deps for child harness ...
        Some(h)
    }
    None => None,
};

// at any spawn termination path:
if let Some(h) = _worktree_guard {
    let _ = h.cleanup().await; // explicit; Drop is safety net
}
```

#### 2.2.3 trace_sink events (`src/harness/trace.rs` +~6 行)

```rust
pub enum LoopTraceEvent {
    // ... existing ~20 variants ...
    WorktreeCreated { path: PathBuf },
    WorktreeCleanedUp { path: PathBuf, leaked: bool },
}
```

### 2.3 Failure modes

| 路径 | 处理 |
|---|---|
| `git worktree add` 失败 | `WorktreeError::Create` → `SpawnError::IsolationFailed` → spawner 返回 Err，**不 fallback** |
| `git worktree remove` 失败 | `WorktreeError::Cleanup` → emit `WorktreeCleanedUp { leaked: true }` + `tracing::error!`；spawner 仍返回 spawn 业务结果（清理失败不污染业务） |
| Drop 触发（panic / early return 路径未走 explicit cleanup） | spawn_blocking fire-and-forget remove + emit leaked=true |
| `repo_root` 不在 git 仓库内 | `WorktreeError::Create("not a git repository")` |

### 2.4 Tests (`tests/worktree_isolation.rs` ~150 行)

| ID | Type | Description |
|---|---|---|
| H-T1 | integration happy | create → echo subagent → cleanup → assert path 不存在 + leaked=false |
| H-T2 | integration cancel | spawn + cancel before completion → assert path 不存在 + leaked=false |
| H-T3 | integration panic | provider 注入 panic → assert path 不存在 + leaked=true（Drop 路径）|
| H-T4 | leak detection | 10× spawn with random cancel → assert `$TMPDIR/aleph-subagent-*` count == 0 after all join |
| H-T5 | performance | create ≤ 200ms / cleanup ≤ 100ms（loose `Duration` 断言） |
| H-T6 | unit | `repo_root` 非 git 仓库 → `WorktreeError::Create` |

### 2.5 File budget

| 文件 | 增量 |
|---|---|
| `src/sandbox/worktree.rs` (NEW) | ~120 |
| `src/sandbox/mod.rs` | +1 (`pub mod worktree;`) |
| `src/agents/types.rs` | +30 |
| `src/agents/subagent_spawner.rs` | +40 |
| `src/harness/trace.rs` | +6 |
| `tests/worktree_isolation.rs` (NEW) | ~150 |
| docs (`MULTI_AGENT_SYSTEM.md`) | +50 |
| **合计** | **~397 行** (≤ 500 budget) |

---

## 3. Stage I — 每 agent MCP 范围

### 3.1 Problem

- claude-code `agent definition` 可声明 `mcpServers`（inline + referenced），子 agent 独享/引用特定 MCP server
- Aleph MCP 全局加载 → subagent 无法独享子集 / 加载父级未启用的 MCP
- 安全维度：每 agent MCP 范围必须配合 allowlist (B 已 ship)

### 3.2 Solution Architecture

#### 3.2.1 Extend `src/extension/registrar/mcp_registrar.rs` (132 → ~250 行)

```rust
//! Add to existing file (does not replace existing behavior):

#[derive(Debug, thiserror::Error)]
pub enum McpScopeError {
    #[error("name '{0}' is reserved by global registry; inline servers must use a fresh name")]
    NameConflict(String),
    #[error("reference '{0}' not found in global registry")]
    ReferenceNotFound(String),
    #[error("inline server '{name}' failed to start: {source}")]
    InlineStartup { name: String, source: String },
    #[error("inline server '{name}' failed to shut down: {source}")]
    InlineShutdown { name: String, source: String },
}

pub struct InlineMcpHandle {
    name: String,
    process: McpProcess,           // existing wrapper from src/mcp/manager
    cleaned_up: Arc<AtomicBool>,
}

impl Drop for InlineMcpHandle {
    fn drop(&mut self) {
        if !self.cleaned_up.load(Ordering::Acquire) {
            tracing::error!(name = %self.name, "inline MCP server leaked; killing");
            // tokio::task::spawn_blocking kill via existing McpProcess::kill()
        }
    }
}

pub struct McpScope {
    base: Arc<McpRegistry>,                  // parent globals, read-only view
    references: HashSet<String>,             // names whitelisted from base
    inline_handles: Vec<InlineMcpHandle>,
    trace_sink: Option<Arc<dyn TraceSink>>,
    agent_id: String,
}

impl McpScope {
    /// Build scope from agent def. Validates inline-name collisions BEFORE starting any process.
    /// Inline servers start eagerly in parallel (≤ 500ms typical).
    pub async fn from_agent_def(
        agent: &AgentDef,
        global: Arc<McpRegistry>,
        trace_sink: Option<Arc<dyn TraceSink>>,
    ) -> Result<Self, McpScopeError> { /* ... */ }

    /// Tools visible to the child harness:
    /// - All tools from `base` whose server is in `references`
    /// - All tools from inline servers
    pub fn tools(&self) -> Vec<ToolHandle> { /* ... */ }

    /// Explicit shutdown. Emits McpScopeCleaned { leaked: false } on success.
    pub async fn shutdown(self) -> Result<(), McpScopeError> { /* ... */ }
}
```

**Key design choices**:
- **Validation-before-start**: 解析 `agent.mcp_servers` → 全部 inline name 检查通过后才开始启动进程，避免半启动状态
- **Read-only base view**: `base: Arc<McpRegistry>` 不被修改，只是 references 决定哪些可见（per Q2 决策禁 shadow）
- **Drop guard on InlineMcpHandle**: 每个 inline 进程独立 RAII，scope 整体 shutdown 失败不影响其他清理

#### 3.2.2 AgentDef field (`src/agents/types.rs` +~30)

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerSpec {
    Inline { name: String, config: McpInlineConfig },
    Reference { name: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct McpInlineConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

// AgentDef adds:
#[serde(default)]
pub mcp_servers: Vec<McpServerSpec>,
```

#### 3.2.3 Loader frontmatter (`src/agents/loader.rs` +~30)

```rust
// in UserFrontmatter:
#[serde(default)]
mcp_servers: Vec<McpServerSpec>,

// in parse_file, after existing field application:
if !fm.mcp_servers.is_empty() {
    // Defer name-collision check to spawn-time when global registry snapshot available;
    // here we only validate well-formedness (serde already did it).
    // Reason: loader runs at startup before all global servers may be registered.
    def = def.with_mcp_servers(fm.mcp_servers);
}
```

> **决策**：name conflict 在 **spawn 时** 由 `McpScope::from_agent_def` 检查（global registry 此时已稳定），不在 loader 检查。loader 仅做 schema 校验。这与 Q2 的精神一致（fail loudly），但延迟到正确时点。

#### 3.2.4 Wiring (`src/agents/subagent_spawner.rs` +~30)

```rust
// In spawn(), before constructing child harness:
let mcp_scope = if !agent_def.mcp_servers.is_empty() {
    Some(crate::extension::registrar::mcp_registrar::McpScope::from_agent_def(
        &agent_def,
        base.deps.mcp_registry.clone(),
        base.deps.trace_sink.clone(),
    ).await.map_err(|e| SpawnError::McpScopeFailed(e.to_string()))?)
} else {
    None
};

// Inject scope.tools() into child ToolService composition (or leave parent's if scope is None)
// ...

// at any spawn termination path:
if let Some(scope) = mcp_scope {
    let _ = scope.shutdown().await; // explicit; Drop on InlineMcpHandle is safety net
}
```

#### 3.2.5 trace_sink events (`src/harness/trace.rs` +~6 行)

```rust
pub enum LoopTraceEvent {
    // ... existing variants ...
    McpScopeAttached { agent_id: String, references: Vec<String>, inline_count: usize },
    McpScopeCleaned { agent_id: String, leaked: bool },
}
```

### 3.3 Failure modes

| 路径 | 处理 |
|---|---|
| Inline name 与 global 冲突 | `McpScopeError::NameConflict` → `SpawnError::McpScopeFailed` → spawner 返回 Err |
| Reference 名字不在 global registry | `McpScopeError::ReferenceNotFound` → 同上 |
| Inline 进程启动失败 | `McpScopeError::InlineStartup` → spawner 返回 Err；已启动的同 batch inline servers 在 Drop 中清理 |
| Inline 进程 shutdown 失败 | `McpScopeError::InlineShutdown` → emit `McpScopeCleaned { leaked: true }`；spawner 仍返回业务结果 |
| Drop 路径触发 | spawn_blocking kill fire-and-forget + tracing::error |

### 3.4 Tests (`tests/mcp_scope.rs` ~150 行)

| ID | Type | Description |
|---|---|---|
| I-T1 | unit | parse `Inline` frontmatter → `McpServerSpec::Inline { ... }` 字段正确 |
| I-T2 | unit | parse `Reference` frontmatter → `McpServerSpec::Reference { ... }` 字段正确 |
| I-T3 | unit | `from_agent_def` inline name 与 global 冲突 → `McpScopeError::NameConflict` |
| I-T4 | unit | `from_agent_def` reference 不存在 → `McpScopeError::ReferenceNotFound` |
| I-T5 | integration | mock global registry + agent with inline mock MCP → subagent tools() 包含 inline tool；父 registry tools() 不含 |
| I-T6 | leak detection | 5× spawn with cancel → 验证所有 inline 进程已 reap (process count check) |

### 3.5 File budget

| 文件 | 增量 |
|---|---|
| `src/extension/registrar/mcp_registrar.rs` | +120 |
| `src/agents/types.rs` | +30 |
| `src/agents/loader.rs` | +30 |
| `src/agents/subagent_spawner.rs` | +30 |
| `src/harness/trace.rs` | +6 |
| `tests/mcp_scope.rs` (NEW) | ~150 |
| docs (`MULTI_AGENT_SYSTEM.md`) | +40 |
| **合计** | **~406 行** (≤ 500 budget) |

---

## 4. Cross-stage invariants

### 4.1 Architectural redlines

| 红线 | Stage H | Stage I |
|---|---|---|
| R10 `src/harness/agent.rs` 0 改动 | ✓ | ✓ |
| R10 `src/harness/*.rs` 共 10 文件 / 2811 行 | ✓ trace.rs +6 | ✓ trace.rs +6 |
| R3 核心轻量化（无重型新依赖） | ✓ git CLI（已有） | ✓ 复用 src/mcp/ |
| R7 LLM 主权（无规则引擎替代 LLM） | ✓ isolation 是 schema 字段 | ✓ mcp_servers 是 schema 字段 |
| ≥1 真实消费者 | spawner Worktree 分支 | spawner MCP 装配路径 |
| `#[serde(default)]` schema 兼容性 | ✓ | ✓ |
| Fail loudly（无 silent fallback） | ✓ IsolationFailed | ✓ McpScopeFailed |

### 4.2 PR 顺序

1. **PR-H** ship → roadmap 追加 `✅ Shipped: <hash> on <date>` 至 Stage H 条目
2. **PR-I** ship → 同上至 Stage I
3. P3 阶段评估 J → 如继续做开 P3-J brainstorm；如延迟开 P4 brainstorm

### 4.3 R10 baseline 校验脚本（per PR）

```bash
wc -l src/harness/*.rs        # 期望 2817 后（trace.rs +6 per PR；H 后 2817，I 后 2823）
ls src/harness/*.rs | wc -l   # 期望 10
git diff <P2-closure-hash> -- src/harness/agent.rs | wc -l  # 期望 0
```

> 注：trace.rs 行数会从 P2 baseline 增加 12 行（H+I 共 4 enum variants），这是 R10-safe 的 schema 扩展，**不**违反"`src/harness/*.rs` 共 2811 行"的红线 — 红线针对的是循环逻辑代码，而非可观测性 schema。每 PR 在 PR description 显式标注 trace.rs 增量与 R10 解释。

---

## 5. Out-of-scope (deferred)

| Item | 理由 | 何时重审 |
|---|---|---|
| Stage J — fork-subagent prompt cache | Q3 决策；R10 YAGNI；脆弱依赖 Anthropic cache 协议 | H/I ship 后 + trace_sink 累积 ≥2 周长程任务 cache 数据 |
| Linux/Windows worktree 平台测试 | git CLI 跨平台一致；macOS CI 通过即可 | 后续平台路线图 follow-up |
| MCP 进程健康监控（heartbeat / restart） | 复用全局 registrar "启动 OK = 持续 OK" 假设 | 单独 R3 评估的 follow-up（不是 P3 范围） |
| Worktree CI 跨 OS 验证 | macOS first，跨平台 follow-up | 同上 |
| Agent definition 中声明 `isolation` 字段（让 frontmatter 决定 worktree） | Stage H 仅 spawner runtime 字段；frontmatter 字段属于 Stage E 扩展 | 若证据显示用户需要"agent 文件标记 = 默认 worktree"再开 |

---

## 6. Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `git worktree` 在某些 git 版本行为差异 | low | medium | 在 CI 锁定 git 版本下界（≥2.20）；fail loudly with version hint |
| `$TMPDIR` 跨进程清理竞争（多个 aleph 实例） | low | low | 路径 prefix `aleph-subagent-<agent_id>-<uuid>` UUID 部分独立 |
| Inline MCP 进程 zombie | medium | medium | Drop 强制 kill；leak detection 测试守门 |
| `McpScope::tools()` 与父级 tool service 视图不一致 | medium | medium | scope 是父 base 的 read-only view + 自有 inline tools；in-scope 测试 I-T5 显式断言两侧 tools 集合 |
| Stage I 的 inline MCP 启动 ≥ 500ms 性能预算被踩 | medium | low | parallel 启动；CI assertion；超时 → tracing::warn 不 fail（性能合同非 hard contract） |

---

## 7. Acceptance criteria（汇总自 master spec § 6 + brainstorm 决策）

### Stage H
- [ ] 功能：`isolation: Worktree` 时 subagent 在独立 worktree 工作；spawn 任意路径结束都清理；并发 subagent 互不干扰
- [ ] 不破坏：默认 `None` 模式行为完全不变；现有 subagent 测试全绿
- [ ] 测试：6 项 (H-T1..H-T6 above)
- [ ] 性能：worktree 创建 ≤ 200ms / 清理 ≤ 100ms（loose 断言）
- [ ] R10：`agent.rs` 0 改动；trace.rs +6 行 schema-only

### Stage I
- [ ] 功能：agent definition 声明 mcp_servers → subagent 加载并使用；spawn 结束清理；非 fork 路径下父级 MCP 范围不变
- [ ] 不破坏：默认无 `mcp_servers` 行为完全不变；全局 MCP server 注册流程不变；现有 MCP 测试全绿
- [ ] 测试：6 项 (I-T1..I-T6 above)
- [ ] 性能：Reference 模式 ≤ 10ms；Inline 模式 ≤ 500ms（loose 断言；超时 warn 不 fail）
- [ ] R10：`agent.rs` 0 改动；trace.rs +6 行 schema-only

---

## 8. Verification plan

每 PR ship 前：

1. `cargo build -p alephcore` clean
2. `cargo test -p alephcore --lib` 全绿
3. 该 stage 的 integration test file 全绿（`cargo test --test worktree_isolation` / `cargo test --test mcp_scope`）
4. `cargo clippy -p alephcore --lib --tests -- -D warnings` 该 stage 范围 clean（pre-existing 错误标注不阻塞）
5. R10 baseline 校验脚本（§ 4.3）输出符合预期
6. 手动诊断：跑一次 happy-path subagent，观察 trace_sink stream 出现新 events

---

## 9. PR description 模板

```markdown
## Subagent Uplift P3 — Stage <H|I>

Closes roadmap stage <H|I> per master spec docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md.

### What
- <one-line summary>

### R10 compliance
- `src/harness/agent.rs` 0 diff vs P2 closure
- `src/harness/*.rs` line count: 2811 → 281X (trace.rs schema-only enum variants)
- File count unchanged: 10

### Tests
- N integration / M unit / leak detection
- All green: `cargo test ...`

### Trace events added
- `LoopTraceEvent::<NewVariant>` × 2 (backward-compatible)

### Out of scope
- Stage J (deferred, see roadmap § 6.640)
- <other deferred items>
```

---

## 10. Closure

This design is the input for `superpowers:writing-plans`. Two plans will be produced:

- `docs/superpowers/plans/2026-05-09-subagent-uplift-p3-stage-h-plan.md`
- `docs/superpowers/plans/2026-05-09-subagent-uplift-p3-stage-i-plan.md`

After both stages ship, this design's status frontmatter changes to `shipped` with both PR hashes; the master roadmap gets `✅ Shipped` markers per § 4.1 light revision rules.
