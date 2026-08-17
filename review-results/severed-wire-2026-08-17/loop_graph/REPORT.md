# Severed-Wire Audit — `src/loop_graph`

- Date: 2026-08-17
- Tree: `/home/zou/data/workspace/Aleph/.worktrees/review-fix-2026-08-17` (HEAD newer than the graphify graph at 9841b5b2; every claim re-verified with `rg`)
- Module: `src/loop_graph` (9 files, 6,461 LOC: `events.rs`, `export.rs`, `inspector.rs`, `mod.rs`, `service.rs`, `snapshot.rs`, `store.rs`, `templates.rs`, `types.rs`)
- Method: PRODUCED–CONSUMED symbol parity (`rg` across `src/` incl. `src/bin/`, `bin/`, `interfaces/`, `shared/`, `desktop/`). No cargo runs. Read-only.
- Prior review refs: no prior severed-wire audit covers `src/loop_graph` (checked `review-results/` — only `REVIEW_PROTOCOL.md`, `logging/`, and unrelated batch reports); context commit e8b7c3cf9 ("init_unified: cut dead first-run module") already dropped the module's test-only-visibility cruft (`render_session_topology_strict` is now `#[cfg(test)]`, no `#[allow(dead_code)]` remains anywhere in the module). `existing_review_ref = null` throughout.

## Wiring verdicts (questions posed by the audit task)

