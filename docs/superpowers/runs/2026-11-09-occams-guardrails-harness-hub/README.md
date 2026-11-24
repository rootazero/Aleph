# Occam's Razor Static Review & Remediation Report

**Date**: 2026-11-09  
**Target Modules**: `src/guardrails`, `src/harness`, `src/hub`  
**Total Scope**: 48 files | 27,798 LOC  
**Methodology**: Parallel Static Review with Subagents (`/rust-occams-razor`), Zero Interim Cargo Checks, Direct Commit to `main` via Isolated Git Worktree, Unified Final Compilation with Memory Control (`CARGO_BUILD_JOBS=2`).

---

## 1. Executive Summary

This static review targeted three foundational infrastructure and execution modules of Aleph (`guardrails`, `harness`, `hub`) under Occam's Razor principles. 

All review batches were file-disjoint and processed concurrently across isolated subagents. Candidate smells and anti-patterns were triaged and classified using the **Jev tier**, while complex invariant analysis, R10 redline defense, async cancellation / lock ordering safety, and surgical code repairs were authored by the **deep reasoning tier**.

All identified bugs, dead code, redundant multi-object allocations, git checkout symlink escapes, schema validation off-by-one errors, and unnecessary string conversions were repaired and committed to `main` without requiring pull requests.

---

## 2. Model Tier Allocation & Workflow

### Jev Tier (Classification, Filtering, Routing)
The **Jev tier** was specifically tasked with high-volume, low-latency static pattern matching, taxonomy classification, and triage routing:
1. **Candidate Sweep**: Scanned all 48 files across the 6 batches for Occam's razor anti-patterns:
   - Gratuitous `.clone()`, `.to_string()`, unnecessary intermediate vectors, and reflection-based deserialization.
   - Deeply nested control flow and unneeded wrapper layers.
   - Dead functions, stale helper functions, and redundant imports.
   - Defensive boundary checks and path traversal / symlink jail gaps.
2. **Taxonomy Classification**: Categorized each detected pattern into one of 5 classes:
   - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
   - `REDUNDANT-ALLOCATION`
   - `CONTROL-FLOW` / `INDIRECTION`
   - `DEFENSIVE-CHECK` / `PANIC-RISK`
   - `CORRECTNESS` / `INVARIANT-TIGHTENING`
3. **Filtering & Routing Decisions**:
   - Filtered out intended public API exports and test invariants.
   - Assigned route tags:
     - `FIX-NOW`: High-priority correctness/performance fixes applied immediately.
     - `FIX-CAREFULLY`: Nuanced invariant or allocation tightening applied with surgical care.
     - `DEFER`: Broad architectural shifts or serialization formats requiring wire migration.
     - `REJECT`: Deliberate design patterns or test assertions.
   - Artifacts generated: `classification/GUARD1.class.md`, `classification/HARN1.class.md`, `classification/HARN2.class.md`, `classification/HARN3.class.md`, `classification/HUB1.class.md`, `classification/HUB2.class.md`.

### Deep Reasoning Tier (Invariant Analysis & Code Authoring)
The **deep reasoning tier** (Minimax-M3 / Pro) handled:
1. **R10 Redlines & Budget Ratchet (`src/harness`)**:
   - Preserved the exact 12 production files (8 top-level + 4 under `agent/`). Zero new production files created.
   - Reduced harness line count (dropping 59 lines below the `CEILING = 5535` ratchet).
   - Ensured no LLM cognitive logic (prompt copy, recovery policies) was added to the harness loop.
2. **Semantic Invariant Verification**:
   - Guardrails screening hot-path was converted to zero-allocation `Cow<'_, str>` without changing filtering semantics.
   - Preserved live-before-transcript sequencing and lock invariants in parallel tool dispatch.
   - Hardened `src/hub` package extraction against Windows drive prefix and symlink jail escapes.
3. **Surgical Patch Authoring & Verification**:
   - Directly authored all minimal diffs and verified clean compilation across the entire workspace.

---

## 3. Batch Partitioning & Commit History

