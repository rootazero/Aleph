# Panel community snapshot transport bridge plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` sequentially. Use a focused read-only review when the transport seam or ownership boundary is uncertain. Every implementation task starts with a watched RED test and ends with recorded GREEN evidence.

**Goal:** Deliver the canonical Core `PanelUiCapabilitySnapshot` to webchat through the existing JSON-RPC/WebSocket control plane, preserving one snapshot authority and adding revision/connection-epoch fencing. This phase establishes production discovery transport only.

**Phase boundary:** The phase ends when a webchat discovery consumer can receive, validate, retain, refresh, and reject stale canonical snapshots. It does not publish community assets, serve `./tabs/` files, add a browser route, mount `PanelShell`, invoke `PanelLoader`, implement install/update/uninstall, change the default release, or claim live Gateway parity.

**Starting point:** `main` at `ba3c64c1d`, which contains the explicit manifest declaration, Core adapter, lifecycle projection, shared protocol contract, webchat fixture, and bilingual validation report. The completed integrity-gated loader must not be reimplemented.

**Architecture:** `ExtensionManager::panel_ui_capability_snapshot()` remains the only snapshot authority. A server-side transport adapter reads that snapshot and uses the existing JSON-RPC/WebSocket endpoint, router, authentication, and error envelope. Webchat owns only a connection-scoped discovery read model. The transport payload is the existing `PanelUiCapabilitySnapshot`; no second Panel DTO or revision counter is allowed. Browser `PanelProjection`, `PanelShell`, and `PanelLoader` remain outside the production community path until asset publication and mount receive separate approval.

## Global constraints

- Create a fresh `panel-community-transport` worktree from current `main`; do not edit shared `main` during implementation.
- Reuse the existing JSON-RPC/WebSocket control plane. A new HTTP/WS endpoint is prohibited. A new read-only JSON-RPC method is allowed only if the audit proves the existing endpoint/router is the correct transport owner and the method is strictly required.
- Do not add a second Panel registry, descriptor/declaration/snapshot shape, permission model, trust model, revision authority, or lifecycle watcher.
- The server transport must read the complete snapshot from Core; it must not reconstruct declarations from `PluginManifest`, `PluginRecord`, generic plugin fields, or webchat descriptors.
- Do not expose credentials, owner generation, runtime handles, filesystem permissions, raw plugin paths, or arbitrary URLs in transport payloads.
- `Ok(empty snapshot)`, transport error, decode error, disconnected state, stale revision, and stale connection epoch must remain distinguishable. An error must never become an empty capability list.
- Webchat must not import Core, Server, `PluginManifest`, or `ExtensionManager` into WASM.
- Do not pass the production transport snapshot into `PanelProjection::from_snapshot`, `PanelShell`, route selection, or `PanelLoader` in this phase. A local fixture may decode the canonical shape into a discovery-only state.
- Do not implement asset publication/server, public hosting, browser route/mount, install/update/uninstall, marketplace, signature service, background refresh, default release changes, ZahirFacade runtime mount, real Gateway integration, `src/harness/`, PTY business logic, or unrelated plugin lifecycle changes.
- Before every Cargo subprocess, run `python qa/browser_live/check_memory.py` from the worktree root and require exit 0. Run Cargo serially from the owning package directory.
- Chinese communication, English code comments, bilingual docs; never write machine-specific absolute paths into repository documents.

## Task 1: Audit the existing control plane and write the RED transport contract

**Inspect:**

- `shared/protocol` JSON-RPC request/response/event types and existing error envelopes;
- Gateway router/handler registration and authentication path;
- existing server-to-webchat snapshot/query/event consumers and reconnect epoch handling;
- `src/extension/mod.rs`, `src/extension/projection.rs`, and `src/extension/lifecycle.rs`;
- `interfaces/webchat/src/{api,context,state}` and current Panel source/projection boundaries.

**Step 1: Record the ownership decision before coding.**

Write a short design note in the plan execution ledger identifying:

- the existing WebSocket endpoint and router owner;
- the server adapter owner that can read `ExtensionManager::panel_ui_capability_snapshot()`;
- the webchat discovery-state owner;
- whether an existing notification/subscription seam can carry snapshot changes;
- why no new HTTP/WS endpoint or duplicate authority is needed.

If the audit finds no safe notification seam, choose query-on-connect/reconnect plus explicit refresh and record live push as deferred. Do not invent a watcher or poller in the harness.

**Step 2: Add the smallest RED tests.**

Add protocol/handler/client tests that describe the intended behavior:

- the response payload is exactly `PanelUiCapabilitySnapshot`;
- a snapshot with declarations and an empty snapshot both decode as successful values;
- malformed payload and transport errors remain errors;
- a stale snapshot revision cannot replace a newer accepted snapshot;
- an old connection epoch cannot write into the current discovery state;
- no test invokes `PanelProjection`, `PanelShell`, or `PanelLoader` for community discovery.

**Step 3: Run the focused tests and record the RED.**

Run only the owning package tests after the memory gate. If the test cannot compile because the selected transport seam is absent, record that exact failure and update the ownership decision before implementing.

## Task 2: Add the server read-only snapshot bridge

**Modify only the proven transport seam.** The handler may be a new JSON-RPC method on the existing endpoint, but it must remain a thin I/O adapter:

