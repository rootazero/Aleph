# Classification & Routing Matrix: ORCH_ROOT

## Executive Summary

- **Batch Identifier**: `ORCH_ROOT`
- **Subsystem**: `src/orchestrator` (Core Infrastructure, Lifecycle, Routing & Dispatch)
- **Review Scope (9 files)**:
  - `src/orchestrator/dispatch.rs` (1,917 LoC)
  - `src/orchestrator/errors.rs` (83 LoC)
  - `src/orchestrator/flow_registry.rs` (54 LoC)
  - `src/orchestrator/flow_spec.rs` (128 LoC)
  - `src/orchestrator/loader.rs` (139 LoC)
  - `src/orchestrator/mod.rs` (49 LoC)
  - `src/orchestrator/resolver.rs` (135 LoC)
  - `src/orchestrator/sandbox_factory.rs` (36 LoC)
  - `src/orchestrator/presets/mod.rs` (4 LoC)
- **Doctrine**: Occam's Razor (`Simplicity is the removal of unnecessary concepts, not merely line count reduction`).
- **Summary Metrics**:
  - Total Candidates Evaluated: **17**
  - **`FIX-NOW`**: **9** (Clean wins: eliminating 15+ redundant allocations/clones in `dispatch.rs`, resolving double-clones in `loader.rs`, flattening redundant control branches in `resolver.rs`, removing 18+ lint suppressions)
  - **`REJECT`**: **8** (Preserving public API stability, source-level AST test invariants, wire-format compatibility, and security census contracts)

