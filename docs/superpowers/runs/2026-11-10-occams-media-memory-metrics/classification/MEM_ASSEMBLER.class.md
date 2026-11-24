# Classification & Routing Matrix: Batch MEM_ASSEMBLER

## Executive Summary

Batch `MEM_ASSEMBLER` encompasses 25 files across three memory submodules:
- `src/memory/assembler/` (Working memory prompt assembly, context block fencing, envelope contracts, skeleton fallback, feedback floor, candidate gathering, hybrid reranking, token hydration, and rendering)
- `src/memory/context/` (Memory fact aggregate root, type enums, cognitive layers, VFS path computation, and compression records)
- `src/memory/context_comptroller/` (Token budget arbitration and priority ranking)

Under Occam's Razor doctrine, these components are evaluated to eliminate unnecessary heap allocations, redundant clones, duplicated structural boilerplate, and string reformatting overhead, while strictly upholding all subsystem invariants:
1. **Mathematical & Budget Invariants**: Token estimation formulas, 70% rerank budget caps, 30% pinned budget limits, and greedy recency-decay rankings (`recency_factor`) must remain exact.
2. **Data & Storage Integrity**: `MemoryFact` VFS paths (`aleph://...`), note categories, cognitive layers (`working` / `episodic` / `semantic` / `raw`), and wire formats (`schema_version: "1.0"`) must remain byte-for-byte compatible.
3. **Prompt Injection & Fencing Guards**: `<memory-context>` and `<MemoryEnvelope>` fences, closing-tag escaping (`</` -> `<\/`), and XML attribute sanitization must remain unbroken.

---

## Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|---|
| MEM-ASM-01 | [`src/memory/assembler/hybrid.rs#L463-L467`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/hybrid.rs#L463-L467) | REDUNDANT-ALLOCATION | In `hydrate`, `truncate_chars(&item.content, remaining_chars).to_string()` allocates a new `String` for every item in every slot, even when un-truncated. | FIX-NOW | Use in-place `item.content.truncate(truncated_len)` when `truncated_len < item.content.len()`; avoids 100% of heap reallocations. |
| MEM-ASM-02 | [`src/memory/assembler/feedback_floor.rs#L81-L82`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/feedback_floor.rs#L81-L82) | REDUNDANT-ALLOCATION | `seen.insert(e.path.clone())` allocates a `String` clone for each floor entry solely to deduplicate in `HashSet`. | FIX-NOW | Use `HashSet<&str>` with `seen.insert(e.path.as_str())`; eliminates all path clones. |
| MEM-ASM-03 | [`src/memory/assembler/feedback_floor.rs#L141`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/feedback_floor.rs#L141) | REDUNDANT-ALLOCATION | `title: note.title.clone()` clones `note.title` even though `note` is dropped at the end of the iteration. | FIX-NOW | Move `title: note.title` directly. |
| MEM-ASM-04 | [`src/memory/assembler/render.rs#L33-L36`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/render.rs#L33-L36) | REDUNDANT-ALLOCATION | `render_markdown_v1` collects non-empty slots into `Vec<&EnvelopeSlot>` to check if all are empty. | FIX-NOW | Use `env.slots.iter().all(|s| s.items.is_empty())` and iterate directly without allocating a vector. |
| MEM-ASM-05 | [`src/memory/assembler/render.rs#L134-L161`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/render.rs#L134-L161) | REDUNDANT-ALLOCATION | `render_item_markdown` allocates an intermediate `header` string via `format!` for every item. | FIX-NOW | Use `std::fmt::Write; write!(out, ...)` to write directly into the preallocated buffer. |
| MEM-ASM-06 | [`src/memory/assembler/render.rs#L164`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/render.rs#L164) | REDUNDANT-ALLOCATION | `item.content.replace("</", "<\\/")` allocates a new `String` unconditionally on every item even if `</` is absent. | FIX-NOW | Gate behind `if item.content.contains("</")` or stream-write to avoid allocation on clean text. |
| MEM-ASM-07 | [`src/memory/assembler/render.rs#L179-L207`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/render.rs#L179-L207) | REDUNDANT-ALLOCATION | `render_xml` performs repeated `out.push_str(&format!(...))` for envelope header, slots, directives, and items. | FIX-NOW | Replace `format!` calls with `std::fmt::Write; write!(out, ...)` directly into `out`. |
| MEM-ASM-08 | [`src/memory/assembler/gather.rs#L194-L195`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/gather.rs#L194-L195) | REDUNDANT-ALLOCATION | In `fetch_notes`, `full_content: sf.fact.content.clone()` clones fact body even though `sf` is consumed by value in `into_iter()`. | FIX-NOW | Move `full_content: sf.fact.content` directly. |
| MEM-ASM-09 | [`src/memory/assembler/gather.rs#L178-L189`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/gather.rs#L178-L189) | REDUNDANT-ALLOCATION | `sf.fact.path.clone()` and `display_id.trim_start_matches(...).to_string()` create multiple intermediate `String`s. | FIX-NOW | Move `sf.fact.path`, borrow `path_no_scheme` as `&str`, and compute `title` before converting. |
| MEM-ASM-10 | [`src/memory/assembler/gather.rs#L94-L110`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/gather.rs#L94-L110) | REDUNDANT-ALLOCATION | `seen_feedback` clones candidate IDs into `HashSet<String>` and again upon insertion. | FIX-CAREFULLY | Borrow `&str` from `pool` after extending with `notes`, avoiding string clones. |
| MEM-ASM-11 | [`src/memory/assembler/rerank.rs#L37-L52`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/rerank.rs#L37-L52) | REDUNDANT-ALLOCATION | In `build_prompt`, `take(30).collect::<String>()`, `replace`, `format!`, and `join("\n")` allocate dozens of transient strings. | FIX-NOW | Use `truncate_chars(&c.full_content, 30)` (zero allocations) and format candidate rows directly. |
| MEM-ASM-12 | [`src/memory/assembler/rerank.rs#L113-L117`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/rerank.rs#L113-L117) | REDUNDANT-ALLOCATION | In `parse_response`, `slot.item_ids.into_iter().filter(...).collect()` allocates a new `Vec<String>`. | FIX-NOW | Use in-place `slot.item_ids.retain(|id| valid_ids.contains(id.as_str()))`. |
| MEM-ASM-13 | [`src/memory/context/enums.rs#L158-L515`](file:///home/zou/data/workspace/Aleph/src/memory/context/enums.rs#L158) | REDUNDANT-ALLOCATION | 7 `FromStr` / `from_str_or_*` implementations call `s.to_lowercase().as_str()`, allocating a heap `String` per parse. | FIX-NOW | Use case-insensitive matching (`eq_ignore_ascii_case`) to parse without heap allocation. |
| MEM-ASM-14 | [`src/memory/context/paths.rs#L8-L13`](file:///home/zou/data/workspace/Aleph/src/memory/context/paths.rs#L8-L13) | REDUNDANT-ALLOCATION | `compute_parent_path` calls `format!("{}/", &trimmed[..pos])` allocating via format machinery. | FIX-NOW | Use `trimmed[..=pos].to_string()` directly since `trimmed[pos]` is already `'/'`. |
| MEM-ASM-15 | [`src/memory/context/fact.rs#L106-L188`](file:///home/zou/data/workspace/Aleph/src/memory/context/fact.rs#L106-L188) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | `MemoryFact::new` and `with_id` duplicate 39 lines of verbatim struct initialization across 28 fields. | FIX-NOW | Extract shared private constructor helper `init_fact` called by both methods. |
| MEM-ASM-16 | [`src/memory/context/fact.rs#L107, L150, L273`](file:///home/zou/data/workspace/Aleph/src/memory/context/fact.rs#L107) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | Repeated 4-line `SystemTime::now().duration_since(...).as_secs()` boilerplate in 3 methods. | FIX-NOW | Consolidate into private helper `current_timestamp_secs() -> i64`. |
| MEM-ASM-17 | [`src/memory/assembler/profile.rs#L32-L40`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/profile.rs#L32-L40) | REDUNDANT-ALLOCATION | `strip_frontmatter(&str) -> String` allocates `String` even when no frontmatter exists. | FIX-NOW | Change signature to `strip_frontmatter(s: &str) -> &str`, allocating only when needed by caller. |
| MEM-ASM-18 | [`src/memory/context_comptroller/comptroller.rs#L44`](file:///home/zou/data/workspace/Aleph/src/memory/context_comptroller/comptroller.rs#L44) | REDUNDANT-ALLOCATION | `let mut final_facts = Vec::new();` reallocates as items are appended. | FIX-NOW | Preallocate with `Vec::with_capacity(kept_facts.len())`. |
| MEM-ASM-19 | [`src/memory/context_comptroller/comptroller.rs#L69-L71`](file:///home/zou/data/workspace/Aleph/src/memory/context_comptroller/comptroller.rs#L69-L71) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | `estimate_tokens(&self, text: &str)` takes unneeded `&self` receiver. | FIX-NOW | Make it `const fn estimate_tokens(text: &str) -> usize`. |
| MEM-ASM-20 | [`src/memory/assembler/rerank.rs#L59-L64`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/rerank.rs#L59-L64) | CONTROL-FLOW / INDIRECTION | Chained `replace` calls copy prompt template 4 times. | DEFER | Crucial prompt-template injection ordering invariant (`{candidates}` before `{query}`). |
| MEM-ASM-21 | [`src/memory/assembler/fallback.rs#L78-L94`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/fallback.rs#L78-L94) | REDUNDANT-ALLOCATION | `skeleton_pack` clones candidate content and metadata. | DEFER | Candidates slice is shared across fallback and tracing paths; changing to consuming API alters signature. |
| MEM-ASM-22 | [`src/memory/assembler/context_block.rs#L45-L62`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/context_block.rs#L45-L62) | CONTROL-FLOW / INDIRECTION | `wrap_memory_context` buffer preallocation and fence wrapping. | REJECT | Already optimal with `String::with_capacity(...)`; strictly verified by scrubber parity tests. |
| MEM-ASM-23 | [`src/memory/assembler/envelope.rs#L8-L86`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/envelope.rs#L8-L86) | ACCIDENTAL-COMPLEXITY | Envelope and item source struct definitions and serde annotations. | REJECT | External serialization contracts and schema v1.0 specifications. |
| MEM-ASM-24 | [`src/memory/assembler/feedback_floor.rs#L154-L162`](file:///home/zou/data/workspace/Aleph/src/memory/assembler/feedback_floor.rs#L154-L162) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | Duplicated `strip_frontmatter` between `profile.rs` and `feedback_floor.rs`. | REJECT | Deliberately kept local per rule-of-three documentation comment. |
| MEM-ASM-25 | [`src/memory/context_comptroller/config.rs#L12-L13`](file:///home/zou/data/workspace/Aleph/src/memory/context_comptroller/config.rs#L12-L13) | DEAD-CODE / ACCIDENTAL-COMPLEXITY | Empty `ComptrollerConfig` struct with dead-code allow. | REJECT | Intentional serialized configuration handle compatibility anchor. |

---

## Invariant Safety Pre-Flight

1. **Hydration Semantics & In-Place Truncation (MEM-ASM-01)**:
   - `truncate_chars` returns a valid UTF-8 slice `&item.content[..byte_len]`.
   - Calling `item.content.truncate(truncated_len)` strictly preserves UTF-8 validity because `truncated_len` is derived directly from a character boundary.
   - `estimate_tokens` output and slot token accounting (`slot.tokens_used`) remain identical.
2. **Deduplication in Feedback Floor (MEM-ASM-02, MEM-ASM-10)**:
   - Storing `&str` in `seen` maintains the exact same uniqueness invariant across partitions (`load_many`) as `String`, without heap allocation.
3. **XML & Markdown Rendering (MEM-ASM-04, MEM-ASM-05, MEM-ASM-06, MEM-ASM-07)**:
   - Output string bytes and fence delimiters (`<memory>`, `<relevant_notes>`, `<MemoryEnvelope>`) remain byte-identical.
   - Closing tag sequence protection (`</` -> `<\/`) is preserved.
4. **Enum Case-Insensitive Parsing (MEM-ASM-13)**:
   - ASCII case-insensitive comparison (`eq_ignore_ascii_case`) produces identical variant matches as `to_lowercase()`, avoiding thousands of allocations during large note index scans.
5. **MemoryFact Aggregate Integrity (MEM-ASM-14, MEM-ASM-15, MEM-ASM-16)**:
   - `init_fact` helper preserves all default field states (`agent: "main"`, `namespace: "owner"`, `layer: L2Detail`, `valid_to: None`).
   - `trimmed[..=pos]` produces the identical string ending in `/` as `format!("{}/", ...)`.

---

## Routing Summary for Deep Reasoning Tier (M3)

- **Total Candidates Identified**: 25
- **Direct Simplifications (`FIX-NOW`)**: 17 candidates
- **Careful Verification Needed (`FIX-CAREFULLY`)**: 1 candidate (`MEM-ASM-10` pool ordering)
- **Deferred (`DEFER`)**: 2 candidates (`MEM-ASM-20` template replacement, `MEM-ASM-21` candidate ownership)
- **Rejected / Intentional (`REJECT`)**: 5 candidates (`MEM-ASM-22`, `MEM-ASM-23`, `MEM-ASM-24`, `MEM-ASM-25`, and hydration heuristic)
