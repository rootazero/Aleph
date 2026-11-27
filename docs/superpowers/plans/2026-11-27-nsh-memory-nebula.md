# NSH Memory Nebula Implementation Plan / NSH 记忆星云图实施计划

> **For agentic workers:** Use superpowers:subagent-driven-development with isolated worktrees and integrate each verified deliverable in dependency order.

**Goal:** Deliver a shared NSH desktop and Web graph for exploring and managing memory notes. / 交付 NSH 桌面与 Web 共用的记忆图谱和笔记管理。

**Architecture:** Core owns graph scope and note mutations. Shared Rust derives stable scene geometry. The single TSX module owns rendering, input and request state. / Core 掌握数据范围和写入，Rust 派生稳定布局，TSX 统一管理交互和请求。

**Tech Stack:** Rust, SQLite, `aleph-protocol`, `shared-ui-logic`, NSH 0.6 Canvas and TSX, v1 IPC and Web.

**Spec:** [2026-11-27 NSH Memory Nebula](../specs/2026-11-27-nsh-memory-nebula-design.md). Results are recorded in the [macOS and Web replay](../runs/2026-11-27-nsh-memory-nebula-verification.md). / 实施结果见验证记录。

## Global Constraints / 共同约束

- Work from `refactor/nsh-memory-nebula`, use one writer per worktree and integrate reviewed commits into the feature branch. / 使用独立分支与工作树。
- 100 notes by default, 500 for the dense fixture. Use `(agent_id,node_id)` as identity. Preserve existing RPC response shape for older callers through additive fields. / 默认 100 个节点，密集验证 500 个，保持旧 RPC 兼容。
- No added layout or rendering dependencies. Core owns authorization and capability projection. / 复用现有渲染与 Core 能力。

## Review Focus / 审阅重点

- Same path across corpora and denied exact addresses → task 1 and task 4.
- Active, dangling and tombstoned relationships with a full neighbor budget → task 1.
- Zoom at a stationary pointer, drag without an accidental selection, offscreen crossing edges → task 2.
- Out-of-order detail and refresh across a context change → task 3.
- Renaming one of two same-title notes, stale revision, and an index failure after a disk change → task 4.

### Task 1 · Core read connectivity / Core 读链路

Files: `shared/protocol/src/memory_graph.rs`, `src/gateway/handlers/graph_types.rs`, `src/gateway/handlers/graph/{query,node_detail}.rs`, scoped existing store files as needed. Inputs: optional `focus: { agent_id, node_id }` on `graph.query`, default overview behavior; exact detail request unchanged. Outputs: optional `focus` on `GraphQueryResponse`, optional `target: NoteAddress` on active outgoing relations, optional `backlink_targets: Vec<NoteAddress>` on detail. Keep previous outgoing and backlink fields. Preserve scope and detail error semantics.

- [ ] Assert focus includes its root and only extant active one-hop neighbors in the exact corpus. Bound the window to 1–500 for scoped focus and order the result deterministically; a denied address follows the missing-note response.
- [ ] Assert only resolvable active relations have targets, and back references carry the actual corpus.
- [ ] Reuse SQLite scopes and the adjacency logic. Drive focus and detail through current runtime admission. Run `cargo test --locked -p alephcore --lib gateway::handlers::graph` and commit.

### Task 2 · NSH canvas and scene / NSH 场景与视口

Files: `interfaces/panel/modules/{NebulaCanvas.tsx,nebulaViewport.ts,nebulaViewport.test.mjs}`, `interfaces/panel/src/lib.rs` and `shared/ui_logic/src/nebula` as needed. Preserve `NebulaModel.scene(graph)` and `Scene.points` index alignment with `graph.nodes` and `Scene.edges`. `NebulaCanvas` keeps `scene`, `selected: () => number`, `onSelect(index)`, and accepts `labels?: () => readonly string[]` plus `focusRequest?: () => { index: number; revision: number } | null`. Canvas provides fit/zoom/density and uses the selected index for emphasis.

- [ ] Assert inverse transforms preserve the pointer anchor across zoom; line clipping keeps viewport crossings; drag sequences never pick on release.
- [ ] Implement NSH input, draw and hit-test against the same projected geometry. Reuse Rust force layout and indexed adjacency, with bounded settling and no layout on interaction. Replace the fixed-height and center-zoom implementation.
- [ ] Run Node viewport tests, `cargo test --locked -p shared-ui-logic nebula`, and `nsh check interfaces/panel`. Commit isolated changes.

### Task 3 · TSX browsing and recovery / TSX 浏览与恢复

Files: `interfaces/panel/modules/MemoryNebula.tsx` plus its tests. Consume task 1 read shapes and task 2 canvas props. Keep `MemoryNebula({ sdk })` and the existing capability mount. Use graph RPC data decoded by `NebulaModel.scene` once per successful graph window.

- [ ] Verify an out-of-window hit opens detail, a relation opens its exact target, and focus/back return to the selected view. Verify overlapping calls and loss of the SDK generation cannot replace current state.
- [ ] Add focus/back, recoverable refresh and detail states, readable relation labels, actionable exact relations, and the keyboard-accessible note list. Keep prior successful state within the same connection generation. Run NSH feature tests and commit.

### Task 4 · Core note management and Panel actions / Core 写入与面板管理

Files: `shared/protocol/src/memory_graph.rs`, `src/gateway/handlers/graph/{mutate,mod,node_detail}.rs`, graph registration, v1 admission/classification, `src/memory/notes/indexer.rs`, capability projection, and `interfaces/panel/modules/MemoryNebula.tsx`. Input: `graph.mutate_note` with `{ address: NoteAddress, revision: string, action: { type: 'update', content: string } | { type: 'rename', title: string } | { type: 'delete' }, idempotency_key: string }`. Output: `{ status: 'updated' | 'renamed' | 'deleted', address: NoteAddress }`, or structured conflict/mutation error. Detail adds optional `revision`. Availability is Core advertised on the existing Nebula contribution, gated on the mutation handler and recognized by both hosts. Keep legacy methods intact.

- [ ] Assert stale revision leaves existing bytes and index intact, rename uses the exact source and rejects an existing destination, denied callers have no side effects, and an index failure after a disk change is surfaced with a distinct error. Cover v1 identity and idempotency routing.
- [ ] Reuse `lock_note_corpus` and existing validated note/index primitives, moving guarded bodies only where needed to avoid re-acquisition. Guard updates with exact content revisions and return the resulting address. Reconcile partial indexes with existing rebuild or incremental repair paths. Preserve explicit relationships and tombstones.
- [ ] Wire edit, rename and confirmed delete in TSX. Retain drafts on failure and use the receipt to refresh. Verify UI and Core through their normal entry points. Commit.

### Task 5 · Integration and documentation / 集成与文档

- [ ] Replay `qa/nsh_s2_shell` for native macOS and Web with normal, dense, and unavailable cases. Record first interactive frame, p95 frame time and peak memory where instrumentation supports them. Compare with the same fixture and disclose coverage.
- [ ] Run `git diff --check`, `just panel-tsx-test`, focused Core and protocol suites, and applicable shell QA. Review dead code against real callers and review the full diff independently.
- [ ] Align `docs/reference/FEATURE_LOCATOR.md` §6.3, `docs/reference/DESKTOP_SHELL.md` and `docs/reference/MEMORY_SYSTEM.md` with the verified contract. Commit the approved design and this plan on the feature branch and leave the branch ready for review.
