# Core Panel Byte Admission Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Preserve the user's sequential execution method.

**Goal:** Complete todo #7 with a lifecycle-owned exact-byte producer and scoped Core reads, unblocking later authenticated HTTP work.

**Architecture:** Relocate the existing publication store into Core; do not create a second store. One manager-owned committed state holds revision and records containing declaration, immutable bytes and internal scope. Lifecycle projection is the only writer; discovery and byte reads are derived from this state using the existing visibility predicate.

**Tech Stack:** Existing Rust/tokio, SHA-256/base64, shared Protocol, tempfile tests; no new runtime dependency.

**Spec:** [approved Core admission design](../specs/2026-10-31-panel-core-byte-admission-design.md), approved by the user on 2026-10-31.

## Global Constraints

- Core owns declaration, trust, bytes admission, scope and revision; no Core import of Gateway server and no Core dependency in WASM.
- Relocate/reuse `PanelAssetPublicationStore`; remove `authorized: bool` from data reads. This is not an HTTP authentication proof and does not complete todo #2.
- Use `PluginRecord.panel.entry` only; canonical root containment and regular-file check; single-file bounded reads; exact SHA-256 bytes/SRI.
- Limits: **4 MiB per entry**, **64 MiB admitted bytes per generation**, counted independently per record and admitted in canonical key order.
- Internal reads reuse `visibility::visible_to`; zero-argument snapshot uses `VisibilityCtx::default()`, never daemon CWD. No raw root, client project claim or credentials on the wire.
- Existing `after_transition` remains the sole projection trigger; whole-state replacement removes stale versions and omitted records. Retain existing trust load/admission-gate semantics.
- Before **every** Cargo invocation run `python qa/browser_live/check_memory.py` from the worktree root. Cargo commands run serially from the owning package (Core is the root package). Never bypass the gate.
- Keep all edits on `panel-community-live-publication`; no merge/push. English code comments, bilingual documents, no machine absolute paths in repository documents.

## Review Focus

- Root/scope replacement with an unchanged plugin/tab ID must revoke the previous project's bytes; Task 3 pins it with real files and scope contexts.
- Mutation of an admitted file must not change previously published bytes, and failed replacement must not fall back to old bytes; Task 2/3 pin both.
- Global-only discovery must not inherit daemon CWD, while byte reads agree with scoped discovery; Task 3 pins default-context and two-project isolation.
- Integrity failure must not mask a stale-revision assertion; Task 1 fixes the known test fixture to supply valid SRI-matching stale bytes.
- Trust-policy change during off-lock file work must not publish a stale policy derivation; Task 3 validates against current policy at commit with no I/O under policy/state locks.

---

## File structure / 文件职责

- `src/extension/panel_publication/mod.rs`: the relocated store, single committed revision, immutable publication result, scoped reads and private publication operations.
- `src/extension/panel_publication/admission.rs`: bounded one-file admission from Core registry records; declaration validation remains in `extension/panel.rs`.
- `src/extension/panel_publication/tests.rs`: real-file store/admission unit tests, no mock filesystem.
- `src/extension/{mod,projection}.rs`: shared state handle, Core facade, lifecycle candidate preparation and atomic commit; lifecycle trigger remains unchanged.
- `src/extension/panel_publication/lifecycle_tests.rs` or existing projection test module: manager/lifecycle acceptance tests, placed where private lifecycle APIs are accessible.
- `src/gateway/server/panel_asset_route.rs`: remove relocated definitions; delete unused module or retain a genuinely needed thin Core adapter, never a second authority.
- `docs/reference/{FEATURE_LOCATOR,TERMINAL_RUNTIME}.md`, publication audit/report and tracked run ledger: update proof boundaries after runtime verification.

### Task 1: Relocate the store and enforce current-generation scoped reads

**Files:** `src/extension/panel_publication/{mod,tests}.rs`, `src/extension/mod.rs` module declaration, `src/gateway/server/{mod,panel_asset_route}.rs`.

**Interfaces:**

- Consume `PanelUiCapabilityDeclaration`, `PanelUiCapabilitySnapshot`, existing integrity/entry validators, `ScopeKey`, `VisibilityCtx`, `visible_to`.
- Produce crate-internal `PanelAssetPublicationStore`, `PanelAssetIdentity`, `PublishedPanelAsset { declaration: PanelUiCapabilityDeclaration, bytes: Arc<[u8]> }`, and `PanelAssetPublicationError`.
- Read API: `snapshot(&self, context: &VisibilityCtx) -> PanelUiCapabilitySnapshot`; `asset(&self, identity: &PanelAssetIdentity, entry: &str, context: &VisibilityCtx) -> Result<PublishedPanelAsset, PanelAssetPublicationError>`.
- Mutation is private to the publication module; task 3 uses its lifecycle-only commit method. Preserve identity/SRI/entry validation and reject records outside the sole current revision. Do not export mutable access through the manager.

- [x] **Step 1:** Add tests showing default context cannot discover/read Project A's record, Project B cannot read A, and changing generation removes old version/revision/entry. Give stale test attempts bytes matching their declaration SRI. Name tests `scoped_snapshot_and_bytes_agree`, `replacement_withdraws_prior_generation`, and `stale_attempt_with_valid_integrity_is_refused` (equivalent precise names are acceptable).
- [x] **Step 2:** Run the narrow tests before implementation; record the missing Core API/incorrect behavior as RED, not an environmental Cargo failure. First inspect and obey any repository test guidance available for Rust.
- [x] **Step 3:** Move the actual store validation logic/tests from Gateway into Core, implement the interfaces, make record mutations private, remove boolean authorization and obsolete Gateway module exports. No HTTP route registration.
- [x] **Step 4:** Run the relocated unit tests to GREEN; keep genuine baseline validation tests, fix only invalid fixture expectations. Run formatting on changed Rust files only, with the memory gate before any Cargo-based formatter.
- [x] **Step 5:** Commit `refactor(panel): move publication ownership into Core` only after focused verification succeeds; record RED/GREEN evidence.

