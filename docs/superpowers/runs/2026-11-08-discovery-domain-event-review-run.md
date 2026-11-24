# Static Review of `src/discovery`, `src/domain`, `src/event` — Run Record / 静态审查运行记录

- Date / 日期: 2026-11-08
- Scope / 范围: `src/discovery/**` (7 files, 2,885 LOC), `src/domain/**` (2 files, 1,194 LOC), `src/event/**` (7 files, 1,964 LOC) — Total 16 files, 6,043 LOC
- Execution mode / 执行方式: 4 parallel static-review batches (subagents, isolated worktree `.worktrees/occams-review`) → Jev triage → 4 fix waves committed directly to `main` without PR → unified memory-gated `CARGO_BUILD_JOBS=2 cargo check --lib --tests -j 2` (Passed in 9m 18s with 0 errors).
- Branch / 分支: `main` (local checkout `/home/zou/data/workspace/Aleph`)
- Baseline commit / 基线提交: `0b8406ed7`
- Final verification commit / 最终验证提交: `7bb262def`

---

## 1. Method / 方法

### 1.1 Batching / 分批划分

Target codebase was divided into 4 disjoint review batches based on semantic affinity and subsystem boundaries:

| Batch / 批次 | Area / 模块范围 | Files / 文件数 | LOC / 代码行数 | Findings / 发现数 | Clean Files |
|---|---|---:|---:|---|---|
| **B1** | `src/discovery/` Core (`mod.rs`, `types.rs`, `paths.rs`, `claude_cache.rs`, `home_scan_census.rs`) | 5 | 1,501 | 10 (2 P1, 5 P2, 3 P3) | 1 |
| **B2** | `src/discovery/scanner.rs` & `src/discovery/scanner/tests.rs` | 2 | 1,384 | 10 (2 P1, 7 P2, 1 P3) | 0 |
| **B3** | `src/domain/` Core (`mod.rs`, `skill.rs` & skill integration points) | 2 | 1,194 | 10 (1 P1, 6 P2, 3 P3) | 1 |
| **B4** | `src/event/` (`mod.rs`, `types.rs`, `handler.rs`, `filter.rs`, `global_bus.rs`, `tests/*`) | 7 | 1,964 | 9 (1 P1, 6 P2, 2 P3) | 3 |
| **Total** | 16 files | **16** | **6,043** | **39** (6 P1, 24 P2, 9 P3) | **5** |

All subagents operated under the `/rust-occams-razor` skill without generating code diffs during the review phase, focusing on logic invariants, error paths, resource leaks, allocation churn, and public API preservation.

---

## 2. Semantic Graph Navigation / 语义图谱导航

We consulted `graphify-out/` and `graphify-out/graph.json` (144,831 nodes, 295,664 links):
- Queried link topology for `src/discovery`, `src/domain`, `src/event` (302 nodes, 575 internal links, 281 outgoing links, 127 incoming links).
- Identified external consumers across `extension`, `gateway`, `thinker`, `browser`, and `tools`, ensuring all public signatures (`DiscoveryManager`, `aleph_home_dir`, `SkillManifest`, `GlobalBus`, `EventFilter`) preserved identical invariants and types.

---

## 3. Model Routing / 模型路由分工说明

Per user instructions, tasks were strictly partitioned between the **Jev** model role and the **Reasoning** model role:
- **Jev Model Role**: High-efficiency classification, filtering, routing, triage, and batch organization.
- **Reasoning Model Role (minimax-M3 / high reasoning)**: Deep AST control-flow simulation, invariant preservation reasoning, Rust borrowing/lifetime analysis, and patch synthesis.

The table below explicitly details every step that utilized the **Jev** model role:

