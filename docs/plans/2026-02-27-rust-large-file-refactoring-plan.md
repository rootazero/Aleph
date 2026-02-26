# Rust Large File Refactoring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split 5 Rust files (each >1000 lines) into clean sub-modules without changing any external behavior.

**Architecture:** Pure module splitting — move code into sub-files, add `pub use` re-exports in `mod.rs` to maintain existing import paths. Each file becomes a directory module. Delete confirmed dead code along the way.

**Tech Stack:** Rust module system (mod.rs / directory modules), `pub(crate)` visibility, `pub use` re-exports.

**Design Document:** `docs/plans/2026-02-27-rust-large-file-refactoring-design.md`

---

## Task 1: Split thinker/prompt_builder.rs (1,939 lines)

**Files:**
- Delete: `core/src/thinker/prompt_builder.rs`
- Create: `core/src/thinker/prompt_builder/mod.rs`
- Create: `core/src/thinker/prompt_builder/sections.rs`
- Create: `core/src/thinker/prompt_builder/messages.rs`
- Create: `core/src/thinker/prompt_builder/cache.rs`
- Create: `core/src/thinker/prompt_builder/tests/mod.rs`
- Create: `core/src/thinker/prompt_builder/tests/build_tests.rs`
- Create: `core/src/thinker/prompt_builder/tests/section_tests.rs`
- Create: `core/src/thinker/prompt_builder/tests/sanitize_tests.rs`

**Step 1: Create directory and mod.rs**

Create `core/src/thinker/prompt_builder/` directory. Move the following into `mod.rs`:
- `use` imports (lines 6-15)
- `SystemPromptPart` struct (lines 22-28)
- `PromptConfig` struct + Default impl (lines 31-82)
- `PromptBuilder` struct (lines 84-88)
- `impl PromptBuilder` with ONLY these methods:
  - `new()` (lines 92-95)
  - `build_system_prompt()` (lines 98-101)
  - `build_system_prompt_with_hydration()` (lines 229-232)
  - `build_system_prompt_with_soul()` (lines 655-658)
  - `build_system_prompt_with_hooks()` (lines 664-692)
  - `build_system_prompt_with_context()` (lines 911-914)
- Sub-module declarations: `mod sections; mod messages; mod cache;`
- `#[cfg(test)] mod tests;`

Re-export everything that was previously public:
```rust
pub use messages::{Message, MessageRole};
```

**Step 2: Create sections.rs**

Move all public `append_*` methods as a separate `impl PromptBuilder` block:
- `append_runtime_context_section()` (lines 127-134)
- `append_hydrated_tools()` (lines 185-222)
- `append_soul_section()` (lines 567-649)
- `append_environment_contract()` (lines 702-740)
- `append_security_constraints()` (lines 748-805)
- `append_silent_behavior()` (lines 812-826)
- `append_protocol_tokens()` (lines 832-841)
- `append_operational_guidelines()` (lines 847-890)
- `append_safety_constitution()` (lines 366-387)
- `append_memory_guidance()` (lines 394-414)
- `append_soul_continuity()` (lines 420-429)
- `append_citation_standards()` (lines 895-903)
- `append_channel_behavior()` (lines 1053-1060)
- `append_user_profile()` (lines 1063-1072)

Also move these **private/migrated** methods (keep them for now — they are still called by tests):
- `append_runtime_capabilities()` (lines 106-124)
- `append_tools()` (lines 137-164)
- `append_generation_models()` (lines 167-174)
- `append_special_actions()` (lines 235-247)
- `append_response_format()` (lines 250-343)
- `append_guidelines()` (lines 346-357)
- `append_thinking_guidance()` (lines 435-476)
- `append_skill_mode()` (lines 479-505)
- `append_skill_instructions()` (lines 508-520)
- `append_custom_instructions()` (lines 523-531)
- `append_language_setting()` (lines 534-558)

