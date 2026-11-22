# Occam's Razor Classification: MEM_DREAMING_STAGES

**Batch ID**: `MEM_DREAMING_STAGES`  
**Subsystem**: `src/memory/dreaming/stages/`  
**Scope**: 21 files (`co_recall_edges.rs`, `corpus_narrative.rs`, `daily_digest.rs`, `feedback_distill.rs`, `goal_lessons_promote.rs`, `graph_recompute.rs`, `graph_recompute_lifecycle_tests.rs`, `index_refresher.rs`, `mention_weave.rs`, `mod.rs`, `note_consolidate.rs`, `note_decay.rs`, `note_drift.rs`, `note_lint.rs`, `note_review.rs`, `note_synthesis.rs`, `note_weave.rs`, `skill_distill.rs`, `skill_lifecycle.rs`, `tool_failure_distill.rs`, `workflow_proposal.rs`)  
**Role**: Jev Static Pattern Reviewer  
**Doctrine**: `/rust-occams-razor` & [BRIEF_COMMON.md](../BRIEF_COMMON.md)

---

## Executive Summary

The `MEM_DREAMING_STAGES` batch implements background memory evolution and maintenance stages (decay, consolidation, drift detection, graph recompute, linking, and distillation). 

Scanning revealed a widespread pattern of **suppressed lints instead of root-cause simplification**: across this module alone, **over 40 `// rust-doctor-disable-next-line excessive-clone` directives** were installed to silence static analyzers rather than borrowing slices or moving owned fields. Key findings include:
1. **Unnecessary String and Vec Allocations in Inner Loops**:
   - `note_decay.rs` clones 3 `String`s for every non-archived note into a temporary candidate vector (`Vec<(String, String, String)>`), generating thousands of allocations per cycle.
   - `note_drift.rs` clones note paths into `Option<String>` in `resolve_link_path` where `Option<&str>` suffices.
   - `note_lint.rs` calls `.to_lowercase()` on every note in `all_notes_snapshot` inside a nested loop for every broken link (O(N·M) allocations).
   - `note_weave.rs` allocates a new `Vec::<String>::new()` on heap in `write_typed_link` instead of passing `&[]`.
   - Repeated prompt generation buffers allocate intermediate `Vec<String>` + `.join()` + `format!` rather than streaming via `write!(&mut buf, ...)`.
2. **Move Opportunities Suppressed by Clones**:
   - In `mention_weave.rs`, `note.title`, `note.aliases`, and `note.links` are cloned into `MentionDoc` despite `note` being an owned local immediately dropped.
   - In `feedback_distill.rs`, `c.source` fields `severity` and `suggested_rule` are cloned rather than borrowed as `&str`.
   - In `goal_lessons_promote.rs`, all lessons are cloned into an intermediate `Vec<String>` before O(N·M) linear filtering against existing facts.
3. **Loop-Invariant Allocations**:
   - In `skill_distill.rs`, `super::render_rejected_block(&rejected_feedback)` is executed inside the `synthesis_paths` loop on every iteration, formatting the identical rejected block repeatedly.

---

## Classification Matrix