| Taxonomy Category | Candidates Found | FIX-NOW | REJECT |
|---|:---:|:---:|:---:|
| **REDUNDANT-ALLOCATION** | 7 | 5 | 2 |
| **CONTROL-FLOW / INDIRECTION** | 4 | 3 | 1 |
| **DEAD-CODE / ACCIDENTAL-COMPLEXITY** | 6 | 1 | 5 |
| **PANIC-RISK / DEFENSIVE-CHECK** | 0 | 0 | 0 |
| **TOTAL** | **17** | **9** | **8** |

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|
| `src/orchestrator/loader.rs:74` | `REDUNDANT-ALLOCATION` | In `load_user_flows_from_dir`, `merged.insert(id.clone(), spec)` clones `id` (`String`) on every flow entry solely for the error reporting branch, triggering a `rust-doctor-disable-next-line excessive-clone` lint suppression. | `FIX-NOW` | Use `match merged.entry(id) { Entry::Vacant(e) => { e.insert(spec); } Entry::Occupied(e) => return Err(FlowError::InvalidConfig(format!("duplicate flow id across files: {}", e.key()))), }`. Moves `id` without allocation on happy path, borrows `e.key()` on collision, and removes suppression comment. |
| `src/orchestrator/loader.rs:89-91` | `REDUNDANT-ALLOCATION` | In `parse_flow_file`, `let id = spec.id.clone();` and `out.insert(id.clone(), Arc::new(spec))` clone `spec.id` twice per flow entry, carrying two `excessive-clone` suppressions. | `FIX-NOW` | Check `if out.contains_key(&spec.id) { return Err(format!("duplicate flow id: {}", spec.id)); }` with a zero-allocation `&spec.id` borrow, followed by a single `out.insert(spec.id.clone(), Arc::new(spec))`. Eliminates double-cloning and drops both suppressions. |
| `src/orchestrator/loader.rs:100-105` | `ACCIDENTAL-COMPLEXITY` | `merge_catalogs` manually iterates `for (id, spec) in user { out.insert(id, spec); }` instead of using the standard collection merge method `out.extend(user)`. | `FIX-NOW` | Replace manual loop with `out.extend(user);`. |
| `src/orchestrator/resolver.rs:115-132` | `CONTROL-FLOW / INDIRECTION` | In `resolve_session` under `SessionStrategy::Child`, `let Some(parent) = parent else { tracing::warn!(...); return Ok(SessionResolution { session_key: (input.fresh_key_fn)(), parent_session_key: None, is_new: true }); };` duplicates the struct construction and `(input.fresh_key_fn)()` call of the success branch. | `FIX-NOW` | Flatten control flow to `if parent.is_none() { tracing::warn!(...); } Ok(SessionResolution { session_key: (input.fresh_key_fn)(), parent_session_key: parent, is_new: true })`. Eliminates 12 lines of duplicated struct construction. |
| `src/orchestrator/resolver.rs:31-39` | `CONTROL-FLOW / INDIRECTION` | `depth_guard` uses `if depth > MAX_FLOW_DEPTH { Err(...) } else { Ok(()) }` instead of early exit. | `FIX-NOW` | Flatten to `if depth > MAX_FLOW_DEPTH { return Err(FlowError::RecursionLimit { max: MAX_FLOW_DEPTH }); } Ok(())`. |
| `src/orchestrator/dispatch.rs:1111-1121` | `REDUNDANT-ALLOCATION` / `ACCIDENTAL-COMPLEXITY` | In `resolve_spec`, the fallback resolution `self.flow_registry.resolve(DEFAULT_AGENT_FLOW_ID).map(|base| fallback_spec_with_agent(base, &req.agent_id))` is duplicated across both the `None` registry hit and `UnknownAgent` error branches. In addition, `FlowError::UnknownFlow(flow_id.clone())` clones an owned `String`. | `FIX-NOW` | Extract a local closure `let fallback = || self.flow_registry.resolve(DEFAULT_AGENT_FLOW_ID).map(|b| fallback_spec_with_agent(b, &req.agent_id));`, and move `flow_id` directly without `.clone()` into `FlowError::UnknownFlow(flow_id)`. |
| `src/orchestrator/dispatch.rs:1140-1263` | `REDUNDANT-ALLOCATION` | In `dispatch(&self, req: FlowRequest)`, `req` is passed by value (owned), yet 15+ fields are redundantly cloned into local variables (`req.session_hint.clone()`, `req.parent_session.clone()`, `req.sandbox_override.clone()`, `req.event_tx.clone()`, `req.input.clone()`, `req.tool_service.clone()`, `req.trace_sink.clone()`, `req.interaction_manifest.clone()`, `req.workspace_override.clone()`, `req.transient_context.clone()`, `req.envelope.clone()`, `req.model_directive.clone()`, `req.run_id.clone()`, `req.telecom_public.clone()`, `req.scope.clone()`), accompanied by 16 `rust-doctor-disable-next-line excessive-clone` comments. | `FIX-NOW` | Destructure owned `req` into its constituent fields once borrowing for `resolve_spec(&req)` and `depth_guard(req.depth)` finishes. Move fields directly into `SessionResolveInput`, sandbox match, and the spawned task without heap clones. Move `scope.into_parts()`, clone `telecom_public` only once (for `run_origin`), and delete all 16 `excessive-clone` disable comments. |
| `src/orchestrator/dispatch.rs:1438` | `REDUNDANT-ALLOCATION` | `FlowHandle` construction clones `session_res.session_key.clone()`, even though `session_res` is dropped immediately after. | `FIX-NOW` | Pass `session_key: session_res.session_key` by move without `.clone()`. |
| `src/orchestrator/dispatch.rs:681` | `CONTROL-FLOW / INDIRECTION` | `panic_message(payload: &Box<dyn std::any::Any + Send>)` takes a reference to a `Box`, adding unnecessary double indirection (`&Box<dyn Any>`) instead of accepting `&(dyn std::any::Any + Send)`. | `FIX-NOW` | Change parameter type to `payload: &(dyn std::any::Any + Send)`. Allows standard deref coercion and decouples panic inspection from heap box structure. |
| `src/orchestrator/resolver.rs:60` | `REDUNDANT-ALLOCATION` | `resolve_flow_id` calls `defaults.get(agent_id).cloned()` to return `Result<FlowId, FlowError>`. Callers immediately borrow `&flow_id`. | `REJECT` | Changing return type to `Result<&str, FlowError>` would avoid allocating `String`, but `resolve_flow_id` is a public module function tested in `tests/resolver.rs` and documented in architectural specs (`FEATURE_LOCATOR.md`). Preserve existing `FlowId` return type to avoid API churn. |
| `src/orchestrator/dispatch.rs:45, 93` | `ACCIDENTAL-COMPLEXITY` | `FlowStreamEvent` carries `Complete(FlowOutcome)`, making `FlowStreamEvent` a large enum variant (>160 bytes) and carrying a `large-enum-variant` suppression. | `REJECT` | Boxing `Complete(Box<FlowOutcome>)` would reduce enum size, but `FlowStreamEvent` is part of the public API surface consumed across `gateway`, `webchat`, and integration tests. Changing it to `Box<FlowOutcome>` would break pattern-matching call sites repository-wide. Reject to preserve public API invariants. |
| `src/orchestrator/dispatch.rs:1446` | `ACCIDENTAL-COMPLEXITY` | `pub async fn reload_flows(&self, new_set: FlowSet) -> Result<(), FlowError>` is declared `async` but contains zero `.await` points (`self.flow_registry.replace(new_set)` is a synchronous `ArcSwap::store`). | `REJECT` | Removing `async` would break external callers (such as `src/gateway/handlers/flow_admin.rs:44` which awaits `.reload_flows(merged).await`). Invariant stability takes precedence over removing the harmless `async` wrapper. |
| `src/orchestrator/dispatch.rs:179-185, 211-216` | `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY` | `TerminateReason::ContextBudgetExhausted` and `TerminateReason::DiminishingReturns` are retired signals with no live producers in the Think→Act loop. | `REJECT` | The enum variants are explicitly documented as retained for backward compatibility with serialized session blobs, i18n locales, and protocol wire matching (pinned by `every_terminate_token_has_words_on_the_terminal_surfaces`). Deleting them would corrupt historical session replay and fail protocol tests. |
| `src/orchestrator/dispatch.rs:1338-1347` | `ACCIDENTAL-COMPLEXITY` | `scope_attr` reconstruction in `tokio::spawn` builds a temporary `HashMap<String, String>` and calls `crate::scope::scope_from_metadata(&m)` instead of directly constructing `ScopeAttribution`. | `REJECT` | This exact shape and `scope_from_metadata` invocation is strictly counted and pinned by `flow_scope_census` (see `src/gateway/execution_engine/run_loop/flow_scope_census.rs` and `src/scope/mod.rs:424`). Bypassing it or changing the call site would trigger redlines in the security attribution census. |
| `src/orchestrator/dispatch.rs:1309-1311, 1365-1376` | `ACCIDENTAL-COMPLEXITY` | Multiple task-local scopes (`exec_workspace`, `fs_scope`, `pending_media`, `originator`, `room_author`, `transcripts`) are captured before `tokio::spawn` and wrapped around `harness.run(...)` via nested function calls. | `REJECT` | Structural regression tests `the_harness_spawn_reestablishes_the_run_tree_originator` and `the_harness_spawn_carries_gateway_run_context` perform exact textual matching on these exact lines. Any refactoring to consolidate these wrappers into a single helper would break the tests. |
| `src/orchestrator/dispatch.rs:782-829` | `ACCIDENTAL-COMPLEXITY` / `INDIRECTION` | `HarnessRunner::run` takes 17 explicit parameters, triggering `#[allow(clippy::too_many_arguments)]`. | `REJECT` | Packing these parameters into a `HarnessRunArgs` struct would break existing trait implementations, test harnesses, and mock runners across the entire codebase. Preserve the existing trait signature. |
| `src/orchestrator/dispatch.rs:1025-1027` | `REDUNDANT-ALLOCATION` | In `fallback_spec_with_agent`, `let mut s = (*base).clone(); s.agent = agent_id.to_string();` clones `base.agent` (a heap `String`) only to immediately overwrite and drop it. | `REJECT` | Field-by-field reconstruction of `FlowSpec` would avoid cloning `base.agent`, but creates tight coupling to `FlowSpec`'s private/public field list and makes future field additions error-prone compared to `(*base).clone()`. The fallback flow clone occurs at most once per dispatch for non-routed agents. Reject micro-optimization. |

