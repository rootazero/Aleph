# Panel Community Manifest Declaration and Discovery Contract Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` sequentially. Use `superpowers:subagent-driven-development` where a focused read-only review or isolated implementation helps. Every task starts with a watched RED test and ends with recorded GREEN evidence.

**Goal:** Add an explicit opt-in Panel UI declaration to plugin manifests and derive the canonical Panel capability snapshot from admitted extension state, while preserving the existing source → snapshot → `PanelProjection` → `TabRegistry` → `PanelLoader` path.

**Phase boundary:** This plan establishes the production discovery contract. It does not publish assets, add a browser route, mount a community module, implement installation/update/uninstall, or change the default release. If no existing server/protocol projection channel can carry the snapshot without adding a new authority, stop at the canonical protocol snapshot and record the transport boundary for a later plan.

**Architecture:** The manifest is an explicit declaration input. Core validates manifest shape and derives a neutral, serializable Panel declaration from admitted plugin state. The existing lifecycle projection remains the sole revision/retraction authority. The shared protocol owns one serializable declaration/snapshot/origin shape; webchat owns browser validation and source implementation. Trust, active status, owner generation, and permissions remain host-derived policy facts. Generic plugin fields never imply a Panel tab.

**Starting point:** `main` at `fc7d8a81b`, which includes the completed community integrity-gated single-file loader. The previous worktree branch was fast-forwarded into `main`; do not repeat its loader implementation.

**Primary references:**

- `docs/superpowers/prompts/2026-10-29-panel-community-manifest-discovery.md`
- `docs/superpowers/plans/2026-10-28-panel-community-mode-contract-plan.md`
- `docs/superpowers/plans/2026-10-28-panel-community-module-loading-plan.md`
- `docs/reports/panel-community-module-loading-validation.md`
- `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`

## Global constraints

- Create a fresh `panel-community-discovery` worktree from current `main`; do not edit shared `main` during implementation.
- Keep one authoritative descriptor/declaration/snapshot shape. If the existing serializable Panel declaration/snapshot/origin types move from webchat to `shared/protocol`, delete the local duplicates and keep only source/validation behavior in webchat.
- Do not infer Panel UI from `PluginManifest.entry`, plugin kind, generic permissions, `capabilities_v2`, or `wasm_capabilities`.
- The manifest cannot choose `PanelUiTrust`, owner generation, active status, filesystem access, transport capabilities, or runtime handles. Reuse existing extension admission and Panel projection policy.
- Community declarations require a relative entry, relative/host-approved public base, explicit `sha256-<base64>` integrity, supported API major, and the existing permission/trust checks. Remote URLs, traversal, missing version, and missing integrity fail closed.
- Use the existing extension lifecycle/projection chokepoint for revision and retraction. Do not add a second watcher, registry, revision counter, or publication author.
- No browser API, Fetch, Blob, WASM module import, PanelShell mount, production `app.rs` route, ZahirFacade runtime mount, asset server, install/update/uninstall, marketplace, signature service, background refresh, default release flip, real Gateway, or unrelated plugin lifecycle change.
- Before every cargo subprocess, run `python qa/browser_live/check_memory.py` from the worktree root and require exit 0. Run cargo from the owning package directory, serially.
- Chinese communication, English code comments, bilingual docs; never write machine-specific absolute paths into repository documents.

## Task 1: Confirm and canonicalize the cross-crate contract

**Files:**

- Inspect: `shared/protocol/src/panel.rs`
- Inspect: `interfaces/webchat/src/panel/source.rs`
- Inspect: root `Cargo.toml`, `interfaces/webchat/Cargo.toml`
- Modify only if confirmed by the dependency audit: `shared/protocol/src/panel.rs`, `interfaces/webchat/src/panel/source.rs`

**Purpose:** The current serializable `PanelUiCapabilityDeclaration`, `PanelUiCapabilitySnapshot`, and `PanelUiCapabilityOrigin` live in webchat, while Core already depends on `aleph-protocol`. Establish one wire shape before Core derives production declarations.

- [ ] **Step 1: Write the RED contract test**

Add protocol-level tests for the exact serialized shape of declaration, origin, and snapshot, including `asset_integrity`, `public_base`, `plugin_version`, `source_revision`, and snapshot revision. Add a source-level census or compile guard proving webchat does not retain a second definition.

- [ ] **Step 2: Run the focused protocol test and confirm RED**

Run from the worktree package directory after the memory gate:

```bash
cargo test -p aleph-protocol panel
```

Expected RED if the protocol does not yet own the declaration/snapshot types or the current webchat shape cannot be moved without adaptation.

- [ ] **Step 3: Promote the existing serializable types without changing their wire shape**

