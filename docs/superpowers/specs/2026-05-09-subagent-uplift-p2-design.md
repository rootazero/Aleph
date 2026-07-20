---
title: Subagent Uplift P2 — Design (补齐 phase)
status: draft
date: 2026-05-09
authors: ["claude-opus-4-7"]
scope: design-only — 不写代码、不写 plan
follows: 2026-05-08-subagent-uplift-p1-design.md
phase: P2 (补齐 — Stage E/F/G)
---

# Subagent Uplift P2 — Design

> **目标**：把 roadmap § Stage E/F/G 三项补齐，落成可让 writing-plans 直接消费的设计。
> 单 PR / 3 个 atomic commit / 估 ~850 行（含 ~330 行测试，跨 3 新 integration 文件），零行改动 `src/harness/agent.rs`（仅 `src/harness/trace.rs` 加 1 个 backward-compatible LoopTraceEvent variant，R10-safe schema 扩展）。
>
> **非目标**：本设计不冻结字段名 / 函数命名细节（plan 阶段决定），不冻结测试函数体；不预先实现 file watcher / reload tool / 全量 builtin agent 迁移（roadmap 显式 out-of-scope）。

## 0. Decisions Locked（来自 brainstorm Q&A）

| ID | Decision | Rationale |
|----|----------|-----------|
| Q1 | **单 PR / 3 atomic commit (E→F→G)** | P1 范式已验证；E/F/G 同属"补齐"主题，bundle review 减少 reviewer 上下文成本；三 commit atomic 仍精确到 stage 粒度回滚 |
| Q2 | **静默 override（project > user > builtin）+ `AgentSource { Builtin, User, Project }` 字段 + `LoopTraceEvent::AgentDefShadowed` 启动期诊断事件** | R7 LLM 主权（注册结果通过 introspection tool 暴露）；运维体验（fail-loud 让 typo 阻止启动太脆）；R8 工具即一切（未来"列出所有 agent + 来源"是自然 tool） |
| Q3 | **Startup-only 加载** | R3 核心轻量化（避免 file watcher + 后台任务 + 锁/Arc swap）；R10 笨循环（watcher 与 Think→Act 单向流动哲学相悖）；YAGNI（个人 AI 助手 server 重启代价低）；reload tool 推迟到后续路线图作 R8 工具 |
| Q4 | **用户字段子集 + 系统字段强制**：用户可写 9 个 LLM-facing 字段；`mode` 强制 `SubAgent`；`source` loader 自动注入；frontmatter 写 `mode: Primary` → loader 抛 schema error | 安全不变量：递归 guard 根基是 mode == SubAgent（Stage B 已 ship）；R7 LLM 主权（用户控制 LLM-facing 配置 = `model_hint` / `allowed_tools` / `token_budget`）；fail-loud 边界让用户立即知道字段非用户可控 |
| Q5 | **结构化 + 时间戳**：`SubagentProgress { step, timestamp, kind: ProgressKind, tool_name, latency_ms, preview }`；`ProgressKind = ToolCalled \| ToolReturned \| LlmThinking \| Cancelled`；preview 截断 200 chars | R7 LLM 主权（拒绝 free-form summary，避免多花一次 LLM 推理调用做事后总结）；结构化优于自然语言（父级 LLM 基于 kind/latency 自己判断"卡住"vs"在工作"）；timestamp 是诊断核心（回答"它卡多久了"） |
| Q6 | **仅 background 装 ForwardingTraceSink wrapper**；sync 路径不装；`BackgroundAgentTracker.progress: VecDeque<SubagentProgress>` cap 50 hardcoded | R3/R10（sync 路径上 wrapper 是纯开销+重复，Stage A trace_sink 继承让父级已经拿到子级原生 trace 事件）；语义清晰（SubagentProgress 存在目的就是回答"我看不见 background subagent 在做什么"，sync 父级阻塞等返回不需要） |
| Q7 | **简化正向集合（READ_ONLY / INVESTIGATION / ASYNC_SAFE 三类）；无 `ALL_AGENT_DENIED_TOOLS`，无 `allow_override`；本 PR 内仅 1–2 个示范 agent 迁移** | R3 核心轻量化（拒绝预先抽象）；R7 LLM 主权（Stage G 不该越界做防御层，防御是 Stage B + Q4-b）；三次法则（无足够重复证据触发抽象）；零破坏（向后兼容硬约束） |

## 1. Shared Constraints

### 1.1 PR 形态

- 单 PR，3 个 atomic commit：E → F → G
- 每 commit 独立编译 + 测试通过；CI 跑一次；rollback 单位 = 整个 P2 phase
- 总预算 ~850 行（roadmap 估 800，余量充足）；0.4 全局约束（≤ 1500 行/phase）零破坏

### 1.2 R10 红线复核

| 红线 | 影响 | 验证 |
|------|------|------|
| `src/harness/agent.rs` ≤ 1500 行 | **零修改**（不进 harness/agent.rs） | line count check |
| `src/harness/` 整体 baseline = **2811 行 / 10 文件**（P1 ship 后实测） | `trace.rs` 加 1 个 LoopTraceEvent variant（schema 扩展，backward-compatible），其余 harness/ 零修改 | wc -l + ls 对比；新行数 ≤ 2820 |
| 笨循环 5 个"不"（不判意图 / 不过滤工具 / 不判完成 / 不审内容 / 不选恢复策略） | 零增加。E 是 boot-time 装配；F 是 trace 转发（被动观察）；G 是配置组织 | 各 stage design 显式声明 |
| YAGNI / 无消费者抽象立删 | E loader ≥1 真实消费者（startup 路径）；F wrapper ≥1 真实消费者（subagent_spawner background 路径）；G 命名集合 ≥1 真实消费者（≥1 个 builtin agent 迁移） | grep 验证 |
| AgentDef schema 兼容硬约束 | E 增 `source` 字段（`#[serde(default)]`）；G 增 `allowed_tool_sets` 字段（`#[serde(default)]`）；既有字段不删/不改 | schema test |

