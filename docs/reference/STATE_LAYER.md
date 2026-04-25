# State Layer Reference

> Living description of how `src/resilience/` actually works as of 2026-04-25.
> Read this before proposing to rename, relocate, or split the module.

## 1. Misleading Name, Living Code

The directory `src/resilience/` no longer hosts any "resilience" middleware.
Its self-description in `src/resilience/mod.rs:1-5` says:

```
//! Resilience Module — Database and Core Types
//!
//! Governance, collaboration, perception, and recovery middleware have been
//! removed as part of the agent loop migration. Only the database layer
//! (StateDatabase) and shared types remain.
```

This is true (governance / collaboration / perception / recovery middleware
were deleted by the P0 agent-loop dissolution) but **misleading**: "Only
… remain" suggests leftover deletable code, while the actual remainder is
**5,031 LOC of live code** powering 28 consumer files across 8 top-level
domains. The roadmap's original P7 exit-artifact "delete gutted
`src/resilience/`" was retracted on 2026-04-25 (see roadmap footnote ⁷).

The directory name is preserved for now — renaming would touch 28 consumers'
`use` statements without solving any current problem.

## 2. ApplicationRecord Pattern

`StateDatabase` (defined at `src/resilience/database/state_database/mod.rs:20`)
is a single-connection multi-tenant SQLite Repository — the Rust equivalent of
Rails' `ApplicationRecord` or Django's shared `Connection`.

**Shape:**

- 1 × `Arc<Mutex<Connection>>` — owned by the struct, shared across all
  domains via interior mutability
- Registers the `sqlite-vec` extension at construction time
  (`DEFAULT_EMBEDDING_DIM = 1024`)
- 6 public methods on the struct itself (`new`, `in_memory`, `new_with_dim`,
  `serialize_embedding`, `store_sticker_description`,
  `load_sticker_description`)
- 11 private `mod xxx` submodules each attach methods to the same struct
  via `impl StateDatabase { ... }`:

| Submodule | LOC | Domain |
|---|---|---|
| `events.rs` | 245 | Agent events |
| `memory_events.rs` | 635 | Memory event sourcing |
| `tasks.rs` | 233 | Agent tasks |
| `sessions.rs` | 211 | Sessions |
| `group_chat.rs` | 177 | Group chat sessions |
| `traces.rs` | 292 | Task traces (Shadow Replay) |
| `paired_users.rs` | 60 | Telegram paired users |
| `channel_offsets.rs` | 53 | Telegram offsets |
| `replay.rs` | 18 | Replay helper |
| `state_database/schema.rs` | 426 | DDL |
| sticker_descriptions (in `state_database/mod.rs`) | — | Sticker storage |

The only **public** submodule is `pub mod migration` (defined in
`src/resilience/database/migration.rs`, 626 LOC) — it owns the three
`add_*` migrations (channel_offsets, paired_users, sticker_descriptions)
called once during binary boot.

**Encapsulation is leak-free:** all 28 consumer files import
`crate::resilience::StateDatabase` (the pub re-export from
`src/resilience/mod.rs:15`). A `grep` for
`use crate::resilience::database::events` (or any other submodule path)
returns **zero matches**. The God-object surface is the boundary; the
internal partitioning is invisible to callers.

**Why this shape is correct:**

- SQLite best practice: a single `Arc<Mutex<Connection>>` avoids
  multi-writer lock contention on the same DB file
  (`~/.aleph/data/state.db`)
- Strong encapsulation: callers cannot bypass the struct to reach
  individual tables
- Refactor-friendly: adding a new domain = add a new private `mod`,
  attach `impl` methods, no consumer change

## 3. Consumer Distribution (28 files / 8 domains)

| Domain | File count | % | Notable consumers |
|---|---|---|---|
| `gateway/` | 9 | 33% | execution_engine, handlers, Telegram interface |
| `memory/` | 7 | 25% | events, integration tests, transcript_indexer |
| `executor/` | 4 | 14% | builtin_registry (builder/config/definitions/registry) |
| `bin/aleph-server/` | 3 | 11% | start/builder agent_init + handlers + subsystems |
| `group_chat/` | 2 | 7% | executor, orchestrator |
| `arena/` | 1 | 4% | storage |
| `lib.rs` | 1 | 4% | crate-level re-export |
| `sync_primitives.rs` | 1 | 4% | shared `Arc`/`Mutex` helper |

**Total: 28 distinct files**, all entering through the single pub type.

## 4. Cross-Domain Types (`src/resilience/types.rs`, 682 LOC)

The `types.rs` module hosts crate-wide vocabulary types. Their consumer
counts (number of distinct files using each type, excluding
`src/resilience/` itself):

| Type | Consumers | Use |
|---|---|---|
| `RiskLevel` | 24 files | Task risk classification (most cross-domain) |
| `Lane` | 10 files | Task execution lane |
| `AgentEvent` | 10 files | Agent event persistence record |
| `TaskStatus` | 9 files | Task status enum (Pending/Running/Completed/Failed/Interrupted) |
| `SessionStatus` | 9 files | Session lifecycle status |
| `SubagentSession` | 3 files | Long-lived subagent session record |
| `AgentTask` | 2 files | Task struct (state + recovery checkpoint) |
| `TaskTrace` | 1 file | Near-dead-code (single consumer) |

These are **de facto crate-wide vocabulary**: physically migrating them
into a different directory would force 24+ consumer-file edits per type
with zero architectural improvement.

## 5. References

- Roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
  §3.3 row 8 + §4.2 P7 row + footnote ⁷
- P7 design: `docs/superpowers/specs/2026-04-25-p7-state-layer-design.md`
- StateDatabase struct: `src/resilience/database/state_database/mod.rs:20`
- Misleading mod.rs comment: `src/resilience/mod.rs:1-5`
- Pub re-export: `src/resilience/mod.rs:15`
- Migration entry: `src/resilience/database/migration.rs`
- Cross-domain types: `src/resilience/types.rs`