1. **Is `service.rs` reached from production, or only from tests? — REACHED (6 call-site clusters).**
   - `notify_goal_settled`: `src/builtin_tools/goal.rs:1140`, `src/gateway/execution_engine/goal_continuation.rs:224,393` (gateway; 3 call sites match the doc's "behind the store CAS" claim)
   - `notify_team_settled`: `src/builtin_tools/team/disband.rs:100`, `src/gateway/handlers/teams/crud.rs:134`
   - `notify_workflow_settled`: `src/teams/dispatcher/schedule/settle.rs:371` (teams dispatcher settle sweep)
   - `governing_owner`: `src/builtin_tools/goal.rs:697` (via `governing_owner_or_refuse`, invoked at `:783,:995,:1171`)
   - `render_session_topology`: `src/orchestrator/harness_bridge/prompt_build.rs:842` (orchestrator prompt build — production)
   - `init_cron_trigger`: `src/executor/builtin_registry/builder/constructor/mod.rs:531`

2. **Is `store.rs` reached from production? — YES.** `LoopGraphStore::open` at `constructor/mod.rs:497`; `open_readonly` + `lint` + `list_nodes` at `src/diagnostics/checks/loop_graph.rs:56-60,92-94` (doctor `core/loop-graph`); `gc`/`lint`/`upsert_*`/`node_ids_with_body` via the `loop_graph` tool (`src/builtin_tools/loop_graph_manage.rs`); `watches_sources`/`owns_reference_sources` via `service.rs:274,469`. `GcReport` flows through the tool's `Gc` action (fields `removed`/`retained_acl` read at `loop_graph_manage.rs:722-726`), though the type name is never spelled outside `store.rs`.

3. **Is `export.rs` reached from production? — YES, one consumer.** `to_dot`/`to_json` both called by the tool's `Export` action (`loop_graph_manage.rs:1160-1161`). But the module doc's other two claimed consumers (Panel, `governance_metrics`) do not exist — see sw-lg-10.

4. **Is `templates.rs` a dead export path? — NO, 7 of 8 consts are production-wired.** `AUDIT_TEMPLATE`, `AUDIT_JOB_NAME`, `AUDIT_DEFAULT_CRON_EXPR` (`loop_graph_manage.rs:16,828,844,855,892,916` — `enable_audit`); `WATCH_DEFAULT_CRON_EXPR`, `WATCH_TEMPLATE_HEADER`, `WATCH_TEMPLATE_FOOTER` (`:980,987-988` — `pair`); `AUDIT_NODE_BODY` (`:787,859,920`, plus re-export at `mod.rs:37`). Only `AUDITOR_LACKS_CRON_MANAGE` is test-only (sw-lg-07).

5. **`loop_graph` ↔ `looping`: no wiring — BY DESIGN, not severed.** `rg -n "looping" src/loop_graph/` → no code references; `rg -ln "loop_graph" src/looping/` → no files. The module doc (R7) states dreaming/optimizers have no write path into the topology (held-out layer). No finding.

6. **`loop_graph` ↔ gateway/orchestrator: wired** (see verdict 1: `goal_continuation.rs`, `teams/crud.rs`, `prompt_build.rs`). The global boot chain (`constructor/mod.rs:507-531`: `init_global` → `init_event_bus` → `SnapshotStore::open` → `spawn_event_persister` → `init_cron_trigger`) is complete and production.

7. **What REMAINS severed after e8b7c3cf9:** the *read-facade half of `inspector.rs`* (`subgraph_for`, `loops_with_coverage`, `summary`), three bus/snapshot knobs (`event_bus()`, `subscriber_count`, `delete_snapshot`), one test-only template const, one stale doc symbol, the redundant `mod.rs` re-export surface, and two doc claims. All low except sw-lg-01 (medium). No form-5 config knobs, no `#[allow(dead_code)]`, no `#[deprecated]` items remain.

## Findings

### sw-lg-01 — `LoopGraphInspector::subgraph_for` + `NodeSubgraph` (orphaned read facade, zero production consumers)

- **Form**: 1 (visible pub symbol with zero production consumers; orphaned pub API) — also 4 (consumed only by tests)
- **Severity**: medium
- **Produced**: `pub fn subgraph_for(&self, node_id: &str) -> Result<Option<NodeSubgraph>>` — `src/loop_graph/inspector.rs:187`; `pub struct NodeSubgraph` (+ its 9 pub fields `node`/`incoming`/`outgoing`/`ancestor_chain`/`descendant_chain`/`coverage_sources`/`governance_chain_anchored`/`all_roots`) — `src/loop_graph/inspector.rs:51-77`. Re-exported at `src/loop_graph/mod.rs:32`.
- **Consumers**: only tests inside `inspector.rs` (`:620, :647, :682, :902, :922`) and a production *comment* explaining why the `List` action deliberately bypasses it (`loop_graph_manage.rs:699-700`). Evidence:
  ```
  $ rg -n "subgraph_for" src/ bin/ interfaces/ shared/ desktop/
  src/loop_graph/inspector.rs:187 (def), :620 :647 :682 :902 :922 (tests)
  src/loop_graph/inspector.rs:23, :75, :437 (doc mentions)
  src/builtin_tools/loop_graph_manage.rs:699 (comment only: "subgraph_for is per-node … neither is this action's … semantics")
  ```
- **Rationale**: the module doc (`inspector.rs:3-13`) sells the inspector as "one inspector, one set of read rules, multiple consumers" for the four readers (`render_session_topology`, `notify_goal_settled`, `governing_owner`, `status`), but all four still hand-roll their reads against the store: `service.rs::render_session_topology_inner` uses `store.get_node/list_edges/list_nodes` (`service.rs:592-598`), `loop_graph_manage::render_status` uses `list_nodes/list_edges/lint` (`loop_graph_manage.rs:258-260`). `subgraph_for` — the facade's headline API, 5 tests, ~90 lines including `walk_chain` — is unreachable from production.
- **Proposed change**: DECIDE. Options: (a) CUT `subgraph_for` + `NodeSubgraph` + the `walk_chain`/`Direction` helpers they exclusively use + 5 tests (`inspector.rs:184-267, 615-655, 680-694, 897-907, 917-928`), removing `NodeSubgraph` from the `mod.rs:32` re-export; (b) CONNECT — migrate `render_session_topology_inner`'s per-node walk onto `subgraph_for` (behavioral risk: prompt bytes must stay byte-identical, and the current render filters edges by kind at render time, which `NodeSubgraph`'s raw `incoming`/`outgoing` pairs already carry); (c) keep as the documented future home. The doc's "multiple consumers" framing is the only contract-ish promise; nothing at runtime breaks either way.
- **Risk**: nil at runtime (zero reachable callers); option (b) is the only risky path and only if it changes prompt bytes.
- **Verification**: `rg -n "subgraph_for" src/ bin/ interfaces/ shared/ desktop/` → no production call. Cargo check/clippy not run (read-only constraint).

### sw-lg-02 — `LoopGraphInspector::loops_with_coverage` (dead helper)

