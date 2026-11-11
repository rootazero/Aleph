# Batch Findings: ORCH_HARNESS (A & B)

## Scope
- `src/orchestrator/harness_bridge/mod.rs`
- `src/orchestrator/harness_bridge/backfill.rs`
- `src/orchestrator/harness_bridge/behavior_resolve.rs`
- `src/orchestrator/harness_bridge/callback.rs`
- `src/orchestrator/harness_bridge/context_blocks.rs`
- `src/orchestrator/harness_bridge/prompt_build.rs`
- `src/orchestrator/harness_bridge/public_lookup_tool.rs`
- `src/orchestrator/harness_bridge/public_run.rs`
- `src/orchestrator/harness_bridge/replay_adapter.rs`
- `src/orchestrator/harness_bridge/runner_impl.rs`
- `src/orchestrator/harness_bridge/session_seed.rs`

## Actions Applied (Reasoning Tier)
1. **`behavior_resolve.rs`**:
   - Removed dead warmup allocation `let _ = UnifiedMessage::user("warmup");`.
   - Removed unused `UnifiedMessage` import.
2. **`mod.rs`**:
   - Simplified field `estimate_overhead_cache` from `Arc<std::sync::RwLock<HashMap<String, context_estimate::OverheadEstimate>>>` to `Arc<context_estimate::OverheadCache>` via type alias.
3. **`callback.rs`**:
   - Deduplicated `reason.to_string()` in `on_safety_block` into a local `reason_str`.
4. **`context_blocks.rs`**:
   - In `live_deadline_status`, pre-allocated `Vec::with_capacity(2)`.
   - In `render_goal_summary`, replaced multi-line string allocation with streaming `write!(out, ...)` directly into the pre-allocated string buffer.
5. **`backfill.rs`**:
   - In `backfill_events_from_messages`, early exit with `_ => continue` on unrecognized roles, moving `stamp_millis` to be called only for valid roles and avoiding redundant seq increment logic.
6. **`prompt_build.rs`**:
   - Changed `last_user_query(&FlowInput) -> &str` instead of returning owned `String`, avoiding heap allocation on every prompt build step.
   - Used `contains_key(&a.id)` instead of `keys().any(|k| k == &a.id)` in `build_agent_catalog`.
   - Reused allocation buffer in `merge_stable_memory_envelopes`.
7. **`public_lookup_tool.rs`**:
   - Used `concat!("query exceeds 512 bytes").into()` for compile-time string literal.
8. **`public_run.rs`**:
   - Replaced multi-part string allocations in `PUBLIC_LOOKUP_SYSTEM_PROMPT` and `PRIVATE_DRAFT_SYSTEM_PROMPT` with `concat!` string constants.
   - In `classify_harness_error`, passed `llm.name()` directly instead of cloning into intermediate `provider_name` variable.
   - In `run_isolated`, hoisted string construction out of loop to perform a single post-loop allocation.
9. **`replay_adapter.rs`**:
   - In `from_session_request`, checked `req.effective_input.is_none()` early before cloning JSON `Value`.
10. **`session_seed.rs`**:
    - In `seed_messages`, moved `origin` by value on the final iteration instead of cloning.
11. **`runner_impl.rs`**:
    - Parsed `SessionKey` once at line 181 and reused `session_id`.
    - Avoided cloning `serving_hint`.
    - Hoisted canonical key string `session_key_str = session_id.to_key_string()` and reused it across metric gauges, token registries, and run callbacks.
    - Optimized `SessionEvent::AssistantMessage` iteration in event loop: tracked `last_assistant_content: Option<&MessageContent>`, cloning the text/thinking only once after the loop finishes.
    - Moved `gauge_model` and `cost_provider` by value into `FlowOutcome`, removing redundant clones and suppressions.

## Invariant & Boundary Preservations (REJECTED by Occam's Razor)
- **`runner_impl.rs` P1–P10 pipeline architecture**: Preserved all explicit steps, trace telemetry, and error classification without collapsing harness boundary semantics.
- **`prompt_build.rs` template structure**: Preserved prompt layout and section ordering, respecting Redline R9.
- **`replay_adapter.rs` event sequencing**: Preserved exact event emission sequence for durable replay compatibility.