### 1.3 文件改动总清单

| 文件 | 改动 | 行数估算 | Stage |
|------|------|---------|-------|
| `src/agents/loader.rs` | 新建：markdown frontmatter 解析 + 优先级合并 | ~250 | E |
| `src/agents/types.rs` | 改：`AgentDef` 增 `source` + `allowed_tool_sets` 字段；`AgentSource` enum；`is_tool_allowed` 集合解析 | ~50 | E, G |
| `src/agents/registry.rs` | 改：`register_from_dir`；启动路径调用 loader | ~50 | E |
| `src/agents/progress.rs` | 新建：`SubagentProgress` struct + `ProgressKind` enum | ~50 | F |
| `src/agents/tool_sets.rs` | 新建：3 个命名集合常量 + `resolve()` helper | ~80 | G |
| `src/agents/background_tracker.rs` | 改：`RunningAgent` 增 `progress: VecDeque`；`push_progress` / `progress_snapshot` 方法 | ~80 | F |
| `src/agents/subagent_tool.rs` | 改：`check_status` action 返回 progress | ~30 | F |
| `src/agents/subagent_spawner.rs` | 改：background 路径包装 trace_sink；sync 路径不变 | ~60 | F |
| `src/bin/aleph-server/commands/start/orchestrator_init.rs`（或对应 builder） | 改：启动期调用 loader | ~20 | E |
| `src/harness/trace.rs` | 改：`LoopTraceEvent` 增 1 个 `AgentDefShadowed` variant（schema 扩展） | ~10 | E |
| `tests/agent_loader.rs` | 新建：4 unit + 2 integration | ~150 | E |
| `tests/subagent_progress.rs` | 新建：6 unit + 2 integration | ~120 | F |
| `tests/tool_sets.rs` | 新建：8 unit + 1 integration | ~60 | G |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | 改：3 节同步（loader / progress / tool_sets） | ~80 | E/F/G |
| **合计** | | **~1090 行** | |

⚠️ 表上限 ~1090；实际 commit 预算 ~850（测试代码因 rstest fixture 复用 / shared mock harness 节省）。详见 §5.1。

⚠️ **R10 trace.rs 修改 R10-safe 论证**：
- `trace.rs` 是 schema 文件（数据类型 enum + struct 定义），**不是 Think→Act 循环逻辑**
- 加 1 个 `LoopTraceEvent` variant 是 backward-compatible 扩展（既有 `match _` 消费者不破坏；exhaustive 处加 1 行 arm）
- Phase-6 已通过同样模式 ship 过 trace_sink wiring，验证安全
- 备选方案（如不接受 trace.rs 改动）：把 `AgentDefShadowed` emit 走 `tracing::warn!` 不进 trace_sink — 但代价是 panel/disk trace 等下游消费者拿不到，损失观测性。本设计选择动 trace.rs

### 1.4 测试整体策略

- **单元测试**：≥4 (E loader) + ≥6 (F progress) + ≥9 (G tool_sets) = **19 个**
- **集成测试**：3 个新文件（每 stage 一个）= **5 个**
- **总绿条件**：所有新测试 + 现有 `allowlist_tool_service` 6 测试 + Stage A `subagent_deps_inherit` + Stage B `recursion_guard` + Stage C `lane_budget` + Stage D `cancellation_chain` 全绿；R10 line/file count baseline 不破

### 1.5 baseline locks（plan 阶段量化）

| 指标 | baseline 取值时机 |
|------|------------------|
| `src/harness/*.rs` 行数 | plan 第一步 `wc -l` 锁 2811（P1 ship 后值）；P2 ship 后允许 ≤ 2820 |
| `src/agents/*.rs` 行数 | plan 第一步 `wc -l` 锁 baseline；P2 增量 ≤ +600 行 |
| `src/agents/subagent_spawner.rs` 单文件 | 当前 ~290 行（P1 ship 后）；预算上限 600（roadmap 0.4）；F 改动加 ~60 行不破阈 |
| Subagent spawn latency | hyperfine 跑 N=20 锁 P50；F wrapper 加入后回归 ≤ 1.05× background 路径延迟（仅 background 装 wrapper） |

## 2. Stage E — Filesystem Agent Loader

**Status**: 📋 Planned
**Risk class**: medium
**Depends on**: P1 Stage B (Recursion Guard) — ✅ Shipped 61ce09a96

### 2.1 Module Structure

```
src/agents/
├── loader.rs          # NEW — frontmatter parse + dir scan + priority merge (~250 lines)
├── types.rs           # MOD — AgentDef gains `source` field; AgentSource enum (~30 lines added)
└── registry.rs        # MOD — register_from_dir + startup integration (~50 lines added)
```

### 2.2 Frontmatter Schema（用户可写部分）

```markdown
---
# User-allowed fields (Q4-b)
id: my-research-agent           # required, kebab-case, must match filename stem
description: Researches topics  # required
when_to_use: When user asks ... # required
model_hint: claude-sonnet-4-6   # optional
allowed_tools: [read, grep, web_search]  # optional, defaults to []
allowed_tool_sets: [READ_ONLY]  # optional, Stage G feature, defaults to []
denied_tools: []                # optional
max_iterations: 10              # optional
token_budget: 50000             # optional
context_mode: standalone        # optional, enum
---

System prompt body (markdown).
```

