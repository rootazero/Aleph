# Jev Classification Report: Batch LOOP2

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-rev-identity-logging-loopgraph`  
**Target Module**: `src/loop_graph`  
**Files Audited**:
- `src/loop_graph/store.rs`
- `src/loop_graph/service.rs`
- `src/loop_graph/inspector.rs`

---

## 1. Executive Summary

Batch LOOP2 covers the governance topology persistence store (`LoopGraphStore`), the session and victory-claim service layer (`service.rs`), and the read-only query and impact analysis inspector (`LoopGraphInspector`).
The subsystem holds high architectural criticality: it enforces the held-out watcher rule, the objective write-protection ACL (§6.2), the three-layer cap (三层封顶), and victory-claim debounce synchronization with the cron scheduler.

Static review identifies substantial Occam's Razor optimization opportunities across all three files:
1. **Redundant Allocations on Hot & Audit Paths**: Unconditional heap allocation of temporary vectors during edge dangling checks (`lint_dangling_edges` and `impact_of_removing`), repeated `format!` String allocations during prompt assembly (`render_session_topology_inner`), un-deduplicated SQL parameter vectors, and heap `String` allocations inside graph traversals (`walk_chain`).
2. **Accidental Complexity & Stringification Round-Tripping**: Parsing and stringifying `NodeKind`/`EdgeKind` enums in `summary()`, duplicate definitions of coverage predicates (`can_cover` vs `can_cover_kind` violating Shape 1), and redundant post-sorting of nodes already sorted by SQLite.
3. **Correctness & Fail-Closed Invariant**: Silent error discard in `upsert_node` where SQLite database failures during node kind checks were swallowed by `if let Ok(...)` instead of propagating fail-closed via `.optional()`.

---

## 2. Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **L2-1** | `src/loop_graph/store.rs:130-143` | `CORRECTNESS` | `if let Ok(existing_kind) = conn.query_row(...)` in `upsert_node` silently swallows database errors (e.g. disk I/O failure, lock timeout, corruption) and proceeds to attempt insert, violating fail-closed invariant. | **FIX-NOW** | Use `.optional().map_err(|e| AlephError::other(format!("loop_graph node kind check: {e}")))?` to fail-closed on actual DB errors. |
| **L2-2** | `src/loop_graph/store.rs:806-819` | `REDUNDANT-ALLOCATION` | `lint_dangling_edges` allocates an array, filters, and collects a `Vec<&str>` on every edge: `[e.from_id.as_str(), e.to_id.as_str()].into_iter().filter(...).collect()`, even when >99.9% of edges are healthy. | **FIX-NOW** | Check `!present.contains(&e.from_id) \|\| !present.contains(&e.to_id)` first; only allocate the `missing` vector when an edge is actually dangling. |
| **L2-3** | `src/loop_graph/store.rs:430-441` | `REDUNDANT-ALLOCATION` | `labels_for_ids` allocates an intermediate `Vec<&str>` via `repeat_n("?", ids.len()).collect().join(",")` and initializes `params: Vec<&dyn ToSql> = vec![&agent_id]` with capacity 1, causing repeated reallocations on push. | **FIX-NOW** | Pre-allocate `Vec::with_capacity(1 + ids.len())` for parameters and avoid intermediate placeholder vector. |
| **L2-4** | `src/loop_graph/store.rs:211-216, 327-332, 487-490, 514-517, 793-796` | `CONTROL-FLOW` | Verbose imperative `let mut out = Vec::new(); for r in rows { out.push(...); }` boilerplate across `list_nodes`, `list_edges`, `edge_sources`, `node_ids_with_body`, and `node_ids_present`. | **FIX-NOW** | Streamline with idiomatic iterator methods: `filter_map(|r| r.ok().flatten()).collect()` and `collect::<Result<Vec<_>>>()`. |
| **L2-5** | `src/loop_graph/service.rs:754-813` | `REDUNDANT-ALLOCATION` | `render_session_topology_inner` constructs the prompt block using repeated `out.push_str(&format!(...))` calls, allocating and discarding 5-10 intermediate heap `String`s per turn. | **FIX-NOW** | Import `std::fmt::Write as _` and use `let _ = write!(out, ...)` directly into the `out` buffer. |
| **L2-6** | `src/loop_graph/service.rs:724-738` | `CONTROL-FLOW` | Neighbor node IDs collected in `render_session_topology_inner` are not deduplicated before querying `labels_for_ids`, generating duplicate SQL `IN (?, ?)` parameters when multiple edges share endpoints. | **FIX-NOW** | Deduplicate `ids` (`ids.sort_unstable(); ids.dedup();`) before calling `labels_for_ids`. |
| **L2-7** | `src/loop_graph/service.rs:799-800` | `ACCIDENTAL-COMPLEXITY` | `roots.sort_by(|a, b| a.id.cmp(&b.id));` redundantly re-sorts root nodes in memory, even though `store.root_nodes_for_render` SQL query already guarantees `ORDER BY id`. | **FIX-NOW** | Remove redundant sort since SQLite query guarantees strict ordering. |
| **L2-8** | `src/loop_graph/service.rs:248-266, 591-603` | `REDUNDANT-ALLOCATION` | `one_line` and `clamp_root_body` unconditionally allocate new `String`s even when strings contain no newline/separator characters and are within character limits. | **FIX-NOW** | Return `Cow<'_, str>`; return `Cow::Borrowed` on the benign path and only allocate `Cow::Owned` when modification/truncation occurs. |
| **L2-9** | `src/loop_graph/service.rs:142-198` | `ACCIDENTAL-COMPLEXITY` | `DEBOUNCE.get_or_init(|| Mutex::new(HashMap::new()))` is duplicated across `debounce_pass`, `debounce_complete`, and `debounce_rollback`. | **FIX-NOW** | Consolidate into private helper `debounce_map() -> &'static Mutex<HashMap<String, Stamp>>`. |
| **L2-10** | `src/loop_graph/inspector.rs:472-516` | `REDUNDANT-ALLOCATION` | `walk_chain` allocates owned `String`s for `visited: HashSet<String>` and `frontier: Vec<String>`, cloning on every traversal step, despite all node IDs being borrowed `&str` references. | **FIX-NOW** | Change to `visited: HashSet<&str>` and `frontier: Vec<&str>`, zeroing heap string allocations during graph walk. |
| **L2-11** | `src/loop_graph/inspector.rs:311` | `REDUNDANT-ALLOCATION` | `let post_edges: Vec<&GraphEdge> = edges.iter().collect();` in `impact_of_removing` allocates an identical copy of references to `edges` without filtering anything. | **FIX-NOW** | Delete `post_edges` and borrow `edges` directly. |
| **L2-12** | `src/loop_graph/inspector.rs:314-318` | `REDUNDANT-ALLOCATION` | `post_by_id` allocates an entire second `HashMap<&str, &GraphNode>` duplicating `pre_by_id` minus the single removed node. | **FIX-NOW** | Eliminate `post_by_id`; use `pre_by_id` with `if id == node_id { None } else { pre_by_id.get(id) }`. |
| **L2-13** | `src/loop_graph/inspector.rs:365-379` | `REDUNDANT-ALLOCATION` | Simulated dangling edge loop in `impact_of_removing` allocates `missing: Vec<&str>` via `[...].into_iter().filter(...).collect()` on every edge. | **FIX-NOW** | Check `!present.contains(...)` first; only allocate `missing` when an edge is dangling. |
| **L2-14** | `src/loop_graph/inspector.rs:380-394` | `CONTROL-FLOW` | `impact_of_removing` re-scans `nodes` and runs linear search `.contains(&n.id)` on both `would_become_naked` and `already_naked` vectors, even though both sets were already partitioned in the preceding loop. | **FIX-NOW** | Append naked findings directly or avoid linear scans over intermediate vectors. |
| **L2-15** | `src/loop_graph/inspector.rs:424-442` | `ACCIDENTAL-COMPLEXITY` | `summary()` stringifies `NodeKind`/`EdgeKind` enums to `&'static str`, inserts into a `BTreeMap`, and then re-parses every string back via `NodeKind::parse` and `EdgeKind::parse`. | **FIX-NOW** | Store `(n.kind, count)` in `BTreeMap<&'static str, (NodeKind, usize)>` to retain exact alphabetical ordering without string re-parsing. |
| **L2-16** | `src/loop_graph/inspector.rs:521` vs `src/loop_graph/store.rs:756` | `ACCIDENTAL-COMPLEXITY` | Shape 1 duplication: `can_cover` in `store.rs` and `can_cover_kind` in `inspector.rs` are duplicate private functions encoding the same coverage source rule (`kind.is_optimization_loop() \|\| kind == NodeKind::Root`). | **FIX-NOW** | Expose `pub(crate) fn can_cover(kind: NodeKind) -> bool` from `store.rs` and remove duplicate `can_cover_kind` in `inspector.rs`. |
| **L2-17** | `src/loop_graph/inspector.rs:213-231` | `CONTROL-FLOW` | `subgraph_for` traverses `edges` twice (once for `incoming`, once for `outgoing`) with separate `filter_map` and cloning. | **FIX-NOW** | Combine into a single pass over `edges` partitioning into `incoming` and `outgoing`. |
| **L2-18** | `src/loop_graph/inspector.rs:258-271` | `REDUNDANT-ALLOCATION` | `coverage_sources` clones `src: GraphNode` a second time after `incoming` already cloned it from `by_id`. | **FIX-NOW** | Move or reference nodes directly to avoid redundant deep clone. |
| **L2-19** | `src/loop_graph/store.rs:347, 463` | `ACCIDENTAL-COMPLEXITY` | `owns_reference_sources` and `watches_sources` forward to `edge_sources`. | **REJECT** | Keep both methods: they define distinct architectural invariants (§6.2 objective ACL vs victory-claim poke) with documented error semantics. |
| **L2-20** | `src/loop_graph/service.rs:676-681` | `DEAD-CODE` | `render_session_topology_strict` is marked `#[cfg(test)]` and only invoked by unit tests. | **REJECT** | Deliberate test harness pattern to differentiate "ungoverned session" from "unreadable store" without dead code in production. |
| **L2-21** | `src/loop_graph/store.rs:551-552` | `ACCIDENTAL-COMPLEXITY` | Large error tuple `(GcReport, AlephError)` in `gc` triggering `#[allow(clippy::result_large_err)]`. | **REJECT** | Deliberate load-bearing contract allowing callers to inspect proposed removals on rollback without heap boxing indirection. |
| **L2-22** | `src/loop_graph/service.rs:47` | `INVARIANT-TIGHTENING` | Slot classification `MissingSemantics::FailsClosed` for `CRON_TRIGGER`. | **REJECT** | Fixed architectural invariant pinned by `census::every_slot_pins_its_own_missing_semantics`. |

