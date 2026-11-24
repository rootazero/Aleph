# NSH Memory Nebula Implementation Plan / NSH 记忆星云图实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect NSH's read-only Nebula to Core, fix Leptos graph addressing and stale state, and tune the shared NSH renderer from measured behavior. / 接通 NSH 只读星云图，修复旧面板寻址和过期状态，以实测优化 NSH 渲染。

**Architecture:** Core owns note addresses, authorization, and Panel capability publication. Both v1 transports invoke the existing handlers; the same TSX module renders native and Web from a shared Rust layout/index, while Leptos retains its WebGL surface. / Core 掌握地址、权限和能力发布；两个 v1 入口复用处理器，NSH 共用 TSX 和 Rust 布局，Leptos 保留 WebGL。

**Tech Stack:** Rust, Serde, SQLite, existing Panel SDK, NSH `=0.6.0` TSX/Canvas, Leptos/WASM.

**Spec:** `docs/superpowers/specs/2026-11-24-memory-nebula-nsh-design.md` (`0b837b322`, approved).

## Global Constraints / 全局约束

- Work only in `.worktrees/memory-nebula-nsh`, branch `refactor/memory-nebula-nsh`. One writer per checkout; review diffs independently. No merge, push, install, public service changes, or edits to `main`. / 仅在隔离检出工作，不合并、不推送、不安装。
- One graph partition, existing default 100 nodes and Web v1 hard frame limit 1 MiB; no third transport, NSH write authorization, storage-format change, default renderer cutover, JSON Canvas file exchange, or new drawing library. / 单分区图窗，不扩大传输帧上限，不改默认面板，不新增写授权。
- Scope/address checks run in Core regardless of publication or client state. Existing nonexistent/denied results remain indistinguishable. Do not turn metadata into permission. / 发布与权限独立；保持拒绝与不存在的等价结果。
- Preserve user delegation limits: parallel only for disjoint deliverables while parent advances, or for specialist, isolation, or independent review; no external model CLI. / 遵守既定子任务授权与模型预算。

## Review Focus / 回归重点

1. Same path in owned and org partitions: exact tuple controls focus, reads and writes (Tasks 1 and 4).
2. Missing graph registrations, or registered handlers backing an Off runtime: no phantom tab in the former; truthful RPC error in the latter (Tasks 2 and 3).
3. Old success **and** old rejection after reconnect, changed selection, or new search: neither may redraw nor replace current status (Tasks 3 and 4).
4. Reciprocal relationships, absent endpoints and selected edges under culling: valid facts survive projection and incident edge preference affects what is drawn (Task 5).
5. UTF-8 and escaped JSON crossing Web v1's limit, and pointer input unavailable on one host: visible error without retry/truncation, portable controls still usable (Tasks 3, 5 and 6).

## File Map / 文件职责

| Area | Primary paths / 主要路径 | Responsibility / 责任 |
|---|---|---|
| Wire and persistence | `shared/protocol/src/{lib.rs,memory_graph.rs}`, `src/gateway/handlers/{graph_types.rs,graph/}`, `src/memory/store/sqlite/notes/store_impl.rs` | Shared read DTOs, exact address, stable window |
| Publication and transports | `src/gateway/handlers/{mod.rs,panel_capabilities.rs}`, `src/gateway/lane.rs`, `src/gateway/server/connection/{mod.rs,local_ipc/tests.rs,web_v1/tests.rs}`, `interfaces/{panel/src/lib.rs,webchat/src/app.rs}` | Publish at existing registry boundary, mount only the three reads, recognize only published module |
| NSH | `interfaces/panel/modules/{MemoryNebula.tsx,memoryNebula.ts,memoryNebula.test.ts}`, `interfaces/panel/{shell/Shell.tsx,src/lib.rs}` | Shared view/lifetime, Rust projection binding, actual draw path |
| Legacy | `interfaces/webchat/src/{api/graph.rs,memory_graph/adapter.rs,platform/wide/views/memory/galaxy/}` | Rust DTO consumption, exact addressed navigation/editor, retain WebGL |
| Pure scene | `shared/ui_logic/src/{lib.rs,nebula/mod.rs,nebula/layout.rs}` | Shared graph index/layout without platform imports |
| Evidence and docs | `qa/nsh_s2_shell/`, `docs/reference/{FEATURE_LOCATOR,MEMORY_SYSTEM,DESKTOP_SHELL}.md`, `docs/superpowers/runs/` | Replay real hosts; update facts and archive measurements |

---

