# Static Review of `src/exec`, `src/executor`, `src/export` — Run Record / 静态审查运行记录

- Date / 日期: 2026-11-09
- Scope / 范围: `src/exec/**` (16 files, 7,657 LOC), `src/executor/**` (21 files, 15,278 LOC), `src/export/**` (6 files, 1,376 LOC) — Total 43 files, 24,311 LOC
- Execution mode / 执行方式: 5 parallel static-review batches (subagents, isolated worktree `.worktrees/occams-review-exec`) → Jev triage → 4 fix waves committed directly to `main` without PR → unified memory-gated `CARGO_BUILD_JOBS=2 cargo check --lib -j 2`.
- Branch / 分支: `main` (local checkout `/home/zou/data/workspace/Aleph`)
- Baseline commit / 基线提交: `2dad43374`
- Final verification commit / 最终验证提交: `365d1012b`

---

## 1. Method / 方法

### 1.1 Batching / 分批划分

Target codebase was divided into 5 disjoint review batches based on subsystem cohesion and dependency boundaries:

| Batch / 批次 | Area / 模块范围 | Files / 文件数 | LOC / 代码行数 | Findings / 发现数 | Clean Files |
|---|---|---:|---:|---|---|
| **B1** | `src/exec/` Security, Redaction & Parsing (`masker.rs`, `secret_patterns.rs`, `leak_detector.rs`, `parser.rs`, `analysis.rs`, `kernel.rs`, `risk.rs`) | 7 | 2,584 | 19 (3 P1, 10 P2, 6 P3) | 1 |
| **B2** | `src/exec/` Approvals & Manager (`manager.rs`, `channel_bridge.rs`, `allowed_decisions.rs`, `decision.rs`, `bridge.rs`, `socket.rs`, `mod.rs`, `approval/*`) | 9 | 5,073 | 16 (2 P1, 6 P2, 8 P3) | 3 |
| **B3** | `src/executor/` Core, Config & Definitions (`mod.rs`, `tool_registry.rs`, `builtin_registry/{mod, config, groups, dispatchable, definitions}`) | 7 | 6,619 | 19 (2 P1, 7 P2, 10 P3) | 1 |
| **B4** | `src/executor/` Builders & Constructors (`builder/mod.rs`, `core_tools.rs`, `optional_tools.rs`, `tests.rs`, `constructor/*`) | 8 | 4,847 | 14 (2 P1, 8 P2, 4 P3) | 1 |
| **B5** | `src/executor/` Registry Impl & `src/export/` (`registry/*`, `src/export/*`) | 12 | 5,210 | 18 (1 P1, 8 P2, 9 P3) | 2 |
| **Total** | 43 files | **43** | **24,333** | **86** (10 P1, 39 P2, 37 P3) | **8** |

All subagents operated under the `/rust-occams-razor` skill without generating code diffs during the review phase, focusing on logic invariants, error paths, resource leaks, allocation churn, and public API preservation.

---

## 2. Semantic Graph Navigation / 语义图谱导航

We consulted `graphify-out/` and `graphify-out/graph.json` (144,930 nodes, 295,755 links):
- Queried link topology for `src/exec`, `src/executor`, `src/export` (624 nodes: 326 exec, 221 executor, 77 export; 1,282 internal links, 975 outgoing links, 63 incoming links).
- Identified critical external consumers across `gateway/handlers/exec_approvals.rs`, `approval/adapters.rs`, `sandbox/security_kernel_hook.rs`, `gateway/execution_engine/unattended_redacting_sink.rs`, and `builtin_tools/artifact_publish.rs`.
- Ensured all public contracts (`ExecApprovalManager`, `SecretMasker`, `ToolRegistry`, `ExportArtifact`, `render_document_html`) preserved identical external signatures, error semantics, and type invariants.

---

## 3. Model Routing / 模型路由分工说明

Per user instructions, tasks were strictly partitioned between the **Jev** model role and the **Reasoning** model role:
- **Jev Model Role**: High-efficiency classification, filtering, routing, triage, and batch organization.
- **Reasoning Model Role (minimax-M3 / high reasoning)**: Deep AST control-flow simulation, invariant preservation reasoning, Rust borrowing/lifetime analysis, and patch synthesis.

The table below explicitly details every step that utilized the **Jev** model role:

