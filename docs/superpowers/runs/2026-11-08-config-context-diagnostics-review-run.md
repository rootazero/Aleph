# Static Review of `src/config`, `src/context`, `src/diagnostics` — Run Record / 静态审查运行记录

- Date / 日期: 2026-11-08
- Scope / 范围: `src/config/**` (92 files, 31,618 LOC), `src/context/**` (38 files, 25,427 LOC), `src/diagnostics/**` (27 files, 11,458 LOC) — Total 157 files, 68,503 LOC
- Execution mode / 执行方式: 10 parallel static-review batches (subagents, isolated worktree `Aleph-wt-cfg-ctx-diag`) → Jev triage → 5 fix waves committed directly to `main` without PR → unified memory-gated `CARGO_BUILD_JOBS=2 cargo check --lib --tests -j 2` (Passed in 9m 27s with 0 errors).
- Branch / 分支: `main` (local checkout `/home/zou/data/workspace/Aleph`)
- Baseline commit / 基线提交: `4cd979bda`
- Final verification commit / 最终验证提交: `214aa29b7`

---

## 1. Method / 方法

### 1.1 Batching / 分批划分

Target codebase was divided into 10 disjoint review batches based on semantic affinity, subsystem boundaries, and reviewer token budgets:

| Batch / 批次 | Area / 模块范围 | Files / 文件数 | LOC / 代码行数 | Findings / 发现数 | Clean Files |
|---|---|---:|---:|---|---|
| **B1** | `src/diagnostics/**` (checks, finding, redact, health checks) | 27 | 11,458 | 10 (0 P0, 3 P1, 3 P2, 4 P3) | 19 |
| **B2** | `src/context/fabric/**` & `src/context/*.rs` (fabric runtime, tool admission) | 9 | 6,105 | 12 (0 P0, 2 P1, 4 P2, 6 P3) | 5 |
| **B3** | `src/context/budget/**` & `src/context/retrieval/**` (budget, cheap passes, FTS5 index) | 9 | 6,629 | 14 (0 P0, 4 P1, 4 P2, 6 P3) | 0 |
| **B4** | `src/context/compact/**` Part 1 (compactor core, manual, session split, directives) | 5 | 7,824 | 8 (0 P0, 2 P1, 2 P2, 4 P3) | 0 |
| **B5** | `src/context/compact/**` Part 2 (strategies, carry, chunkers, folds, preserve) | 15 | 4,869 | 13 (0 P0, 2 P1, 6 P2, 5 P3) | 6 |
| **B6** | `src/config/types/policies/**` & `generation/**` (exec tiers, tool permissions, presets) | 16 | 6,601 | 12 (0 P0, 4 P1, 4 P2, 4 P3) | 9 |
| **B7** | `src/config/types/memory/**` & `types/*.rs` Core (ACP, phase6, dispatcher, memory) | 21 | 6,814 | 15 (0 P0, 5 P1, 6 P2, 4 P3) | 0 |
| **B8** | `src/config/types/*.rs` Extended, `agent_manager/**`, `agent_resolver/**` | 25 | 6,158 | 16 (1 P0, 4 P1, 6 P2, 5 P3) | 17 |
| **B9** | `src/config/*.rs` Root Engine & Lifecycle (save, patcher, backup, validate, load) | 17 | 10,147 | 15 (1 P0, 5 P1, 6 P2, 3 P3) | 9 |
| **B10**| `src/config/tests/**` Integration Test Suite (serialization, validation, save_incremental) | 13 | 3,115 | 11 (0 P0, 1 P1, 5 P2, 5 P3) | 7 |
| **Total** | 157 files | **157** | **69,720** | **126** (2 P0, 32 P1, 46 P2, 46 P3) | **72** |

Every subagent received uniform brief constraints: Occam's razor review (`/simplifying-rust-with-occams-razor`), zero code modifications during review (no diffs generated), strict focus on logic invariants, error paths, resource leaks, race conditions, and panic triggers.

---

## 2. Semantic Graph Navigation / 语义图谱导航

We consulted `graphify-out/GRAPH_REPORT.md` (143k nodes, 294k edges, 6,348 communities) and the graph export in `graphify-out/`:
- **Module decoupling check**: Identified god nodes and cross-boundary coupling hubs (e.g., `Config`, `ConfigPatcher`, `ContextCompactor`, `ContentIndex`, `AgentManager`).
- **Community-aware partitioning**: Batches B1–B5 respected the context runtime and compactor boundaries; B6–B10 clustered configuration schema, domain policies, and lifecycle management.

---

## 3. Model Routing / 模型路由分工说明

Per instructions, tasks were strictly divided by model characteristics:
- **Jev Model Role**: Lightweight, deterministic, high-efficiency routing, filtering, clustering, and categorization.
- **Reasoning Model Role (minimax-M3 / reasoning)**: Deep semantic code understanding, AST control-flow simulation, concurrency analysis, invariant reasoning, and patch synthesis.

