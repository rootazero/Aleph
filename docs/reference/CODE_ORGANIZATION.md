# Code Organization Guide

> When a single file grows too large, it becomes a wall — you can see it, but you can't see through it.

This guide establishes the principles and patterns for organizing Rust code in Aleph. It complements [DESIGN_PATTERNS.md](DESIGN_PATTERNS.md) (which covers API ergonomics) by focusing on **file and module structure**.

---

## Table of Contents

- [1. Core Principles](#1-core-principles)
- [2. File Naming Conventions](#2-file-naming-conventions)
- [3. When to Split a File](#3-when-to-split-a-file)
- [4. Standard Module Patterns](#4-standard-module-patterns)
- [5. Anti-Patterns (Real Examples)](#5-anti-patterns-real-examples)
- [6. Reference Examples](#6-reference-examples)
- [7. Refactoring Backlog](#7-refactoring-backlog)

---

## 1. Core Principles

### Single Responsibility

Each file owns exactly one concept. A file that defines a type should not also implement its persistence. A file that implements a manager should not also define the types it manages.

Ask yourself: *"If I describe this file in one sentence, do I need the word 'and'?"* If yes, it needs to be split.

### Separation of Concerns

Split along these natural fault lines:

| Concern | Where it lives |
|---------|---------------|
| Type definitions (struct/enum) | `types.rs` or `model.rs` |
| Trait implementations (conversion, display) | Separate `impl` blocks or `impl_xxx.rs` |
| Business logic | The main module file |
| External integrations (DB, network) | `store.rs`, `executor.rs`, or named adapter files |
| Test doubles | `mock.rs` inside `#[cfg(test)]` or `tests/` |

### Visibility Minimization

Use `pub(crate)` for cross-module access instead of `pub`. Reserve `pub` for public API surface. Internal implementation details should be `pub(super)` or private.

---

## 2. File Naming Conventions

These names carry semantic meaning across the codebase. Use them consistently:

| Filename | Contents | Notes |
|----------|----------|-------|
| `mod.rs` | Module entry point, re-exports | Keep thin; avoid business logic here |
| `types.rs` | Enums and value objects | No methods beyond `Display`/`FromStr`/`Default` |
| `model.rs` | Aggregate roots and entities | Core domain model with field definitions |
| `pool.rs` | Connection pools, resource pools | Lifecycle management for shared resources |
| `factory.rs` | Constructor functions, builder types | `create_*` functions, `*Builder` structs |
| `executor.rs` | Execution logic calling external systems | Tools, plugins, shell commands |
| `registry.rs` | Lookup and query logic | Read-heavy access to registered components |
| `callback.rs` | Event handlers and hook implementations | Responses to lifecycle events |
| `mock.rs` | Test doubles (Mock, Stub, Fake) | Always gated under `#[cfg(test)]` |
| `error.rs` | Error type definitions | Domain-specific error enums |

---

## 3. When to Split a File

### Hard Triggers (must split)

- **Line count ≥ 500** and the file contains more than one logical concept
- **Multiple `impl Trait for T` blocks** for different traits on different types in one file
- **Production code mixed with test doubles** — `MockFoo` living next to `Foo`
- **God Object** — a single struct with 20+ public methods spanning unrelated concerns

### Soft Triggers (should consider splitting)

- A single `impl` block exceeds 300 lines
- A function exceeds 100 lines
- The file's `use` imports span more than 3 unrelated modules

### How to Decide What to Extract

1. **Identify clusters**: Group all definitions by "which concept does this belong to?"
2. **Find the seam**: Look for the boundary where two clusters only interact through a narrow interface
3. **Name the new file**: If you can't name it using the conventions in §2, the split boundary is wrong

---

## 4. Standard Module Patterns

### Pattern A: Single-Struct Module

For modules centered on one struct with supporting types.

```
my_module/
├── mod.rs          # MyStruct definition + core impl
├── types.rs        # Enums and value objects used by MyStruct
└── error.rs        # MyModuleError enum
```

**Example**: `dispatcher/executor/` — `Executor` struct with `ExecutorConfig` and `ExecutorError`.

### Pattern B: Domain Model Module

For modules with rich domain models (DDD aggregate roots).

```
memory/
├── mod.rs          # Module entry, re-exports
├── types.rs        # Enums: MemoryLayer, MemoryCategory, FactType, etc.
├── model.rs        # MemoryFact (AggregateRoot), CompressionSession
├── anchor.rs       # ContextAnchor, MemoryEntry (query/search structures)
└── store/          # Persistence implementations
    └── lance/
        └── facts.rs
```

**Use when**: The module has 3+ enums AND a core aggregate root type.

### Pattern C: Manager / God Object Split

For large manager structs with many public methods spanning distinct responsibilities.

```
extension/
├── mod.rs           # ExtensionManager (thin facade, delegates to sub-components)
├── executor.rs      # PluginExecutor — tool/hook/command execution
├── registry.rs      # SkillRegistry — skill/command/agent lookup
├── controller.rs    # ServiceController — start/stop/status of services
├── loader.rs        # Plugin loading and discovery
└── types.rs         # ExtensionConfig, LoadSummary
```

**Rule**: `ExtensionManager` holds `Arc<PluginExecutor>`, `Arc<SkillRegistry>`, `Arc<ServiceController>` and delegates. Its own `impl` block should have fewer than 10 methods.

**Use when**: A struct has 15+ public methods spanning 3+ unrelated concerns.

### Pattern D: Startup / Builder Split

For complex initialization sequences (the "flat script" anti-pattern).

```
bin/aleph_server/commands/
├── start.rs         # Entry point: parse args, call ServerBuilder::build().run()
└── builder/
    ├── mod.rs       # ServerBuilder struct definition
    ├── providers.rs # initialize_providers()
    ├── tools.rs     # initialize_tools()
    ├── gateway.rs   # initialize_gateway()
    ├── channels.rs  # initialize_channels()
    └── config.rs    # setup_config_watcher()
```

**Rule**: The `start` function should be fewer than 50 lines — just argument parsing and `Builder::new().build()?.run().await`.

**Use when**: An initialization function exceeds 200 lines or initializes 4+ independent subsystems.

---

## 5. Anti-Patterns (Real Examples)

### Anti-Pattern 1: The God Object

**File**: `extension/mod.rs` (1159 lines, 46 public methods)

```rust
// ❌ One struct doing everything
impl ExtensionManager {
    // Lifecycle
    pub async fn load_all(&self) { ... }
    pub async fn reload(&self, name: &str) { ... }
    // Skill execution
    pub async fn execute_skill(&self, ...) { ... }
    pub async fn invoke_skill_tool(&self, ...) { ... }
    // Service management
    pub async fn start_service(&self, name: &str) { ... }
    pub async fn stop_service(&self, name: &str) { ... }
    pub fn get_service_status(&self, name: &str) { ... }
    // Plugin execution
    pub async fn call_plugin_tool(&self, ...) { ... }
    pub async fn execute_plugin_hook(&self, ...) { ... }
    // MCP configuration
    pub fn get_mcp_servers(&self) { ... }
    // ... 36 more methods
}
```

**Problem**: Skills, services, plugins, and MCP are independent concerns. A change to service management requires touching the same file as skill execution.

**Fix**: Apply Pattern C — `ExtensionManager` becomes a facade delegating to `PluginExecutor`, `ServiceController`, and `SkillRegistry`.

---

### Anti-Pattern 2: The Flat Script

**File**: `src/bin/aleph-server/commands/start/mod.rs`

```rust
// ❌ One function doing 700 lines of initialization
pub async fn start_server(args: StartArgs) -> Result<()> {
    // 50 lines: provider initialization
    // 80 lines: session manager setup
    // 120 lines: channel registry
    // 90 lines: agent registry
    // 110 lines: tool registration
    // 150 lines: WebSocket binding
    // 60 lines: PID file handling
    // 40 lines: signal handling
    // ... continues for 710 lines
}
```

**Problem**: Impossible to test subsystems in isolation. A change to tool registration risks breaking signal handling.

**Fix**: Apply Pattern D — `ServerBuilder` in `src/bin/aleph-server/commands/start/builder/` where each subsystem has its own initialization method.

---

### Anti-Pattern 3: The Type Dumping Ground

**File**: `memory/context.rs` (1302 lines, 14 top-level types, 31+ impl blocks)

```rust
// ❌ All types in one file
pub enum FactType { ... }         // classification enum
pub enum MemoryLayer { ... }      // classification enum
pub enum MemoryCategory { ... }   // classification enum
pub struct MemoryFact { ... }     // aggregate root
pub struct CompressionSession { } // domain model
pub struct ContextAnchor { ... }  // query structure
pub struct FactStats { ... }      // statistics
// Each enum has: impl Display + impl FromStr + impl Default
// That's 8 × 3 = 24 impl blocks just for enums
```

**Problem**: `MemoryFact` (the aggregate root) is buried among 13 other types. Finding where to add business logic requires reading through enum boilerplate.

**Fix**: Apply Pattern B — `types.rs` for the 6 classification enums, `model.rs` for `MemoryFact` + `CompressionSession`, `anchor.rs` for `ContextAnchor` + `MemoryEntry`.

---

## 6. Reference Examples

### Good: `memory/store/sqlite/mod.rs`

Despite the line count, this file has **one job**: implement `MemoryStore` for SQLite backend.

```
Schema management (table creation, migrations)
  └── init_schema, migrate_v1_to_v2, ...

impl MemoryStore for SqliteMemoryBackend
  └── store, retrieve, search, delete trait methods

Private helpers
  └── query builders, batch operations, connection pooling
```

**Why it works**: Every line of code serves the same trait implementation. The private functions are helpers for those implementations, not unrelated utilities. A new developer reading this file has one question to answer: *"how does SQLite store memories?"*

**When high line count is acceptable**: When a file implements a well-defined interface (a trait or protocol) and the complexity comes from the depth of that implementation, not from breadth of concerns.

**When the language sets the price** (`store_impl.rs`, 2,560 production lines).
Rust requires one `impl Trait for Type` to be a single block, so a 53-method
trait implementation cannot be split across files the way an inherent impl can.
Every way of doing it anyway costs a layer that buys nothing on its own:

- 53 delegating wrappers (`lock; call a free fn in a topic module`) — the SQL
  becomes unit-testable against a bare `Connection`, at the price of a second
  name for every method;
- splitting `NoteStore` itself into `NoteIndexStore` / `NoteLinkStore` /
  `NoteVectorStore` / `NoteGraphStore` / `NoteGovernanceStore` — architecturally
  the right shape (P5, interface minimisation) and the only one with no wrapper
  layer, but it touches 25 `dyn NoteStore` consumers and every test mock.

Neither is a mechanical move, so neither belongs in a change whose point is that
it changes no behaviour. **Recorded as a declared leftover on 2026-08-23**, with
the sub-trait split as the preferred shape when it is taken. Compare
`note_retrieval/`, split the same day for free: those are *inherent* methods, so
the type simply gained more `impl` blocks and the only cost was `pub(super)`
where a stage calls another stage.

#### Round 1 newly-declared leftovers (2026-09-22)

The Round-1 split pass surveyed every file ≥3000 lines; most were either
splittable or already decomposed. The following ten are kept whole — the
"why" matches the `store_impl.rs` shape above (single concept whose depth
the language forbids mechanically extracting). See
[`docs/superpowers/specs/2026-09-22-large-file-split-design.md`](../superpowers/specs/2026-09-22-large-file-split-design.md)
§5.2 for the full table; key entries below.

#### `src/bin/aleph-server/commands/start/mod.rs` (4067 行)

**保留理由**：文件本身 L58-63 明确写「`start_server` is a single ~2270-line monolithic bootstrap sequence. Its hundreds of locals are threaded through sequential phases and consumed at the tail, so the body cannot be split into helper fns without changing data flow」。顶层已有 5 个子模块 (`builder` / `orchestrator_init` / `helpers` / `runtime_warmup` / `bootstrap_factories`) 拆出约 6300 行，剩余主体不可拆。

**何时重新评估**：若作者本人重构 L58-63 描述的 body 为多 fn，先重打开本任务。

#### `src/extension/mod.rs` (1884 行)

**保留理由**：已 27 个 .rs 文件 + 6 子目录（manifest/marketplace/registrar/registry/runtime/types）；4 个 `impl ExtensionManager` 块已拆到 plugin_ops / service_ops / skill_ops / projection — 这就是 Pattern C 想做的事，只是文件名不同。`loader.rs` / `registry/` / `types/` 名字已被私有模块占用，强行重命名代价远超收益。

**何时重新评估**：若需新增跨现有 plugin_ops/service_ops/skill_ops/projection 的 facade 层。

#### `src/extension/hooks/mod.rs` (1278 行)

**保留理由**：6 个 sibling 子模块已拆（executor 1538 / consent 823 / json_output 349 / output_budget 402 / user_settings 614）；mod.rs 是 partial decomposition（5 types + 12 pub fn + 5 impl）；不是 thin facade。

**何时重新评估**：若 `executor.rs`（现 1538 行，比 mod.rs 还大）需要进一步拆分为 rail / clipboard / tool 三子文件。

#### `src/gateway/handlers/agent.rs` (3657 行)

**保留理由**：1300 行 production + **2357 行 `mod tests`**。用户裁定「测试不拆」使纯 Pattern A 拆分仅是 relocation（test 从 agent.rs 抽出到 agent/tests.rs）；相对路径迁移需手工修改 50+ 处「super::super::」引用，估计需 touch 20+ tests；原始 hybrid split 代价超过收益。

**何时重新评估**：若 tests 模块迁出到独立 `agent/tests.rs` 后收益>代价，或允许拆分 tests 子模块。

#### `src/browser/page_state/fetch_chromium.rs` (7236 行)

**保留理由**：单 capture 算术（DOMSnapshot + 跨源 stitch）；source_scan 守卫锁定 RawDom 构造位置。

**潜在风险**：若强行拆为 stitch.rs + capture.rs，会让 `only_the_page_state_fetchers_construct_a_raw_dom` 守卫定位漂移。

**何时重新评估**：lines 突破 10000；或 Rust 加 `impl` 跨文件语法。

#### `src/executor/builtin_registry/definitions.rs` (4115 行)

**保留理由**：纯 catalog 数据；模块文档禁描述字面重复。

**何时重新评估**：若 catalog 拆为多源（plugins 注入条目）且每个源都有独立测试。

#### `src/gateway/event_visibility.rs` (3686 行)

**保留理由**：SessionOwnership 与 EventVisibilityIndex 互锁；2 impl 服务同一概念。

**何时重新评估**：若 SessionIdentity 衍生第二概念。

#### `src/builtin_tools/desktop/native.rs` (3665 行)

**保留理由**：平台实现深度；macOS 一份，Linux/Windows 平行。

**何时重新评估**：Linux/Windows 也达到同等规模。

#### `src/capability/census.rs` (3540 行)

**保留理由**：单一推导规则 + 11 个 guard tests；source_scan relative path 强耦合。

**何时重新评估**：若 guard tests 拆为多文件测试。

#### `src/memory/dreaming/mod.rs` (3464 行)

**保留理由**：13 个兄弟子文件已就位（含 stages/、evolution/）；mod.rs 仅 orchestration。

**何时重新评估**：orchestration 突破 5000 行。

---

## 7. Refactoring Backlog

Files identified for refactoring, ordered by priority. Each item links to the pattern that should be applied.

### Round 1 (2026-09-22) outcomes

Completed: `src/gateway/server/handler.rs` (PR #1, commit `68f3290f`), `src/builtin_tools/workflow_tool.rs` (PR #4, commit `ab617384b`), `src/gateway/session_projector.rs` (PR #6, commit `3a9c882ae`).

Reclassified as declared leftovers (see §6): `start/mod.rs` (4067), `extension/mod.rs` (1884), `extension/hooks/mod.rs` (1278), `handlers/agent.rs` (3657).

### P0 — Critical (职责严重混杂)

| File | Lines | Problem | Pattern | Status |
|------|-------|---------|---------|--------|
| `src/gateway/server/handler.rs` | 3527 | 5 lifecycle phases + 0 types | Pattern D | ✅ Round 1 |
| `src/bin/aleph-server/commands/start/mod.rs` | 4067 | Single large function, no structure | Pattern D (→ declared leftover §6) | ❌→§6 |
| `src/extension/mod.rs` | 1884 | God Object, 46 public methods | Pattern C (→ declared leftover §6) | ❌→§6 |

### P1 — High (明显可拆分)

| File | Lines | Problem | Pattern | Status |
|------|-------|---------|---------|--------|
| `src/memory/context/mod.rs` | ~600 | Types now split across `context/` submodules | Pattern B | ✅ prior round |
| `src/memory/note_retrieval/mod.rs` | 1808 → 81 | 880-line inherent impl + 875 lines of inline tests | Pattern A + `tests.rs` | ✅ 2026-08-23 |
| `src/memory/store/sqlite/notes/store_impl.rs` | 2571 | One `impl NoteStore` block, 53 methods | **Blocked (declared leftover §6)** | ❌→§6 |
| `src/browser/mod.rs` | 244 (post-split) | Two unrelated classes: `BrowserService` + `BrowserPool` | Pattern A + `pool.rs` | ✅ prior round |
| `src/gateway/execution_engine.rs` | 1088 → 40 files | Two engine implementations, state models mixed in | Pattern A + `types.rs` | ✅ prior round |
| `src/gateway/handlers/agent.rs` | 3657 | 1300 production + 2357 tests; tests stay whole | Pattern A (→ declared leftover §6) | ❌→§6 |

### P2 — Medium (可优化)

| File | Lines | Problem | Pattern | Status |
|------|-------|---------|---------|--------|
| `src/tools/server.rs` | (renamed/removed) | `AlephToolServer` and `AlephToolServerHandle` mirror each other | Use `Deref` or macro delegation | ✅ prior round |
| `src/extension/hooks/mod.rs` | 1278 | Partial decomposition (→ declared leftover §6) | Pattern C | ❌→§6 |
| `src/builtin_tools/workflow_tool.rs` | 5100 | 6 DTO + 4 impl + PhaseTally + main | Pattern A | ✅ Round 1 (commit `ab617384b`) |
| `src/gateway/session_projector.rs` | 3359 | 3 struct + 4 impl + main | Pattern A | ✅ Round 1 (commit `3a9c882ae`) |

### P3 — Low (轻度优化)

| File | Lines | Problem | Pattern | Status |
|------|-------|---------|---------|--------|
| `src/thinker/prompt_builder/` | ~1200 → 1570 | `Message`/`MessageRole` belong in `types.rs` | Extract `types.rs` | partially done |
| `src/dispatcher/types/unified.rs` | (renamed/removed) | 28 `with_*` builder methods bloating `impl UnifiedTool` | Extract `UnifiedToolBuilder` | ✅ prior round |
| `src/providers/profile_manager.rs` | (renamed/removed) | Review for separation of auth vs. profile concerns | TBD after review | ✅ prior round |

---

## Git Worktree 注意事项

`EnterWorktree` 会在每次 Bash 命令后强制重置 CWD 到 worktree 目录，即使 `cd` 切回主仓库也无效。因此在同一会话内执行 `git worktree remove` 会导致 Shell 永久损坏。**正确做法**：在 `EnterWorktree` 会话内只合并不删除，用新会话清理 worktree；或不用 `EnterWorktree`，手动用绝对路径管理。

---

*Last updated: 2026-09-22 (Round 1 split: handler.rs / workflow_tool.rs / session_projector.rs + 4 new declared leftovers). See git log for change history.*
