# Logic Review Report
**Module**: thinker
**Scope**: Full module static audit — 86 files across src/thinker/ (root, layers/, prompt_builder/, memory_context_provider/, streaming/, hooks/)
**Date**: 2026-05-22
**Mode**: strict

## Findings

### [Critical] Division-by-zero in truncate_with_head_tail
- **Location**: `src/thinker/prompt_budget.rs:96`
- **Trigger condition**: Call `truncate_with_head_tail(content, max, 0.0, 0.0)` with both ratios set to 0.0
- **Expected behavior**: Function should gracefully handle zero ratios, perhaps splitting evenly
- **Actual behavior**: `head_ratio + tail_ratio == 0.0` causes division by zero: `(usable as f64 * head_ratio / (head_ratio + tail_ratio)) as usize`
- **Suggested fix**: Guard against zero sum. Split evenly when both ratios are 0.0.
- **Status**: ✅ Fixed in commit 21140c2a4

### [Critical] Layer count assertion out of sync with default_layers()
- **Location**: `src/thinker/prompt_pipeline.rs:483`
- **Trigger condition**: Running `cargo test test_default_layers_count`
- **Expected behavior**: Test assertion should match actual layer count
- **Actual behavior**: Test asserts `layer_count() == 37`, but `default_layers()` registers 38 layers (ToolRuntimeStateLayer was added after the comment was last updated)
- **Suggested fix**: Update assertion to 38 and refresh comment documenting the layer count evolution
- **Status**: ✅ Fixed in commit 21140c2a4

### [Warning] Token estimation uses byte length instead of char count
- **Location**: `src/thinker/cache.rs:109`
- **Risk**: CJK text (3 bytes per char) will be over-estimated by ~3x, causing cache threshold (`MIN_CACHE_TOKENS = 1024`) to be exceeded prematurely. English text with multi-byte chars (emojis, smart quotes) is also affected.
- **Current impact**: Medium — affects cache enablement decisions for non-ASCII prompts
- **Suggestion**: Use `system_prompt.chars().count()` instead of `system_prompt.len()` for token estimation
- **Status**: ✅ Fixed in commit 21140c2a4

### [Warning] Partial tag threshold too low for attributes
- **Location**: `src/thinker/streaming/block_state.rs:94,114`
- **Risk**: Hard-coded 20-byte threshold for detecting partial XML tags. A tag like `<antthinking attr="value">` exceeds 20 bytes, causing the parser to emit incomplete tag content instead of waiting for more input.
- **Current impact**: Medium — streaming thinking-block detection may split mid-tag
- **Suggestion**: Increase threshold to 64 bytes to accommodate tag names + attributes
- **Status**: ✅ Fixed in commit 21140c2a4

### [Warning] Potential overflow in token budget calculation
- **Location**: `src/thinker/memory_context_provider/memory.rs:30`
- **Risk**: `(self.config.max_output_chars as u64 * 2 / 3) as u32` — if `max_output_chars` is near `u64::MAX`, the multiplication can overflow before division. While `max_output_chars` is realistically bounded, the code lacks defensive arithmetic.
- **Current impact**: Low — `max_output_chars` defaults to 8000, far from overflow territory
- **Suggestion**: Use `saturating_mul` and `saturating_div` for defensive integer math
- **Status**: ✅ Fixed in commit 21140c2a4

## Summary

| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 3 |
| Suggested Test | 0 |

## Automated Verification

- `cargo check -p alephcore` — ✅ Passed (3m 17s, 6 pre-existing dead_code warnings unrelated to thinker)
- `cargo test` — Blocked by pre-existing compilation error in `src/utils/pii.rs` (unrelated uncommitted change), not caused by thinker modifications

## Files Modified

| File | Change |
|------|--------|
| `src/thinker/prompt_budget.rs` | Guard division-by-zero in `truncate_with_head_tail` |
| `src/thinker/prompt_pipeline.rs` | Update layer count assertion 37→38 |
| `src/thinker/cache.rs` | Use `chars().count()` for CJK-safe token estimation |
| `src/thinker/streaming/block_state.rs` | Increase partial-tag threshold 20→64 |
| `src/thinker/memory_context_provider/memory.rs` | Use saturating arithmetic for token budget |

## Aleph-Specific Invariant Checklist

| Invariant | Status | Notes |
|-----------|--------|-------|
| R1 Brain-Limb Separation | ✅ Pass | No platform APIs in thinker/ |
| R8 LLM Sovereignty | ✅ Pass | No regex-based intent routing |
| Sync Primitives Import Rule | ✅ Pass | Uses `crate::sync_primitives` for Arc/Mutex/RwLock |
| Lock Hierarchy | ✅ Pass | No cross-hierarchy lock acquisitions observed |
| TOCTOU Prevention | ✅ Pass | No check-then-act patterns on shared state |
| Memory Namespace Isolation | ✅ Pass | MemoryContextProvider uses proper namespace scoping |

## Cross-Module Findings

None — all findings are contained within the thinker module.