Required imports at top of sections.rs:
```rust
use super::{PromptBuilder, PromptConfig};
use crate::agent_loop::ToolInfo;
use crate::dispatcher::tool_index::HydrationResult;
use crate::thinker::context::{DisableReason, DisabledTool, EnvironmentContract};
use crate::thinker::interaction::Capability;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};
use crate::thinker::soul::SoulManifest;
```

**Step 3: Create messages.rs**

Move into a separate `impl PromptBuilder` block + standalone types:
- `build_messages()` (lines 967-1027)
- `build_observation()` (lines 1030-1050)
- `Message` struct (lines 1076-1080)
- `MessageRole` enum (lines 1083-1088)
- `impl Message` (lines 1090-1114)
- `truncate_str()` (lines 1117-1127)
- `format_attachment()` (lines 1130-1166)

Required imports:
```rust
use crate::agent_loop::{LoopState, Observation, StepSummary, ToolInfo};
use crate::core::MediaAttachment;
use super::PromptBuilder;
```

**Step 4: Create cache.rs**

Move:
- `build_system_prompt_cached()` (lines 924-939)
- `build_static_header()` (lines 944-964)

Required imports:
```rust
use crate::agent_loop::ToolInfo;
use super::{PromptBuilder, SystemPromptPart};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};
```

**Step 5: Create tests/ directory**

Split `#[cfg(test)] mod tests` (lines 1170-1939) into three files:

`tests/mod.rs`:
```rust
mod build_tests;
mod section_tests;
mod sanitize_tests;
```

`tests/build_tests.rs` — tests that call `build_*` entry points:
- `test_build_system_prompt_with_soul()` (line 1178)
- `test_thinking_guidance_disabled_by_default()` (line 1203)
- `test_thinking_guidance_enabled()` (line 1213)
- `test_thinking_guidance_with_soul()` (line 1241)
- `test_build_system_prompt_with_context_includes_runtime_context()` (line 1261)
- `test_build_system_prompt_with_context_no_runtime_context()` (line 1300)
- `test_full_prompt_with_all_enhancements_background_mode()` (line 1442)
- `test_interactive_prompt_minimal_token_overhead()` (line 1542)
- `test_build_system_prompt_with_hooks()` (line 1648)

`tests/section_tests.rs` — tests that call `append_*` methods:
- `test_append_protocol_tokens_with_silent_reply()` (line 1321)
- `test_append_protocol_tokens_without_silent_reply()` (line 1343)
- `test_append_operational_guidelines_background()` (line 1363)
- `test_append_operational_guidelines_cli()` (line 1377)
- `test_append_operational_guidelines_messaging_skipped()` (line 1390)
- `test_append_safety_constitution()` (line 1403)
- `test_append_memory_guidance()` (line 1416)
- `test_append_citation_standards()` (line 1428)
- `test_append_channel_behavior_telegram_group()` (line 1614)
- `test_append_channel_behavior_terminal()` (line 1625)
- `test_append_soul_continuity()` (line 1636)

`tests/sanitize_tests.rs` — all sanitization tests:
- `test_sanitize_soul_identity_injection_markers()` (line 1669)
- `test_sanitize_soul_directives_control_chars()` (line 1688)
- `test_sanitize_soul_expertise_format_chars()` (line 1707)
- `test_sanitize_soul_voice_tone()` (line 1725)
- `test_sanitize_soul_addendum()` (line 1746)
- `test_sanitize_custom_instructions_control_chars()` (line 1764)
- `test_sanitize_custom_instructions_preserves_newlines()` (line 1781)
- `test_sanitize_language_strict()` (line 1796)
- `test_sanitize_runtime_capabilities_light()` (line 1815)
- `test_sanitize_generation_models_light()` (line 1832)
- `test_sanitize_skill_instructions_moderate()` (line 1847)
- `test_sanitize_security_notes_light()` (line 1864)
- `test_sanitize_channel_behavior_light()` (line 1880)
- `test_sanitize_user_profile_light()` (line 1894)
- `test_full_prompt_no_injection_markers_from_soul()` (line 1911)