- **Form**: 1
- **Severity**: low
- **Produced**: `pub fn loops_with_coverage(&self) -> Result<Vec<(GraphNode, Vec<GraphNode>)>>` — `src/loop_graph/inspector.rs:439`
- **Consumers**: none; single test at `src/loop_graph/inspector.rs:957` (`loops_with_coverage_skips_non_optimization_nodes`, declared at `:930`). The only other mention is the same `loop_graph_manage.rs:699` comment as sw-lg-01. Evidence: `rg -n "loops_with_coverage" src/ bin/ interfaces/ shared/ desktop/` → `inspector.rs:439 (def), :437 (doc), :930, :957 (tests)`, `loop_graph_manage.rs:699 (comment)`.
- **Rationale**: pure helper for the same unwired facade half as sw-lg-01; no doc promise of a consumer, no runtime reach.
- **Proposed change**: CUT — delete `inspector.rs:437-457` (fn + doc) and the test at `:930-958`. Safe: no production consumer, no behavioral effect.
- **Risk**: nil.
- **Verification**: `rg -n "loops_with_coverage" src/ bin/ interfaces/ shared/ desktop/` → only def/tests/comment above.

### sw-lg-03 — `LoopGraphInspector::summary` + `TopologySummary` + `TopologySummary::render` (documented test-only aggregate API)

- **Form**: 4 (produced but consumed only by tests)
- **Severity**: low
- **Produced**: `pub fn summary(&self) -> Result<TopologySummary>` — `src/loop_graph/inspector.rs:394`; `pub struct TopologySummary` — `:118-123`; `pub fn render(&self) -> String` — `:129`. Re-exported at `mod.rs:32`.
- **Consumers**: tests only — `src/builtin_tools/loop_graph_manage.rs:1967-1969` (`status_counts_agree_with_inspector_summary`, inside `mod tests` which starts at `:1366`) and `src/loop_graph/inspector.rs:862`. The doc comment is explicit: "the only production consumer of the inspector today is the `impact` action … `loop_graph status` does NOT consume it today … If status's aggregate half ever moves, it moves HERE" (`inspector.rs:396-410`). Evidence: `rg -n "\.summary\(\)" src/ bin/` → `inspector.rs:862 (test)`, `loop_graph_manage.rs:1968 (test)`; other `.summary()` hits are unrelated types (`sandbox.summary()`).
- **Rationale**: deliberately-kept, honestly-documented test-only API; the test pins `status`'s totals to the summary (consistency lock). Neither severed-by-accident nor load-bearing in production.
- **Proposed change**: DECIDE. (a) Keep as-is (documented future single-source + pinning test); (b) CUT `summary`/`TopologySummary`/`render` + both tests and drop `TopologySummary` from `mod.rs:32` — the consistency pin would move into `render_status` itself. No runtime behavior either way.
- **Risk**: nil.
- **Verification**: `rg -n "\.summary\(\)" src/ bin/ interfaces/ shared/ desktop/` → only the two test sites.

### sw-lg-04 — `SnapshotStore::delete_snapshot` (declared-but-never-wired stub)

- **Form**: 2 (declared-but-never-wired stub)
- **Severity**: low
- **Produced**: `pub fn delete_snapshot(&self, id: i64) -> Result<bool>` — `src/loop_graph/snapshot.rs:302-309`. Doc: "Deliberately NOT exposed through the `loop_graph` tool today; only a future admin RPC will reach it" (`:300-301`).
- **Consumers**: none. Evidence: `rg -n "delete_snapshot" src/ bin/ interfaces/ shared/ desktop/` → `snapshot.rs:302 (def)`, `snapshot.rs:750,752 (test delete_removes_a_snapshot)`; the other hits are unrelated (`teams/snapshots/tests.rs:423`, `gateway/.../modify.rs:1138` tempfile name). The tool's `Snapshot` op accepts only `capture | list | diff` (`loop_graph_manage.rs:1180-1252`), so no `get`/`delete` path exists.
- **Rationale**: a deliberate stub with a documented future caller, but today it is pub API with zero reach — the exact form-2 shape. It also keeps the snapshot table append-only-ish (rows can only grow; the `capture` insert path has no delete counterpart anywhere).
- **Proposed change**: DECIDE. (a) Keep until the admin RPC lands (documented intent); (b) CUT `:299-309` + the test assertions at `:750,752` (test `delete_removes_a_snapshot` would shrink to list-only). Runtime impact nil either way; cutting removes the only deletion path in the crate for audit rows — a governance-adjacent judgment call.
- **Risk**: nil today; if kept, note the audit-log rows have no retention path.
- **Verification**: `rg -n "delete_snapshot" src/ bin/ interfaces/ shared/ desktop/` → def + tests only.

### sw-lg-05 — `TopologyEventBus::subscriber_count` (dead observer metric)