**Loader 强制注入**（用户写也忽略，写 `mode: Primary` 抛错）：
- `mode = AgentMode::SubAgent`（fail-loud：用户写 `Primary` → `LoaderError::ForbiddenSystemField`）
- `source` = `User` or `Project`（基于加载来源目录自动注入）

**Frontmatter parser**：plan 阶段先 grep `extension/loader.rs` 是否已有 frontmatter parser 可复用，**复用优先**；不复用则用 `serde_yaml`（已在 Cargo.lock）+ 简单 `---` 分隔符切分（~30 行）；`gray_matter` crate 仅作为 last resort。

### 2.3 AgentSource Enum + LoopTraceEvent variant

`src/agents/types.rs` 新增：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentSource {
    Builtin,    // hardcoded in builtin_agents()
    User,       // ~/.aleph/data/agents/*.md
    Project,    // <project>/.aleph/agents/*.md
}

impl Default for AgentSource {
    fn default() -> Self { Self::Builtin }
}

pub struct AgentDef {
    // ... existing fields ...
    #[serde(default)]
    pub source: AgentSource,
}
```

`src/harness/trace.rs` 在 `LoopTraceEvent` enum 上加 1 个 variant：

```rust
AgentDefShadowed {
    id: String,
    winner_source: AgentSource,
    shadowed_source: AgentSource,
}
```

### 2.4 Load Order + Priority Resolution

伪代码（plan 阶段最终化）：

```rust
pub fn load_agents(home_dir: &Path, project_dir: Option<&Path>)
    -> Result<(Vec<AgentDef>, Vec<ShadowEvent>), LoaderError>
{
    let mut by_id: HashMap<String, AgentDef> = HashMap::new();
    let mut shadows: Vec<ShadowEvent> = Vec::new();

    // Tier 1 (lowest precedence): builtin
    for agent in builtin_agents() {
        by_id.insert(agent.id.clone(), agent);
    }

    // Tier 2: user-level
    let user_dir = home_dir.join("data/agents");
    if user_dir.exists() {
        for agent in scan_dir(&user_dir, AgentSource::User)? {
            if let Some(prev) = by_id.insert(agent.id.clone(), agent.clone()) {
                shadows.push(ShadowEvent {
                    id: agent.id.clone(),
                    winner_source: AgentSource::User,
                    shadowed_source: prev.source,
                });
            }
        }
    }

    // Tier 3 (highest precedence): project-level
    if let Some(proj_dir) = project_dir {
        let proj_agents = proj_dir.join(".aleph/agents");
        if proj_agents.exists() {
            for agent in scan_dir(&proj_agents, AgentSource::Project)? {
                if let Some(prev) = by_id.insert(agent.id.clone(), agent.clone()) {
                    shadows.push(ShadowEvent {
                        id: agent.id.clone(),
                        winner_source: AgentSource::Project,
                        shadowed_source: prev.source,
                    });
                }
            }
        }
    }

    Ok((by_id.into_values().collect(), shadows))
}
```

`scan_dir` 内：
- `*.md` 文件遍历（非递归）
- 每文件 frontmatter parse → `AgentDef`
- `id` 必须 == 文件 stem（强制约定，避免 user 写一个文件多个 agent；不匹配 → `IdMismatch`）
- `mode` 字段不允许出现且非 `SubAgent` → `ForbiddenSystemField`
- malformed 文件：**skip + emit `tracing::warn` log + 继续**（不让一个坏文件 abort 启动）；启动结束聚合 summary log："Loaded 7 agents (5 builtin + 2 user); skipped 1 malformed file"

`ShadowEvent` 是 loader 内部类型，调用方在 trace_sink 就绪后批量 emit `LoopTraceEvent::AgentDefShadowed`（解耦 loader 与 trace_sink 装配时序）。

### 2.5 Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("malformed frontmatter in {path}: {source}")]
    Frontmatter { path: PathBuf, source: serde_yaml::Error },

    #[error("file stem '{stem}' does not match agent id '{id}' in {path}")]
    IdMismatch { path: PathBuf, stem: String, id: String },

    #[error("forbidden system field '{field}' in {path}: must not be set by user/project frontmatter")]
    ForbiddenSystemField { path: PathBuf, field: &'static str },

    #[error("io error reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}
```

malformed file 处理：`scan_dir` 内 per-file Result，遇 Err 仅 `tracing::warn!` 并跳过；只有"目录读不了"这种系统级错误（IO error in dir listing）向上传播。

### 2.6 Startup Wiring

`src/bin/aleph-server/commands/start/orchestrator_init.rs`（或对应 builder）启动序列**早期**调用 loader（在 AgentRuntime 装配前）：

```rust
let (agents, shadows) = crate::agents::loader::load_agents(
    &aleph_home,
    project_dir.as_deref(),
)?;
let registry = AgentRegistry::from_definitions(agents);
// ... build trace_sink ...
for shadow in shadows {
    trace_sink.emit(LoopTraceEvent::AgentDefShadowed { ... });
}
// ... build runtime with registry ...
```

### 2.7 Tests