All test files use `use super::super::*;` to access PromptBuilder, PromptConfig, etc.

**Step 6: Verify**

```bash
cd core && cargo check
cargo test --lib thinker::prompt_builder
cargo clippy -- -W clippy::all
```
Expected: All 39 tests pass, no new warnings.

**Step 7: Commit**

```bash
git add core/src/thinker/prompt_builder/ -A
git add core/src/thinker/prompt_builder.rs  # staged deletion
git commit -m "thinker: split prompt_builder.rs into 4 sub-modules + test directory"
```

---

## Task 2: Split gateway/execution_engine.rs (1,044 lines)

**Files:**
- Delete: `core/src/gateway/execution_engine.rs`
- Create: `core/src/gateway/execution_engine/mod.rs`
- Create: `core/src/gateway/execution_engine/engine.rs`
- Create: `core/src/gateway/execution_engine/simple.rs`
- Create: `core/src/gateway/execution_engine/tests.rs`

**Step 1: Create mod.rs with shared types**

Move these to `mod.rs`:
- All `use` imports needed by types
- `ExecutionEngineConfig` struct + Default impl (lines 28-46)
- `RunRequest` struct (lines 48-61)
- `RunState` enum (lines 63-78)
- `RunStatus` struct (lines 80-89)
- `ActiveRun` struct (lines 91-111) — **remove `abort_senders` related code if unused**
- `ExecutionError` enum (lines 905-928)
- `ExecutionAdapter` trait definition (extract from the impl blocks — find the trait definition)

Sub-module declarations:
```rust
mod engine;
mod simple;
#[cfg(test)]
mod tests;

pub use engine::ExecutionEngine;
pub use simple::SimpleExecutionEngine;
```

Verify existing re-exports in `core/src/gateway/mod.rs` line 133 still resolve:
```rust
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine};
```

**Step 2: Create engine.rs**

Move `ExecutionEngine<P, R>` (lines 113-127) struct and its full `impl` block (lines 138-580):
- `new()`, `execute()`, `get_status()`, `cancel()`, `list_active_runs()`
- Private: `store_abort_sender()`, `format_history()`, `run_agent_loop()`
- `ExecutionAdapter` impl for `ExecutionEngine` (lines 851-876)

**Delete dead code:**
- Remove `abort_senders: HashMap<...>` field from struct
- Remove `store_abort_sender()` method
- Remove `store_abort_sender()` call from `run_agent_loop()`

**Step 3: Create simple.rs**

Move `SimpleExecutionEngine` (lines 129-134) struct and its `impl` block (lines 586-841):
- `new()`, `execute()`, `run_simple_loop()`, `get_status()`, `cancel()`, `default()`
- `ExecutionAdapter` impl for `SimpleExecutionEngine` (lines 883-903)

**Step 4: Create tests.rs**

Move test module (lines 930-1043):
- `TestEmitter` struct + impl
- `test_simple_execution_engine_basic()`
- `test_simple_execution_engine_run()`

**Step 5: Verify**

```bash
cd core && cargo check
cargo test --lib gateway::execution_engine
cargo clippy -- -W clippy::all
```

**Step 6: Commit**

```bash
git add core/src/gateway/execution_engine/ -A
git add core/src/gateway/execution_engine.rs
git commit -m "gateway: split execution_engine.rs into 3 sub-modules, remove dead abort_senders"
```

---

## Task 3: Split memory/context.rs (1,302 lines)

**Files:**
- Delete: `core/src/memory/context.rs`
- Create: `core/src/memory/context/mod.rs`
- Create: `core/src/memory/context/enums.rs`
- Create: `core/src/memory/context/fact.rs`
- Create: `core/src/memory/context/compression.rs`
- Create: `core/src/memory/context/paths.rs`
- Create: `core/src/memory/context/tests/mod.rs`
- Create: `core/src/memory/context/tests/enum_tests.rs`
- Create: `core/src/memory/context/tests/fact_tests.rs`

