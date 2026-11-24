# Occam's Razor Remediation: MEM_RETRIEVAL_RERANK

## Batch Summary
- Targets: `src/memory/note_retrieval/*`, `src/memory/rerank/*`, `src/memory/curated/*`, `src/memory/scratchpad/*`
- Files Touched: 5 files
- Classification source: `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_RETRIEVAL_RERANK.class.md` (Jev tier)
- Remediation tier: Deep reasoning (minimax-M3 / compiler-grade invariants)

## Key Remediations
1. **`src/memory/note_retrieval/scoring.rs` & `src/memory/note_retrieval/pipeline.rs`**:
   - Generalized `mmr_reorder` to accept `&[T]` where `T: AsRef<str>`, allowing `pipeline.rs` to pass borrowed `&str` references directly and eliminating cloning of full note markdown strings.
   - Refactored `apply_rerank` to reuse indexed `slots: Vec<Option<ScoredFact>>`, eliminating intermediate `HashMap<String, ScoredFact>` heap allocation and string-key lookups.
2. **`src/memory/rerank/mod.rs`**:
   - Replaced O(N*M) linear scan in `blend_scores` with a pre-indexed HashMap lookup table, reducing score blending to O(N + M).
3. **`src/memory/curated/store.rs`**:
   - Fixed `CuratedError::NoMatch` in `apply_one` to report the unmatched `old_text` rather than the replacement `content`.
4. **`src/memory/scratchpad/manager.rs`**:
   - Scoped the 500-note cap line scanner in `append_note` strictly to the `## Notes` section via `extract_section`, preventing checklist items in `## Plan` from erroneously exhausting the note quota.