| File | Location | Taxonomy Category | Route | Description & Occam's Razor Remediation |
|---|---|---|---|---|
| `corpus_narrative.rs` | L85–L88 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `split_once("===PURPOSE===")` creates `(o, p)` which are converted to owned `String`s via `.to_string()` even though `overview_gen.write` and `purpose_gen.write` accept `&str`. Keep `(&str, &str)` slices directly. |
| `corpus_narrative.rs` | L44–L52 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `targets` clones `path` and `category` into `Vec<(String, String)>` just to take 40 items and load content. Borrow indices or note references before loading. |
| `daily_digest.rs` | L50–L55 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `category` is cloned before `load_content(&path)`; borrowing `&ctx.notes[idx].category` after `load_content` completes eliminates the clone. |
| `daily_digest.rs` | L61–L64 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `note_summaries.join("\n")` intermediate string allocation nested inside `format!`. |
| `daily_digest.rs` | L72–L73 | `CORRECTNESS / INVARIANT-TIGHTENING` | `FIX-CAREFULLY` | Does not check `super::is_provider_exhausted(&e)` before mapping to `AlephError::other`, unlike other LLM stages; quota/rate-limit errors may fail to abort cycle. |
| `workflow_proposal.rs` | L95–L100 | `REDUNDANT-ALLOCATION` | `FIX-CAREFULLY` | `proposal::canonical_name(&chain)` allocates a new `String` for every chain before entry inspection. |
| `graph_recompute.rs` | L48–L52 | `ACCIDENTAL-COMPLEXITY` | `FIX-CAREFULLY` | Calls `store.list_notes(&agent_id)` from DB to get paths for hydration, ignoring `ctx.notes` which already holds all note paths. |
| `graph_recompute.rs` | L147–L180 | `REDUNDANT-ALLOCATION` | `DEFER` | Constructs intermediate `serde_json::Value` trees via `json!({...})` for `sparse_communities` and `surprising` edges before serializing. Needs `#[derive(Serialize)]` on types in `src/memory/notes/graph/insights.rs` (outside batch). |
| `graph_recompute.rs` | L198–L212 | `CONTROL-FLOW` | `FIX-NOW` | `merge_related` uses `sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.cmp(&y.1)))`; replace with `sort_unstable_by` over key tuples. |
| `stages/mod.rs` | L207–L226 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `render_rejected_block`: maps items to `Vec<String>`, joins with `\n`, then embeds into `format!`. Stream entries directly into a single `String` buffer via `writeln!`. |
| `stages/mod.rs` | L135 | `CONTROL-FLOW` | `FIX-NOW` | `rejected_fingerprints.iter().any(|f| f == fp)` linear search; if converted to `HashSet` or slice comparison, provides O(1) membership check. |
| `mention_weave.rs` | L41–L45 | `ACCIDENTAL-COMPLEXITY` | `FIX-CAREFULLY` | `store.list_notes(&agent_id)` re-enumerates all note entries from DB when `ctx.notes` already provides note paths. |
| `mention_weave.rs` | L58–L70 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `note.title.clone()`, `note.aliases.iter().cloned()`, and `note.links.clone()` (suppressed with `excessive-clone`) are cloned from `note` before `note` is dropped. Move fields into `MentionDoc` directly. |
| `note_synthesis.rs` | L51–L54, L101 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `note_paths.to_vec()` creates a clone of the path vector, which is cloned again inside `build_synthesis_note`. Pass or move paths cleanly. |
| `note_synthesis.rs` | L58–L64 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `previews.join("\n\n")` intermediate vector join inside `format!`. |
| `skill_lifecycle.rs` | L71, L102, L113 | `ACCIDENTAL-COMPLEXITY` | `FIX-NOW` | `archive_candidates: Vec<String>` is collected and joined with `join(",")` solely for a single `tracing::info!` log. Format conditionally or stream into logger. |
| `goal_lessons_promote.rs` | L87–L94 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Clones all `goal.lessons` into intermediate `desired` Vec, then performs O(N·M) search with `!existing.contains(f)`. Pre-index `existing` into `HashSet<&str>` and push new facts directly. |
| `skill_distill.rs` | L65–L76 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Two separate passes over `reject_records`: one clones fingerprints, the other moves feedback. Can be unified in a single loop with pre-allocated capacity. |
| `skill_distill.rs` | L116 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `render_rejected_block(&rejected_feedback)` is called inside the `for path in &synthesis_paths` loop; hoist outside loop since `rejected_feedback` is loop-invariant. |
| `skill_distill.rs` | L105–L115 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `candidates_block`: creates `entries: Vec<String>`, joins with `,\n`, then wraps in `format!`. Stream directly into buffer. |
| `note_lint.rs` | L136–L143 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Inside loop over broken links, runs `all_notes_snapshot.iter().filter(|e| e.filename.to_lowercase() == target_lower)` — allocating strings for every note on every link check (O(N·M)). Precompute lowercased map once. |
| `note_lint.rs` | L350–L365 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `ensure_frontmatter`: 3 separate `patched.push_str(&format!(...))` calls. Replace with `write!(&mut patched, ...)`. |
| `note_review.rs` | L248–L257 | `ACCIDENTAL-COMPLEXITY` | `FIX-NOW` | `build_review_prompt`: computes `facts.chars().count() >= 1500` on every fact iteration (O(N²) unicode char count), followed by `.take(1500).collect()`. Maintain running char count instead. |
| `note_review.rs` | L136, L179 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `candidate.replay_op.clone()` and `let mut admitted = candidate.clone()` clone candidate data despite `candidate` being an owned value consumed in terminal branches. |
| `note_drift.rs` | L231–L248 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `resolve_link_path` returns `Option<String>` by cloning `entry.path.clone()` (suppressed with `excessive-clone`). Return `Option<&str>` to avoid allocations at call site. |
| `note_drift.rs` | L316–L322 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `apply_supersede_banner`: formats heading string with `format!`, checks `content.contains`, then calls `supersede_banner` which formats the identical heading again. Format once. |
| `note_consolidate.rs` | L76–L83 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `by_category: HashMap<String, Vec<usize>>` clones `note.category` for every note in the context (suppressed with `excessive-clone`). Use `HashMap<&str, Vec<usize>>`. |
| `note_consolidate.rs` | L182–L185 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `note_list.push_str(&format!(...))` in a loop over previews. Use `write!(&mut note_list, ...)`. |
| `note_consolidate.rs` | L287–L295 | `CONTROL-FLOW` | `FIX-NOW` | Double bounded indexing `if let (Some(...), Some(...)) = (titles.get(i), titles.get(j))` when loop bounds are `0..titles.len()` and `(i+1)..titles.len()`. Use direct indexing `(&titles[i], &titles[j])`. |
| `note_consolidate.rs` | L550–L555 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `fresh_facts`: filters and clones `absorbed_note.facts` into a new vector when `absorbed_note` is already owned. |
| `note_consolidate.rs` | L612–L618 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `keeper_path.clone()` and `absorbed_path.clone()` pushed to `ctx.report.merged_pairs`, then moved into `tracing::info!`. Pass references to `tracing::info!` and move strings directly. |
| `note_decay.rs` | L124–L136 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `last_hits: HashMap<String, i64>` clones `note.path` for every note (suppressed with `excessive-clone`). Use `HashMap<&str, i64>`. |
| `note_decay.rs` | L344–L355 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `candidates: Vec<(String, String, String)>` clones 3 Strings (`path`, `category`, `filename`) for every non-archived note in the vault. Replace with `Vec<usize>` (indices into `ctx.notes`) for zero string allocations. |
| `note_decay.rs` | L203–L211 | `REDUNDANT-ALLOCATION` | `FIX-CAREFULLY` | `low_score_notes`: clones `note.path`, `note.category`, and `filename.to_string()` for each candidate. |
| `note_weave.rs` | L498 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Calls `append_to_note(&ctx.agent_id, from, &Vec::<String>::new(), &to_link)` allocating a new empty `Vec` on heap. Replace with `&[]`. |
| `note_weave.rs` | L304–L320 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `seen: HashSet<(String, String)>`: clones `(l.from.clone(), l.to.clone())` on EVERY pair in `filter` (suppressed with `excessive-clone` twice). Check deduplication with borrowed string keys or insert only unique pairs. |
| `note_weave.rs` | L144–L172 | `ACCIDENTAL-COMPLEXITY` | `FIX-CAREFULLY` | Runs 2 SQL queries (`get_outgoing_links`, `get_incoming_links_any`) per note in vault even when `materialized_isolated` is non-empty. |
| `note_weave.rs` | L220–L224 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `orphan_batch`: `orphans.iter().take(...).cloned().collect()` allocates a cloned vector of paths while `peer_batch` borrows `&String`. |
| `feedback_distill.rs` | L511–L527 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Clones `severity` and `suggested_rule` from `c.source` in loop (suppressed with `excessive-clone`), then allocates `String` via `format!` into `corrections_block.push_str`. Borrow `&str` and use `write!`. |
| `feedback_distill.rs` | L502–L506 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `existing_candidates` formatted via `entries: Vec<String>` + `.join(",\n")` + `format!`. Stream into buffer. |
| `feedback_distill.rs` | L260–L267 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Two passes over `reject_records` cloning fingerprints and triples. Combine into one pass. |
| `tool_failure_distill.rs` | L295–L306 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `evidence.push_str(&format!(...))` in nested loops over failures and samples. Stream with `write!(&mut evidence, ...)`. |
| `tool_failure_distill.rs` | L285–L289 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | `candidate_paths` formatted via `entries: Vec<String>` + `.join(",\n")` + `format!`. Stream into buffer. |
| `tool_failure_distill.rs` | L272–L279 | `REDUNDANT-ALLOCATION` | `FIX-NOW` | Two passes over `reject_records` cloning fingerprints and triples. Combine into one pass. |
| `co_recall_edges.rs` | L97–L111 | `CONTROL-FLOW` | `FIX-CAREFULLY` | `discipline_edges` does `get` then `entry` (double HashMap lookup per node endpoint). Consolidate lookups. |
| `graph_recompute_lifecycle_tests.rs` | L1–L75 | `DEFENSIVE-CHECK` | `REJECT` | Valid async lifecycle cancellation and timeout test. Retain unchanged. |
| `index_refresher.rs` | L1–L45 | `CLEAN` | `REJECT` | Minimal stage implementation (32 lines). Retain unchanged. |