**Step 1: Create mod.rs**

Move:
- `SINGLE_TURN_TOPIC_ID` const (line 22)
- `ContextAnchor` struct + impl (lines 8-59)
- `MemoryEntry` struct + impl (lines 68-129)
- `default_namespace()`, `default_workspace_id()` helper fns (lines 61-66)

Sub-module declarations + re-exports:
```rust
mod enums;
mod fact;
mod compression;
mod paths;
#[cfg(test)]
mod tests;

pub use enums::{
    FactType, FactSource, MemoryLayer, MemoryCategory,
    MemoryTier, MemoryScope, FactSpecificity, TemporalScope,
};
pub use fact::{MemoryFact, default_strength};
pub use compression::{CompressionSession, FactStats, CompressionResult};
pub use paths::{PRESET_PATHS, compute_parent_path};
```

Verify `core/src/memory/mod.rs` line 90-94 re-exports still resolve:
```rust
pub use context::{
    CompressionResult, CompressionSession, ContextAnchor, FactSource, FactSpecificity,
    FactStats, FactType, MemoryCategory, MemoryEntry, MemoryFact, MemoryLayer, MemoryScope,
    MemoryTier, TemporalScope, compute_parent_path, PRESET_PATHS,
};
```

**Step 2: Create enums.rs**

Move all 8 enums with their full impl blocks:
- `FactType` (lines 135-246)
- `FactSource` (lines 248-302)
- `MemoryLayer` (lines 304-348)
- `MemoryCategory` (lines 350-399)
- `MemoryTier` (lines 402-451)
- `MemoryScope` (lines 453-502)
- `FactSpecificity` (lines 519-564)
- `TemporalScope` (lines 566-611)

Each enum includes: struct definition, `as_str()`, `from_str_or_default()`, `impl FromStr`, `impl Display`.

**Step 3: Create fact.rs**

Move:
- `default_strength()` fn (lines 704-706)
- `MemoryFact` struct (lines 633-702)
- `impl Entity for MemoryFact` (lines 708-714)
- `impl AggregateRoot for MemoryFact` (line 716)
- `impl MemoryFact` with all builder methods (lines 718-890)

Required imports:
```rust
use super::enums::{FactType, FactSource, MemoryLayer, MemoryCategory, MemoryTier, MemoryScope, FactSpecificity, TemporalScope};
use crate::domain::{AggregateRoot, Entity};
```

**Step 4: Create compression.rs**

Move:
- `CompressionSession` struct + impl (lines 892-929)
- `FactStats` struct (lines 931-944)
- `CompressionResult` struct + impl (lines 946-964)

**Step 5: Create paths.rs**

Move:
- `PRESET_PATHS` const array (lines 505-517)
- `compute_parent_path()` fn (lines 618-624)

**Step 6: Create tests/ directory**

Split test module (lines 966-1302):

`tests/mod.rs`:
```rust
mod enum_tests;
mod fact_tests;
```

`tests/enum_tests.rs` — all enum round-trip tests (FactType, FactSource, MemoryLayer, etc.)

`tests/fact_tests.rs` — MemoryFact builder tests, ContextAnchor tests, path utility tests

**Step 7: Verify**

```bash
cd core && cargo check
cargo test --lib memory::context
cargo clippy -- -W clippy::all
```

**Step 8: Commit**

```bash
git add core/src/memory/context/ -A
git add core/src/memory/context.rs
git commit -m "memory: split context.rs into 5 sub-modules (enums, fact, compression, paths)"
```

---

## Task 4: Split cron/mod.rs (1,356 lines)

**Files:**
- Modify: `core/src/cron/mod.rs` (keep struct + error + new/set_executor)
- Create: `core/src/cron/schema.rs`
- Create: `core/src/cron/crud.rs`
- Create: `core/src/cron/query.rs`
- Create: `core/src/cron/executor.rs` (rename if conflicts with existing file)
- Create: `core/src/cron/lifecycle.rs`
- Create: `core/src/cron/tests.rs`