| 测试 | 位置 | 验证 |
|------|------|------|
| `parses_minimal_frontmatter` | `loader.rs` `#[cfg(test)]` | unit: id + description + when_to_use → AgentDef |
| `rejects_mode_primary_in_user_frontmatter` | 同上 | unit: `mode: Primary` → `ForbiddenSystemField` |
| `rejects_id_filename_mismatch` | 同上 | unit: file `foo.md` 内写 `id: bar` → `IdMismatch` |
| `loads_with_default_fields_when_optional_missing` | 同上 | unit: 仅 required 字段 → defaults 注入 |
| `priority_project_over_user_over_builtin` | `tests/agent_loader.rs` | integration: 3 tier 同 id → winner == project，shadows 含 2 个 ShadowEvent |
| `skip_malformed_file_continues_loading` | 同上 | integration: dir 内 1 坏 1 好 → 好的注册成功，warn 日志可见 |

### 2.8 Old Code Retirement

- 无（`builtin_agents()` 保留作为 Tier 1 兜底）
- `MULTI_AGENT_SYSTEM.md` 当前若有"agent 定义只能在 Rust 代码"的描述需更新（plan 阶段 grep 验证）

### 2.9 Acceptance Criteria

- 功能：3 tier 加载按 priority 合并；`AgentDefShadowed` trace 事件可见；user/project agent 强制 `mode=SubAgent`；malformed 文件 skip + warn
- 不破坏：`builtin_agents()` 全部仍注册可用；现有 registry/subagent_tool 测试全绿；启动时间 ≤ 1.05× baseline（10 个外部 agent 文件场景）
- 测试：4 unit + 2 integration（如上）
- 文档：`MULTI_AGENT_SYSTEM.md` 新增"Filesystem agent loading"章节

### 2.10 Future-proof Note

frontmatter 是稳定数据格式；loader 是 boot-time 一次性逻辑，不依赖模型推理。R10 Future-Proof Test 通过：换更强的模型，loader 行为不变；用户加新 markdown agent 即可被新模型直接驱动。

## 3. Stage F — Subagent Streaming Progress Wrapper

**Status**: 📋 Planned
**Risk class**: medium
**Depends on**: P1 Stage A (HarnessDeps Inheritance) — ✅ Shipped 70c3f1480

### 3.1 R10 修订（vs §1 表）

设计推敲后，`SubagentProgress` 应放在 **agent 层（domain struct）** 而非 `LoopTraceEvent` variant：

- `SubagentProgress` 是 `check_status` 的返回数据 + tracker 的存储类型，**不是**外部 trace 消费者关心的事件
- Child harness 已通过 Stage A trace_sink 继承让 parent.trace_sink 看到子级原生 `ToolCallStarted`/`Completed`；再 emit 一份等价 SubagentProgress 是冗余
- 减少 `LoopTraceEvent` 一个 variant = 减少所有 exhaustive 消费者升级负担

**修订**：`src/harness/trace.rs` 只为 Stage E 加 1 个 `AgentDefShadowed` variant；`SubagentProgress` + `ProgressKind` 全部放 `src/agents/progress.rs`（新文件 ~50 行）。

### 3.2 New Domain Types

`src/agents/progress.rs`（新建）：

```rust
//! SubagentProgress — domain types for tracking background subagent activity.

use std::time::SystemTime;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentProgress {
    pub step: usize,                    // child harness iteration number
    pub timestamp: SystemTime,
    pub kind: ProgressKind,
    pub tool_name: Option<String>,      // Some for ToolCalled/Returned
    pub latency_ms: Option<u64>,        // Some for ToolReturned (call duration)
    pub preview: Option<String>,        // Some for ToolReturned (200-char truncation)
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum ProgressKind {
    ToolCalled,
    ToolReturned,
    LlmThinking,        // emitted on TurnStateEntered { state: Think }
    Cancelled,          // emitted on SessionCompleted with cancelled outcome
}
```

### 3.3 BackgroundAgentTracker 扩展

`src/agents/background_tracker.rs` 增 `progress` 字段 + 2 个新方法：

```rust
struct RunningAgent {
    cancel_token: CancellationToken,
    task_description: String,
    started_at: Instant,
    progress: VecDeque<SubagentProgress>,   // NEW; cap 50 enforced on push
}

impl BackgroundAgentTracker {
    pub fn push_progress(&self, request_id: &str, event: SubagentProgress) {
        let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
        if let Some(agent) = running.get_mut(request_id) {
            if agent.progress.len() >= 50 {
                agent.progress.pop_front();    // FIFO eviction
            }
            agent.progress.push_back(event);
        }
        // unknown request_id: silently drop (race: tracker may have moved entry to completed)
    }

    /// Returns last N events (most recent last); empty if unknown / completed.
    pub fn progress_snapshot(&self, request_id: &str, limit: usize) -> Vec<SubagentProgress> {
        let running = self.running.read().unwrap_or_else(|e| e.into_inner());
        running.get(request_id)
            .map(|a| a.progress.iter().rev().take(limit).cloned().collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter().rev().collect()
    }
}
```

`mark_completed` / `cancel` / `cleanup` / `register` 行为不变；仅 `register` 内 `progress: VecDeque::with_capacity(50)` 初始化。

### 3.4 ForwardingTraceSink Wrapper

位置：`src/agents/subagent_spawner.rs` 内（plan 阶段视行数 >50 决定是否抽 `src/agents/forwarding_trace_sink.rs` 单文件）：