---

## Routing & Action Plan

### Tier 1: FIX-NOW (High Confidence / Immediate Cleanup)
1. **Suppressed Clones & Field Moves**:
   - `mention_weave.rs`: Move `note.title`, `note.aliases`, and `note.links` into `MentionDoc` without clones.
   - `feedback_distill.rs`: Borrow `severity: &str` and `suggested_rule: Option<&str>` instead of cloning.
   - `note_drift.rs`: Return `Option<&str>` from `resolve_link_path` instead of `Option<String>`.
   - `note_consolidate.rs`: Key `by_category` with `&str` instead of `String`.
   - `note_decay.rs`: Key `last_hits` with `&str` instead of `String`. Replace `candidates: Vec<(String, String, String)>` with `Vec<usize>`.
   - `note_weave.rs`: Replace `&Vec::<String>::new()` with `&[]` in `write_typed_link`.
2. **Buffer Formatting (Replace `push_str(&format!(...))` with `write!`)**:
   - `note_lint.rs`: `ensure_frontmatter`
   - `note_consolidate.rs`: `note_list` batch prompt building
   - `feedback_distill.rs`: `corrections_block` prompt building
   - `tool_failure_distill.rs`: `evidence` prompt building
   - `stages/mod.rs`: `render_rejected_block`