**Step 1: Audit existing cron/ submodules for naming conflicts**

Check that `schema.rs`, `crud.rs`, `query.rs`, `executor.rs`, `lifecycle.rs`, `tests.rs` don't already exist. If `executor.rs` conflicts, use `job_executor.rs`.

**Step 2: Trim mod.rs to core definitions**

Keep in `mod.rs`:
- Module doc comment (lines 1-50)
- Existing submodule declarations (lines 53-59): config, chain, delivery, resource, scheduler, template, webhook_target
- **Add new** submodule declarations: schema, crud, query, job_executor, lifecycle
- All `pub use` re-exports (lines 61-66)
- `CronResult` type alias (line 81)
- `JobExecutor` type alias (line 82)
- `CronError` enum (lines 85-105)
- `CronService` struct definition (lines 113-127)
- `impl CronService { new(), set_executor() }` (lines 130-277 minus schema methods)
- `#[cfg(test)] mod tests;`

Note: `new()` calls `init_schema()` and `migrate_schema()`. After moving those to `schema.rs`, change calls to `self.init_schema()` / `self.migrate_schema()` — they will resolve because Rust's `impl` blocks across files share the same `self`.

**Step 3: Create schema.rs**

Move as `impl CronService`:
- `init_schema()` (lines 162-228)
- `migrate_schema()` (lines 229-276)

**Step 4: Create crud.rs**

Move as `impl CronService`:
- `add_job()` (lines 282-372)
- `update_job()` (lines 374-446)
- `delete_job()` (lines 449-470)
- `enable_job()` (lines 472-510)
- `disable_job()` (lines 511-535)

**Step 5: Create query.rs**

Move as `impl CronService`:
- `JOBS_SELECT` const (lines 574-580)
- `row_to_cron_job()` (lines 536-584)
- `get_job()` (lines 585-609)
- `list_jobs()` (lines 611-631)
- `RUNS_SELECT` const (lines 672-673)
- `row_to_job_run()` (lines 633-667)
- `get_job_runs()` (lines 668-689)
- `save_run_sync()` (lines 691-719)

**Step 6: Create job_executor.rs**

Move as `impl CronService`:
- `finalize_job_sync()` (lines 720-771)
- `get_next_run()` (lines 772-782)
- `check_and_run_jobs()` — both `#[cfg(feature = "cron")]` and non-cron stubs (lines 863-1063)

**Step 7: Create lifecycle.rs**

Move as `impl CronService`:
- `start()` (lines 784-844)
- `stop()` (lines 845-862)
- `cleanup_history()` (lines 1065-1089)
- `startup_catchup()` (lines 1091-1145)

**Step 8: Create tests.rs**

Move `#[cfg(test)] mod tests` (lines 1149-1356) with all 14 tests.

**Step 9: Verify**

```bash
cd core && cargo check
cargo test --lib cron
cargo clippy -- -W clippy::all
```

**Step 10: Commit**

```bash
git add core/src/cron/ -A
git commit -m "cron: split mod.rs into 6 sub-modules (schema, crud, query, executor, lifecycle)"
```

---

## Task 5: Split poe/worker.rs (1,128 lines)

**Files:**
- Delete: `core/src/poe/worker.rs`
- Create: `core/src/poe/worker/mod.rs`
- Create: `core/src/poe/worker/agent_loop_worker.rs`
- Create: `core/src/poe/worker/callback.rs`
- Create: `core/src/poe/worker/gateway.rs`
- Create: `core/src/poe/worker/placeholder.rs`
- Create: `core/src/poe/worker/tests/mod.rs`
- Create: `core/src/poe/worker/tests/mock_worker.rs`
- Create: `core/src/poe/worker/tests/worker_tests.rs`

**Step 1: Create mod.rs**

Move:
- `StateSnapshot` struct + impl (lines 30-84)
- `Worker` trait (lines 100-139)
- Module-level imports for shared types