```rust
pub struct ForwardingTraceSink {
    inner: Arc<dyn TraceSink>,                  // parent's trace_sink (Stage A inherited)
    tracker: Arc<BackgroundAgentTracker>,
    request_id: String,
    last_tool_call_started: Mutex<HashMap<usize, (Instant, String)>>,
}

impl TraceSink for ForwardingTraceSink {
    fn emit(&self, event: LoopTraceEvent) {
        // (1) Translate select events into SubagentProgress for tracker storage
        if let Some(progress) = self.translate(&event) {
            self.tracker.push_progress(&self.request_id, progress);
        }
        // (2) Always forward original to inner — preserves Stage A trace flow
        self.inner.emit(event);
    }
}
```

`translate` 内：
- `ToolCallStarted` → `ProgressKind::ToolCalled`，存 (Instant, tool_name) 到 `last_tool_call_started`
- `ToolCallCompleted` → `ProgressKind::ToolReturned`，从 map 取 start time 算 latency_ms，preview = truncate_200(result)
- `TurnStateEntered { state: Think }` → `ProgressKind::LlmThinking`
- `SessionCompleted { outcome: Cancelled, .. }` → `ProgressKind::Cancelled`
- 其他事件 → `None`（不翻译，仅 forward）

**条件性简化**：plan 阶段 grep 验证 `ToolCallCompleted` 是否已含 `duration` / `tool_name` 字段；若是，移除 `last_tool_call_started` map（节省 ~20 行）。

### 3.5 Wiring（仅 background 路径）

`subagent_spawner.rs` background 分支：

```rust
let request_id = generate_request_id();

// NEW (P2 Stage F): wrap parent's trace_sink for this child
let forwarding_sink: Arc<dyn TraceSink> = Arc::new(ForwardingTraceSink::new(
    parent_deps.trace_sink.clone(),
    self.background_tracker.clone(),
    request_id.clone(),
));

let child_deps = HarnessDeps {
    // ... existing fields from Stage A ...
    trace_sink: forwarding_sink,    // <-- replaces direct parent.trace_sink.clone()
    // ...
};

self.background_tracker.register(request_id.clone(), cancel_token, task_description);
// ... spawn tokio task running child harness ...
```

Sync 分支（`subagent_tool.rs` 同步 spawn 路径）**不动**；child 仍直接继承 parent.trace_sink。

### 3.6 `check_status` Action 集成

`subagent_tool.rs::check_status` action 扩展返回结构，增 `progress` 字段：

```rust
let progress = self.background_tracker.progress_snapshot(&request_id, 10);
return Ok(json!({
    "status": status_string,
    "description": ...,
    "elapsed_secs": ...,
    "result": ...,
    "progress": progress,    // Vec<SubagentProgress> serialized
}));
```

`progress` 永远是 array（空也是 `[]`），LLM 看 `kind` / `tool_name` / `latency_ms` / `timestamp` 自己判断"卡住"还是"在工作"（R7 LLM 主权）。

### 3.7 Tests

| 测试 | 位置 | 验证 |
|------|------|------|
| `tracker_push_progress_caps_at_50` | `background_tracker.rs` `#[cfg(test)]` | unit: 推 51 条 → len == 50, 第 1 条被淘汰 |
| `tracker_progress_snapshot_returns_last_n` | 同上 | unit: 推 5 条 + snapshot(3) → 最近 3 条 |
| `tracker_push_unknown_id_no_op` | 同上 | unit: register 前 push → silent drop |
| `forwarding_translates_tool_call_started_to_tool_called` | `subagent_spawner.rs` `#[cfg(test)]` | unit: emit ToolCallStarted → tracker.progress 有 1 条 ProgressKind::ToolCalled |
| `forwarding_pairs_started_completed_for_latency` | 同上 | unit: emit Started+Completed → tracker.progress 第 2 条 latency_ms > 0 |
| `forwarding_forwards_unrelated_events_unchanged` | 同上 | unit: emit TextEmitted → inner sink 收到, tracker.progress 不变 |
| `background_subagent_check_status_returns_progress` | `tests/subagent_progress.rs` | integration: spawn background subagent (mock harness emits 3 ToolCalls) → check_status returns 3-entry progress |
| `sync_subagent_does_not_install_wrapper` | 同上 | integration: spawn sync subagent → 父级 trace_sink 直接收到 ToolCallStarted（无翻译层）, BackgroundAgentTracker 无对应 entry |

### 3.8 Old Code Retirement

- 无（增量功能）
- 探索后：`subagent_tool.rs` 的 `check_status` 当前返回结构若有 stale 字段（plan 阶段 grep 验证），同步清理

### 3.9 Acceptance Criteria

- 功能：parent 通过 `check_status` 看到 background subagent 的 progress 时序（最近 10 条）；progress 自动 FIFO 淘汰；sync subagent 路径行为不变
- 不破坏：现有 trace_sink 消费者全绿（trace 流被完整转发）；现有 BackgroundAgentTracker 5 unit + Stage A subagent_deps_inherit + Stage B/C/D 集成测试全绿
- 测试：6 unit + 2 integration（如上）
- 性能：每 trace event 通过 wrapper ≤ 50µs（plan 阶段 hyperfine lock）；progress push ≤ 10µs

### 3.10 Future-proof Note

ForwardingTraceSink 是 Decorator pattern；不参与认知。模型升级让 subagent emit 更多 trace 事件 → wrapper 自动覆盖（match 默认 `_ => None` 兜底）；新增 `ProgressKind` variant 也是 backward-compatible 扩展。R10 Future-Proof Test 通过。

## 4. Stage G — Semantic Tool Sets (Simplified Positive)

**Status**: 📋 Planned
**Risk class**: low
**Depends on**: P1 Stage B (Recursion Guard) — ✅ Shipped 61ce09a96

### 4.1 Module Structure