### Task 2: Admit bounded exact bytes from explicit lifecycle records

**Files:** `src/extension/panel_publication/{admission,tests,mod}.rs`.

**Interfaces:**

- Consume `PluginRecord`, `derive_panel_declaration` / `derive_panel_snapshot` and host `OwnerTrustPolicy`.
- Produce `PreparedPanelPublication` from `async fn prepare(records: Vec<PluginRecord>, policy: &OwnerTrustPolicy) -> PreparedPanelPublication` (associated method may be used).
- `PreparedPanelPublication` contains validated immutable candidate bytes and the original Core facts needed for final policy validation; it has no published revision and grants no external write capability.
- Final private-module operation: `commit(&mut self, prepared: PreparedPanelPublication, policy: &OwnerTrustPolicy) -> Vec<PanelProjectionDiagnostic>` allocates the next checked revision and replaces all records. A generation-wide allocation failure clears records/marks unavailable; `asset` refuses and the compatibility snapshot contains no declarations.

- [x] **Step 1:** Add real temporary-root tests: known single-file bytes return exactly the expected literal bytes, generic plugin entry cannot supply them, changed file after admission leaves the retained bytes unchanged; mismatched SRI, missing file, directory, escaping symlink/path and oversized entries are omitted. Exercise the exact 4 MiB boundary and deterministic 64 MiB budget with multiple valid records; conditional symlink creation must report an explicit platform skip if unavailable.
- [x] **Step 2:** Run minimum new tests to observe RED before adding filesystem admission.
- [x] **Step 3:** Implement bounded `tokio::fs` reads (limit + 1), canonical containment, regular-file validation, exact-byte SRI and deterministic budget admission. No locks across I/O, no raw paths in result/diagnostic codes, no remote fetch/dependency traversal or second manifest parser.
- [x] **Step 4:** Run admission/store tests to GREEN, including per-entry failure yielding a new empty generation instead of retaining old bytes and revision exhaustion refusing reads. Verify no new runtime dependency.
- [x] **Step 5:** Commit `feat(panel): admit bounded canonical module bytes` and record evidence.

### Task 3: Connect the real manager lifecycle and prove scope/withdrawal

**Files:** `src/extension/{mod,projection}.rs`, lifecycle acceptance tests, docs and ledger.

**Interfaces:**

- Replace `panel_snapshot` handles in `ExtensionManager` and `Views` with the single Core publication state handle.
- Preserve `pub fn panel_ui_capability_snapshot(&self) -> PanelUiCapabilitySnapshot`, implementing it via `VisibilityCtx::default()`.
- Add `pub(crate) fn panel_ui_capability_snapshot_for(&self, context: &VisibilityCtx) -> PanelUiCapabilitySnapshot` and `pub(crate) fn panel_ui_asset(&self, identity: &PanelAssetIdentity, entry: &str, context: &VisibilityCtx) -> Result<PublishedPanelAsset, PanelAssetPublicationError>`.
- `panel_ui_capability_revision()` reads that same state's committed revision.
- `PluginProjection` may return a derived snapshot for logging/tests but must not retain a second manager-owned authority. Clone registry facts, prepare files off-lock, then validate current trust and atomically commit under the consistent policy-read/state-write lock order.

- [x] **Step 1:** Write real-manager regression tests through existing lifecycle transitions: explicit Panel file admitted; generic entry different; disable/unload, failed reload and cross-version replacement withdraw bytes; Global/Project A/Project B reads match discovery; root/scope replacement under the same ID denies old context. Extend metadata-only projection fixtures with actual files and SRI, not an admission bypass. Add concurrent readers checking returned declaration/SRI/bytes alignment while replacement happens.
- [x] **Step 2:** Run the minimal lifecycle contract to RED against the existing manager implementation.
- [x] **Step 3:** Wire the existing lifecycle projection to prepare/commit; preserve the sole `after_transition` trigger and existing load-gate trust semantics. No contextful HTTP constructor, new client project parameter or route is added. Preserve zero-context RPC's shared wire shape while narrowing its records to Global.
- [x] **Step 4:** Run focused Core store/admission/lifecycle tests, existing declaration/manifest/projection/RPC tests, affected Terminal source/projection/registry/loader tests and JS loader tests. Run `cargo check -p alephcore --lib` and appropriate full Core test command, with serial Cargo and the memory gate each time. Record named baseline failures/skips/build blockers accurately; do not label partial runs green. Do not claim real Gateway/browser proof.
- [x] **Step 5:** Update approved spec status, publication audit/report, `FEATURE_LOCATOR` §6.11, `TERMINAL_RUNTIME` and tracked run ledger to distinguish proven Core admission from still-deferred route-owned authentication/browser delivery. Run one fresh read-only whole-branch review focused on ownership, path exposure, integrity, revision, scope, dependency direction and passive discovery; fix Important/Critical findings and rerun relevant tests.
- [ ] **Step 6:** Commit `feat(panel): publish scoped assets through lifecycle`; finish tracked-clean with local commits only. Mark todo #7 complete only after actual runtime acceptance evidence, leaving #2–#5 pending with their real dependencies.

## Execution note / 执行说明

The user already approved the design and requested completing #7 using sequential `superpowers:executing-plans`; preserve that authorization and proceed after plan self-review. No additional execution-method choice is required. Changes to the approved scope require discussion; routine file names/body details do not. / 用户已批准设计并要求顺序完成 #7，沿用授权，plan 自检后执行；不重复询问执行方式，重大 scope 变更另议。