- **Form**: 1
- **Severity**: low
- **Produced**: `pub fn subscriber_count(&self) -> usize` — `src/loop_graph/events.rs:145-147`. Doc: "the count is exposed for a future observer-metric …, not read by anything yet" (`:140-143`); module doc admits "Nothing reads it today; the earlier claim that `governance_metrics` did was aspirational" (`:21-24`).
- **Consumers**: none. Evidence: `rg -n "subscriber_count" src/ bin/ interfaces/ shared/ desktop/` → `events.rs:145 (def)`, `:171, :244-251 (tests)`, `:104 (Debug impl field name)`; `gateway/event_bus.rs:456,565-573` and `event/bus.rs:6` are a different subsystem's same-named method.
- **Rationale**: honest doc, zero consumers — the "how many live observers" metric (bus died?) never got a reader. The Debug impl uses `receiver_count()` directly, so removing the pub method loses nothing.
- **Proposed change**: DECIDE. (a) CUT `:140-147` + the assertions at `:171, :244-251` (test `subscriber_count_reflects_live_subscribers_only` and the first assert of `publish_with_no_subscribers_is_silent_no_error`); (b) keep as the documented future metric. No runtime change either way.
- **Risk**: nil.
- **Verification**: `rg -n "subscriber_count" src/ bin/ interfaces/ shared/ desktop/` → def + tests only.

### sw-lg-06 — `loop_graph::event_bus()` (test-only global bus reader)

- **Form**: 4
- **Severity**: low
- **Produced**: `pub fn event_bus() -> Option<TopologyEventBus>` — `src/loop_graph/mod.rs:69-71` (pair with `init_event_bus` at `:64-66`, which IS production: `constructor/mod.rs:509`).
- **Consumers**: tests only — `src/builtin_tools/loop_graph_manage.rs:1775,1778` (`node_action_publishes_upsert_to_global_bus`, inside `mod tests` at `:1366`). Production reads the global bus through `publish()` (`mod.rs:75-79`, which touches `EVENT_BUS` directly) and hands the bus to `spawn_event_persister` as a parameter (`constructor/mod.rs:530`, subscribe at `mod.rs:98`). Evidence: `rg -n "loop_graph::event_bus" src/ bin/ interfaces/ shared/ desktop/` → only `loop_graph_manage.rs:1775,1778`; other `event_bus()` hits (`clarification/ask.rs`, `teams/…`) are the gateway/team event-emitter buses.
- **Rationale**: the global-read half of the bus exists only so the e2e test can subscribe to the process-global bus; no production code needs to read it back out.
- **Proposed change**: DECIDE. (a) Keep — it is the test's only window into the global bus and costs 3 lines; (b) CUT `mod.rs:68-71` and rework the test to subscribe via the bus instance before `init_event_bus` (loses the "what the global bus actually receives" end-to-end property). Runtime behavior identical.
- **Risk**: nil.
- **Verification**: `rg -n "event_bus\(\)" src/ bin/ interfaces/ shared/ desktop/` → def at `mod.rs:69`, test uses at `loop_graph_manage.rs:1775,1778`.

### sw-lg-07 — `templates::AUDITOR_LACKS_CRON_MANAGE` (test-only const; "single source" claim does not hold in production)

- **Form**: 4 (produced, consumed only by tests) — with a form-5 flavor: the doc describes a single-source mechanism that does not exist
- **Severity**: low
- **Produced**: `pub const AUDITOR_LACKS_CRON_MANAGE: &str = "loop-auditor 审计员没有 cron_manage";` — `src/loop_graph/templates.rs:56`. Doc: "The one fact … both governance templates must carry, asserted from a single place so they cannot drift apart again" (`:48-54`).
- **Consumers**: only the test `every_template_that_delegates_to_the_auditor_names_its_one_blind_spot` (`templates.rs:118-122`). Evidence: `rg -n "AUDITOR_LACKS_CRON_MANAGE" src/ bin/ interfaces/ shared/ desktop/` → `templates.rs:56 (def)`, `:122 (test)`. The two templates hardcode the text as plain string literals (`AUDIT_TEMPLATE` at `:16-17`; `WATCH_TEMPLATE_HEADER` at `:79-80`); the const is never interpolated (`concat!` cannot embed a const, and the templates are used as verbatim `&'static str` for `CronJob` prompts and `j.prompt == AUDIT_TEMPLATE` comparisons at `loop_graph_manage.rs:828`).
- **Rationale**: the const is a *test oracle*, not a production single source — the "cannot drift apart" mechanism is a test assertion, and only while the test runs. Not severed-by-accident, but the doc overstates the mechanism.
- **Proposed change**: DECIDE. (a) Keep as test oracle and fix the doc (`templates.rs:48-54`) to say "guarded by test, not interpolation"; (b) CUT the const and inline the literal in the test. Runtime identical either way.
- **Risk**: nil.
- **Verification**: `rg -n "AUDITOR_LACKS_CRON_MANAGE" src/ bin/ interfaces/ shared/ desktop/` → def + test only.