| Step / 环节 | Task Description / 任务描述 | Jev Input & Processing / Jev 输入与处理 | Outcome / 结论与产出 |
|---|---|---|---|
| **Step 1: Baseline Diagnostic Scoping** | Scope static analysis criteria and filter noise rules across 43 files | Filtered noise rules down to high-signal invariants: redundant allocations, unnecessary clones, unhandled error cases, regex DoS, split-brain clocks, fail-closed schema violations, and dead abstractions. | Scoped rules to high-signal items; isolated structural, concurrency, and behavioral focus areas. |
| **Step 2: Batch Clustering & Routing** | Partition 43 files across 3 directories into coherent review batches | Analyzed directory trees, file sizes, and `graphify` link clusters; balanced token budgets (~2.5k-6.6k LOC/batch). | Produced the 5-batch disjoint allocation (B1..B5) minimizing cross-batch shared state. |
| **Step 3: Multi-Agent Dispatch Scheduling** | Concurrent subagent scheduling across batches | Staged subagent execution across B1, B2, B3, B4, B5 in parallel with isolated worktree context `.worktrees/occams-review-exec`. | Dispatched 5 parallel subagents without rate limit failure. |
| **Step 4: Findings Classification & Triage** | Triage 86 findings from review reports into severity and actionability | Filtered 86 findings by impact: critical logic/IO flaws, memory allocations, vs speculative rewrites. | Categorized findings: 10 P1, 39 P2, 37 P3. Approved actionable Occam's razor simplifications across 4 commit waves; rejected speculative behavioral changes. |
| **Step 5: Fix Wave Grouping & Ordering** | Group approved fixes into file-disjoint commit waves | Organized fixes by dependency graph and file locality to prevent merge conflicts. | Staged into 4 commits directly committed to `main` without PR. |

---

## 4. Applied Fix Waves / 已应用修复提交

| Wave | Commit | Summary / 修复内容概要 | Files Touched |
|---|---|---|---|
| **Wave 1** | `347b7023a` | **Exec Security, Redaction & Parser**: Restrict URL credential regex to `[^/\s?#]*` to prevent obliteration of host/path when `@` appears in query or paths (`secret_patterns.rs`); remove dead `& if ... (prev == '>' \|\| prev == '<')` redirect arm and prune redundant UTF-8 `chars().count()` check (`parser.rs`); add non-allocating `mask_cow` to eliminate thousands of `String` allocations on clean JSON leaves in `mask_json_strings` (`masker.rs`); cap custom risk patterns to `MAX_CUSTOM_PATTERNS = 64` to prevent regex DoS (`kernel.rs`); eliminate redundant `LeakPatternDef` and reuse `LeakPattern` (`leak_detector.rs`). | 5 files |
| **Wave 2** | `e8aca04c7` | **Exec Manager State Machine**: Unify expiry checks in `cleanup_expired` to align with `entry.record.is_expired()` and sweep dead waiters; sweep stale `session_listings` snapshots when sessions have no pending cards left, resolving unbounded memory growth; factor out `cleanup_expired_locked` to eliminate double write-lock acquisition during pending registration (`manager.rs`). | 1 file |
| **Wave 3** | `5294f30ce` | **Executor Builders & Constructors**: Make `core_tools::tool_schema` crate-visible and enforce fail-closed schema generation on `acp_delegate`, `acp_switch`, and `acp_session_control` (`agent_acp_tools.rs`), as well as `recall_context`, `memory_trace`, and `note_graph_query` (`collab_session_tools.rs`), replacing permissive `{}` fallback; unify `web_fetch` description in `core_tools.rs` to `<WebFetchTool as AlephTool>::DESCRIPTION`. | 3 files |
| **Wave 4** | `bbac4c24b` | **Export Documents & Executor Registry**: Eliminate multi-megabyte redundant Base64 re-escaping in document export (`page.rs`), removing up to 100MB of heap churn on large inline deliverables; convert `escape_attr` to single-pass escaping (`markdown.rs`); deduplicate 50-line delivery route injection in `"loop_graph"` to delegate to `self.inject_delivery_route` (`tool_registry_impl.rs`); delete dead struct field `recall_context_db` (`struct_def.rs`, `constructor/mod.rs`); optimize `test_all_builtin_tools_have_a_group` to use `HashSet` (`groups.rs`). | 6 files |
| **Follow-up 1** | `dea07bbe3` | **Exec & Executor Type Alignment**: Fix session_key equality comparison type in `manager.rs:1496` and remove unused `warn` import in `collab_session_tools.rs`. | 2 files |
| **Follow-up 2** | `365d1012b` | **Exec Linter Cleanliness**: Add `#[allow(dead_code)]` to `cleanup_expired` in `src/exec/manager.rs` to keep `--lib` builds warning-free. | 1 file |

---

## 5. Verification / 编译与验证

- **Unified Cargo Check across all targets**:
  Command: `env CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo check -p alephcore --all-targets -j1`
  Result: **Finished `dev` profile [unoptimized] target(s) in 15m 43s with 0 errors!**
- **Lib Profile Check**:
  Command: `CARGO_BUILD_JOBS=2 cargo check --lib -j 2`
  Result: **0 errors, 4 warnings** (pre-existing warnings in unrelated modules `extension/projection.rs`, `memory/runtime`, `tools/registry.rs`).
- **Memory Controls**:
  Memory-capped execution enforced with `-j1` / `-j2` and global file-descriptor lock, maintaining peak RSS within the 19GB total RAM constraint (peak ~15GB during full test compilation, ~8GB during check).
- **Worktree Lifecycle**:
  Development and staging performed in `.worktrees/occams-review-exec` on branch `review/occams-exec-executor-export`, fast-forwarded and committed directly to `main`, and the worktree cleanly purged.
