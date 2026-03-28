

# Module: components

## Summary
- Files reviewed: 30
- Issues found: 3
- Issues fixed: 3

## Fixes

1. **`task_planner.rs:230-264`** Logic bug in `identify_parallel_groups` depth calculation → Fixed with proper transitive depth computation using `depth_map: HashMap<String, usize>`
   - **Before**: Depth always equaled `step.depends_on.len()`, so sequential chains A→B→C incorrectly assigned depth 1 to both B and C, grouping them as parallel
   - **After**: Depth = max(depth of dependencies) + 1, correctly producing depths 0, 1, 2 for A→B→C

2. **`session_compactor/model_limits.rs:117-123`** Non-deterministic HashMap iteration for prefix matching → Fixed to always select the longest (most specific) matching prefix
   - **Before**: `for (key, limit) in &self.model_limits` — first match wins, order depends on HashMap internals
   - **After**: Tracks `best_match` by key length, ensuring "claude-3-opus" beats "claude-3" for "claude-3-opus-20240229"

3. **`session_recorder/mod.rs:252-258`** Silent error swallowing with `.unwrap_or(1)` → Fixed to propagate database errors via `map_err()?`
   - **Before**: All query errors silently defaulted sequence to 1, masking DB corruption or schema issues
   - **After**: Properly returns `RecorderError::Database` on failure

## Notes

**Reviewed and found clean:**
- UTF-8 safety: All string truncation in `compactor.rs` correctly uses `is_char_boundary()` pattern (lines 277, 298, 333). No unsafe `&s[..n]` byte slicing found.
- Lock safety: All `Mutex::lock()` calls in `session_recorder` use `.map_err(|e| RecorderError::Lock(...))?` — no `.unwrap()` on locks.
- No `static mut`, no SQL injection (no user-controlled filter strings in format!).
- No `unwrap()`/`expect()` on user-facing paths (timestamps use `.unwrap_or_default()`).

**Architecture observations:**
- `intent_analyzer.rs` uses rule-based keyword matching for complexity detection. The module comment explicitly notes this is intentional (R8 compliance: LLM handles full intent classification, this is only multi-step routing). Acceptable.
- `task_planner.rs:114-172` `infer_tool()` uses keyword matching — has a TODO for LLM-based planning (line 294). Low priority since it's a fallback path.
- `split_by_keyword_case_insensitive()` returns lowercased segments instead of original text — functionally harmless since consumers also lowercase for tool inference. Not worth the complexity of a position-mapping fix.