### sw-lg-08 — Stale doc reference to non-existent `install_into_store`

- **Form**: 3 (consumer/reference to a symbol that no longer exists)
- **Severity**: low
- **Produced**: doc line `src/loop_graph/events.rs:112`: "…the store does NOT hold a reference to this bus. Wiring is done by `install_into_store`."
- **Consumers of the symbol**: none — `rg -n "install_into_store" src/ bin/ interfaces/ shared/ desktop/` → single hit at `events.rs:112`. No such function exists anywhere (wiring is `loop_graph::init_event_bus` + `spawn_event_persister`, `constructor/mod.rs:509,530`).
- **Rationale**: the sentence names a wiring function that was never written (or was renamed away); a reader of the doc cannot find it. Pure doc drift, but this module's doc discipline is otherwise careful, so the stale cross-reference stands out.
- **Proposed change**: CUT (doc-only): replace "Wiring is done by `install_into_store`" with "Wiring is done at boot by `loop_graph::init_event_bus` + `spawn_event_persister` (`builtin_registry/builder/constructor/mod.rs`)" — or drop the sentence. No code change.
- **Risk**: nil.
- **Verification**: `rg -n "install_into_store" src/ bin/ interfaces/ shared/ desktop/` → `events.rs:112` only.

### sw-lg-09 — Orphaned `mod.rs` re-export surface (8 names never imported by name)

- **Form**: 6 (pub items re-exported but unused)
- **Severity**: low
- **Produced**: `src/loop_graph/mod.rs:32-35`:
  - `:32` `pub use inspector::{ImpactReport, LoopGraphInspector, NodeSubgraph, TopologySummary};`
  - `:33-34` `pub use service::notify_team_settled; pub use service::notify_workflow_settled;`
  - `:35` `pub use snapshot::{EventRecord, Snapshot, SnapshotStore, SnapshotSummary, TopologyDiff};`
- **Consumers by name**: only `LoopGraphInspector` (`loop_graph_manage.rs:1095,1266,1967`), `SnapshotStore` (`constructor/mod.rs:515`, `loop_graph_manage.rs:157,1818`), `TopologyDiff` (`loop_graph_manage.rs:1297-1298`). Evidence:
  ```
  $ rg -n "loop_graph::(ImpactReport|NodeSubgraph|TopologySummary|EventRecord|Snapshot\b|SnapshotSummary|notify_team_settled|notify_workflow_settled)" src/ bin/ interfaces/ shared/ desktop/
  → no matches
  $ rg -n "loop_graph::service::(notify_team_settled|notify_workflow_settled)" src/
  → src/builtin_tools/team/disband.rs:100, src/gateway/handlers/teams/crud.rs:134, src/teams/dispatcher/schedule/settle.rs:371
  ```
- **Rationale**: all production call sites reach `notify_team_settled`/`notify_workflow_settled` via the `service::` path, and `ImpactReport`/`EventRecord`/`Snapshot`/`SnapshotSummary`/`NodeSubgraph`/`TopologySummary` flow only through inferred return types (the tool's `impact`/`events`/`snapshot` actions read their fields without ever naming them). The re-exports are redundant public surface: two fully shadowed functions plus six names with no importer. (Same shape as the logging audit's sw-lo-02, which was DECIDE.)
- **Proposed change**: DECIDE. Deleting the 6 unused names from `:32`/`:35` and both `service::` re-exports at `:33-34` is safe (no in-repo importer — no cross-crate use either: `rg -n "loop_graph" interfaces/ shared/ desktop/` → nothing outside `src/`), but these are crate-public API removals, so keep-vs-drop is a library-contract call. If cut: `mod.rs:33-34` first (pure redundancy), then the names folded into sw-lg-01/03 if those methods are also cut.
- **Risk**: nil within the repo.
- **Verification**: the two `rg` commands above.

