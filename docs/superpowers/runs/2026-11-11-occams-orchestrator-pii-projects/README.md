# Occam's Razor Review Run: `src/orchestrator`, `src/pii`, `src/projects`

## Run Metadata
- **Date**: 2026-11-11
- **Target Modules**:
  - `src/orchestrator` (root, deps_builder, harness_bridge)
  - `src/pii` (allowlist, engine, rules)
  - `src/projects` (store, backfill, bindings)
- **Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-orch-pii-proj`
- **Branch**: `review/occams-orch-pii-projects` -> `main`

---

## Architecture & Model Tier Allocation

This review followed the dual-tier subagent workflow:

1. **Jev Model Tier (`flash` model / Fast Categorization & Filtering)**:
   - **Task**: Static AST inspection, smell detection against Occam's razor catalog, triage and classification into FIX-NOW vs REJECT.
   - **Execution**: 6 subagents ran in parallel across 6 scoped batches (`PII`, `PROJECTS`, `ORCH_ROOT`, `ORCH_DEPS`, `ORCH_HARNESS_A`, `ORCH_HARNESS_B`).
   - **Artifacts Delivered**: 6 markdown classification reports stored in `classification/*.class.md` (total 109 candidate smells cataloged).

2. **Reasoning Model Tier (Senior Reviewer & Code Mutation)**:
   - **Task**: Invariant validation against Redlines (R1-R10), cross-crate boundary contract checks (e.g. `src/gateway/`), AST code refactoring, zero-cost abstraction transformations, and per-batch commits.
   - **Decisions**: Rejection of harmful simplifications (e.g., breaking cross-module signatures or test invariants); execution of verified simplifications directly on code.

---

## Batch Commit Manifest

| Batch | Scope | Commit Hash | Key Simplifications |
|---|---|---|---|
| **PII** | `src/pii` | `f711a28a5` | `&'static [Regex]` slices eliminating `Arc` heap allocation; stack reverse iterator Luhn check; stack byte buffers for ID card validation; pre-hoisted severity conversions. |
| **PROJECTS** | `src/projects` | `38501a4cf` | Direct `aleph_protocol::scope::project_scope_id` derivation; `rows.collect::<Result<Vec<_>, _>>()`; `Cow<'_, str>` zero-copy queries; boolean probes `SELECT current_session_key IS NOT NULL`. |
| **ORCH_ROOT** | `src/orchestrator` (root) | `202cdc3ac` | `Entry` API in `load_user_flows_from_dir`; `presets.extend(user)`; early depth guard exits; pass `FlowRequest` by value moving 15+ fields and eliminating 16 clone suppressions. |
| **ORCH_DEPS** | `src/orchestrator/deps_builder` | `7ec03ef3a` | In-place model window evaluation; zero-alloc ASCII case-insensitive comparison; `effective_summary_model` returning borrowed references; derived `Default` for `StabilityTriple`. |
| **ORCH_HARNESS** | `src/orchestrator/harness_bridge` | `a6b31899e` | Removed dead warmup allocations; single canonical session key hoisting; delayed assistant message text extraction to end of event loop; moved `serving_model` & `serving_provider` by value; compile-time `concat!` prompts. |

---

## Invariant Preservation & Negative Scope
- **Preserved `conversation_of` signature in `src/projects`**: Kept returning `Option<(String, BindingPeerKind, String)>` to preserve cross-module contract with `src/gateway/handlers/projects_channel.rs:220`.
- **Preserved literal AST tokens in `src/orchestrator/dispatch.rs`**: Maintained tokens asserted by source-verification unit tests.
- **Preserved Redline R1/R9 boundaries**: No prompt templating logic moved into harness deterministic logic.