Sub-module declarations + re-exports:
```rust
mod agent_loop_worker;
mod callback;
mod gateway;
mod placeholder;
#[cfg(test)]
mod tests;

pub use agent_loop_worker::AgentLoopWorker;
pub use gateway::{GatewayAgentLoopWorker, create_gateway_worker};
pub use placeholder::PlaceholderWorker;
#[cfg(test)]
pub use tests::mock_worker::MockWorker;
```

Verify `core/src/poe/mod.rs` lines 149-152 still resolve:
```rust
pub use worker::{
    AgentLoopWorker, GatewayAgentLoopWorker, PlaceholderWorker, StateSnapshot, Worker,
    create_gateway_worker,
};
```

**Step 2: Create callback.rs**

Move:
- `PoeLoopCallback` struct (lines 149-159)
- `impl PoeLoopCallback` (lines 161-221)
- `impl LoopCallback for PoeLoopCallback` (lines 223-323)

This is internal-only (`pub(crate)` or private).

**Step 3: Create agent_loop_worker.rs**

Move:
- `AgentLoopWorker<T, E, C>` struct (lines 351-380)
- `impl AgentLoopWorker` (lines 382-459)
- `impl Worker for AgentLoopWorker` (lines 461-626)

Import callback: `use super::callback::PoeLoopCallback;`

**Step 4: Create gateway.rs**

Move:
- `GatewayAgentLoopWorker` type alias (lines 734-738)
- `create_gateway_worker()` factory fn (lines 767-832)

**Step 5: Create placeholder.rs**

Move:
- `PlaceholderWorker` struct + impl (lines 641-703)
- `impl Worker for PlaceholderWorker` (lines 669-703)
- `truncate_instruction()` helper (lines 706-712)

**Step 6: Create tests/ directory**

`tests/mod.rs`:
```rust
pub mod mock_worker;
mod worker_tests;
```

`tests/mock_worker.rs` — MockWorker struct + impl (lines 845-957), gated with `#[cfg(test)]`

`tests/worker_tests.rs` — All 12 tests (lines 963-1128)

**Step 7: Verify**

```bash
cd core && cargo check
cargo test --lib poe::worker
cargo clippy -- -W clippy::all
```

**Step 8: Commit**

```bash
git add core/src/poe/worker/ -A
git add core/src/poe/worker.rs
git commit -m "poe: split worker.rs into 5 sub-modules (agent_loop_worker, callback, gateway, placeholder)"
```

---

## Task 6: Final Verification

**Step 1: Full build check**

```bash
cd core && cargo check --all-features
```

**Step 2: Full test suite**

```bash
cd core && cargo test
```

**Step 3: Clippy**

```bash
cd core && cargo clippy --all-features -- -W clippy::all
```

**Step 4: Verify no public API changes**

Grep for all external `use` statements found in the exploration phase and confirm they still compile:
- `use crate::thinker::prompt_builder::{Message, MessageRole, PromptBuilder, PromptConfig}`
- `use crate::gateway::execution_engine::{ExecutionEngine, ...}`
- `use crate::memory::context::{ContextAnchor, FactType, MemoryFact, ...}`
- `use crate::poe::worker::{Worker, StateSnapshot, ...}`

**Step 5: Final commit**

If any fixups were needed:
```bash
git add -A && git commit -m "refactor: fix import paths after large file splitting"
```

---

## Summary

| Task | File | Before | After (max) | New Files | Dead Code Removed |
|------|------|--------|-------------|-----------|-------------------|
| 1 | prompt_builder.rs | 1,939 | ~350 | 8 | 11 migrated methods (if unused) |
| 2 | execution_engine.rs | 1,044 | ~350 | 4 | abort_senders + store_abort_sender |
| 3 | context.rs | 1,302 | ~450 | 8 | 0 |
| 4 | cron/mod.rs | 1,356 | ~260 | 6 | 0 |
| 5 | poe/worker.rs | 1,128 | ~280 | 8 | 0 |
| 6 | Final verification | — | — | 0 | — |