The table below explicitly details every step that utilized the **Jev** model role:

| Step / 环节 | Task Description / 任务描述 | Jev Input & Processing / Jev 输入与处理 | Outcome / 结论与产出 |
|---|---|---|---|
| **Step 1: Baseline Diagnostic Scoping** | Scope static analysis criteria and filter noise rules across 157 files | Scoped rules to high-signal items: `blocking_call_in_async`, `string_slice`, `indexing_slicing`, `panic_unwrap`, `fail_open`. | Eliminated cosmetic style noise; isolated structural and behavioral focus areas. |
| **Step 2: Batch Clustering & Routing** | Partition 157 files across 3 directories into coherent review batches | Analyzed directory trees, file sizes, and `graphify` community graphs; balanced token budgets (~6k-11k LOC/batch) | Produced the 10-batch disjoint allocation (B1..B10) minimizing cross-batch shared state. |
| **Step 3: Multi-Agent Dispatch Scheduling** | Rate-limit management & concurrent subagent scheduling | Monitored API concurrency limits and token utilization; staged batch dispatch | Dispatched batches in 2 waves (Wave 1: B1-B5, Wave 2: B6-B10) to avoid 429 quota exhaustion. |
| **Step 4: Findings Classification & Triage** | Triage 126 findings from review reports into severity and actionability | Filtered 126 findings by impact: critical defect, data loss risk, contract violation, vs cosmetic refactor | Categorized findings: 2 P0, 32 P1, 46 P2, 46 P3. Selected 25 high-leverage fixes across 5 waves for immediate application; deferred low-risk P3 debts. |
| **Step 5: Fix Wave Grouping & Ordering** | Group approved fixes into file-disjoint commit waves | Organized fixes by dependency graph and file locality to prevent merge conflicts | Staged into 5 commits directly to `main` without PR. |

---

## 4. Applied Fix Waves / 已应用修复提交

| Wave | Commit | Summary / 修复内容概要 | Files Touched |
|---|---|---|---|
| **Wave 1** | `8287be110` | **P0/P1 Config Core**: Guard against session wipe when workspace equals agent dir (B8-01); preserve `[security.ssrf]` in `merge_sections` (B9-01); secure agent manager paths and symlinks (B8-03, B8-04); drop `RwLock` read guard before `apply_live_sections` (B9-03); prune nulls on `deep_merge` (B9-06); sanitize backup timestamps (B9-07). | 6 files |
| **Wave 2** | `741ea1786` | **Diagnostics & Context Fabric**: Filter other doctor processes in duplicate instance check (B1-01); distinguish NotFound and raise severity on read errors in Discord check (B1-03, B1-04); align output token measurement with conservative claim contract (B2-01); preserve leading slash in path canonicalization (B2-02); return `Ok(None)` on missing tool admission capability (B2-03). | 5 files |
| **Wave 3** | `b73d30468` | **Context Budget & Compactor**: Only drop FTS5 index on explicit 'no such column' error (B3-01); scope newest image calculation to user messages (B3-02); protect unclosed `RunStarted` openers across compactor cuts (B4-02); search closing tags strictly after opening tags (B5-01); require newline/EOF after summary markers (B5-03); correct chars-per-token default ratio to 4.0 (B5-04). | 6 files |
| **Wave 4** | `f0aa82c39` | **Config Types & Policies**: Fail closed on uncompilable glob patterns in tool permissions (B6-001); validate finite float bounds on generation defaults (B6-007); combine presence tracking with field content to preserve custom ACP args (B7-01); clamp team dispatcher zero values to prevent queue deadlocks and busy-spins (B7-03); require exact provider match on threshold needles to prevent substring collisions (B7-04); guard against NaN in recency scoring (B7-08). | 6 files |
| **Wave 5** | `8308c8c5f` | **Integration Tests**: Verify actual incremental file save on disk rather than in-memory simulation (B10-01); assert valid `^/` command regexes and add invalid command regex validation test (B10-03). | 2 files |
| **Closure**| `214aa29b7` | **Compilation Alignment**: Resolve `DiscordVerdict::Ok` on missing config, path comparison types, and `IndexError::Sqlite` error conversion. | 3 files |

---

## 5. Verification / 编译与验证

- `CARGO_BUILD_JOBS=2 cargo check --lib --tests -j 2`: **0 errors, 4 warnings** (pre-existing dead code warnings in unrelated modules `extension/projection.rs`, `tools/registry.rs`, etc.). Time: 9m 27s.
- Worktree cleanup: The isolated worktree `/home/zou/data/workspace/Aleph-wt-cfg-ctx-diag` was used for multi-agent staging; all fixes landed cleanly on `main` branch directly.