### Task 1: Canonical read contract and ordering / 共享读契约与排序

**Files:** Wire and persistence in File Map; tests in existing graph query/detail and note-store test modules.

**Interfaces:** Produce `aleph_protocol::memory_graph::{NoteNodeDto, NoteLinkDto, GraphQueryResponse, OutgoingLinkDto, NoteDetailResponse, SearchResultDto, GraphSearchResponse}`; keep existing wire spellings. Add `NoteNodeDto.agent_id: Option<String>` with default/skip-none for decoding and empty compatibility. Core mapper `entry_to_dto(entry: &NoteIndexEntry) -> NoteNodeDto` sets `Some(entry.agent_id.clone())`. `SearchResultDto.agent_id` remains required. Add `NoteAddress { agent_id: String, node_id: String }` (`Clone + Eq + Hash`) and `NoteNodeDto::address(&self) -> Option<NoteAddress>`; map the DTO's `id` to `node_id` without normalization.

- [ ] Add `graph_node_address_is_the_partition_actually_read`: seed two partitions with the same path and distinct title/body; assert `query.nodes[0].agent_id == Some(owned_partition)`; passing the returned tuple to detail returns only the owned body. Assert denied query JSON equals unused partition JSON and includes no address. Pin JSON missing `agent_id` → `address().is_none()` and a tied window returning paths in ascending order. Keep mutation request DTOs Core-local.
- [ ] Run `cargo test -p alephcore --lib gateway::handlers::graph` and `cargo test -p aleph-protocol --test memory_graph`; demonstrate red assertions or missing shared interface before wiring. Name the standalone note-store case `graph_window_ties_use_path_order` and run `cargo test -p alephcore --lib graph_window_ties_use_path_order`.
- [ ] Move the read DTO definitions unchanged except address/needed `Clone`/`Deserialize` derives, re-export in `graph_types.rs` for existing handler imports, and add a final unique `n.path ASC` tie-breaker after degree and time in the existing window SQL. Do not deduplicate reciprocal rows or change the partition resolver.
- [ ] Rerun all three test selectors and require the new cases to pass. Record and investigate failures against unchanged baseline before accepting regressions as preexisting.
- [ ] Commit the protocol, mapper and ordering slice after review.

### Task 2: Publish the mounted capability and wire v1 / 发布能力并接通 v1

**Files:** Publication and transports in File Map; also inspect existing Leptos snapshot consumers for built-in admission versus asset load.

**Interfaces:** Produce `aleph_protocol::memory_graph::{GRAPH_QUERY_METHOD, GRAPH_SEARCH_METHOD, GRAPH_NODE_DETAIL_METHOD, NEBULA_TAB_ID}` (`graph.query`, `graph.search`, `graph.node_detail`, `memory-nebula`); `panel_capabilities::with_memory_nebula(snapshot: PanelUiCapabilitySnapshot, mounted: bool) -> PanelUiCapabilitySnapshot` for both snapshot result shapes. `HandlerRegistry::memory_nebula_mounted(&self) -> bool` derives from presence of all three fixed handlers; no independent registration table. Use trusted `aleph:memory-nebula` (`BuiltIn`, existing API major, entry `shell/memory-nebula`), not an extension asset.