| Step / 环节 | Task Description / 任务描述 | Jev Input & Processing / Jev 输入与处理 | Outcome / 结论与产出 |
|---|---|---|---|
| **Step 1: Baseline Diagnostic Scoping** | Scope static analysis criteria and filter noise rules across 16 files | Filtered noise rules down to high-signal invariants: redundant allocations, unnecessary clones, unhandled error cases, unbounded file reads, and dead trait abstractions. | Scoped rules to high-signal items; isolated structural and behavioral focus areas. |
| **Step 2: Batch Clustering & Routing** | Partition 16 files across 3 directories into coherent review batches | Analyzed directory trees, file sizes, and `graphify` link clusters; balanced token budgets (~1k-2k LOC/batch). | Produced the 4-batch disjoint allocation (B1..B4) minimizing cross-batch shared state. |
| **Step 3: Multi-Agent Dispatch Scheduling** | Concurrent subagent scheduling across batches | Staged subagent execution across B1, B2, B3, B4 in parallel with isolated worktree context. | Dispatched 4 parallel subagents without hitting API rate limits. |
| **Step 4: Findings Classification & Triage** | Triage 39 findings from review reports into severity and actionability | Filtered 39 findings by impact: critical logic/IO flaws, memory allocations, vs speculative rewrites. | Categorized findings: 6 P1, 24 P2, 9 P3. Approved actionable Occam's razor simplifications across 4 commit waves; rejected speculative behavioral changes. |
| **Step 5: Fix Wave Grouping & Ordering** | Group approved fixes into file-disjoint commit waves | Organized fixes by dependency graph and file locality to prevent merge conflicts. | Staged into 4 commits directly committed to `main` without PR. |

---

## 4. Applied Fix Waves / 已应用修复提交

| Wave | Commit | Summary / 修复内容概要 | Files Touched |
|---|---|---|---|
| **Wave 1** | `8b2d79c37` | **Discovery Core & Claude Cache**: Canonicalize start/stop once up-front and avoid parent re-canonicalization syscalls during upward walk (`paths.rs`); add non-allocating `is_valid_path_component` predicate; eliminate per-key `Vec<&Value>` allocation in `parse_installed_plugins`; bound `installed_plugins.json` disk reads to 10MB to prevent OOM (`claude_cache.rs`). | 2 files |
| **Wave 2** | `f25cca07d` | **Discovery Scanner & Tests**: Require `.is_file()` on marker checks to prevent false-positive qualification of directories named `SKILL.md/` (`scanner.rs`); replace eager 8-element `PathBuf` array with lazy iterator short-circuiting in `has_plugin_manifest`; replace $O(N^2)$ dedup with `HashSet<PathBuf>` in `get_all_directories`; move `PathBuf` by value in `classify_entry` to eliminate double allocations; add characterization tests for invalid component names and marker directory rejection (`tests.rs`). | 2 files |
| **Wave 3** | `a33f2c999` | **Domain & Event Subsystems**: Add inherent `pub fn id(&self) -> &SkillId` to `SkillManifest` (`skill.rs`); implement `Borrow<str>`, `AsRef<str>`, `PartialEq<str>`, and `PartialEq<&str>` on `SkillId` and `PluginId`; implement `fmt::Display` for `SkillSource`, `InstallKind`, `PromptScope`; implement `FromStr` for `InstallKind` and `PromptScope`; optimize `Os::deserialize` with `Cow<'de, str>`; guard `GlobalBus::broadcast` sender clone with `receiver_count() > 0`; replace `once_cell::sync::Lazy` with `std::sync::LazyLock` (`global_bus.rs`); reorder `EventFilter::matches` to evaluate integer `event_types` first before string HashSet lookups (`filter.rs`). | 3 files |
| **Wave 4** | `8f1687db8` & `7bb262def` | **Compilation Alignment & Import Cleanup**: Add turbofish `::<'de, str>` to `Cow::deserialize`; remove 8 obsolete `use crate::domain::Entity;` imports across the codebase made redundant by `SkillManifest::id` inherent method; remove unused constants in `scanner.rs`. | 10 files |

---

## 5. Verification / 编译与验证

- `CARGO_BUILD_JOBS=2 cargo check --lib --tests -j 2`: **0 errors, 4 warnings** (pre-existing dead code warnings in unrelated modules `extension/projection.rs`, `memory/runtime`, `tools/registry.rs`). Time: 9m 18s.
- `CARGO_BUILD_JOBS=2 cargo test --lib discovery:: -j 2`: **57 passed, 0 failed** (1.07s).
- `CARGO_BUILD_JOBS=2 cargo test --lib domain:: -j 2`: **70 passed, 0 failed** (0.01s).
- `CARGO_BUILD_JOBS=2 cargo test --lib event:: -j 2`: **35 passed, 0 failed** (0.06s).
- **Total Tests Passed**: **162 / 162 unit tests passed with 0 failures**.
- Worktree cleanup: The isolated worktree `.worktrees/occams-review` and branch `review/occams-disc-dom-evt` were cleanly removed; all commits landed directly on `main`.