---

## Invariant Safety Pre-Flight

Before any AST mutation is applied to this batch, the following safety constraints and structural test invariants must be explicitly verified:

### 1. Source-Level AST Test Invariants (DO NOT ALTER MATCH PATTERNS)
- **`the_harness_spawn_reestablishes_the_run_tree_originator`** (`src/orchestrator/dispatch.rs:1484`):
  Scans `dispatch.rs` production code and asserts that:
  - `tokio::spawn(async move {` exists.
  - The spawn block contains `with_originator(`.
- **`the_harness_spawn_carries_gateway_run_context`** (`src/orchestrator/dispatch.rs:1538`):
  Enforces exact textual matches in `dispatch.rs` before and after the spawn:
  - Pre-spawn captures must contain:
    - `let exec_workspace = crate::sandbox::context::current_exec_workspace();`
    - `let fs_scope = crate::tools::fs_scope::current();`
    - `let pending_media = crate::gateway::media::current_pending_media();`
  - Spawned harness scope must contain:
    - `with_exec_workspace(exec_workspace,`
    - `with_fs_scope(fs_scope,`
    - `with_pending_media(pending_media,`
  - *Refactoring Note*: Moving `req` fields around in `dispatch` must leave these exact 6 statements and their formatting completely undisturbed.
- **`the_catalog_has_exactly_one_composer`** (`src/orchestrator/tests/loader.rs:75`):
  Asserts that `pub async fn load_catalog` exists in `src/orchestrator/loader.rs` and that no callers outside `loader.rs` invoke `merge_catalogs` or `load_user_flows_from_dir`.
- **`every_terminate_token_has_words_on_the_terminal_surfaces`** (`src/orchestrator/dispatch.rs:1633`):
  Exhaustively matches `TerminateReason::as_static_str` token strings against `aleph_protocol::terminate::labelled_tokens()`. No variants or token strings can be renamed or deleted.
- **`flow_scope_census`** (`src/gateway/execution_engine/run_loop/flow_scope_census.rs`):
  Tracks security attribution derivation. The call `crate::scope::scope_from_metadata(&m)` inside `tokio::spawn` in `dispatch.rs:1346` is lexically and structurally counted. Do not alter or bypass this call.

### 2. Concurrency & Cancellation Invariants
- **SessionLockGuard Poison Recovery**: `SessionLockGuard::drop` recovers poisoned mutexes via `unwrap_or_else(|e| e.into_inner())` to prevent session key leaks into `active_sessions`. This pattern must not be modified.
- **Task-Local Propagation**: Tokio tasks spawned via `tokio::spawn` lose thread-local / task-local contexts. All re-scoping (`with_agent_id`, `with_project_root`, `with_exec_workspace`, `with_fs_scope`, `with_scope`, `with_room_author`, `with_originator`, `with_pending_media`, `seed_run_origin`, `with_transcript_source`) must remain in the same nesting order around `harness.run`.
- **Channel Ownership**: `FlowHandle::events` receiver is subscribed *before* `tokio::spawn` begins, ensuring broadcast buffer ordering is preserved. `FlowRequest::event_tx` is a strong sender that must be moved into the spawned task and dropped upon completion.