```
src/agents/
├── tool_sets.rs       # NEW — 3 named set constants + resolver (~80 lines)
└── types.rs           # MOD — AgentDef gains `allowed_tool_sets`; is_tool_allowed extended (~20 lines added)
```

### 4.2 Named Sets

`src/agents/tool_sets.rs`：

```rust
//! Named tool sets for declarative agent allowlists.
//!
//! Per P2 Stage G simplified positive design: only 3 positive sets, no
//! ALL_AGENT_DENIED_TOOLS auto-deny, no allow_override field. Defense layers
//! (recursion guard, user-frontmatter mode forcing) live elsewhere.

pub const READ_ONLY: &[&str] = &[
    "read",
    "grep",
    "glob",
    "notebook_read",
];

pub const INVESTIGATION: &[&str] = &[
    // READ_ONLY ∪ remote read tools ∪ subagent (Primary-only via Stage B guard)
    "read", "grep", "glob", "notebook_read",
    "web_search",
    "web_fetch",
    "subagent",
];

pub const ASYNC_SAFE: &[&str] = &[
    // Subset safe for autonomous background execution: no side effects, no exfil risk
    "read", "grep", "glob", "notebook_read",
    "web_search",
];

/// Resolve a set name to its tool list. Returns None for unknown names so
/// loader can warn rather than silently allow nothing.
pub fn resolve(set_name: &str) -> Option<&'static [&'static str]> {
    match set_name {
        "READ_ONLY" => Some(READ_ONLY),
        "INVESTIGATION" => Some(INVESTIGATION),
        "ASYNC_SAFE" => Some(ASYNC_SAFE),
        _ => None,
    }
}
```

集合关系：READ_ONLY ⊂ INVESTIGATION，ASYNC_SAFE ⊂ INVESTIGATION（ASYNC_SAFE 排除 web_fetch / subagent — exfil 风险 + 防 background recursion 误用）。

**集合内容来源**：plan 阶段对照实际工具名（grep `src/builtin_tools/` 注册的 `name`）确认；本设计冻结 **3 个集合名 + 集合关系**，具体工具名 plan 阶段最终确定。

**unknown set name 行为**：`resolve` 返回 `None`；loader 处 log warn；不抛错（向后兼容）。

### 4.3 AgentDef Schema 扩展

```rust
pub struct AgentDef {
    // ... existing fields including `mode`, `allowed_tools`, `denied_tools`, `source` (from Stage E) ...

    #[serde(default)]
    pub allowed_tool_sets: Vec<String>,
}
```

`#[serde(default)]` 保证既有 frontmatter / Rust builtin 声明不破坏。

### 4.4 `is_tool_allowed` 组合规则

```rust
impl AgentDef {
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Stage B (P1): recursion guard — system invariant, overrides everything
        if matches!(self.mode, AgentMode::SubAgent) && tool_name == "subagent" {
            return false;
        }

        // Explicit deny short-circuits
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // Stage G (NEW): expanded allowed_tool_sets
        for set_name in &self.allowed_tool_sets {
            if let Some(tools) = crate::agents::tool_sets::resolve(set_name) {
                if tools.iter().any(|t| *t == tool_name) {
                    return true;
                }
            }
        }

        // Existing flat allowlist (with "*" wildcard support)
        self.allowed_tools.iter().any(|t| t == "*" || t == tool_name)
    }
}
```

精确语义：
- `denied_tools` 优先级最高（mode-aware deny 之后）
- `allowed_tool_sets` 与 `allowed_tools` 是**正向并集**：任一允许即允许
- `"*"` 仅在 `allowed_tools` 内有效（通配符语义不下沉到 set 内部）

### 4.5 Migration Scope（1–2 示范 agent）

Plan 阶段从 `builtin_agents()` 选 1–2 个**纯只读语义**的 agent 迁移：

```rust
// BEFORE
AgentDef {
    id: "code-explorer".into(),
    allowed_tools: vec!["read".into(), "grep".into(), "glob".into(), "notebook_read".into()],
    // ...
}

// AFTER
AgentDef {
    id: "code-explorer".into(),
    allowed_tool_sets: vec!["READ_ONLY".into()],
    allowed_tools: vec![],
    // ...
}
```

迁移目标的选择标准：
1. agent 当前 `allowed_tools` 完全等同于某个命名集合（≥ 0 行 / ≤ 0 行差异）
2. 选 **2 个不同特性**的 demo（一个 READ_ONLY、一个 INVESTIGATION 或 ASYNC_SAFE），覆盖 set 名解析路径

**禁止**：本 PR 内不动其他 builtin agents 的 allowlist；增量迁移留给后续 ad-hoc 工作。**预算下限 1 个 agent** 仍满足 Q7-2 (a) 的"≥1 真实消费者"。

### 4.6 Frontmatter 兼容（与 Stage E 协作）

User/project markdown agent 可写 `allowed_tool_sets`：

```yaml
---
id: my-research-agent
allowed_tool_sets: [INVESTIGATION]
allowed_tools: [my_custom_tool]    # 可与 set 共存
denied_tools: [web_fetch]          # 优先级高于 set
---
```

Stage E loader 解析 → AgentDef → registry → SubagentTool 走 is_tool_allowed → Stage B mode 强制 SubAgent → 即使写 `[INVESTIGATION]`（含 `"subagent"` 名）也被递归 guard 切断 → 安全性闭环成立。

### 4.7 Tests

