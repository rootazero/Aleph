# Occam's Razor Remediation: MEM_STORE

## Batch Summary
- Targets: `src/memory/store/markdown/*`, `src/memory/store/sqlite/*`, `src/memory/store/raw_memory.rs`, `src/memory/integration_tests/*`
- Files Touched: 5 files
- Classification source: `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_STORE.class.md` (Jev tier)
- Remediation tier: Deep reasoning (minimax-M3 / compiler-grade invariants)

## Key Remediations
1. **`src/memory/store/markdown/store_impl.rs`**:
   - Eliminated tautological condition `if let Some(started) = Some(metadata.started_at)` (Shape 2: predicate is unconditionally true), directly assigning `metadata.duration_seconds = Some(ended_at - metadata.started_at);`.
2. **`src/memory/store/sqlite/notes/store_impl.rs`**:
   - In `recall_hit_counts`: replaced `chunk.iter().cloned().chain(...)` with `chunk.iter().map(|s| s.as_str()).chain(...)`, eliminating up to 999 heap `String` clones per query chunk.
3. **`src/memory/store/raw_memory.rs`**:
   - Avoided intermediate `String` allocations when parsing `"session_end"` in `from_persisted` by pattern matching on `&str` directly.
4. **`src/memory/store/sqlite/sessions.rs`**:
   - Replaced verbose manual matching on `Err(rusqlite::Error::QueryReturnedNoRows)` with idiomatic `OptionalExtension::optional()` in `get_dream_status` and `get_daily_insight`.
5. **`src/memory/integration_tests/mod.rs`**:
   - Removed stray `println!` in integration tests.
