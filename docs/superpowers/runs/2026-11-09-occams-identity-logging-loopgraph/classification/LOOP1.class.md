# Jev Classification Report: Batch LOOP1

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-identity-logging-loopgraph`  
**Target Module**: `src/loop_graph` (Core, Types, Events, Templates, Snapshot & Export)  
**Files Audited**:
- `src/loop_graph/mod.rs`
- `src/loop_graph/types.rs`
- `src/loop_graph/events.rs`
- `src/loop_graph/templates.rs`
- `src/loop_graph/snapshot.rs`
- `src/loop_graph/export.rs`

---

## 1. Executive Summary

Batch LOOP1 encompasses the core topology primitives of Aleph's loop graph governance layer: node/edge vocabulary, capability-slot globals, topology event broadcast bus, prompt templates, time-travel snapshot storage, snapshot diffing, and Graphviz DOT / JSON exporters.

The module exhibits disciplined architecture with strong domain documentation, clear R7/R9/R10 boundary separation, and fail-soft/fail-closed static capability bindings. However, static review under Occam's Razor identified several high-impact accidental complexity and redundant allocation anti-patterns:
1. **Dynamic AST Allocation in Serialization Paths**: `SnapshotStore::append_event` and `export::to_json` materialize intermediate dynamic `serde_json::Value` ASTs on the heap before immediately stringifying them, inducing dozens to hundreds of transient allocations per event and per export.
2. **Heavy Allocation during Diff Sorting**: `diff_sort_key` in `snapshot.rs` returns a 5-tuple of owned `String`s (allocating 4–5 heap strings per diff item during sorting), despite comparing only string slices.
3. **Unnecessary String Cloning & Truncation Allocations**: `SnapshotStore::capture` unconditionally allocates owned heap strings for `label` even on short strings, and `truncate_chars` in `export.rs` allocates an owned `String` even when the input does not exceed the character limit.
4. **Roundtrip Deserialization & Redundant Parse Branching**: In `diff_inner`, typed `EdgeKind` values are stringified in `map_edges` only to be re-parsed by `EdgeKind::parse`, resulting in an unreachable `None` branch.
5. **Ergonomic and Indirection Cleanups**: Redundant qualified paths (`tokio::sync::broadcast::error::RecvError`, `crate::sync_primitives::Ordering`) and duplicate column reads in rusqlite row mappers.

---

## 2. Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **L1-1** | `src/loop_graph/snapshot.rs:480-539` | `REDUNDANT-ALLOCATION` | `diff_sort_key` returns `(u8, String, String, String, String)` by cloning `id`, `from_id`, `to_id`, allocating `edge_kind.as_str().to_string()`, and pushing multiple `String::new()`. During `out.sort_by_key(diff_sort_key)`, 4–5 heap strings are allocated for every element on each comparison. | **FIX-NOW** | Return `(u8, &str, &str, &str)` borrowing directly from `TopologyDiff`. Zero heap allocations during sort; identical total ordering. |
| **L1-2** | `src/loop_graph/snapshot.rs:329-348` | `REDUNDANT-ALLOCATION` | `append_event` converts `TopologyEvent` to a dynamic `serde_json::Value` AST, allocates an owned `kind: String`, and stringifies `payload_json` from that AST. This double-serializes every broadcast event and allocates dozens of small heap nodes per event write. | **FIX-NOW** | Serialize `ev` directly to `payload_json: String` in one pass. Extract `kind` via borrowed tag deserializer `#[derive(Deserialize)] struct Tag<'a> { kind: &'a str }` or match helper. Zero intermediate AST allocation; preserves dynamic tag derivation. |
| **L1-3** | `src/loop_graph/snapshot.rs:195-202` | `REDUNDANT-ALLOCATION` | `capture` unconditionally heap-allocates an owned `String` for `label` via `label.to_string()` or `.collect()`, even though `rusqlite::params![label, ...]` borrows `&str`. It also traverses the entire string twice when exceeding max characters. | **FIX-NOW** | Slice `&str` directly via `label.char_indices().nth(LABEL_MAX_CHARS).map_or(label, \|(i, _)\| &label[..i])`. Zero heap allocation for short and long labels. |
| **L1-4** | `src/loop_graph/export.rs:116-131` | `REDUNDANT-ALLOCATION` | `to_json` uses `serde_json::json!({ ... "nodes": nodes, "edges": edges })` to build a full dynamic JSON AST on the heap before pretty-printing, allocating hundreds of transient `Value` objects for larger graphs. | **FIX-NOW** | Define private borrowed view `#[derive(Serialize)] struct JsonExport<'a> { agent_id: &'a str, node_count: usize, edge_count: usize, nodes: &'a [GraphNode], edges: &'a [GraphEdge] }` and serialize directly. Zero intermediate AST allocations; byte-identical output. |
| **L1-5** | `src/loop_graph/export.rs:69` | `REDUNDANT-ALLOCATION` | `out.push_str(&format!("// loop_graph export, agent={agent_id}\n"));` allocates an intermediate `String` via `format!` only to append it to `out`, while `std::fmt::Write` is already in scope. | **FIX-NOW** | Use `writeln!(out, "// loop_graph export, agent={agent_id}").map_err(...)` or `out.push_str(...)` without `format!`. |
| **L1-6** | `src/loop_graph/export.rs:85-94, 204-216` | `REDUNDANT-ALLOCATION` | `truncate_chars` does `s.to_string()` when `s` is shorter than `max`. In `to_dot`, `dot_escape` is called on both `label` and `tooltip`, allocating intermediate strings even when `tooltip` is empty (`""`). | **FIX-NOW** | Return `Cow<'_, str>` from `truncate_chars` (borrowed if under max). Avoid calling `dot_escape` on empty tooltip. |
| **L1-7** | `src/loop_graph/snapshot.rs:368-369, 582-583` | `ACCIDENTAL-COMPLEXITY` | In `row_to_summary` and `list_events`, `r.get(2)?` / `r.get(1)?` is called twice consecutively on the same column to extract `ms` and compute `iso_from_ms(r.get(...)?)`. | **FIX-NOW** | Bind `let ts_ms: i64 = r.get(...)?;` once and reuse for both numeric and ISO timestamp fields. |
| **L1-8** | `src/loop_graph/snapshot.rs:249-253, 375-378` | `CONTROL-FLOW` | Manual imperative accumulation loops (`let mut out = Vec::new(); for r in rows { out.push(r.map_err(...)?); }`) in `list_snapshots` and `list_events`. | **FIX-NOW** | Replace with idiomatic `rows.map(\|r\| r.map_err(...)).collect::<Result<Vec<_>>>()`. |
| **L1-9** | `src/loop_graph/mod.rs:202-211` | `CONTROL-FLOW` | `spawn_event_persister` uses fully-qualified `tokio::sync::broadcast::error::RecvError::Lagged` and `RecvError::Closed` across verbose match arms. | **FIX-NOW** | Import `use tokio::sync::broadcast::error::RecvError;` and flatten match branches to `Err(RecvError::Lagged(n))` and `Err(RecvError::Closed)`. |
| **L1-10** | `src/loop_graph/events.rs:37, 170, 175` | `ACCIDENTAL-COMPLEXITY` | Fully-qualified paths `crate::sync_primitives::Ordering::Relaxed` and `crate::sync_primitives::Ordering::AcqRel` used inline instead of importing `Ordering`. | **FIX-NOW** | Add `Ordering` to `use crate::sync_primitives::{Arc, AtomicU64, Ordering};`. |
| **L1-11** | `src/loop_graph/snapshot.rs:418-461, 473-478` | `ACCIDENTAL-COMPLEXITY` | `map_edges` converts `e.kind: EdgeKind` to `&str` via `.as_str()`, and `diff_inner` re-parses it via `EdgeKind::parse(key.2)`. Because `e.kind` is already a valid enum variant, `EdgeKind::parse` can never fail; the `None => out.push(TopologyDiff::EdgeUnknown)` arm is unreachable dead code. | **FIX-CAREFULLY** | Key `map_edges` by `(&str, &str, EdgeKind)` directly. Retain `TopologyDiff::EdgeUnknown` in enum for public wire stability, but eliminate string round-tripping and unreachable error arms in `diff_inner`. |
| **L1-12** | `src/loop_graph/export.rs:174-185` | `REDUNDANT-ALLOCATION` | `dot_id` unconditionally iterates and allocates a new `String` for every node and edge identifier, even when the id consists entirely of valid characters (`[A-Za-z0-9_]`). | **FIX-CAREFULLY** | Return `Cow<'_, str>`; borrow unchanged string if `id.chars().all(\|c\| c.is_ascii_alphanumeric() \|\| c == '_')`. |
| **L1-13** | `src/loop_graph/export.rs:187-202` | `REDUNDANT-ALLOCATION` | `dot_escape` always allocates a new `String` with `with_capacity(s.len())`, even when no escapable characters (`"`, `\`, `\n`, `\r`, `<`, `>`, `&`) are present. | **FIX-CAREFULLY** | Check if string contains escapable characters via fast byte scan before allocating; return `Cow::Borrowed(s)` on clean strings. |
| **L1-14** | `src/loop_graph/events.rs:97-106` | `ACCIDENTAL-COMPLEXITY` | `inner: Arc<broadcast::Sender<TopologyEvent>>` wraps `Sender` in an outer `Arc`. `tokio::sync::broadcast::Sender` already uses internal reference counting and is cheap to clone. | **FIX-CAREFULLY** | Verify `broadcast::Sender` clone semantics with `subscriber_count()`; if exact parity is confirmed, replace `Arc<Sender>` with `Sender`. |
| **L1-15** | `src/loop_graph/snapshot.rs:584-585` | `PANIC-RISK` | In `row_to_summary`, `r.get::<_, i64>(3)? as usize` casts signed database integer directly to `usize`. Corrupted or negative database values could underflow or panic on 32-bit platforms. | **FIX-CAREFULLY** | Use `usize::try_from(val).unwrap_or(0)` for defensive numeric conversions. |
| **L1-16** | `src/loop_graph/events.rs:168-176` | `DEAD-CODE` | `TopologyEventBus::record_lag` and `dropped_events` are only referenced in unit tests. | **REJECT** | Keep: deliberate diagnostic API surface for the event bus, documented in `GRAPH_LAYER.md`. |
| **L1-17** | `src/loop_graph/types.rs:248-260, 298-307` | `ACCIDENTAL-COMPLEXITY` | `GraphNode::new` and `GraphEdge::new` take generic `impl Into<String>` for multiple string fields. | **REJECT** | Keep: standard idiomatic Rust builder ergonomics permitting both zero-allocation moves of `String` and convenient `&str` pass-through. |
| **L1-18** | `src/loop_graph/templates.rs:68-70` | `ACCIDENTAL-COMPLEXITY` | `AUDIT_NODE_BODY` is re-exported at `mod.rs` root, whereas other template constants remain under `templates::*`. | **REJECT** | Keep: deliberate public constant re-export acting as the well-known body marker for the loop graph layer. |
| **L1-19** | `src/loop_graph/types.rs:338` vs `src/loop_graph/snapshot.rs:209` | `ACCIDENTAL-COMPLEXITY` | Local helper `fn now_ms() -> i64` in `types.rs` vs inline `Utc::now().timestamp_millis()` in `snapshot.rs`. | **REJECT** | Keep: 1-line localized timestamp calls; DRYing across module boundaries violates Occam's Razor § Idiomatic Rust (avoid premature abstractions for 1-line standard library calls). |

---

## 3. Invariant Safety Pre-Flight

1. **Public API & Wire Contracts**:
   - `TopologyEvent`, `SnapshotSummary`, `Snapshot`, `TopologyDiff`, `GraphNode`, `GraphEdge`, `NodeKind`, `EdgeKind`, `Origin` struct and enum definitions remain completely unchanged.
   - Serde field names, tags (`#[serde(tag = "kind", rename_all = "snake_case")]`), and enum discriminators remain 100% stable.
   - `to_dot` and `to_json` output formats, ordering guarantees, and field schemas remain byte-for-byte identical.
   - `export.rs` comparator order (`sorted_topology`: `(kind, id)` for nodes, `(from_id, to_id, kind)` for edges) remains unaltered, protecting `json_bytes_are_identical_for_shuffled_insertion_order`.

2. **Behavioral Equivalence**:
   - `diff_sort_key` returns `(u8, &str, &str, &str)` which produces the identical total lexicographic order as the previous `(u8, String, String, String, String)`.
   - `SnapshotStore::capture` label truncation at `LABEL_MAX_CHARS` (200 chars) via `char_indices().nth()` matches the exact character boundary previously produced by `chars().take(200).collect()`.
   - `SnapshotStore::append_event` preserves dynamic serde `kind` extraction and exact JSON payload persistence in the `events` SQLite table.

3. **Performance Impact**:
   - Eliminates intermediate `serde_json::Value` AST tree generation in `append_event` on every bus event write.
   - Eliminates intermediate `serde_json::Value` AST tree generation in `to_json` for the entire graph.
   - Eliminates 4–5 heap `String` allocations per diff item in `SnapshotStore::diff_snapshots` sorting.
   - Eliminates unconditional heap string allocation in `SnapshotStore::capture` for labels.
   - Eliminates redundant heap string allocations in `export::to_dot` header and tooltip formatting.