- read `ExtensionManager::panel_ui_capability_snapshot()` once for one response;
- serialize the shared snapshot without reconstructing or filtering declarations;
- use existing auth, request correlation, error envelope, and handler registration;
- return a structured transport error when the Core snapshot cannot be read or serialized;
- never expose an internal `RwLock`, registry, trust policy, plugin root, or Core type to the client.

If an existing snapshot query can carry the value without changing its contract, extend that projection only when it remains the established owner. Do not smuggle Panel data through an unrelated plugin list or generic capability response.

**Tests:**

- explicit community declaration and empty snapshot round-trip through the server handler;
- source revision, snapshot revision, integrity, public base, origin, trust, and status survive byte-for-byte semantic serialization;
- replacement and retraction return complete replacement snapshots;
- handler never derives a row from a generic plugin record;
- auth and malformed request failures use the existing fail-closed envelope.

Commit the server seam with a conventional message such as:

```text
feat(panel): expose canonical discovery snapshot
```

## Task 3: Add the webchat discovery-only consumer

**Files:** Use the existing webchat connection/state owner identified by Task 1. Keep `interfaces/webchat/src/panel/source.rs` and `interfaces/webchat/src/panel/projection.rs` focused on their existing source and browser validation responsibilities.

Add a connection-scoped discovery model containing only:

- the latest accepted `PanelUiCapabilitySnapshot`;
- connection epoch;
- last accepted snapshot revision;
- explicit loading/ready/error/disconnected state.

The consumer must:

- request the full snapshot after connection establishment and after reconnect;
- accept a snapshot only when its epoch is current and its revision is newer than the retained revision, or when initializing an empty state under the current epoch;
- discard stale revisions and old-epoch results without mutating the current state;
- preserve `Err` as `Err`, rather than converting it into an empty declaration list;
- keep community discovery state separate from the production browser `PanelProjection` until a later mount phase.

**Tests:**

- first successful non-empty snapshot;
- successful empty snapshot;
- replacement and retraction revisions;
- stale revision rejection;
- old-epoch response rejection after reconnect;
- malformed payload and transport error preservation;
- built-in Terminal projection remains unchanged;
- no route or loader call occurs when a community snapshot arrives.

Commit with a message such as:

```text
feat(panel): consume discovery snapshots over control plane
```

## Task 4: Decide change delivery and reconnect semantics

If an existing event/subscription channel is proven suitable, add a notification carrying the same complete `PanelUiCapabilitySnapshot`. The notification is advisory; reconnect and explicit refresh always use a complete snapshot query as the recovery source. A lagged or dropped notification must never create a partial registry.

If no suitable notification channel exists, do not add a new broadcaster, watcher, or polling loop. Document that production discovery refreshes on connect/reconnect and explicit refresh, and leave live push deferred to the next transport decision.

Tests must prove:

- a notification with revision `N+1` replaces `N` atomically;
- duplicate and stale notifications are harmless;
- a dropped notification is repaired by reconnect/full snapshot;
- malformed notification does not erase the last good snapshot;
- disconnect does not turn the last known snapshot into a false successful empty snapshot.

Commit only if this task adds a real existing-channel integration:

```text
feat(panel): fence discovery snapshot updates
```

## Task 5: Protocol, webchat, and regression verification

Run the affected checks after their required memory gates and serial package boundaries:

- shared protocol serialization/round-trip tests;
- server JSON-RPC handler/request tests;
- webchat discovery-state tests;
- existing webchat Panel projection/registry/loader tests;
- existing Node loader tests;
- generated glue tests only when the clean artifact is available;
- `cargo fmt --check`, affected `cargo check`, affected `cargo clippy`, `git diff --check`.

A local fake control-plane fixture may be used, but it must be labeled deterministic fixture evidence. Do not claim real Gateway/live parity without an authorized disposable target. If the local Panel build again stops at the missing `tailwindcss` script, record the exact command and boundary.

## Task 6: Documentation and independent review

Update the bilingual:

- `docs/reference/FEATURE_LOCATOR.md` §6.11;
- `docs/reference/TERMINAL_RUNTIME.md` only where the transport boundary changes the current text;
- add `docs/reports/panel-community-snapshot-transport-validation.md`.

The report must state exactly:

- which existing endpoint/router and handler own the transport;
- that Core remains the only snapshot/revision authority;
- response, error, reconnect, epoch, and revision semantics;
- whether live notifications were implemented or deferred;
- that no asset publication, browser mount, install lifecycle, default release, or live Gateway evidence was added;
- exact test counts and all skipped/blocked checks.

Run an independent read-only review focused on duplicate authorities, error-to-empty reversals, stale epoch/revision writes, Core-to-WASM dependency direction, and accidental route/mount/asset changes. Resolve every blocking finding before handoff.

## Acceptance criteria

The phase is complete only when:

- one existing JSON-RPC/WebSocket control plane carries the canonical shared snapshot;
- the server adapter reads Core state and performs no Panel derivation or policy decision;
- webchat has a discovery-only consumer with explicit ready/error/disconnected state;
- stale revisions and old connection epochs cannot overwrite current discovery state;
- empty success is distinguishable from transport/decode failure;
- reconnect/full snapshot is a recovery source, and notification lag cannot create partial state;
- no community descriptor reaches production route selection, `PanelShell`, `PanelLoader`, asset fetch, or module import;
- built-in Terminal and integrity-gated loader behavior remain green;
- the validation report records the exact transport seam and the remaining asset/browser/install boundaries;
- the implementation worktree is tracked-clean and no merge/push occurs during implementation.