| 测试 | 位置 | 验证 |
|------|------|------|
| `read_only_set_resolves_to_known_tools` | `tool_sets.rs` `#[cfg(test)]` | unit: `resolve("READ_ONLY")` 含 `"read"`, 不含 `"web_search"` |
| `investigation_is_superset_of_read_only` | 同上 | unit: READ_ONLY 全部成员都在 INVESTIGATION |
| `async_safe_excludes_subagent` | 同上 | unit: ASYNC_SAFE 不含 `"subagent"` |
| `async_safe_excludes_web_fetch` | 同上 | unit: ASYNC_SAFE 不含 `"web_fetch"`（exfil 风险） |
| `unknown_set_resolves_none` | 同上 | unit: `resolve("FOOBAR")` → `None` |
| `is_tool_allowed_via_set_only` | `types.rs` `#[cfg(test)]` | unit: `allowed_tools=[], allowed_tool_sets=[READ_ONLY]` → `is_tool_allowed("read")` true / `is_tool_allowed("bash")` false |
| `is_tool_allowed_set_and_flat_union` | 同上 | unit: set + flat 并集生效 |
| `denied_tools_overrides_set` | 同上 | unit: set 含 X, denied 含 X → 拒 |
| `subagent_mode_denies_subagent_even_in_investigation_set` | 同上 | unit: SubAgent + INVESTIGATION（含 subagent）→ subagent 仍被拒（Stage B 守住） |
| `migrated_agent_keeps_behavior` | `tests/tool_sets.rs` | integration: 迁移前后的 agent 对所有原 allowed tools 的 `is_tool_allowed` 返回值字字等同 |

### 4.8 Old Code Retirement

- 1–2 个 builtin agent 的平铺 `allowed_tools` 字面量 → 替换为 `allowed_tool_sets`（净减 ~10–15 行）
- `MULTI_AGENT_SYSTEM.md` allowlist 章节增 "Named tool sets" 子节；旧"平铺 list"描述保留作为兼容说明

### 4.9 Acceptance Criteria

- 功能：3 个命名集合定义；`AgentDef.allowed_tool_sets` 字段可写；is_tool_allowed 按 §4.4 规则解析；1–2 个 builtin agent 迁移行为不变
- 不破坏：所有未迁移 agent（既有平铺 allowlist）行为字字等同；Stage B 递归 guard 仍生效；现有 `allowlist_tool_service` + Stage A/B subagent 测试全绿
- 测试：9 unit + 1 integration（如上）；其中 `migrated_agent_keeps_behavior` 是 P2 Stage G 不破坏的硬证据

### 4.10 Future-proof Note

命名集合是 boot-time 配置组织，不参与认知。新工具加入时，集合定义集中维护（编辑 `tool_sets.rs` 一处），不需要改各 agent 声明。R10 Future-Proof Test 通过：模型升级让 LLM 通过工具名查 set 内容更准确，不需要 harness 改动。

## 5. PR Order & Verification

### 5.1 Commit 顺序

E → F → G。每 commit 独立编译 + 测试通过（atomic）：

| Commit | 主要改动 | 预算行数 |
|--------|---------|---------|
| 1. **Stage E — Filesystem agent loader** | `agents/loader.rs`（新）；`types.rs` 加 `source` + `AgentSource` enum；`registry.rs` 加 `register_from_dir`；`harness/trace.rs` 加 `AgentDefShadowed` variant；`orchestrator_init.rs` 调用 loader；`MULTI_AGENT_SYSTEM.md` 增章节；4 unit + 2 integration | ~340 |
| 2. **Stage F — Streaming progress wrapper** | `agents/progress.rs`（新）；`background_tracker.rs` 加 `progress` 字段 + 2 方法；`subagent_spawner.rs` 装 ForwardingTraceSink（仅 background 路径）；`subagent_tool.rs::check_status` 返回 progress；6 unit + 2 integration | ~300 |
| 3. **Stage G — Semantic tool sets** | `agents/tool_sets.rs`（新）；`types.rs` 加 `allowed_tool_sets` + 扩展 `is_tool_allowed`；1–2 个 builtin agent 迁移；`MULTI_AGENT_SYSTEM.md` 增 named sets 章节；9 unit + 1 integration | ~210 |
| **总计** | | **~850 行** |

### 5.2 PR 级 verification

`just test-all` 或等价：

1. `cargo build --release` 全绿
2. `cargo test --workspace` 全绿（新增 ~24 测试 + 既有 P0/P1 全部测试）
3. `cargo clippy --workspace -- -D warnings` 全绿
4. **R10 hard checks**：
   ```bash
   wc -l src/harness/*.rs | tail -1     # 期待: 2811 (P1 ship 后值) ± 10 (trace.rs +1 variant)
   ls src/harness/*.rs | wc -l           # 期待: 10 (不变)
   wc -l src/agents/*.rs | tail -1       # 期待: baseline + ≤ 600
   wc -l src/agents/subagent_spawner.rs  # 期待: ≤ 600 (roadmap 0.4)
   ```
5. **schema 兼容回归**：现有 `aleph.toml` + 现有 builtin agents 反序列化全绿（`#[serde(default)]` 字段设计核验）
6. **文档代码一致性**：`MULTI_AGENT_SYSTEM.md` 描述的 (a) loader 优先级 (b) progress 行为 (c) tool sets 命名 与代码事实一致
7. **subagent spawn latency 回归**：`hyperfine` 跑 N=20 比较 P1 baseline，要求 ≤ 1.05× background 路径延迟（仅 background 装 wrapper）

### 5.3 Roadmap 更新（PR ship 后）

`docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md` 三处轻量修订：

