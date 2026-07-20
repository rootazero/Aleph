# Module: thinker

## Summary
- Path: `src/thinker/` (16 top-level `.rs` files, ~7,751 lines)
- Issues found: 0 high-confidence, 3 informational
- After filtering (threshold=80): 0 fixes required

## Reviewers (4 parallel)
- Security (UTF-8 safety, locks, unwrap, static mut, races)
- Logic (state machines, error propagation, boundaries, off-by-one)
- Architecture (R1/R3/R4/R8/R9/R10 redlines + P1-P6)
- Quality (dead code, DRY, function length, HashMap order, pub scope)

## High-Confidence Issues
None.

## Informational Observations (no action)

### 1. Lock pattern conformance
- **File**: `src/thinker/{mod,runtime_context,prompt_builder/cache_monitor}.rs`, `src/thinker/memory_context_provider/{mod,tests}.rs`
- **Severity**: n/a (positive observation)
- **Evidence**: Every `lock().unwrap()` across the module uses `unwrap_or_else(|e| e.into_inner())` — the recommended pattern from checklist §1. Lock poisoning will not cascade across threads.

### 2. UTF-8 budget math
- **File**: `src/thinker/prompt_budget.rs:124-130`, `src/verification/extension_stop_gate.rs:95-104`
- **Severity**: n/a (positive observation)
- **Evidence**: `char_byte_offset()` uses `char_indices().nth(n).map_or(s.len(), |(i, _)| i)` — UTF-8 safe by construction. `truncate_chars` walks back with `is_char_boundary(end)` before `&s[..end]`. `truncate_with_head_tail` uses `saturating_sub` everywhere and `char_byte_offset`. No panic on multi-byte text.

### 3. HashMap ordering in security-sensitive paths
- **File**: `src/thinker/mod.rs:204-229`, `src/thinker/runtime_context.rs:167`, `src/thinker/prompt_builder/cache_monitor.rs:55`
- **Severity**: n/a (no action)
- **Evidence**: All HashMaps are either provider-registry keyed by model name (not iterated for security decisions), cache-keyed for memoization (no security impact), or ephemeral. No R8/R10 violation.

### 4. R1/R3/R4/R8/R9/R10 redlines
- **Severity**: n/a (clean)
- **Evidence**:
  - **R1** (no platform APIs): `grep -E 'cocoa|objc|metal|appkit|coregraphics|...' src/thinker` → 0 matches.
  - **R3** (no heavy deps): no `reqwest|isahc|hyper|tonic|tensorflow|ort|burn|candle`.
  - **R4** (no interface business logic): no imports from `interfaces/*`.
  - **R8** (no deterministic intent routing): no `regex::|Regex::new|RegexBuilder::new|RegexSet` usage anywhere in the module.
  - **R9**: configurable operations exposed via `ToolService` (out of scope) — no leaky tools here.
  - **R10**: no middleware cognitive judgment — all reasoning delegated to the LLM via prompts.

## Production-grade patterns observed
- Heavy use of `BTreeMap` where determinism matters (e.g., `markdown_skill/spec.rs` — outside thinker but related).
- `OnceLock` / `LazyLock` for global state (e.g., `REPO_ROOT_CACHE`).
- `saturating_sub` arithmetic on `usize` to prevent underflow.
- `tracing` for structured logging instead of `println!`.
- `# Errors` doc sections on public APIs that return `Result`.

## Conclusion
`src/thinker/` is well-disciplined and matches project redlines. No changes required.
