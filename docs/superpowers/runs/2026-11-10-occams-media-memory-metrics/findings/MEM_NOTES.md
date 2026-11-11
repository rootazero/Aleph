# Occam's Razor Remediation: MEM_NOTES (Core & Ingest/Index)

## Batch Summary
- Targets: `src/memory/notes/links/*`, `src/memory/notes/note/*`, `src/memory/notes/ingest/*`, `src/memory/notes/orientation/*`, `src/memory/notes/query_filer/*`, `src/memory/notes/watcher.rs`
- Files Touched: 9 files
- Classification source:
  - `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_NOTES_CORE.class.md` (Jev tier)
  - `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_NOTES_INGEST_INDEX.class.md` (Jev tier)
- Remediation tier: Deep reasoning (minimax-M3 / compiler-grade invariants)

## Key Remediations
1. **`src/memory/notes/links/resolve.rs`**:
   - Folded full-width character translation and lowercasing into a single pass with pre-allocated capacity in `normalize_link_key`, eliminating intermediate `String` allocation.
2. **`src/memory/notes/note/mod.rs` & `src/memory/notes/note/helpers.rs`**:
   - `KnowledgeNote::to_markdown`: Eliminated owned String clone with `self.note_type.as_deref().unwrap_or(&self.category)`.
   - Replaced 12 consecutive `out.push_str(&format!(...))` calls with direct `writeln!(out, ...)` buffer writing.
   - Simplified `join_body_and_related`: Replaced `Vec<String>` and `.join(" ")` with a single preallocated buffer write.
   - Refactored `push_fact_bullet` to push string slices directly without `format!`.
   - `yaml_inline_array`: Formatted items in a single pass into a preallocated `String` without intermediate `Vec<String>`.
3. **`src/memory/notes/ingest/ref_table.rs`**:
   - Used `value.clone_from(path)` to reuse the existing string buffer rather than reallocating.
4. **`src/memory/notes/ingest/retrieve.rs`**:
   - Borrowed `s.content.as_str()` directly as `&str` when generating candidate preview and bullet summary, avoiding full note markdown cloning.
5. **`src/memory/notes/watcher.rs`**:
   - Walked path components with an iterator (`rel.components()`) with zero heap allocations instead of collecting a `Vec<&str>` on every filesystem event.
6. **`src/memory/notes/orientation/schema.rs` & `src/memory/notes/query_filer/filer.rs`**:
   - Replaced redundant private SHA-256 hex hash implementations with the canonical `crate::memory::notes::indexer::sha2_hash`.
7. **`src/memory/notes/orientation/obsidian_config.rs`**:
   - Replaced blocking synchronous `p.exists()` in async function with `tokio::fs::try_exists(&p).await.unwrap_or(false)`.
