# Severed-Wire Audit — `src/canvas/`

**Date:** 2026-08-17
**Module:** `src/canvas/{assets.rs, doc_io.rs, mod.rs, selection.rs, store.rs, validate.rs}`
**Method:** PRODUCED − CONSUMED symbol parity via `rg` across `src/`, `interfaces/`, `shared/` (skipping `bin/` which is not present in this worktree). Read-before-write triage: every "no consumer" claim is back-stopped by an `rg` invocation.

## Inventory — produced surface

### `mod.rs` (re-exports)
- `pub use store::{CanvasError, CanvasListing, CanvasStore};`

### `store.rs` — `pub` API on `CanvasStore` / `CanvasError` / `CanvasListing`
| Symbol | Location |
|---|---|
| `pub enum CanvasError` | store.rs:36 |
| `pub struct CanvasListing` | store.rs:63 |
| `pub struct CanvasStore` | store.rs:72 |
| `pub fn new(root: PathBuf)` | store.rs:83 |
| `pub fn with_event_bus(self, bus)` | store.rs:95 |
| `pub async fn create(...)` | store.rs:103 |
| `pub async fn get(&self, id)` | store.rs:152 |
| `pub async fn list(&self) -> Vec<CanvasRow>` | store.rs:164 |
| `pub async fn list_entries(&self) -> Vec<CanvasListing>` | store.rs:174 |
| `pub async fn apply(...)` | store.rs:244 |
| `pub async fn delete(&self, id)` | store.rs:294 |

### `assets.rs` — `pub` API on `CanvasStore` (asset surface)
| Symbol | Location |
|---|---|
| `pub async fn put_asset(&self, id, mime, bytes)` | assets.rs:94 |
| `pub async fn read_asset(&self, id, asset_id)` | assets.rs:196 |
| `pub async fn sweep_orphan_assets(&self, id)` | assets.rs:234 |

### `selection.rs` — process-global selection table
| Symbol | Location |
|---|---|
| `pub fn set(canvas_id, shape_ids)` | selection.rs:97 |
| `pub fn get(canvas_id) -> Vec<String>` | selection.rs:113 |

### `doc_io.rs` / `validate.rs`
All `pub(super)` items are consumed only within `src/canvas/`. No external (crate-root) callers. Excluded from external-parity audit.

## Inventory — production consumers

```bash
$ rg -n "crate::canvas::CanvasStore" src/ interfaces/ shared/
src/executor/builtin_registry/config.rs:39
src/executor/builtin_registry/builder/tests.rs:298
src/builtin_tools/canvas.rs:6
src/gateway/server/mod.rs:456
src/gateway/server/mod.rs:705
src/gateway/server/canvas_asset_route.rs:63
src/gateway/handlers/canvas.rs:54
src/gateway/handlers/canvas_error.rs:24
src/builtin_tools/canvas.rs:51
src/bin/aleph-server/commands/start/mod.rs:1265
src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:169
src/bin/aleph-server/commands/start/builder/handlers/canvas.rs:5
src/bin/aleph-server/commands/start/builder/handlers/canvas.rs:19

$ rg -n "selection::(set|get)" src/ interfaces/ shared/
src/builtin_tools/canvas.rs:831        (selection::get)
src/gateway/handlers/canvas.rs:146     (selection::get)
src/gateway/handlers/canvas.rs:196     (selection::get)
src/gateway/handlers/canvas.rs:337     (selection::set)

$ rg -n "store\.put_asset|store\.read_asset" src/ interfaces/ shared/
src/gateway/server/canvas_asset_route.rs:263   (read_asset)
src/gateway/server/canvas_asset_route.rs:403   (put_asset)
src/gateway/handlers/canvas.rs:290            (put_asset)
src/gateway/handlers/canvas.rs:308            (read_asset)
src/builtin_tools/canvas.rs:297                (put_asset)
src/builtin_tools/canvas.rs:349                (put_asset)
```

