Pre-existing compilation errors are all outside `core/src/core/` (in `mcp/auth/storage.rs`, `builtin_tools/code_exec.rs`, etc.). The module under review is clean.

---

# Module: core/src/core/

## Summary
- Files reviewed: 3 (`mod.rs`, `capability.rs`, `types.rs`)
- Total lines: ~170
- Issues found: 0
- Issues fixed: 0

## Analysis

### 1. Security & Robustness
- **UTF-8 safety**: No string slicing — clean
- **Lock safety**: No mutex/lock usage — N/A
- **unwrap/expect**: Only in `#[cfg(test)]` code (acceptable) — clean
- **SQL injection**: No DB queries — N/A
- **static mut**: None — clean

### 2. Code Quality
- No dead code or unused imports
- No duplication
- All functions are short and focused
- `CompressionStats.facts_by_type` uses `HashMap` but it's purely data display, not security-rule ordering — acceptable

### 3. Logic Correctness
- `Capability` enum uses discriminant values (0, 1, 2) with derived `PartialOrd`/`Ord` — sort order is deterministic and correct
- `sort_by_priority` takes ownership and returns sorted vec — clean API
- All branches in `parse` and `as_str` are exhaustive
- Test coverage is thorough (parse, display, sort, ord, case-insensitive, invalid input)

### 4. Architecture Compliance
- Pure domain types with no external dependencies beyond `serde` — follows P2 (high cohesion), P6 (simplicity)
- No platform-specific code — complies with R1
- No business logic — complies with P1 (low coupling)

## Notes
This is one of the cleanest modules in the codebase. It's a small, focused domain-types module doing exactly what it should. No changes needed.