### sw-lg-10 — `export.rs` doc claims consumers that do not exist (Panel, `governance_metrics`)

- **Form**: 5 (name-drift residue: doc describes consumers that are not there)
- **Severity**: low
- **Produced**: `src/loop_graph/export.rs:8-10` ("JSON: machine-facing, for the Panel to render its own visualization … and for `governance_metrics` to serialize the topology without round-tripping through `list_nodes`/`list_edges`") and `:99-101` ("The Panel uses this to render its own graph view without re-reading SQLite, and `governance_metrics` uses it to log the topology at audit boundaries").
- **Consumers of `to_json`/`to_dot`**: exactly one production caller — the tool's `Export` action (`loop_graph_manage.rs:1160-1161`). Evidence:
  ```
  $ rg -n "loop_graph::to_json|loop_graph::to_dot" src/ bin/ interfaces/ shared/ desktop/
  → src/builtin_tools/loop_graph_manage.rs:1160 ("dot" => …to_dot…), :1161 ("json" => …to_json…)
  $ rg -n "to_json|to_dot" src/gateway/ interfaces/webchat/
  → no loop_graph/topology export wiring (gateway has no graph-export endpoint; webchat is a WASM crate that cannot call the core)
  $ rg -n "loop_graph" src/builtin_tools/governance_metrics.rs
  → only a prompt string at :135 ("for the loop topology use loop_graph(action=\"status\")") — no code consumer
  ```
- **Rationale**: unlike `events.rs` (whose module doc was corrected to admit the aspirational `governance_metrics` claim), `export.rs` still presents the Panel and `governance_metrics` as consumers. Neither exists; the module doc's own "What this is NOT — Not a renderer … the Panel's graph canvas lives in `interfaces/webchat/…/views/canvas/`" (`:27-29`) contradicts the claim, and the canvas has no endpoint to fetch DOT/JSON through.
- **Proposed change**: DECIDE (doc-only). Either correct `:8-10` and `:99-101` to say "consumed by the `loop_graph` tool's `export` action; a Panel visualization would need a gateway endpoint", or keep as roadmap prose. No behavior change.
- **Risk**: nil.
- **Verification**: the `rg` commands above.

## Summary

| Count | Severity |
|------|----------|
| 1 | medium |
| 9 | low |

| Count | Decision |
|------|----------|
| 2 | CUT (sw-lg-02, sw-lg-08) |
| 8 | DECIDE (sw-lg-01, 03-07, 09-10) |
| 0 | CONNECT |

## Deliberately skipped / negative results

- `#[cfg(test)]` items are out of scope for production parity: `mod::set_global_for_test` (`mod.rs:122`), `service::render_session_topology_strict` (`service.rs:577`, already honestly gated and documented).
- `watcher_is_pokeable` (`service.rs:171`) — has exactly one production consumer, in-module: `immediate_review_reaches` (`service.rs:724`), which IS called by the tool's `link` action (`loop_graph_manage.rs:647`). Wired, not severed (its doc's "one predicate, all readers" is mildly overstated — `pair`'s success line checks only `target_has_victory_claim`).
- `SnapshotStore::get_snapshot` (`snapshot.rs:249`) — zero *direct* production callers but consumed by `diff_snapshots` (`snapshot.rs:289,292`), which is reached by the tool's `snapshot diff` op (`loop_graph_manage.rs:1231`). Transitively wired; `Snapshot`/`SnapshotSummary`/`EventRecord` similarly flow through production return types (see sw-lg-09 for the re-export half only).
- `TopologyEvent` variants, `BUS_CAPACITY`, `LABEL_MAX_CHARS`, `MAX_TRAVERSAL_STEPS`, `MAX_HANDLE_CHARS`, `MAX_ROOT_BODY_CHARS`, `ROOT_BODY_TRUNCATED`, `WATCH_DEBOUNCE`, `DEFAULT_AGENT`, `cadence_rank`, `can_cover` (both), `GcReport` — all production-reachable; no findings.
- No `#[allow(dead_code)]` or `#[deprecated]` items remain in the module (previous audits cleaned them).
- No cross-crate consumers (`interfaces/`, `shared/`, `desktop/` reference no loop_graph symbol) — the pub surface is in-crate only, which bounds the blast radius of every CUT/DECIDE above.
- No cargo builds/tests were run (read-only constraint). Every path:line above was verified against the working tree at audit time.