| Public symbol | Production caller(s) | Test-only caller(s) |
|---|---|---|
| `CanvasStore::new` | `bin/.../start/mod.rs:1265` | 25+ test sites |
| `CanvasStore::with_event_bus` | `bin/.../start/mod.rs:1265` | `store.rs:856` |
| `CanvasStore::create` | `gateway/handlers/canvas.rs:144`, `builtin_tools/canvas.rs:818` | tests |
| `CanvasStore::get` | `gateway/handlers/canvas.rs:100`, `builtin_tools/canvas.rs:177` | tests |
| `CanvasStore::list` | **NONE** | `store.rs:719, 1027` |
| `CanvasStore::list_entries` | `gateway/handlers/canvas.rs:171`, `builtin_tools/canvas.rs:785` | tests |
| `CanvasStore::apply` | `gateway/handlers/canvas.rs:232`, `builtin_tools/canvas.rs:214, 219` | tests |
| `CanvasStore::delete` | `gateway/handlers/canvas.rs:260` | tests |
| `CanvasStore::put_asset` | `gateway/handlers/canvas.rs:290`, `gateway/server/canvas_asset_route.rs:403`, `builtin_tools/canvas.rs:297, 349` | tests |
| `CanvasStore::read_asset` | `gateway/handlers/canvas.rs:308`, `gateway/server/canvas_asset_route.rs:263` | tests |
| `CanvasStore::sweep_orphan_assets` | **NONE** | `assets.rs:522, 554, 572, 626, 639` |
| `selection::set` | `gateway/handlers/canvas.rs:337` | tests |
| `selection::get` | `builtin_tools/canvas.rs:831`, `gateway/handlers/canvas.rs:146, 196` | tests |
| `CanvasError` (enum + variants) | `gateway/handlers/canvas_error.rs:24, 40-46, 65-73`, `gateway/handlers/canvas.rs:54-285`, `gateway/server/canvas_asset_route.rs:63, 265`, `builtin_tools/canvas.rs:51, 189-227, 495`, `shared/protocol/src/jsonrpc.rs:54` | tests |
| `CanvasListing` (struct) | none-by-name; fields read via iterator after `list_entries()` | tests |
| `CanvasStore` (type) | many, see above | tests |

## Findings

### sw-canvas-1 — `CanvasStore::sweep_orphan_assets` has no production consumer

- **Module:** `src/canvas`
- **Files:** `src/canvas/assets.rs:234`
- **Severity:** low
- **Form:** 1 (no consumer)
- **Produced:** `pub async fn CanvasStore::sweep_orphan_assets(&self, id: &str) -> Result<usize, CanvasError>` — public method that takes a canvas write lock, computes the referenced asset set, and removes assets older than `ORPHAN_GRACE` that are not in the referenced set.
- **Produced location:** `src/canvas/assets.rs:234`
- **Consumer location:** none in production
- **Evidence:**
  ```bash
  $ rg -n "sweep_orphan_assets" src/ interfaces/ shared/
  src/canvas/assets.rs:14:    (doc comment)
  src/canvas/assets.rs:234:   (definition)
  src/canvas/assets.rs:252:   (doc comment)
  src/canvas/assets.rs:522:   (test)
  src/canvas/assets.rs:554:   (test)
  src/canvas/assets.rs:572:   (test)
  src/canvas/assets.rs:626:   (test)
  src/canvas/assets.rs:639:   (test)
  ```
  All non-definition matches are either doc comments or tests inside `#[cfg(test)]`. No caller anywhere in `src/`, `interfaces/`, `shared/`, or `bin/`.
- **Decision:** DECIDE
- **Rationale:** The orphan-reaping logic is exercised in production via the in-apply sweep (`src/canvas/store.rs:284` calls `self.sweep_assets_with(id, &referenced)` while still inside the per-canvas critical section). The standalone `sweep_orphan_assets` is therefore functionally redundant for the production workload — every `apply` already sweeps in passing against the just-committed reference set. The method is documented as a public surface (`pub`, full module-doc paragraph, five test cases) which makes it look intentional rather than dropped scaffolding. It is most likely a future hook for a periodic cron / CLI sweep that was deferred. Two clean options:
  - CUT: delete the public method and its 5 tests; the in-apply sweep covers the only live code path.
  - KEEP + CONNECT: leave the surface but wire it to a one-shot CLI subcommand or a recurring task. The tests already document the contract.
