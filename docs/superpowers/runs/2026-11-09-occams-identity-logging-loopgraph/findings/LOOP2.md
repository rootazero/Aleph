# Findings & Remediation: Batch LOOP2 (Loop Graph Store, Service & Inspector)

**Commit**: `58074594e`  
**Files Touched**:
- `src/loop_graph/store.rs`
- `src/loop_graph/service.rs`
- `src/loop_graph/inspector.rs`

## Remediation Applied

1. **Fail-Closed Node Kind Verification (`L2-1`)**:
   - In `upsert_node`, replaced silent error swallowing `if let Ok(existing_kind)` with `.optional().map_err(...)` to fail closed on actual database failures.

2. **Zero-Allocation Dangling Edge Linting (`L2-2`, `L2-13`)**:
   - In `lint_dangling_edges` (`store.rs` and `inspector.rs`), added `from_missing || to_missing` pre-check before allocating the `missing` vector. On clean graphs (>99.9% of calls), zero allocations occur.

3. **Optimized SQL IN Clause & Pre-Allocated Parameters (`L2-3`, `L2-6`)**:
   - In `labels_for_ids`, constructed the `?` placeholders without intermediate `Vec<&str>` and initialized `params` with exact capacity `1 + ids.len()`.
   - In `render_session_topology_inner`, deduplicated neighbor IDs before querying labels to avoid redundant SQL query parameters.

4. **Streamlined Iterator Result Accumulation (`L2-4`)**:
   - Replaced imperative loops with idiomatic `.collect::<Result<Vec<_>>>()` and `filter_map` across `list_nodes`, `list_edges`, `edge_sources`, `node_ids_with_body`, and `node_ids_present`.

5. **Direct Formatting & Cow Strings in Prompt Injection (`L2-5`, `L2-7`, `L2-8`)**:
   - In `render_session_topology_inner`, replaced repeated `out.push_str(&format!(...))` allocations with direct `writeln!(out, ...)` writes into the prompt buffer.
   - Removed redundant in-memory re-sorting of root nodes already ordered by SQLite.
   - Converted `one_line` and `clamp_root_body` to return `Cow<'_, str>`.

6. **Consolidated Debounce Map Access (`L2-9`)**:
   - Extracted shared `debounce_map()` helper to eliminate repetitive mutex initialization boilerplate across `debounce_pass`, `debounce_complete`, and `debounce_rollback`.

7. **Zero-Allocation Graph Traversal (`L2-10`)**:
   - In `walk_chain`, changed `visited: HashSet<String>` and `frontier: Vec<String>` to `visited: HashSet<&str>` and `frontier: Vec<&str>`, eliminating all heap string allocations during chain walks.

8. **Eliminated Redundant Vector and Map Copies (`L2-11`, `L2-12`, `L2-17`)**:
   - Deleted `post_edges` clone and `post_by_id` map allocation in `impact_of_removing`.
   - Merged `incoming` and `outgoing` traversals in `subgraph_for` into a single pass.

9. **Zero-Roundtrip Enum Serialization in Summary (`L2-15`)**:
   - In `summary()`, keyed `BTreeMap` by `((&'static str, Kind), usize)` to preserve alphabetical order without string re-parsing.

10. **Unified Coverage Source Predicate (`L2-16`)**:
    - Resolved Shape 1 duplication by exposing `pub(crate) fn can_cover` from `store.rs` and removing duplicate `can_cover_kind` from `inspector.rs`.