---

## 3. Invariant Safety Pre-Flight

1. **Public API Contracts**:
   - `LoopGraphStore::open`, `open_readonly`, `upsert_node`, `get_node`, `delete_node`, `list_nodes`, `upsert_edge`, `delete_edge`, `list_edges`, `gc`, and `lint` signatures and return types remain strictly unchanged.
   - `LoopGraphInspector::subgraph_for`, `impact_of_removing`, `summary`, `recent_events`, `recent_events_before` signatures remain unchanged.
   - `notify_goal_settled`, `notify_team_settled`, `notify_workflow_settled`, `governing_owner`, and `render_session_topology` signatures and return contracts (`bool`, `Option<String>`) remain intact.
2. **Behavioral Equivalence**:
   - Store SQL schema (`graph_nodes`, `graph_edges`), column constraints, `COALESCE` update semantics, and write-once invariants (`origin`, `kind`) are preserved.
   - Lint findings (`悬空边`, `裸奔优化环`, `伪造的看守覆盖`, `治理链未锚定`, `快环拥有慢环参照`) retain exact text formatting and detection logic.
   - Debounce states (`InFlight`, `Done`, `HeldForSameNode`, `HeldForOtherNode`) and rollback semantics are unaffected.
   - Session prompt injection format and byte limits (`MAX_HANDLE_CHARS = 80`, `MAX_ROOT_BODY_CHARS = 600`) remain identical.
3. **Performance Impact**:
   - Eliminates N heap vector allocations in `lint_dangling_edges` on clean topologies.
   - Eliminates multiple intermediate `format!` string allocations per session prompt render turn in `render_session_topology_inner`.
   - Eliminates intermediate `Vec` allocations in `labels_for_ids` placeholder generation and deduplicates query parameters.
   - Eliminates all heap `String` allocations inside `walk_chain` graph traversals.
   - Eliminates duplicate `HashMap` allocation and redundant `post_edges` clone in `impact_of_removing`.
   - Removes round-trip string parsing of `NodeKind` and `EdgeKind` in `summary()`.