- **Proposed change (if CUT):** drop `sweep_orphan_assets` (`src/canvas/assets.rs:234-247`), drop the 5 tests that only call it (lines ~517, 552, 569, 622, 638 in `sweep_spares_young_orphans…`, `sweep_never_touches…`, `a_dedup_re_put_renews…`, `apply_sweeps_old_orphans…`, `a_stray_file…`, `sweep_on_a_missing_canvas…`), and update the module-doc references at lines 14 and 252.
- **Proposed change (if CONNECT):** register a `canvas.sweep_assets` CLI subcommand or wire `sweep_orphan_assets` into `tasks/cron` so the explicit entry point has a real wire, and update the in-apply sweep to be a fallback rather than the primary path.
- **Risk:** low either way. The five tests pin the orphan-grace semantics; if the team later adds a real consumer, re-introducing the method from the doc/tests is mechanical. CUT is the painless-severed-wire path.
- **Verification:**
  - `rg -n "sweep_orphan_assets"` after CUT should return 0 matches outside the deleted function's signature line (and then 0 after that line is gone).
  - `rg -n "sweep_assets_with"` after CUT should still return `store.rs:284` (the in-apply sweep survives).
  - `cargo test -p alephcore --lib canvas::` should pass; no other test in the workspace references the method.
- **Existing review ref:** none.

### sw-canvas-2 — `CanvasStore::list` has no production consumer

- **Module:** `src/canvas`
- **Files:** `src/canvas/store.rs:164`
- **Severity:** low
- **Form:** 1 (no consumer)
- **Produced:** `pub async fn CanvasStore::list(&self) -> Vec<CanvasRow>` — a thin wrapper that drops the `owner_user_id` from `list_entries()` and returns only the wire rows.
- **Produced location:** `src/canvas/store.rs:164`
- **Consumer location:** none in production
- **Evidence:**
  ```bash
  $ rg -n "store\.list\(\)|CanvasStore::list\b" src/ interfaces/ shared/
  src/canvas/store.rs:719:    let rows = store.list().await;            (test)
  src/canvas/store.rs:1027:   assert!(store.list().await.is_empty());  (test)
  ```
  The other `store.list()` matches in the workspace (`projects/store.rs`, `teams/sessions/store.rs`, `skill/usage.rs`, `memory/dreaming/...`, etc.) are on different `Store` types, not `CanvasStore`.
  ```bash
  $ rg -n "CanvasStore::list\b" src/ interfaces/ shared/
  src/canvas/store.rs:164:    (definition)
  src/canvas/store.rs:1027:   (test, refers to .list())
  ```
- **Decision:** DECIDE
- **Rationale:** The doc comment on `list()` says *"callers that must visibility-filter (every RPC/tool face) use the listing form instead — this one has already dropped the attribution the predicate needs."* In other words, the method exists for callers that legitimately do NOT need owner attribution. Today there is no such caller — both `gateway/handlers/canvas.rs:171` (`canvas.list` RPC) and `builtin_tools/canvas.rs:785` (`canvas` tool `List` action) use `list_entries()` because they need to filter on `owner_user_id`. The method is therefore dormant but correctly designed for a future "admin sees all" caller (an operator tool, a migration script, etc.).
- **Proposed change (if CUT):** remove `CanvasStore::list` (`src/canvas/store.rs:159-167`) and the 2 test sites that call it (`store.rs:719` in `a_corrupt_doc_json_is_skipped_loudly_by_list_but_errors_on_get` and `store.rs:1027` in `delete_removes_the_directory_and_every_surface_agrees`). The `list_entries()` form is already what every production caller uses.
- **Proposed change (if KEEP):** leave as-is; no future work implied, just dormant.
- **Risk:** low. CUT removes a public method that has zero callers, and its only two test sites have direct `list_entries()` equivalents (the corrupted-doc test, the empty-after-delete test). The doc comment explaining the design rationale can move into `list_entries` if useful.
- **Verification:**
  - `rg -n "CanvasStore::list\b|\.list\(\)" src/canvas/store.rs` after CUT should only match the `.list()` calls inside tests that have been replaced with `.list_entries().into_iter().map(|e| e.row).collect()`.
  - `rg -n "CanvasStore::list\b"` after CUT should return 0 matches.
  - `cargo test -p alephcore --lib canvas::` should pass; no external test imports `CanvasStore::list`.
