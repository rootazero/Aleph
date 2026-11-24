# Occam's Razor Remediation: MEM_DREAMING (Core & Stages)

## Batch Summary
- Targets: `src/memory/dreaming/*`, `src/memory/dreaming/evolution/*`, `src/memory/dreaming/stages/*`
- Files Touched: 9 files
- Classification source:
  - `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_DREAMING_CORE.class.md` (Jev tier)
  - `docs/superpowers/runs/2026-11-10-occams-media-memory-metrics/classification/MEM_DREAMING_STAGES.class.md` (Jev tier)
- Remediation tier: Deep reasoning (minimax-M3 / compiler-grade invariants)

## Key Remediations
1. **`src/memory/dreaming/distill_action.rs`**:
   - Replaced JSON AST serialization via `serde_json::to_value` with direct `match s { Severity::... => "..." }` mapping to static string slices.
2. **`src/memory/dreaming/mutation_gate.rs`**:
   - Eliminated heap allocation of `Vec<&HashSet<_>>` in `detect_merge_cycle` by iterating directly with index arithmetic over `self.merge_history`.
   - Replaced `collect::<Vec<&HashMap<_>>>()` in `detect_synthesis_churn` with zero-allocation `.front()` and `.skip(1)`.
   - Used `saturating_add` in `detect_wasted_distillation` for overflow protection.
3. **`src/memory/dreaming/evolution/score.rs`**:
   - Eliminated redundant `union` and `difference` set passes: computed `union = kf.len() + af.len() - overlap` and `novel_in_absorbed = af.len() - overlap` in O(1).
   - Replaced lowercased heap string allocations in `share_leading_token` with `eq_ignore_ascii_case`.
4. **`src/memory/dreaming/validation.rs`**:
   - Replaced per-note `HashSet` collection of `CATEGORY_DIRS` with zero-allocation `CATEGORY_DIRS.contains(&cat)`.
   - Refactored `extract_yaml_value` to take and return string slices (`&'a str`), eliminating intermediate string formatting.
5. **`src/memory/dreaming/stages/note_decay.rs`**:
   - Keyed `last_hits` with `&str` instead of `String`, eliminating string clones for every note in the vault.
   - Replaced `candidates: Vec<(String, String, String)>` with `candidate_indices: Vec<usize>` and borrowed `&str` slices, eliminating 3 heap allocations per note.
6. **`src/memory/dreaming/stages/note_weave.rs`**:
   - Replaced `&Vec::<String>::new()` with `&[]` in `write_typed_link`.
   - Borrowed `&String` in `orphan_batch` instead of cloning into an owned vector.
7. **`src/memory/dreaming/stages/mention_weave.rs`**:
   - Moved `note.title`, `note.aliases`, and `note.links` directly into `MentionDoc` without clones.
8. **`src/memory/dreaming/stages/corpus_narrative.rs`**:
   - Retained borrowed `&str` slices for `overview_body` and `purpose_body` rather than converting to owned `String`s.
9. **`src/memory/dreaming/stages/mod.rs`**:
   - Streamed `render_rejected_block` directly into a single preallocated buffer via `writeln!`, eliminating intermediate `Vec<String>` and `.join()`.
