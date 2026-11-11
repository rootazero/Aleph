# Classification & Routing Matrix: MEM_RETRIEVAL_RERANK

## Executive Summary

- **Batch Scope**: 33 files across 5 functional areas in `src/memory/`:
  - `src/memory/curated/` (budget, format, legacy, mod, snapshot, store, tests)
  - `src/memory/note_retrieval/` (builder, expansion, mod, multi_agent, pipeline, relation_surface, scoring, signals, single_agent, tests, trace)
  - `src/memory/rerank/` (cohere, jina, mod, pinecone, provider, siliconflow, vllm, voyage)
  - `src/memory/ripple/` (config, mod, task, tests)
  - `src/memory/scratchpad/` (manager, mod, template)
- **Primary Anti-Patterns Identified**:
  1. **Scoring & Retrieval Loop Allocations**:
     - `pipeline.rs`: Clones full note markdown strings before MMR (`scoring::mmr_reorder` takes `&[String]` instead of `&[impl AsRef<str>]`).
     - `pipeline.rs`: Clones both `agent` and `id` strings into a temporary tuple for every single fact in `apply_scoring` loop (`.get(&(f.fact.agent.clone(), f.fact.id.clone()))`).
     - `signals.rs`: Clones entire `ScoredFact` objects into `HashMap<&str, Vec<ScoredFact>>` buckets merely to extract note IDs and scores.
     - `pipeline.rs`: In `apply_rerank`, converts integer indices into `String` (`i.to_string()`), constructs an intermediate `HashMap<String, ScoredFact>`, and removes by string key instead of direct O(1) indexed slot lookup.
  2. **Rerank & Blending Inefficiencies**:
     - `rerank/mod.rs`: `blend_scores` performs an O(N*M) linear search (`reranked.iter().find(|r| r.index == i)`) and clones every document id string (`doc.clone()`).
     - Rerank provider clients (`cohere.rs`, `jina.rs`, `siliconflow.rs`, `voyage.rs`, `vllm.rs`, `pinecone.rs`): Repeated `api_url()` string recomputations and `format!("Bearer {}", api_key)` allocations on every request; `vllm.rs` parses URLs with `Url::parse` on every call.
  3. **Curated & Scratchpad Memory Inefficiencies & Precision Issues**:
     - `curated/store.rs`: Clones the entire `Vec<String>` of entries on every `add` and `replace` call solely to validate character budget.
     - `curated/snapshot.rs`: Counts characters twice and allocates owned `String`s for truncation when borrowing `&str` via `char_indices` is zero-alloc.
     - `scratchpad/manager.rs`: `update_timestamp` uses 4-level deeply nested `if let` blocks with redundant bounds checks; `append_note` note-cap scanner counts lines starting with `- [` across the entire file, falsely counting `## Plan` checklist items towards the 500-note cap.
     - `ripple/task.rs`: `total_hops` unconditionally reports `max_hops` even when BFS exploration terminates early.

---

