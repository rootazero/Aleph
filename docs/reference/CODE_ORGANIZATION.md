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

---

## 7. Refactoring Backlog

Files identified for refactoring, ordered by priority. Each item links to the pattern that should be applied.

### P0 — Critical (职责严重混杂)

| File | Lines | Problem | Pattern |
|------|-------|---------|---------|
| `src/bin/aleph-server/commands/start/mod.rs` | ~800 | Single large function, no structure | Pattern D (refactored to `builder/`) |
| `src/extension/mod.rs` | 1159 | God Object, 46 public methods | Pattern C |

### P1 — High (明显可拆分)

| File | Lines | Problem | Pattern |
|------|-------|---------|---------|
| `src/memory/context/mod.rs` | ~600 | Types now split across `context/` submodules | Pattern B (completed) |
| `src/memory/note_retrieval/mod.rs` | 1808 → 81 | 880-line inherent impl + 875 lines of inline tests | Pattern A + `tests.rs` (completed 2026-08-23) |
| `src/memory/store/sqlite/notes/store_impl.rs` | 2560 | One `impl NoteStore` block, 53 methods | **Blocked — see below** |
| `src/browser/mod.rs` | 1459 | Two unrelated classes: `BrowserService` + `BrowserPool` | Pattern A + `pool.rs` |
| `src/gateway/execution_engine.rs` | 1088 | Two engine implementations, state models mixed in | Pattern A + `types.rs` |

### P2 — Medium (可优化)

| File | Lines | Problem | Pattern |
|------|-------|---------|---------|
| `src/tools/server.rs` | 1034 | `AlephToolServer` and `AlephToolServerHandle` mirror each other | Use `Deref` or macro delegation |
| `src/extension/hooks/mod.rs` | 993 | Similar to `extension/mod.rs` | Pattern C |
| `src/dispatcher/model_router/health/status.rs` | 997 | Health status logic mixed with model routing | Pattern A |

### P3 — Low (轻度优化)

| File | Lines | Problem | Pattern |
|------|-------|---------|---------|
| `src/thinker/prompt_builder/` | ~1200 | `Message`/`MessageRole` belong in `types.rs` | Extract `types.rs` |
| `src/dispatcher/types/unified.rs` | 1003 | 28 `with_*` builder methods bloating `impl UnifiedTool` | Extract `UnifiedToolBuilder` |
| `src/providers/profile_manager.rs` | 1024 | Review for separation of auth vs. profile concerns | TBD after review |

---

## Git Worktree 注意事项

`EnterWorktree` 会在每次 Bash 命令后强制重置 CWD 到 worktree 目录，即使 `cd` 切回主仓库也无效。因此在同一会话内执行 `git worktree remove` 会导致 Shell 永久损坏。**正确做法**：在 `EnterWorktree` 会话内只合并不删除，用新会话清理 worktree；或不用 `EnterWorktree`，手动用绝对路径管理。

---

*Last updated: 2026-02-23. See git log for change history.*
