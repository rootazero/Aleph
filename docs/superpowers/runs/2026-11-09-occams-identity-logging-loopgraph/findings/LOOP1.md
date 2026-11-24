# Findings & Remediation: Batch LOOP1 (Loop Graph Core & Events, Templates, Snapshot & Export)

**Commit**: `2b4ee2863`  
**Files Touched**:
- `src/loop_graph/snapshot.rs`
- `src/loop_graph/export.rs`
- `src/loop_graph/events.rs`
- `src/loop_graph/mod.rs`

## Remediation Applied

1. **Zero-Allocation Diff Sorting (`L1-1`)**:
   - `diff_sort_key` previously returned `(u8, String, String, String, String)` cloning 4-5 strings per comparison during `sort_by_key`.
   - Now returns `(u8, &'a str, &'a str, &'a str)` borrowing directly from `TopologyDiff`, eliminating all heap allocations during snapshot diff sorting while preserving exact lexicographical ordering.

2. **Single-Pass Event Serialization (`L1-2`)**:
   - In `SnapshotStore::append_event`, replaced double-serialization through dynamic `serde_json::Value` with a single `serde_json::to_string(ev)` pass and zero-copy borrowed `EventTag` extraction, eliminating transient AST allocations on every broadcast bus write.

3. **Zero-Allocation Label Truncation (`L1-3`)**:
   - In `SnapshotStore::capture`, replaced `chars().take(200).collect()` and `label.to_string()` with zero-allocation `char_indices().nth(LABEL_MAX_CHARS)` slice.

4. **Zero-AST Graph JSON Exporter (`L1-4`)**:
   - In `export::to_json`, replaced dynamic `serde_json::json!` AST with private borrowed view struct `JsonExport<'a>`, eliminating hundreds of heap node allocations during graph serialization.

5. **Direct Formatting & Cow Strings in DOT Export (`L1-5`, `L1-6`, `L1-12`, `L1-13`)**:
   - Used `writeln!` directly into output buffer in `to_dot`.
   - Converted `truncate_chars`, `dot_id`, and `dot_escape` to return `Cow<'_, str>`, borrowing untouched slices on clean identifiers and strings.

6. **SQLite Row Mapping Deduplication (`L1-7`, `L1-8`, `L1-15`)**:
   - Bound timestamp columns once in `row_to_summary` and `list_events`.
   - Replaced imperative loops with idiomatic `.collect()`.
   - Used defensive `usize::try_from` for row counts.

7. **Direct Enum Map Keying (`L1-11`)**:
   - Keyed `map_edges` by `(&str, &str, EdgeKind)` directly, eliminating string round-tripping through `.as_str()` and unreachable error branches in `diff_inner`.

8. **Ergonomic Imports (`L1-9`, `L1-10`)**:
   - Imported `Ordering` in `events.rs` and `RecvError` in `mod.rs`.