- **Existing review ref:** none.

### sw-canvas-3 — `CanvasListing` re-exported but never name-imported externally

- **Module:** `src/canvas`
- **Files:** `src/canvas/mod.rs:18`, `src/canvas/store.rs:63, 174, 207`
- **Severity:** low
- **Form:** 6 (orphaned public API surface / borderline)
- **Produced:** `pub struct CanvasListing { pub owner_user_id: Option<String>, pub row: CanvasRow }` and the `pub use store::CanvasListing` re-export.
- **Produced location:** `src/canvas/mod.rs:18` (re-export), `src/canvas/store.rs:63` (definition)
- **Consumer location:** none by name
- **Evidence:**
  ```bash
  $ rg -n "CanvasListing" src/ interfaces/ shared/
  src/canvas/mod.rs:18:        pub use store::{CanvasError, CanvasListing, CanvasStore};
  src/canvas/store.rs:63:      pub struct CanvasListing {
  src/canvas/store.rs:174:     pub async fn list_entries(&self) -> Vec<CanvasListing> {
  src/canvas/store.rs:207:         rows.push(CanvasListing {
  ```
  No `use crate::canvas::CanvasListing` or qualified `canvas::CanvasListing` outside the canvas module itself.
  ```bash
  $ rg -n "use crate::canvas::CanvasListing|canvas::CanvasListing" src/ interfaces/ shared/
  (no matches)
  ```
  Despite the missing name import, the struct IS consumed — `list_entries()` returns `Vec<CanvasListing>`, and the production callers (`gateway/handlers/canvas.rs:171` and `builtin_tools/canvas.rs:785`) read the fields through `entry.owner_user_id.as_deref()` and `entry.row.project_id.as_deref()` after `.into_iter()`.
- **Decision:** KEEP (no action)
- **Rationale:** `CanvasListing` is in the return type of the public method `list_entries`, so the struct must be `pub` — and `mod store` is private (`mod store` rather than `pub mod store`), so the struct must be re-exported through `mod.rs`. The struct is correctly exposed; the absence of a `use ... CanvasListing` import is purely cosmetic. This is not a severed wire — the struct has live readers via struct-field access through the iterator returned by `list_entries()`. It is mentioned here for completeness because the audit lens flagged it.
- **Proposed change:** none.
- **Risk:** none.
- **Verification:** n/a.

### sw-canvas-4 — `PRUNE_AT` constant duplicated across 3 files (name-drift risk, form 5)