3. **Loop-Invariant & O(N·M) Algorithmic Fixes**:
   - `skill_distill.rs`: Hoist `render_rejected_block` out of `synthesis_paths` loop.
   - `note_lint.rs`: Precompute lowercased filename map for `all_notes_snapshot` before broken link checks.
   - `note_review.rs`: Replace O(N²) `facts.chars().count()` in `build_review_prompt` loop with running counter.
   - `goal_lessons_promote.rs`: Pre-index `existing` facts into `HashSet<&str>` to avoid O(N·M) linear filtering and intermediate `desired` Vec allocation.

### Tier 2: FIX-CAREFULLY (Invariant / Concurrency Sensitive)
1. `daily_digest.rs`: Error propagation parity (`is_provider_exhausted(&e)`).
2. `note_weave.rs`: Deduplication keys in `seen: HashSet` without excessive clones during iterator chaining.
3. `graph_recompute.rs` & `mention_weave.rs`: Avoid redundant `store.list_notes(&agent_id)` DB trips if `ctx.notes` can be safely used without violating stage update ordering.

### Tier 3: DEFER / REJECT
1. **DEFER**: `graph_recompute.rs` `json!({...})` mapping for insights serialization. Requires modifying `SparseCommunity` / `SurprisingEdge` in `src/memory/notes/graph/insights.rs` (belongs to `MEM_NOTES_CORE` batch).
2. **REJECT**: `graph_recompute_lifecycle_tests.rs` test harness and `index_refresher.rs` (clean stage).