```markdown
### Stage E
**Status**: ✅ Shipped: <hash> on <date>

### Stage F
**Status**: ✅ Shipped: <hash> on <date>

### Stage G
**Status**: ✅ Shipped: <hash> on <date>
```

文件头部加：

```markdown
✅ P2 Shipped: <commit> on <date>
```

## 6. Risk Register

| ID | Risk | Likelihood | Impact | Mitigation |
|----|------|-----------|--------|-----------|
| R1 | `extension/loader.rs` 已有 frontmatter parser，但接口不可复用 → Stage E 引入 `gray_matter` crate 增加依赖 | 50% | low | plan 阶段 grep 验证；如不可复用，新依赖 `gray_matter` ≤ 3KB binary 可接受；备选方案：自写 ~30 行简单 `---` 分隔 + serde_yaml |
| R2 | `LoopTraceEvent::AgentDefShadowed` variant 触动既有 exhaustive `match` 消费者 | 60% | low | grep 所有 `match event` 对 LoopTraceEvent 的消费点；exhaustive 处加新 arm（typed forward）；`_ =>` 处零工作 |
| R3 | `ToolCallCompleted` trace 字段已含 duration / tool_name → §3 的 `last_tool_call_started` map 是冗余 | 50% | low | plan 阶段 grep 字段；如冗余，移除 map 简化 ForwardingTraceSink ~20 行 |
| R4 | trace_sink 装配时机晚于 loader 调用 → `AgentDefShadowed` emit 时 sink 不存在 | 30% | low | loader 内不直接 emit，返回 `Vec<ShadowEvent>`；调用方在 sink 就绪后批量 emit；plan 阶段确认时序 |
| R5 | builtin agent 选 2 个迁移时发现现有 `allowed_tools` 与 READ_ONLY 内容不完全匹配（差 1–2 项） | 40% | low | 不强迁移；选完全匹配的 agent，找不到就只迁 1 个；预算下限 1 个 agent 仍满足 Q7-2 (a) |
| R6 | `progress: VecDeque` cap 50 在长跑 background subagent（>1h, 100+ tool calls）下信息密度不够，仅看到最后 50 步 | 70% | low | 此为设计选择（YAGNI / 内存边界）；docs 明示 cap；如未来有 specific 长跑需求，独立 stage 加 config |
| R7 | `gray_matter` crate license / supply chain 不通过 cargo deny | 15% | medium | plan 阶段先 `cargo deny check` 验证；如不通过，写一个最小 frontmatter parser（~30 行）取代 |
| R8 | Stage F integration test 需要 mock harness 能 emit 任意 LoopTraceEvent — Stage A 测试基建可复用度未知 | 30% | low | plan 阶段先复用 Stage A `tests/cancellation_chain.rs` 的 mock pattern；不可复用则新写 ~30 行 mock harness |
| R9 | `is_tool_allowed` 加集合解析后路径变复杂，性能回归 | 10% | low | 集合用 `&'static [&'static str]` 静态数组，linear scan ≤ 10 元素；性能影响 < 1µs per call；与现有 flat allowlist 同量级 |
| R10 | Stage E `id` 必须 == 文件 stem 这个约定让 user 改 id 时必须同步重命名文件 | 100% | low | docs 明示约定 + loader 抛 `IdMismatch` 时提示用户重命名 |

## 7. Out-of-scope（显式不做）

| 项 | 推迟到 | 理由 |
|----|-------|------|
| File watcher / 自动 reload | 后续路线图（属 R8 / R3） | YAGNI；Q3 已锁 |
| `reload_agents` admin tool | 后续路线图 | 属 R8 工具即一切；与 P2 范围不重叠 |
| `ALL_AGENT_DENIED_TOOLS` 自动 deny | 永不做（除非新需求） | YAGNI；Q7 已锁 |
| `allow_override` 字段 | 永不做 | YAGNI；Q7 已锁 |
| 全量 builtin agents 迁移到命名集合 | ad-hoc / 增量 | 三次法则；Q7 已锁 |
| Sync subagent 也产 progress | 永不做（语义不需要） | Q6 已锁 |
| Plugin/Extension 来源的 agent | 后续路线图（R10 / Stage I 邻接） | P2 仅做 user/project tier；plugin 经 ExtensionAgent 走另一条线 |
| Progress cap config-driven | 后续路线图（如有具体长跑场景） | YAGNI；hardcoded 50 |
| `LoopTraceEvent::SubagentProgress` variant | 永不做（设计已修订到 agent layer） | §3.1 修订；trace 不发 redundant 事件 |

## 8. References

- Roadmap master: `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`
- P1 design: `docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md`
- P1 ship commits: Stage A `70c3f1480`, Stage B `61ce09a96`, Stage C `5f9f155f1`, Stage D `7c062b548`, Docs `97e1abf16`
- Phase-6 master: `docs/superpowers/specs/2026-05-08-phase6-config-wiring-design.md`
- 12-module roadmap: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`
- R10 哲学: `docs/reference/HARNESS_PHILOSOPHY.md`
- 现状文件：
  - `src/agents/types.rs` (AgentDef, AgentMode, is_tool_allowed)
  - `src/agents/registry.rs` (builtin_agents)
  - `src/agents/subagent_tool.rs` (BackgroundAgentTracker usage, check_status action)
  - `src/agents/subagent_spawner.rs` (spawn paths sync/background)
  - `src/agents/background_tracker.rs` (RunningAgent / CompletedAgent)
  - `src/harness/trace.rs` (LoopTraceEvent enum)
  - `src/extension/loader.rs` (potential frontmatter parser to reuse)