- **Module:** `src/canvas/doc_io.rs` (and 2 outside this audit's scope)
- **Files:** `src/canvas/doc_io.rs:33`, `src/gateway/btw/seed/gate.rs:50`, `src/gateway/session_store/file_backend/meta.rs:62`
- **Severity:** low
- **Form:** 5 (name-drift residue / shape duplication)
- **Produced:** `const PRUNE_AT: usize = 128;` in three places.
- **Produced location:** `src/canvas/doc_io.rs:33`
- **Consumer location:** each file's own `slot()` (live) and the per-file test (`doc_io.rs:213`, `btw/seed/gate.rs:142`, `session_store/file_backend/meta.rs:330`).
- **Evidence:**
  ```bash
  $ rg -n "PRUNE_AT" src/ interfaces/ shared/
  src/canvas/doc_io.rs:33
  src/canvas/doc_io.rs:62
  src/canvas/doc_io.rs:213
  src/canvas/doc_io.rs:217
  src/gateway/btw/seed/gate.rs:50
  src/gateway/btw/seed/gate.rs:69
  src/gateway/btw/seed/gate.rs:142
  src/gateway/btw/seed/gate.rs:146
  src/gateway/session_store/file_backend/meta.rs:62
  src/gateway/session_store/file_backend/meta.rs:85
  src/gateway/session_store/file_backend/meta.rs:330
  src/gateway/session_store/file_backend/meta.rs:334
  ```
  The three locks tables are modeled on each other (the doc_io module doc explicitly cites `gateway/session_store/file_backend/meta.rs` as the "twin" — see `src/canvas/doc_io.rs:1-19`). The constant `128` is repeated literally in all three places with no shared definition.
- **Decision:** DECIDE
- **Rationale:** The three locks tables are sibling implementations of the same pattern. The `PRUNE_AT = 128` constant is the eviction bound; if one changes to `256` to support a longer-lived workload, the others silently drift. Tests at each site pin the local constant, so an accidental drift will not break tests, just behavior. This is the textbook "name-drift residue" form 5 — a constant describing a property of the lock table that has been hand-copied rather than shared. A small refactor (one shared `const` in `sync_primitives` or a sibling module) would centralize the bound, but the duplication is contained and the team may have intentionally kept the constants local so each locks table can tune independently.
- **Proposed change:** if the team agrees the three lock tables should agree, hoist `PRUNE_AT` to `crate::sync_primitives` (or `canvas::doc_io::PRUNE_AT` as the canonical home, with the other two re-exporting from it). Out of scope for this report as a "cut" but worth flagging.
- **Risk:** low. A centralization is a trivial const-hygiene change; leaving as-is means future divergence.
- **Existing review ref:** none.

## Symbols that PASS the parity check

The remaining public surface is healthy:

- **`CanvasStore` struct** — used by `gateway/server`, `gateway/handlers`, `builtin_tools`, `executor/builtin_registry`, `bin/aleph-server` boot wiring. Multiple live call sites.
- **`CanvasError` + variants** — used by `gateway/handlers/canvas_error.rs` (the canonical error→JSONRPC-code mapper), `builtin_tools/canvas.rs`, `gateway/handlers/canvas.rs`, `gateway/server/canvas_asset_route.rs`, and the shared protocol doc.
- **`CanvasStore::new`** — used by `bin/aleph-server/commands/start/mod.rs:1265` (production boot path).
- **`CanvasStore::with_event_bus`** — used by the same production boot path.
- **`CanvasStore::create / get / list_entries / apply / delete`** — all have at least one production caller (RPC handlers in `gateway/handlers/canvas.rs` and/or the `canvas` builtin tool in `builtin_tools/canvas.rs`).
- **`CanvasStore::put_asset`** — three production paths: `gateway/handlers/canvas.rs:290` (`canvas.asset.put` RPC), `gateway/server/canvas_asset_route.rs:403` (the asset HTTP route's test fixture creates assets), `builtin_tools/canvas.rs:297, 349` (the `canvas` builtin tool). Note `canvas_asset_route.rs:403` is inside the `#[cfg(test)] mod tests` of the route file — production path is `gateway/handlers/canvas.rs:290` and `builtin_tools/canvas.rs:297, 349`.
- **`CanvasStore::read_asset`** — `gateway/handlers/canvas.rs:308` (`canvas.asset.get` RPC) and `gateway/server/canvas_asset_route.rs:263` (the `/canvas-asset/...` byte route).
- **`selection::set`** — `gateway/handlers/canvas.rs:337` (`canvas.selection.set` RPC).
- **`selection::get`** — read back inside `canvas.get` (`gateway/handlers/canvas.rs:146, 196`) and inside the tool's `get` action (`builtin_tools/canvas.rs:831`).
- **All `pub(super)` items** in `doc_io.rs` / `validate.rs` are consumed only within `src/canvas/` and are correctly internal.
- **All `pub mod selection` / `selection.rs` private items** (`Entry`, `SelectionTable`, `MAX_LIVE`, `table()`) — not part of the public surface, by design.

## RPC dispatch parity check (bonus lens)

All canvas RPC methods are properly registered in `gateway/method_visibility.rs` and `method_census.rs` and bound to handlers by `register_canvas_handlers` in `bin/aleph-server/commands/start/builder/handlers/canvas.rs`:

| RPC method | Visibility treatment | Census class | Handler binding |
|---|---|---|---|
| `canvas.create` | None (creation, no addressed record) | `Class::Open` | `handlers::canvas::handle_create` |
| `canvas.list` | `Treatment::ListFiltered` | n/a (filtered) | `handlers::canvas::handle_list` |
| `canvas.get` | `Treatment::KeyChecked` | `Class::Open` | `handlers::canvas::handle_get` |
| `canvas.apply` | `Treatment::KeyChecked` | `Class::Open` | `handlers::canvas::handle_apply` |
| `canvas.delete` | `Treatment::KeyChecked` | `Class::Open` | `handlers::canvas::handle_delete` |
| `canvas.asset.put` | `Treatment::KeyChecked` | `Class::Open` | `handlers::canvas::handle_asset_put` |
| `canvas.asset.get` | `Treatment::KeyChecked` | `Class::Open` | `handlers::canvas::handle_asset_get` |
| `canvas.selection.set` | `Treatment::KeyChecked` | n/a (not in census) | `handlers::canvas::handle_selection_set` |

No registration drift. No client ghost. No name drift between registration and handler. Clean.

## Negative findings (what I did NOT find)

- No `#[allow(dead_code)]` items masking severed functions in this module.
- No `todo!()` / `unimplemented!()` stubs in `src/canvas/`.
- No handler that returns `Ok(success)` without doing anything (form 2).
- No classifier-vs-handler name-drift (form 5) inside `src/canvas/`.
- No `#[cfg(feature = "X")]`-gated code where `X` is not a declared feature (form 6).
- No `pub` items in `src/canvas/` whose `Display` impl, `From` impl, or trait impl is itself a severed wire.

## Recommended actions (priority order)

1. **sw-canvas-1** — Decide CUT vs CONNECT for `sweep_orphan_assets`. CUT is painless; CONNECT requires a real cron/CLI hook. Severity low, so default to **DECIDE** unless the team wants to invest the wire-up.
2. **sw-canvas-2** — Decide CUT vs KEEP for `CanvasStore::list`. CUT is painless; KEEP is harmless dormant surface. Severity low, default to **DECIDE**.
3. **sw-canvas-3** — No action. Re-export of `CanvasListing` is required by the public `list_entries()` signature.
4. **sw-canvas-4** — Optional: hoist `PRUNE_AT` to a shared `sync_primitives` constant. Out of scope for this audit's cut list; flagging only.

## Sanity-check of paths/lines (for the fixer)

| File | Line | Symbol |
|---|---|---|
| src/canvas/mod.rs | 18 | `pub use store::{CanvasError, CanvasListing, CanvasStore};` |
| src/canvas/store.rs | 36 | `pub enum CanvasError` |
| src/canvas/store.rs | 63 | `pub struct CanvasListing` |
| src/canvas/store.rs | 72 | `pub struct CanvasStore` |
| src/canvas/store.rs | 83 | `pub fn new` |
| src/canvas/store.rs | 95 | `pub fn with_event_bus` |
| src/canvas/store.rs | 103 | `pub async fn create` |
| src/canvas/store.rs | 152 | `pub async fn get` |
| src/canvas/store.rs | 164 | `pub async fn list` — **sw-canvas-2 finding** |
| src/canvas/store.rs | 174 | `pub async fn list_entries` |
| src/canvas/store.rs | 244 | `pub async fn apply` |
| src/canvas/store.rs | 294 | `pub async fn delete` |
| src/canvas/assets.rs | 94 | `pub async fn put_asset` |
| src/canvas/assets.rs | 196 | `pub async fn read_asset` |
| src/canvas/assets.rs | 234 | `pub async fn sweep_orphan_assets` — **sw-canvas-1 finding** |
| src/canvas/selection.rs | 97 | `pub fn set` |
| src/canvas/selection.rs | 113 | `pub fn get` |
| src/canvas/doc_io.rs | 33 | `const PRUNE_AT: usize = 128;` — **sw-canvas-4 finding** |