Move or re-export the three serializable types into `shared/protocol/src/panel.rs`. Keep `PanelUiCapabilitySource`, `BuiltInTerminalSource`, `FixturePanelUiCapabilitySource`, and browser-only validation in webchat. Do not add a second descriptor or a second integrity validator.

- [ ] **Step 4: Run protocol and webchat regression tests**

```bash
cargo test -p aleph-protocol panel
cargo test --lib --no-default-features --features panel-modular panel::projection
```

- [ ] **Step 5: Commit the contract seam**

```bash
git add shared/protocol/src/panel.rs interfaces/webchat/src/panel/source.rs interfaces/webchat/src/panel/projection.rs
 git commit -m "refactor(panel): canonicalize capability snapshot contract"
```

If the dependency audit proves the types already have a safe shared owner, keep the task documentation-only and record that no move was necessary.

## Task 2: Add an explicit manifest Panel UI section

**Files:**

- Modify: `src/extension/manifest/types.rs`
- Modify: `src/extension/manifest/adapter.rs` only where canonical manifest decoding requires it
- Test: existing manifest unit-test module and the matching extension tests

**Proposed schema:** Use one optional `panel` section in both supported JSON/TOML manifest forms. The exact field names must follow repository naming conventions, but the semantic fields are:

- `tab_id`, `label`, `icon`, `order`
- `entry` as a relative path
- `api_major`
- `permissions`
- `public_base` or an explicitly host-provided relative asset address, if the dependency audit shows `public_base` must not be manifest-owned
- required `asset_integrity` for community declarations

The section must not contain trust, owner generation, runtime handles, credentials, remote URLs, or arbitrary capability grants.

- [ ] **Step 1: Write RED parser tests**

Cover:

- absent `panel` remains backward compatible;
- valid JSON and TOML declarations round-trip with exact fields;
- unknown Panel fields fail closed or produce the repository’s existing structured diagnostic;
- missing `tab_id`, relative entry, supported API major, or integrity is rejected;
- traversal, absolute/remote entry, malformed integrity, and empty required strings are rejected;
- generic plugin `entry`, `kind`, `capabilities_v2`, and `wasm_capabilities` do not synthesize a Panel declaration;
- legacy manifests without the section remain unchanged.

- [ ] **Step 2: Run the focused manifest tests and confirm RED**

Run the affected `alephcore` manifest test target after the memory gate. Record the exact current failure instead of broadening the schema based on guesswork.

- [ ] **Step 3: Implement the minimal manifest type**

Add an optional, explicitly named `PanelUiManifest` with strict field handling. Reuse existing path and plugin manifest validation where it owns the boundary. Keep parser compatibility for existing manifests and do not change generic plugin entry semantics.

- [ ] **Step 4: Run focused manifest tests and the affected Core check**

Run the focused tests, `cargo check` for the owning package, and the relevant clippy target with the required memory gates.

- [ ] **Step 5: Commit the manifest contract**

```bash
git add src/extension/manifest/types.rs src/extension/manifest/adapter.rs
 git commit -m "feat(extension): declare opt-in panel UI manifests"
```

## Task 3: Derive one canonical Panel declaration from admitted plugin state

**Files:**

- Add or modify the smallest owning Core module under `src/extension/`
- Modify: `src/extension/projection.rs` only through its existing projection authority
- Test: Core extension manifest/projection tests

- [ ] **Step 1: Write RED adapter tests**

Prove that the pure adapter:

- namespaces the descriptor key with the admitted plugin id and declared tab id;
- derives plugin version, origin, active status, source revision, and trust from host inputs/policy rather than manifest fields;
- requires explicit integrity and supported API major;
- preserves deterministic ordering;
- rejects duplicate qualified keys, invalid paths, invalid public bases, missing versions, and policy-rejected trust;
- emits no declaration for disabled, blocked, errored, unloaded, or removed plugins;
- never reads generic plugin `entry` as a Panel entry;
- does not add credentials, filesystem permissions, or runtime handles to the output.

- [ ] **Step 2: Run the adapter tests and confirm RED**

Run only the affected Core test target after the memory gate. The RED must identify the missing adapter or missing manifest field, not an unrelated workspace failure.

- [ ] **Step 3: Implement a pure adapter**

Create the smallest named concept, such as `PanelUiManifest::to_capability_declaration(...)`, or place it beside the existing extension projection if that is the established owner. It may return a typed rejection/diagnostic. It must not call browser APIs or mutate the plugin lifecycle. Trust must be an explicit host policy input; never default a manifest to `Verified`.

- [ ] **Step 4: Run adapter and protocol round-trip tests**

Verify that the produced declaration serializes to the canonical protocol shape and can be consumed by the existing webchat projection without a second conversion authority.

