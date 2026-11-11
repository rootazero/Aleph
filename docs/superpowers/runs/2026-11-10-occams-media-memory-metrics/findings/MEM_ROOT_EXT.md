# Occam's Razor Remediation: MEM_ROOT_EXT

## Batch Summary
- Targets: `src/memory/content_scanner.rs`, `src/memory/session_memory_mode.rs`, `src/memory/namespace.rs`, `src/memory/project_scope.rs`, `src/memory/extensions/registry.rs`, `src/memory/flush/registry.rs`
- Files Touched: 6 files
- Classification source: `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_ROOT_EXT.class.md` (Jev tier)
- Remediation tier: Deep reasoning (minimax-M3 / compiler-grade invariants)

## Key Remediations
1. **`src/memory/content_scanner.rs`**:
   - Added ASCII fast-path to skip `normalize_homoglyphs` and heap String allocations on clean ASCII content.
2. **`src/memory/session_memory_mode.rs`**:
   - Replaced `.to_ascii_lowercase()` heap allocation with zero-allocation `eq_ignore_ascii_case` comparisons in `from_id`.
3. **`src/memory/namespace.rs`**:
   - Derived `Copy` on `NamespaceScope` and added zero-allocation `as_str() -> &'static str`.
4. **`src/memory/project_scope.rs`**:
   - Replaced multi-step loop allocations in `project_namespace` with a single `format!("proj-{:02x}{:02x}{:02x}{:02x}", ...)` expression.
   - Refactored `owner_adoption_base` to strip suffixes via slice methods without string formatting.
5. **`src/memory/extensions/registry.rs`**:
   - Borrowed `ext.name()` as `&str` directly in dispatch logging across `on_retrieve`, `on_capture`, `on_session_switch`, `on_pre_compress`, and `on_delegation`, eliminating 5 repeated heap allocations per hook iteration.
6. **`src/memory/flush/registry.rs`**:
   - Used `map.get_mut(agent)` in `FlushRegistry::begin` before inserting, avoiding allocating `agent.to_string()` on concurrent flushes.