| Batch ID | Subsystem / Files | Files / LOC | Commit SHA | Summary of Changes |
|---|---|---|---|---|
| **GUARD1** | `src/guardrails/*` | 9 files / 2,057 LOC | `023d43747` | Zero-allocation hot path with `Cow<'_, str>` on clean inputs/outputs; lazy `Option<Value>` rewrite in tool guardrail; eliminated `.map(str::to_string)` in history replay. |
| **HARN1** | `src/harness` core & infra | 13 files / 6,300 LOC | `d5a29c61a` | Unified `SessionCompleted` event emission (eliminated 4 clones and 23 duplicate lines); zero-allocation tool failure dedup probe; derived `Debug` on `NoopHarnessCallback`. |
| **HUB1** | `src/hub` foundations & trust | 8 files / 2,106 LOC | `4649e594f` | Implemented `FromStr` on `ExtensionKind` eliminating JSON reflection; static SQL queries; borrowed candidate resolution; hex encoding; fast-path injection scanner. |
| **HARN3** | `src/harness` act & wiring | 6 files / 9,260 LOC | `d5fe1828e` | Zero-allocation batch duplicate probe; eliminated deep `ToolOutput` clone on memo hit; `.take()` sanitized JSON values; conditional trace clones. |
| **HUB2** | `src/hub` operations | 7 files / 2,858 LOC | `5f39efdfc` | Fixed symlink escape and Windows drive bypass in git skill checkout; fixed schema budget <=1024 check; zero-Box SQLite dynamic queries; single-pass origins reconciliation. |
| **HARN2** | `src/harness` think & prompt | 5 files / 6,216 LOC | `4e9eaeb39` | Reused surviving `ToolCall` blocks; zero-allocation `orphan_result_note`; borrowed `response.text.as_deref()`; eliminated format! macro in single-turn text assembly; ~230 LOC test boilerplate reduction. |
| **POST-CHECK** | `src/guardrails/registry.rs` | 1 file | `dbafb764d` | Fixed simultaneous mutable and immutable borrow of `events` in `screen_user_messages`. |
| **POST-CHECK** | `src/hub/cache.rs` | 1 file | `9a6cd4d4c` | Removed dead `matches_query` helper after SQL lowercased needle extraction. |

---

## 4. Key Bug Fixes & Invariant Tightening

1. **Git Skill Symlink Breakout in Hub (`HUB2`)**:
   - In `src/hub/install.rs`, leaf extraction in git checkouts previously did not guard against directory symlinks pointing outside the checkout tree. Now verifies symlink rejection on `src_leaf` and validates that `canonicalize(src_leaf)` starts with `canonicalize(checkout)`.
2. **Windows Drive Prefix Path Escape (`HUB2`)**:
   - In `src/hub/install.rs`, `subdir` checks previously only checked `starts_with('/')`. Windows paths like `C:\...` could bypass this check. Hardened to check for `:`, absolute paths, and parent directory components.
3. **Schema Property Budget Off-By-One (`HUB2`)**:
   - In `src/hub/hub_catalog.rs`, `validate_config_schema` used saturating subtraction `budget.saturating_sub(map.len())`, falsely rejecting valid schemas containing exactly 1024 properties. Fixed to allow budgets up to the ceiling.
4. **Duplicate Allocation & Cloning in Replayed Message Screening (`GUARD1`)**:
   - `screen_session_input` previously cloned every single replayed user message via `.map(str::to_string)`. Refactored to borrow directly from `events[idx]` and use `Cow<'_, str>` for zero allocations on clean conversations.
5. **Multi-Object Duplication on Session Termination (`HARN1`)**:
   - In `src/harness/agent.rs`, duplicate branches constructed `SessionCompleted` events by cloning `final_text`, `terminate_reason`, `token_breakdown`, and `tool_timeline`. Unified into a single emission path, eliminating 4 major clones.
6. **Double Parse & Format in Harness Prompt Assembly (`HARN2`)**:
   - In `src/harness/agent/prompt.rs`, assistant tool calls were parsed twice per turn. Switched to direct reuse of surviving blocks.
   - In `src/harness/agent/think.rs`, replaced `response.text_content()` string allocations with zero-copy `response.text.as_deref()`.

---

## 5. Verification

1. **Memory-Controlled Full Workspace Compilation**:
   ```bash
   CARGO_BUILD_JOBS=2 cargo check --lib --all-targets
   ```
   Result: **Pass (exit code 0)** in 9m 43s with zero errors across all libraries, binaries, and tests under host memory constraints.

2. **R10 Budget Ratchet Verification**:
   ```bash
   CARGO_BUILD_JOBS=2 cargo test --lib harness::tests::budget
   ```
   Result:
   - `the_harness_is_still_exactly_the_twelve_files_r10_names ... ok`
   - `the_harness_line_budget_does_not_grow ... ok` (Measured line count 5476 vs `CEILING = 5535`, with 59 lines of headroom).

3. **Subsystem Unit Test Suites**:
   - `CARGO_BUILD_JOBS=2 cargo test --lib guardrails`: **55 passed; 0 failed** (0.16s).
   - `CARGO_BUILD_JOBS=2 cargo test --lib hub`: **186 passed; 0 failed** (0.26s).
   - `CARGO_BUILD_JOBS=2 cargo test --lib harness::tests`: **144 passed; 0 failed; 1 ignored** (0.56s).

4. **Final Commit on `main`**: `9a6cd4d4c`