- [ ] **Step 5: Commit the adapter**

```bash
git add src/extension shared/protocol/src/panel.rs
 git commit -m "feat(panel): derive declarations from admitted extensions"
```

## Task 4: Join the existing lifecycle projection without adding a second authority

**Files:**

- Inspect and, only if required: `src/extension/projection.rs`, `src/extension/lifecycle.rs`, `src/extension/mod.rs`
- Inspect existing server/protocol projection channels before adding any new one
- Test: existing projection/lifecycle tests plus new Panel snapshot tests

- [ ] **Step 1: Write RED transition tests**

Prove that one existing lifecycle transition recomputes the Panel snapshot and that the snapshot revision/retraction behavior matches the existing plugin projection authority. Cover load, disable, unload, replacement, parse failure, and readiness/error transitions.

- [ ] **Step 2: Confirm the current projection boundary is insufficient**

Read the actual publication path. If an existing protocol/server view already carries a suitable snapshot, thread the Panel snapshot through that path. If no such path exists, do not create a new HTTP/WS route in this phase; expose only the canonical pure snapshot and record the transport boundary as deferred.

- [ ] **Step 3: Implement the smallest integration**

Use the existing `Views::after_transition`/projection owner and the same monotonic revision semantics. There must be exactly one author of the derived Panel snapshot. Removal must replace the previous snapshot so old declarations and integrity metadata cannot remain visible.

- [ ] **Step 4: Run lifecycle/projection tests and source census**

Run the affected extension tests and the existing single-author projection census. Record whether a transport seam was found or explicitly deferred.

- [ ] **Step 5: Commit only if production projection changed**

```bash
git add src/extension
 git commit -m "feat(panel): project explicit extension declarations"
```

## Task 5: Consume the canonical snapshot in webchat without Core coupling

**Files:**

- Modify only if required: `interfaces/webchat/src/panel/source.rs`, `interfaces/webchat/src/panel/projection.rs`
- Test: webchat projection/registry tests and a local protocol round-trip fixture

- [ ] **Step 1: Write RED round-trip tests**

Feed a serialized declaration produced by the Core adapter into the existing webchat source/projection path. Prove that the same descriptor, trust, permissions, public base, integrity, and revision are validated; prove that invalid or policy-rejected declarations remain fail-closed.

- [ ] **Step 2: Implement the smallest decoder/source bridge**

Consume the shared protocol snapshot. Do not import `PluginManifest`, `ExtensionManager`, or Core into WASM. Reuse the current `PanelProjection`, `TabRegistry`, and `PanelLoader`.

- [ ] **Step 3: Run all Panel regressions**

Run the existing Rust projection/loader tests, Node loader tests, generated-glue test when the artifact is available, and the community artifact/protocol/host fixtures. Do not add a production route or fixture source to `app.rs`.

- [ ] **Step 4: Commit the webchat bridge**

```bash
git add interfaces/webchat/src/panel
 git commit -m "feat(panel): consume canonical discovery snapshots"
```

## Task 6: Documentation, report, and independent review

**Files:**

- Modify: `docs/reference/FEATURE_LOCATOR.md` §6.11
- Modify: `docs/reference/TERMINAL_RUNTIME.md` only where the new discovery boundary affects the documented runtime
- Add: `docs/reports/panel-community-manifest-discovery-validation.md`
- Update the ignored execution ledger under `.superpowers/sdd/` if the local tool workflow requires it

- [ ] Document the explicit manifest schema, the single canonical snapshot owner, the trust/integrity/permission ownership, revision/retraction semantics, and the fact that production asset publication/browser mount remain deferred.
- [ ] Run `git diff --check`, all affected checks, and a fresh independent read-only review focused on duplicate authorities, inferred UI, fail-open policy, stale snapshot retraction, and accidental production route/mount changes.
- [ ] Leave the new worktree tracked-clean, with no merge or push.

## Acceptance criteria

The phase is complete only when all of the following are true:

- A generic plugin with only `entry`, `kind`, or WASM capabilities never produces a Panel declaration.
- A valid explicit manifest declaration produces exactly one canonical, serializable Panel declaration with host-derived status/trust/revision and required integrity.
- Invalid or unadmitted declarations fail closed and never enter the browser loader.
- Disable, unload, replacement, and parse/error transitions retract or replace declarations through the existing single projection authority.
- Webchat consumes the shared snapshot without a Core dependency or duplicate declaration shape.
- Terminal and the completed integrity-gated loader fixtures remain green.
- No asset server, production route, browser mount, install/update/uninstall flow, default release change, or real Gateway evidence was added.
- The final report lists actual test counts, generated-glue/artifact availability, skipped live Gateway checks, and any deferred transport seam.