- [ ] Add `nebula_publication_tracks_mounted_reads` covering incomplete/full registration, preservation of unrelated declarations/revision, and both context-free and contextual snapshot shapes. Add native/Web transport assertions that the exact three reads reach their handler while `graph.update_note`, `graph.rename_note`, `graph.delete_note` remain `METHOD_NOT_FOUND`; pair with the existing Core partition tests (a stub's successful result does not prove authorization).
- [ ] Run `cargo test -p alephcore --lib gateway::handlers::panel_capabilities` and `cargo test -p alephcore --lib gateway::server::connection`; demonstrate failures for missing mounted methods/declaration.
- [ ] Compose the declaration at the registry snapshot boundary, using the same projection for legacy and context-aware requests. A complete registered read surface remains mounted across runtime `stop/start`; Core may return its existing unavailable error while stopped. Do not treat live data availability as mounting or silently alter same-revision declarations. Filter the reserved renderer key before Leptos builds its DOM registry; neither direct URLs nor another module's tab strip may import its TSX entry. NSH may recognize the renderer key but must not synthesize it in `SHELL_MODULES`. Preserve caller revalidation and add explicit Query lane overrides for `graph.query` and `graph.node_detail` so all three reads work without idempotency keys. Verify the lane through prepare, and don't widen v1 beyond these reads.
- [ ] Rerun the Core selectors and `cargo test --manifest-path interfaces/panel/Cargo.toml --lib` (where this host can build NSH). Verify rejection paths including malformed/disabled discovery and an unavailable MemoryRuntime through production handlers in Task 6.
- [ ] Commit the mounted read/publication slice after review.

### Task 3: Shared TSX read-only loop / NSH 只读闭环

**Files:** NSH in File Map, `interfaces/panel/App.test.tsx`, `interfaces/panel/nsh.json` only if existing app packaging requires module registration. Extend the existing SDK, never a new transport client.

**Interfaces:** Export `MemoryNebula(props: { sdk: PanelSdk })` (`PanelSdk` from `interfaces/panel/shell/sdk.ts`); consume existing `sdk.invoke`, `sdk.connection`, disposal and generation. State keys are full address tuples; params retain `{agent_id?, limit?}`, `{agent_id?, query, limit?}`, `{agent_id, node_id}`. Default graph limit is 100, omitted selector means Core default agent, and address-less nodes cannot open detail. Render list/search/detail before enhancing Canvas.

- [ ] Add `memory_nebula_discovery_and_request_lifetime` cases: no graph invocation without an enabled published tab; exact detail params after an off-window result; same id in another partition never selects the visible node; old query result, search rejection and detail success ignored after their respective scope changes; disconnected/runtime-unavailable/oversize errors shown, no automatic retry loop. Use `sdk.test.ts`'s controlled promise pattern; include keyboard-selectable list controls.
- [ ] Run `just panel-tsx-test` (its `nsh test`, `nsh check`, Node Web host test); confirm the new cases fail for missing module/guard. If host toolchain blocks a stage, record the exact block and run independent stages separately.
- [ ] Map only the authorized-to-render declaration to `MemoryNebula`; call the existing SDK with separate per-operation request generations and the current mount/connection lifetime. Invalidate detail/search immediately on new scope/target and on disposal. Show useful user-visible states; graph, search and detail failures stay local to their request class. Off-window search opens detail and explicitly reports that it is not in the current graph.
- [ ] Rerun `just panel-tsx-test`; package with `just panel-tsx-web` and `just panel-tsx-bundle` where supported. Exercise the production read loop in Task 6; do not mistake mocked connection responses for Gateway evidence.
- [ ] Commit the NSH read loop after review.

### Task 4: Legacy addresses and lifetime / 旧面板地址与请求生命周期

**Files:** Legacy in File Map including `galaxy/mod.rs` and `galaxy/node_detail_panel.rs`; audit `interfaces/webchat/src/platform/phone/memory/graph.rs` and all `memory_graph::adapter` importers before removing types.

**Interfaces:** Preserve `GraphApi` method wire parameters; feed them the selected `NoteAddress`. Use `NoteAddress` for detail cache/selection identity, while the current single-partition scene may keep path-indexed geometry. Consume shared read DTOs and use adapters only for genuinely UI-specific derived types.

- [ ] Add regression assertions: `(owned, wiki/n)` and `(org, wiki/n)` are distinct cache/selection keys; edit/update/rename/delete use the selected address; editor completion from an obsolete scope cannot reset a new selection; latest graph request wins even when the prior one fails later. Exercise the preexisting phone/wide consumers so moved DTO fields cannot break them silently.
- [ ] Run `cargo test -p aleph-panel --lib memory_graph` and `cargo test -p aleph-panel --lib galaxy`. Assert failure on the specific old state/address behavior before replacing it.
- [ ] Derive graph hit/detail/edit/cache addresses from the returned nodes/hits exclusively; on successful edit invalidate only current data associated with the addressed source (and new ID for rename). Introduce separate graph/search/detail generation guards; gate both success and rejection. An older server response without `agent_id` is renderable but must never trigger an address-based operation using the base selector.
- [ ] Rerun the selectors and `cargo check -p aleph-panel --target wasm32-unknown-unknown`; remove the superseded adapter DTOs and only imports with no consumers after an `rg` census.
- [ ] Commit the legacy safety slice after review.

### Task 5: Reuse layout, draw and pick from one projection / 复用布局与拾取投影

**Files:** Pure scene and NSH in File Map, plus Leptos `galaxy/gl/{layout3d,math,edge_lod,scene}.rs` and `galaxy/galaxy_build.rs` as consumers. The camera, GPU buffers and legacy UI stay where they are.

**Interfaces:** Move `ForceLayout::new(node_count: usize, edges: &[(u32, u32)], communities: &[Option<u32>]) -> ForceLayout`, `seed(&self, ids: &[String]) -> Vec<Vec3>`, `step(&mut self, pos: &mut [Vec3])`, `set_alpha(&mut self, alpha: f32)`, `energy(&self, pos: &[Vec3]) -> f32`, and `converged(&self) -> bool` with their unchanged algorithm/tests; preserve old imports via re-export only where live consumers need them. Provide `NebulaIndex::new(nodes: &[NoteNodeDto], edges: &[NoteLinkDto]) -> NebulaIndex` with ordered directed relation rows and a distinct canonical spring pair list. For the NSH Rust bridge expose `NebulaModel::scene(graph: JsonValue) -> JsonValue`, reusing Panel Rust binding conventions; output coordinates and edge indices rather than new authority facts. TSX owns viewport and hit tests over those projected positions.

- [ ] Lock test expectations: reciprocal real edges = two directed relations and one spring; missing endpoint = zero admitted edges for that row; virtual edge confidence remains none. Add a pinned Canvas check for rendered scene and selectable nodes; a selected incident edge survives competition with an unrelated edge under a constrained visible budget. A click at the projection of a visible node selects the same address after zoom; an inaccessible gesture does not block equivalent button/list operations.
- [ ] Run `cargo test -p shared-ui-logic --lib nebula`, `cargo test -p aleph-panel --lib galaxy`, and `just panel-tsx-test` with the new regression cases. Require failure on the duplicate-relation or selected-edge assertion before extraction.
- [ ] Extract only pure Rust primitives and index logic. Use deterministic layout, small redraw batches that do not block input, and a common projection for both drawing and picking. Use native/Web-supported zoom/reset and list selection as the baseline; keep pointer gestures only when confirmed on both. Give incident edges first draw slots, with stable tie-breaking; expose all valid adjacency through the details affordance independent of culling. Don't change legacy camera/world-fit equations.
- [ ] Rerun Task 5 test commands and measure on normal/dense fixtures before changing algorithms, scheduling or wire bounds. Record baseline/after frame timings; only tune measured hot paths, no speculative Rayon or Barnes–Hut.
- [ ] Commit the renderer slice after review.

### Task 6: Real transports, evidence, docs and final review / 真机、文档与提交

**Files:** Evidence and docs in File Map. Extend existing `qa/nsh_s2_shell/{run.py,web-panel.cjs}` only as needed. New receipt: `docs/superpowers/runs/2026-11-24-memory-nebula-nsh-verification.md`.

**Interfaces:** Reuse `just panel-tsx-bundle panel-tsx-web`, production Gateway and the isolated macOS/Desktop IPC + Chrome/Web v1 recipe in `qa/nsh_s2_shell/README.md`; use temporary `HOME`/`ALEPH_HOME`, random port, test-owned devices/fixtures. If native macOS equipment is unavailable, prove Web independently and report the native gate as outstanding; no Windows stand-in gets credit as macOS NSH.

- [ ] Add cases to the QA recipe using normal and dense 100-node windows, owned vs foreign fixtures sharing a path, and an oversized escaped/non-ASCII detail. Assert on both real v1 entries that the three reads reach Core, mutations are not mounted, scope preserves denial, and availability errors remain errors. Capture serialized UTF-8 JSON-RPC sizes; Web's over-limit error must not return a partial note or retrigger a loop.
- [ ] Execute the existing isolated macOS/Web recipe and supported host tests on the candidate. Record revisions, platform/host, command, exact outcomes, node and edge counts, timing sample size and frame percentiles, bytes, view evidence and skipped cases. If normal dense graph exceeds size, add a measured Core projection bound with explicit truncation metadata and tests of old wire/client behavior; don't raise the frame ceiling. Re-run only the affected surfaces after tuning plus one end-to-end regression pass.
- [ ] Review `docs/reference/FEATURE_LOCATOR.md` graph and scope entries, `docs/reference/MEMORY_SYSTEM.md` and `docs/reference/DESKTOP_SHELL.md`; separate implemented read scope from deferred native parity, fix stale graph path references and record what was removed. Keep evidence separate from reference truth, distinguish rendering demo/backends from product QA.
- [ ] Run `git diff --check`, targeted cargo/NSH commands, and verify the whole diff and clean scoped status. Commission independent whole-branch review with a different provider/model when possible; resolve correctness and authorization findings before completion. Commit evidence/docs in the isolated branch. Report every unavailable validation; do not call a failed/pending gate green.
