All 374 thinker tests pass. Here's the summary:

# Module: thinker

## Summary
- Files reviewed: 62
- Issues found: 8
- Issues fixed: 8

## Fixes

1. **`prompt_sanitizer.rs:143`** UTF-8 unsafe slice → replaced `&result[..pos]` and `&result[pos + marker.len()..]` with `.get(..).unwrap_or_default()`

2. **`soul.rs:240`** UTF-8 unsafe slice → replaced `&trimmed[3..]` with `trimmed.get(3..).unwrap_or_default()`

3. **`soul.rs:242-243`** UTF-8 unsafe slice → replaced `after_first[..end_pos]` and `after_first[end_pos + 4..]` with `.get(..).unwrap_or_default()`

4. **`user_profile.rs:78`** UTF-8 unsafe slice → replaced `&after_first[..end_pos]` with `.get(..).unwrap_or_default()`

5. **`streaming/block_state.rs:174-184`** UTF-8 unsafe slices in `find_opening_tag()` → all 4 direct slices replaced with `.get(..).unwrap_or_default()`

6. **`streaming/block_state.rs:196-198`** UTF-8 unsafe slices in `find_closing_tag()` → 2 slices replaced with safe `.get()`

7. **`streaming/block_state.rs:209`** UTF-8 unsafe slice in `check_code_fence()` → `self.buffer[3..]` replaced with `.get(3..).unwrap_or_default()`

8. **`mod.rs:207,263,238,270`** HashMap non-deterministic iteration → `keys().next()` replaced with `keys().min()` for deterministic fallback provider selection; `list_providers()` now returns sorted results

## Notes
- The `.expect("registry must have at least one provider")` at `mod.rs:265` was reviewed but kept — it's backed by an invariant enforced in `remove()` which refuses to remove the last provider. Converting the trait to return `Result` would be a large refactor with minimal practical benefit.
- Lock safety is already correct throughout — all locks use `.unwrap_or_else(|e| e.into_inner())`.
- No `static mut` found.
- No SQL injection patterns found (no LanceDB usage in thinker).
- The pre-existing compile error in `agent_init.rs` is unrelated to these changes.
