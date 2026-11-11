# Occam's Razor Remediation: Batch ORCH_ROOT (`src/orchestrator/*.rs`)

## 1. Summary of Changes
- **`src/orchestrator/loader.rs`**:
  - In `load_user_flows_from_dir`, used `Entry` API (`Entry::Vacant` / `Entry::Occupied`) on `merged`, eliminating `id.clone()` and removing lint suppression comment.
  - In `parse_flow_file`, checked `out.contains_key(&spec.id)` before insertion, eliminating double-clone of `spec.id` and removing 2 lint suppression comments.
  - In `merge_catalogs`, replaced manual loop with `presets.extend(user)`.
- **`src/orchestrator/resolver.rs`**:
  - In `depth_guard`, simplified to early exit `if depth > MAX_FLOW_DEPTH { return Err(...); } Ok(())`.
  - In `resolve_session`, unified `SessionStrategy::Child` fallback to single `SessionResolution` instantiation, removing 12 lines of duplicated code.
- **`src/orchestrator/dispatch.rs`**:
  - In `panic_message`, changed parameter to `payload: &(dyn std::any::Any + Send)`, removing unnecessary Box indirection.
  - In `resolve_spec`, extracted local fallback closure, eliminating duplicated lookup and removing `flow_id.clone()` allocation on missing flow.
  - In `dispatch`, destructured `req: FlowRequest`, moving fields (`input`, `tool_service`, `trace_sink`, `interaction_manifest`, `workspace_override`, `transient_context`, `envelope`, `model_directive`, `run_id`, `scope`) directly without intermediate clones, and removed 16 `excessive-clone` lint suppressions.
  - In `FlowHandle` return, moved `session_res.session_key` by value without cloning.

## 2. Invariant Safety Pre-Flight
- Textual patterns pinned by `the_harness_spawn_reestablishes_the_run_tree_originator` and `the_harness_spawn_carries_gateway_run_context` (`with_originator`, `with_exec_workspace`, `with_fs_scope`, `with_pending_media`) were preserved verbatim.
- `scope_from_metadata` security census call in `dispatch.rs` preserved intact.
- Retained `TerminateReason` backward compatibility variants and unboxed `FlowStreamEvent::Complete(FlowOutcome)`.