## Detailed Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
| :--- | :--- | :--- | :--- | :--- |
| [`src/memory/curated/budget.rs#L47-L55`](file:///home/zou/data/workspace/Aleph/src/memory/curated/budget.rs#L47-L55) | `REDUNDANT-ALLOCATION` | Intermediate `pct_label` `format!` allocation before outer header `format!`. | `FIX-NOW` | Select string slice literal `prefix` ("OVER BUDGET — ", "NEAR LIMIT — ", "") and format in a single pass. |
| [`src/memory/curated/format.rs#L15-L18`](file:///home/zou/data/workspace/Aleph/src/memory/curated/format.rs#L15-L18) | `REDUNDANT-ALLOCATION` | `e.trim().to_string()` allocates `String` before `filter(|e| !e.is_empty())` drops empty chunks. | `FIX-NOW` | Filter on `&str` slice before calling `String::from` (`.map(str::trim).filter(!str::is_empty).map(String::from)`). |
| [`src/memory/curated/legacy.rs#L18-L37`](file:///home/zou/data/workspace/Aleph/src/memory/curated/legacy.rs#L18-L37) | `ACCIDENTAL-COMPLEXITY` | Backward-tolerant legacy MEMORY.md loader. | `REJECT` | Clean, minimal single-entry parsing logic conforming to spec D2. |
| [`src/memory/curated/mod.rs#L28-L61`](file:///home/zou/data/workspace/Aleph/src/memory/curated/mod.rs#L28-L61) | `ACCIDENTAL-COMPLEXITY` | Curated configuration definition. | `REJECT` | Canonical configuration types; no simplification required. |
| [`src/memory/curated/snapshot.rs#L45-L49`](file:///home/zou/data/workspace/Aleph/src/memory/curated/snapshot.rs#L45-L49) | `REDUNDANT-ALLOCATION` | Full character count scan followed by string allocation in `render_user_block` and `render_open_loops_block` (`L152-L156`). | `FIX-NOW` | Slice `&str` directly using `char_indices().nth(char_limit)` to borrow without heap allocation. |
| [`src/memory/curated/snapshot.rs#L177-L184`](file:///home/zou/data/workspace/Aleph/src/memory/curated/snapshot.rs#L177-L184) | `REDUNDANT-ALLOCATION` | Double `format!` in `user_header` for budget label. | `FIX-NOW` | Collapse status label selection to static `&str` prefix and format in one pass. |
| [`src/memory/curated/store.rs#L161-L173`](file:///home/zou/data/workspace/Aleph/src/memory/curated/store.rs#L161-L173) | `REDUNDANT-ALLOCATION` | `new_entries = st.entries.clone()` clones entire vector of entries solely to measure `used_chars`. | `FIX-CAREFULLY` | Compute incremental delta (`used_chars(&st.entries) + content.chars().count() + ENTRY_DELIMITER.chars().count()`) before pushing. |
| [`src/memory/curated/store.rs#L207-L222`](file:///home/zou/data/workspace/Aleph/src/memory/curated/store.rs#L207-L222) | `REDUNDANT-ALLOCATION` | `new_entries = st.entries.clone()` clones entire vector of entries solely to measure `used_chars` during `replace`. | `FIX-CAREFULLY` | Compute incremental delta (`current_used - old_chars + new_chars`) and mutate in-place if within budget. |
| [`src/memory/curated/store.rs#L456-L476`](file:///home/zou/data/workspace/Aleph/src/memory/curated/store.rs#L456-L476) | `REDUNDANT-ALLOCATION` | Allocates temporary `matches: Vec<usize>` and `unique: HashSet<_>` during substring search. | `DEFER` | Bounded entry list (< 50 items); low frequency and low allocation volume. |
| [`src/memory/curated/store.rs#L518`](file:///home/zou/data/workspace/Aleph/src/memory/curated/store.rs#L518) | `CORRECTNESS / INVARIANT-TIGHTENING` | `apply_one` returns `CuratedError::NoMatch(content.to_string())` reporting replacement text instead of `old_text`. | `FIX-NOW` | Report `old_text.to_string()` in `CuratedError::NoMatch` when lookup fails. |
| [`src/memory/curated/tests.rs#L1-L86`](file:///home/zou/data/workspace/Aleph/src/memory/curated/tests.rs#L1-L86) | `ACCIDENTAL-COMPLEXITY` | Property tests and concurrency harnesses. | `REJECT` | Test scaffolding. |
| [`src/memory/note_retrieval/builder.rs#L39-L53`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/builder.rs#L39-L53) | `REDUNDANT-ALLOCATION` | Builder methods take `&Config` and call `.clone()`. | `DEFER` | Preserve public builder API stability; initialization-time only. |
| [`src/memory/note_retrieval/expansion.rs#L62-L67`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/expansion.rs#L62-L67) | `REDUNDANT-ALLOCATION` | `seen.insert(peer.clone())` unconditionally clones `peer` before checking if it was already present. | `FIX-NOW` | Check `!seen.contains(&peer)` with borrowed `&str` before cloning into `seen` and `collected`. |
| [`src/memory/note_retrieval/expansion.rs#L77`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/expansion.rs#L77) | `REDUNDANT-ALLOCATION` | Clones all collected peer paths into `paths: Vec<String>` for hydration. | `DEFER` | Store API `get_notes_with_content` requires `&[String]`. |
| [`src/memory/note_retrieval/mod.rs#L40-L73`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/mod.rs#L40-L73) | `ACCIDENTAL-COMPLEXITY` | Retrieval constants and struct definition. | `REJECT` | Clean architecture definition; adheres to Occam's Razor. |
| [`src/memory/note_retrieval/multi_agent.rs#L125-L127`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/multi_agent.rs#L125-L127) | `REDUNDANT-ALLOCATION` | `all_results.push(r.to_scored_fact(agent_id))` clones strings in `NoteSearchResult`. | `DEFER` | `to_scored_fact` signature is standardized across note store interfaces. |
| [`src/memory/note_retrieval/pipeline.rs#L67-L70`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/pipeline.rs#L67-L70) | `REDUNDANT-ALLOCATION` | In loop over candidates, calls `.get(&(f.fact.agent.clone(), f.fact.id.clone()))`, allocating 2 strings per fact. | `FIX-CAREFULLY` | Refactor map keying or borrow lookup to eliminate per-iteration string cloning in scoring hot path. |
| [`src/memory/note_retrieval/pipeline.rs#L91`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/pipeline.rs#L91) | `REDUNDANT-ALLOCATION` | `facts.iter().map(|f| f.fact.content.clone()).collect()` clones full note contents for MMR. | `FIX-NOW` | Update `scoring::mmr_reorder` to accept `&[T]` where `T: AsRef<str>`, passing borrowed `&str`. |
| [`src/memory/note_retrieval/pipeline.rs#L118-L124`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/pipeline.rs#L118-L124) | `REDUNDANT-ALLOCATION` | `strip` closure returns owned `String` via `to_string()` where `&str` borrowing is sufficient. | `FIX-NOW` | Let `strip` return `&str`: `p.strip_prefix("note://").unwrap_or(p)`. |
| [`src/memory/note_retrieval/pipeline.rs#L218-L237`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/pipeline.rs#L218-L237) | `REDUNDANT-ALLOCATION` / `ACCIDENTAL-COMPLEXITY` | Converts indices to `String` (`i.to_string()`), builds `HashMap<String, ScoredFact>`, and looks up by string key. | `FIX-NOW` | Use indexed vector slots `Vec<Option<ScoredFact>>` and parse index or index directly, avoiding `HashMap` and string allocations. |
| [`src/memory/note_retrieval/relation_surface.rs#L16-L22`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/relation_surface.rs#L16-L22) | `REDUNDANT-ALLOCATION` | `seen.insert(to.clone())` clones `to` for set tracking, then `.cloned()` clones both fields. | `FIX-NOW` | Use `seen: HashSet<&str>` and `seen.insert(to.as_str())` to avoid cloning candidate paths during filtering. |
| [`src/memory/note_retrieval/relation_surface.rs#L33-L40`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/relation_surface.rs#L33-L40) | `REDUNDANT-ALLOCATION` | Allocates `parts: Vec<String>` with individual `format!` calls before `.join()` and outer `format!`. | `FIX-NOW` | Format footer directly into a single string buffer with `write!`. |
| [`src/memory/note_retrieval/scoring.rs#L74-L79`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/scoring.rs#L74-L79) | `ACCIDENTAL-COMPLEXITY` | Docstring claims "Lowercase whitespace-token set", but implementation does not lowercase (zero-alloc design). | `FIX-NOW` | Update docstring to match implementation accurately without introducing lowercasing allocations. |
| [`src/memory/note_retrieval/scoring.rs#L106`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/scoring.rs#L106) | `REDUNDANT-ALLOCATION` | `mmr_reorder(contents: &[String], ...)` forces callers to allocate `Vec<String>`. | `FIX-NOW` | Generalize to `mmr_reorder<T: AsRef<str>>(contents: &[T], ...)`. |
| [`src/memory/note_retrieval/scoring.rs#L131`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/scoring.rs#L131) | `CONTROL-FLOW / INDIRECTION` | `remaining.remove(best_pos)` performs O(N) shift in MMR greedy loop. | `FIX-NOW` | Use `remaining.swap_remove(best_pos)` for O(1) removal. |
| [`src/memory/note_retrieval/signals.rs#L57-L67`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/signals.rs#L57-L67) | `REDUNDANT-ALLOCATION` | Clones entire `ScoredFact` objects into `HashMap<&str, Vec<ScoredFact>>` buckets in `record_recall_by_owner`. | `FIX-NOW` | Group borrowed/minimal tuples `(f.fact.id.clone(), f.score)` directly by owner, avoiding cloning heavy `ScoredFact` structs. |
| [`src/memory/note_retrieval/signals.rs#L101-L114`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/signals.rs#L101-L114) | `REDUNDANT-ALLOCATION` | Clones entire `ScoredFact` objects into `HashMap<&str, Vec<ScoredFact>>` buckets in `fetch_reinforcement_counts_by_owner`. | `FIX-NOW` | Group `f.fact.id.clone()` strings by owner directly, avoiding cloning entire `ScoredFact` structs. |
| [`src/memory/note_retrieval/single_agent.rs#L134`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/single_agent.rs#L134) | `REDUNDANT-ALLOCATION` | Clones `results` into `ScoredFact` when `results` vector is no longer needed. | `DEFER` | Consistent with note store contract across retrieval pipeline. |
| [`src/memory/note_retrieval/tests.rs#L1-L875`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/tests.rs#L1-L875) | `ACCIDENTAL-COMPLEXITY` | Retrieval unit and integration tests. | `REJECT` | Test scaffolding. |
| [`src/memory/note_retrieval/trace.rs#L1-L75`](file:///home/zou/data/workspace/Aleph/src/memory/note_retrieval/trace.rs#L1-L75) | `ACCIDENTAL-COMPLEXITY` | Telemetry tracer. | `REJECT` | Zero-allocation when `TraceSink::Off`; clean conditional observational telemetry. |
| [`src/memory/rerank/mod.rs#L37-L52`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/mod.rs#L37-L52) | `REDUNDANT-ALLOCATION` | `config.clone()` in every match branch in `build_provider`. | `FIX-NOW` | Clone `config` once prior to match dispatch or move when constructing providers. |
| [`src/memory/rerank/mod.rs#L69-L85`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/mod.rs#L69-L85) | `REDUNDANT-ALLOCATION` / `CONTROL-FLOW / INDIRECTION` | In `blend_scores`, O(N*M) linear scan `reranked.iter().find(|r| r.index == i)` and clones `doc.clone()`. | `FIX-NOW` | Build direct index-lookup array for `rerank_score` (converting O(N*M) to O(N + M)) and eliminate redundant scans. |
| [`src/memory/rerank/cohere.rs#L34-L50`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/cohere.rs#L34-L50) | `REDUNDANT-ALLOCATION` | `api_url(&self) -> String` repeatedly formats URL on every request. | `FIX-NOW` | Precompute `api_url` during provider construction in `new()`. |
| [`src/memory/rerank/cohere.rs#L93`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/cohere.rs#L93) | `REDUNDANT-ALLOCATION` | Allocates `format!("Bearer {}", self.config.api_key)` string on every request. | `FIX-NOW` | Precompute header value or format efficiently. |
| [`src/memory/rerank/jina.rs#L33-L48`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/jina.rs#L33-L48) | `REDUNDANT-ALLOCATION` | Repeated `api_url` formatting on every request. | `FIX-NOW` | Precompute `api_url` in `new()`. |
| [`src/memory/rerank/jina.rs#L91`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/jina.rs#L91) | `REDUNDANT-ALLOCATION` | Allocates `format!("Bearer {}", self.config.api_key)` string on every request. | `FIX-NOW` | Precompute or borrow auth header value. |
| [`src/memory/rerank/pinecone.rs#L34-L44`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/pinecone.rs#L34-L44) | `REDUNDANT-ALLOCATION` | Repeated `api_url` formatting on every request. | `FIX-NOW` | Precompute `api_url` in `new()`. |
| [`src/memory/rerank/provider.rs#L1-L136`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/provider.rs#L1-L136) | `ACCIDENTAL-COMPLEXITY` | Rerank trait and provider config definitions. | `REJECT` | Core capability abstraction. |
| [`src/memory/rerank/siliconflow.rs#L33-L48`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/siliconflow.rs#L33-L48) | `REDUNDANT-ALLOCATION` | Repeated `api_url` formatting and `format!("Bearer ...")` on every request. | `FIX-NOW` | Precompute `api_url` in `new()`. |
| [`src/memory/rerank/vllm.rs#L33-L56`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/vllm.rs#L33-L56) | `REDUNDANT-ALLOCATION` / `CONTROL-FLOW / INDIRECTION` | In `api_url`, calls `assert_safe_endpoint(raw)` and parses URL with `Url::parse` on every request. | `FIX-NOW` | Validate endpoint security once at initialization time in `new()`. |
| [`src/memory/rerank/voyage.rs#L33-L48`](file:///home/zou/data/workspace/Aleph/src/memory/rerank/voyage.rs#L33-L48) | `REDUNDANT-ALLOCATION` | Repeated `api_url` formatting and `format!("Bearer ...")` on every request. | `FIX-NOW` | Precompute `api_url` in `new()`. |
| [`src/memory/ripple/config.rs#L1-L39`](file:///home/zou/data/workspace/Aleph/src/memory/ripple/config.rs#L1-L39) | `ACCIDENTAL-COMPLEXITY` | Data structures for ripple exploration. | `REJECT` | Minimal, pure data definitions. |
| [`src/memory/ripple/mod.rs#L1-L14`](file:///home/zou/data/workspace/Aleph/src/memory/ripple/mod.rs#L1-L14) | `ACCIDENTAL-COMPLEXITY` | Ripple module definitions. | `REJECT` | Clean re-export. |
| [`src/memory/ripple/task.rs#L37`](file:///home/zou/data/workspace/Aleph/src/memory/ripple/task.rs#L37) | `REDUNDANT-ALLOCATION` | `let mut current_level = seed_facts.clone()` clones owned `seed_facts` vector. | `FIX-CAREFULLY` | Borrow seeds during first hop to avoid duplicating full vector of facts. |
| [`src/memory/ripple/task.rs#L112`](file:///home/zou/data/workspace/Aleph/src/memory/ripple/task.rs#L112) | `CORRECTNESS / INVARIANT-TIGHTENING` | `total_hops: self.config.max_hops` reports maximum configured hops even when BFS terminates early. | `FIX-NOW` | Track completed hops (`actual_hops`) and return truthful count. |
| [`src/memory/ripple/tests.rs#L1-L184`](file:///home/zou/data/workspace/Aleph/src/memory/ripple/tests.rs#L1-L184) | `ACCIDENTAL-COMPLEXITY` | Ripple unit tests. | `REJECT` | Test scaffolding. |
| [`src/memory/scratchpad/manager.rs#L486-L490`](file:///home/zou/data/workspace/Aleph/src/memory/scratchpad/manager.rs#L486-L490) | `CORRECTNESS / INVARIANT-TIGHTENING` | Note-cap check filters entire scratchpad for lines starting with `- [`, incorrectly counting `## Plan` checklist items towards 500-note cap. | `FIX-NOW` | Scope line count to `## Notes` section using `extract_section(&content, NOTES_HEADER)`. |
| [`src/memory/scratchpad/manager.rs#L560-L565`](file:///home/zou/data/workspace/Aleph/src/memory/scratchpad/manager.rs#L560-L565) | `REDUNDANT-ALLOCATION` | Formats each item into `Vec<String>` before calling `.join("\n")`. | `FIX-NOW` | Write directly into a preallocated `String` buffer. |
| [`src/memory/scratchpad/manager.rs#L701-L710`](file:///home/zou/data/workspace/Aleph/src/memory/scratchpad/manager.rs#L701-L710) | `CONTROL-FLOW / INDIRECTION` | 4 levels of nested `if let` blocks with redundant `.get()` range slicing in `update_timestamp`. | `FIX-NOW` | Flatten using guard clauses / early returns and direct string slicing. |
| [`src/memory/scratchpad/manager.rs#L820-L834`](file:///home/zou/data/workspace/Aleph/src/memory/scratchpad/manager.rs#L820-L834) | `REDUNDANT-ALLOCATION` | `normalize_single_in_progress` clones every `PlanItem` in the slice. | `DEFER` | Keeps pure functional contract without mutating caller slice. |
| [`src/memory/scratchpad/mod.rs#L1-L37`](file:///home/zou/data/workspace/Aleph/src/memory/scratchpad/mod.rs#L1-L37) | `ACCIDENTAL-COMPLEXITY` | Module declarations. | `REJECT` | Clean re-exports. |
| [`src/memory/scratchpad/template.rs#L1-L111`](file:///home/zou/data/workspace/Aleph/src/memory/scratchpad/template.rs#L1-L111) | `ACCIDENTAL-COMPLEXITY` | Markdown template constants and tests. | `REJECT` | Clean and self-contained. |

---

## Category Breakdowns & Deep Analysis

### 1. `REDUNDANT-ALLOCATION` (Highest Yield Area)
- **MMR Note Content Cloning (`pipeline.rs#L91`)**: `pipeline.rs` currently iterates over all `facts` and calls `f.fact.content.clone()` to build a `Vec<String>` purely because `scoring::mmr_reorder` was defined taking `&[String]`. Changing `mmr_reorder` to accept `&[T] where T: AsRef<str>` allows passing `Vec<&str>` without copying thousands of bytes of note markdown.
- **Scoring Loop Map Lookup Clones (`pipeline.rs#L67-L70`)**: Clones two owned `String`s (`f.fact.agent.clone()` and `f.fact.id.clone()`) on every single fact candidate just to query `reinforcement_counts.get(&(agent, id))`.
- **Full ScoredFact Cloning in Signals (`signals.rs#L57-L67`, `L101-L114`)**: Both `record_recall_by_owner` and `fetch_reinforcement_counts_by_owner` allocate `HashMap<&str, Vec<ScoredFact>>`, cloning every `ScoredFact` (including vector embeddings, content, and metadata). This can be replaced with direct grouping of lightweight tuples `(f.fact.id.clone(), f.score)` and `f.fact.id.clone()`.
- **Rerank Index Conversions (`pipeline.rs#L218-L237`)**: `apply_rerank` turns integer positions `0..N` into strings (`i.to_string()`), allocates `originals: Vec<(String, f32)>`, and indexes `facts` into `HashMap<String, ScoredFact>`. Replacing this with a positional `Vec<Option<ScoredFact>>` eliminates the `HashMap` allocation and string allocations entirely.
- **Rerank Provider Overhead (`rerank/*.rs`)**: Each provider creates `api_url` anew via string formatting and allocates `format!("Bearer ...")` on every POST request. Precomputing or borrowing these strings in provider structs removes repetitive HTTP setup allocations.
- **Curated Store Vector Clones (`curated/store.rs#L161-L173`, `L207-L222`)**: Clones the entire vector of entries (`st.entries.clone()`) on every add or replace merely to pass it to `used_chars`. Computing incremental character deltas avoids all full-vector clones.

### 2. `CONTROL-FLOW / INDIRECTION`
- **O(N*M) Linear Scan in `blend_scores` (`rerank/mod.rs#L69-L85`)**: For each original item `i`, `blend_scores` scans `reranked` linearly (`find(|r| r.index == i)`). An array lookup or indexed map reduces this to O(N + M).
- **Nested `if let` Chains in `update_timestamp` (`scratchpad/manager.rs#L701-L710`)**: 4-level deep nesting with `.find()`, `.get()`, and `.or_else()`. Flattening with guard clauses makes the intent immediate and removes unnecessary slicing checks.
- **vLLM Endpoint Security Checking (`rerank/vllm.rs#L33-L56`)**: Validates endpoint safety via full URL parsing inside `api_url` on every request instead of during initialization.

### 3. `CORRECTNESS / INVARIANT-TIGHTENING`
- **Scratchpad Note Cap Bug (`scratchpad/manager.rs#L486-L490`)**: `append_note` checks `content.lines().filter(|l| l.trim_start().starts_with("- [")).count() >= MAX_NOTES`. Because plan items in `## Plan` also start with `- [ ]`, `- [~]`, `- [x]`, active plan steps are counted as notes, causing busy plans to trigger premature note-cap rejections. Scoping to `## Notes` fixes this.
- **Ripple Exploration Hop Count Reporting (`ripple/task.rs#L112`)**: When BFS runs out of unexplored neighbors at hop 0 or 1, `total_hops` still returns `self.config.max_hops`, falsifying telemetry. Tracking `actual_hops` preserves observability truth.
- **Curated Error Reporting on Replace (`curated/store.rs#L518`)**: In `apply_one`, if an entry cannot be found, it returns `CuratedError::NoMatch(content.to_string())` which quotes the replacement string instead of `old_text`.

---

## Routing Summary

| Route Tag | Count | Primary Focus |
| :--- | :--- | :--- |
| **FIX-NOW** | 22 | Direct redundant allocation removals, MMR generic refactor, scratchpad nesting flattening, note-cap scope fix, blend_scores O(N+M) lookup, provider precomputed URLs. |
| **FIX-CAREFULLY** | 4 | Curated store incremental budget math (`add` / `replace`), `pipeline.rs` tuple key lookup refactoring, ripple `seed_facts` borrow. |
| **DEFER** | 6 | Public API / trait signatures (`NoteSearchResult::to_scored_fact`, `ExpansionConfig` builder clones, `get_notes_with_content(&[String])`). |
| **REJECT** | 20 | Test files (`tests.rs`), pure data types (`RippleConfig`, `CuratedConfig`), clean zero-alloc telemetry sinks (`TraceSink::Off`). |
| **Total Candidates Evaluated** | **52** | Across 33 target files. |

---

## Invariant Safety Pre-Flight

1. **Retrieval Mathematics & Fusion**:
   - Cosine similarity, recency decay curve (`0.5^(age / half_life)`), reinforcement multiplier (`1 + w * ln(1 + hits)`), and MMR optimization formulas remain strictly intact.
   - Truncation boundaries, default thresholds, and tie-breaking order (score desc then path asc) remain byte-for-byte identical.
2. **Curated Memory Budget Invariants**:
   - Strict character count accounting (counting CJK codepoints, not UTF-8 bytes) is preserved.
   - Sentinel `\n§\n` delimiter counting and round-trip parsing (`parse(serialize(x)) == x`) remain strictly preserved.
3. **External Rerank Provider Wire Formats**:
   - Cohere, Jina, SiliconFlow, Voyage, vLLM, and Pinecone request payloads (`top_n`, `top_k`, `documents`, `query`) and authentication headers (`Bearer`, `Api-Key`, `X-Pinecone-Api-Version`) remain wire-compatible with upstream APIs.
4. **Scratchpad Lifecycle & Plan Checkbox States**:
   - The 3-state checkbox lifecycle (`[ ]`, `[~]`, `[x]`) and single `InProgress` invariant are strictly preserved.
   - Plan item indices remain 0-based and document-order stable.
