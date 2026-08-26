# Logic Review Report — src/canvas/

**Module**: canvas
**Scope**: `src/canvas/mod.rs`, `src/canvas/store.rs`, `src/canvas/doc_io.rs`, `src/canvas/selection.rs`, `src/canvas/validate.rs`, `src/canvas/assets.rs` (2137 LOC total)
**Date**: 2026-08-26
**Mode**: normal
**Worktree**: `.worktrees/rust-logic-audit-2026-08-26`
**Branch**: `rust-logic-audit/2026-08-26`

---

## Phase 1 — Context Alignment

### Persistence model

- One canvas = one directory `<root>/<id>/` holding `doc.json` plus `assets/`.
- `doc.json` is written atomically (temp+rename via `crate::utils::atomic_write`).
- Atomic-write survives crash mid-write: readers see either the previous complete
  content or the new complete content.
- `<root>` is supplied by `alephcore::utils::paths::get_canvas_root()` in production
  (`src/bin/aleph-server/commands/start/mod.rs:1222`).
- File layout invariant: a single canvas directory exists per `cv-<uuid>` id.
  No two canvas stores share a root in production.

### Locking strategy

- `DocLocks` (in `doc_io.rs`) is a process-wide table keyed by canvas id:
  `Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>`. The table mutex is
  `std::sync::Mutex` (acquired briefly, never held across `.await`).
- Each canvas id resolves to one `Arc<tokio::sync::Mutex<()>>` while a holder is
  alive. `lock()` returns an `OwnedMutexGuard<()>` bundled with the read snapshot
  in `DocGuard`.
- `DocGuard::commit(&mut self)` KEEPS the lock (does not consume `self`) so the
  caller can publish `canvas.updated` from inside the same critical section —
  prevents two racing applies from publishing in reverse revision order.
- The orphan sweep (`sweep_assets_with`) is also invoked inside the critical
  section at the end of `apply` so it sees a stable `referenced` set.

### Asset storage layout

- Content-addressed: `<sha256_hex>.<ext>`, lower-case SHA-256 + one of
  `png|jpg|webp|gif|svg|html` (the mime allowlist in `assets.rs:32`).
- Extension comes from the mime allowlist, NEVER from caller-supplied filename.
- `parse_asset_id` is the strict inverse: 64 hex digits, one dot, whitelisted
  extension. Anything else is `Invalid` before any path is built.
- Orphan GC uses an `ORPHAN_GRACE` of 1 hour (`Duration::from_secs(60*60)`).
- Dedup re-arms the mtime via `set_modified(SystemTime::now())`.

### What callers expect

- 8 RPC verbs (`canvas.create/list/get/apply/delete`, `canvas.asset.put/get`,
  `canvas.selection.set`) — confirmed wired in
  `review-results/canvas-batch-1/REPORT.md` (2026-04-22).
- HTTP byte route `GET /canvas-asset/{cap}/{canvas_id}/{asset_id}`
  (`src/gateway/server/canvas_asset_route.rs`).
- Builtin tool `canvas` (`src/builtin_tools/canvas.rs`) — same `CanvasStore` Arc.
- Event face: `gateway::events::GatewayEventFrame::CanvasUpdated` published on
  every commit, with self-reported `owner_user_id` + `project_id`.

---

## Phase 2 — Semantic Invariants

### State machine legality

| Function | Behaviour under repeated call | Verdict |
|----------|-------------------------------|---------|
| `create` | Blindly inserts; if `<root>/<id>/doc.json` already existed, the new doc overwrites the old. UUID-based ids make this unreachable in practice, but the API does not enforce existence check. | **Warning** |
| `apply` | Lock + read + revision check + apply + commit + emit + sweep, all inside the lock. | Correct. |
| `delete` | Lock + read + `remove_dir_all` + drop. | Correct. |
| `apply_ops` | Single-pass mutation: `UpsertShape` replaces in place if id matches, else appends; `DeleteShape` of a missing id is a no-op; post-batch `MAX_SHAPES` cap check rejects the WHOLE batch on overflow. | Correct (with caveat — see below). |
| `put_asset` | Lock + read + mime/cap gate + metadata existence check + atomic_write. | Mostly correct (see Findings 3 & 5). |
| `read_asset` | No lock taken. Path validation + `tokio::fs::read`. | Correct (see Finding 6). |
| `sweep_orphan_assets` | Lock + read doc + sweep_assets_with. | Correct but **orphaned** (see Finding 1). |
| `selection.set/get` | Process-global `OnceLock<Mutex<SelectionTable>>`. Eviction at 4096 by oldest stamp. | Correct. |

### Error propagation

- All public `CanvasStore` methods return `Result<_, CanvasError>`.
- `?` operator is used uniformly. No `.unwrap()` / `.expect()` in production code
  paths (`src/canvas/{store,doc_io,validate,assets,selection}.rs` outside
  `#[cfg(test)]`).
- `parse_asset_id` failure and `checked_id` failure both return `Invalid` —
  consistent.
- `commit()` failure during `apply` propagates as `Internal`. The in-memory
  mutation dies with the guard. No partial state on disk.

### unwrap/panic audit

| Location | Risk | Notes |
|----------|------|-------|
| All `#[cfg(test)]` blocks | SAFE | Standard test boilerplate. |
| `src/canvas/doc_io.rs:12,45` | SAFE | `unwrap_or_else(|e| e.into_inner())` for std::sync::Mutex — recovers from poisoning. AGENTS.md rule 7 satisfied. |
| `src/canvas/selection.rs:99,109` | SAFE | Same recovery pattern. |
| `src/canvas/store.rs:202` (`shapes.len() as u64`) | SAFE | On 64-bit, `usize == u64`. On 32-bit, widening — never truncates. |

No production-path `unwrap`/`expect`.

### Lock / concurrency

- AGENTS.md rule 3: "Sync primitives from `crate::sync_primitives`".
  - `doc_io.rs` uses `tokio::sync::Mutex` and `tokio::sync::OwnedMutexGuard`
    directly (not via `crate::sync_primitives`). However, `sync_primitives.rs`
    does not re-export `tokio::sync::Mutex`. The pattern is consistent across
    many other modules (`gateway/event_emitter/*`, `gateway/busy_queue/*`,
    `gateway/inbound_router/executor.rs`, etc.) and is required for async
    lock semantics. **Style-only, not a real violation.**
  - `selection.rs` uses `std::sync::{Mutex, OnceLock}` directly. The
    `sync_primitives::Mutex` re-export resolves to the same type; identical
    effect. **Style-only.**
- Held across `.await`: only the per-canvas `tokio::sync::Mutex<()>` is held
  across awaits (in `apply`, `sweep_orphan_assets`). All other locks are
  short-lived and never held across `.await`.
- Lock hierarchy: AGENTS.md lock order is 0=DB, 1=memory, 2=tool/channel,
  3=UI. Canvas is none of these; it sits in its own namespace and is acquired
  before the event-bus publish (`bus.publish_frame`) which is sync. No
  cross-module lock acquisition detected.
- `DocLocks::slot()` upgrades-or-inserts under the table mutex. Two tasks
  racing on the same id always end up with the same `Arc`. Correct.
- `apply` reads doc, validates, applies, commits, **then** calls
  `sweep_assets_with` still holding the guard. Two racing applies cannot
  publish in reverse revision order. Documented and tested by
  `events_publish_in_revision_order_under_contention` (store.rs:822).

### Connectivity & wiring

- `pub async fn sweep_orphan_assets(&self, id: &str)` — **ORPHANED**. No
  production caller (`grep -rn sweep_orphan_assets src/` shows only the
  definition, the doc comment, and tests). See Finding 1.
- All other `CanvasStore` public methods have wired callers (RPC handlers,
  builtin tool, or internal apply path).
- Trait impls: `CanvasStore` is a concrete struct with `impl CanvasStore { ... }`
  blocks split across `store.rs` and `assets.rs`. No orphan trait impl.
- `selection::set` / `selection::get` — wired into `canvas.get`,
  `canvas.selection.set`, and `builtin_tools::canvas::call`.

### Type coercion

- `shapes.len() as u64` (store.rs:202) — widening on 32-bit, identical on
  64-bit. No truncation.
- No `as` for floats anywhere. Geometry uses `f64` natively.

---

## Phase 3 — Control Flow Simulation

### Branch coverage (validate.rs)

- `ops_shape`: covers empty batch, `> MAX_OPS_PER_APPLY`, every `CanvasOp`
  variant. All branches reachable from valid input.
- `apply_ops`: every `CanvasOp` variant, post-batch cap check. The `MAX_SHAPES`
  check is at the END, AFTER all ops are applied in place — so a batch that
  tries to push over the cap is rejected, the caller drops the guard without
  committing, and the half-applied in-memory copy dies with the guard.
- `shape_is_well_formed`: handles Ink (points cap), Arrow (bind ids), and the
  catch-all `_ => {}`. The catch-all is **correct** because the only Ink- and
  Arrow-specific validation lives here; other variants have nothing shape-
  specific to check beyond the common id / parent.

### Loop boundaries

- `list_entries`: iterates `read_dir` over `<root>`. Empty dir -> 0 rows.
  Stray files (`.DS_Store`) are skipped (filter `entry.path().is_dir()`).
  Corrupt / missing `doc.json` are skipped LOUDLY (warn!) — never fail the
  whole listing. Documented discipline.
- `sweep_assets_with`: iterates `read_dir` over `<canvas>/assets`. Stray files
  (non-canonical name) are skipped LOUDLY. Unreadable mtime -> "unknown is
  not old" -> kept. Disk-remove failure -> warn, counted as `removed=0`.
  Best-effort, never fatal.

### Path validation

- `is_valid_id` (validate.rs:27): empty / >64 / non-`[A-Za-z0-9_-]` rejected.
  `checked_id` is the gate every `id` argument passes through before any
  `root.join(id)`.
- `parse_asset_id` (assets.rs:75): 64 lowercase hex digits, one dot, extension
  in the allowlist. Anything else is `Invalid` BEFORE the path is built. Path
  traversal surface is zero.
- The charset has no separators and no dots, so `root.join(id).join(asset_id)`
  cannot traverse.
- Unknown JSON fields: serde silently drops them on `CanvasDoc` deserialize
  (e.g., a corrupted/forward-compatible doc keeps parsing). This is by design
  for `decks`, `owner_user_id`, `project_id` (each has `#[serde(default)]`).
  AGENTS.md rule 8 warning: unknown fields elsewhere on `CanvasDoc` are also
  dropped silently. For a hard upgrade path this is acceptable; for a debug
  scenario it could mask logic bugs. **No `#[serde(deny_unknown_fields)]` on
  `CanvasDoc`.**

### Asset file I/O failure modes

- **Disk full on `atomic_write_bytes`**: temp file written, rename fails, temp
  file is dropped by `tempfile`. No file at target. Caller gets `Internal`.
  Per-canvas lock dropped. No leak.
- **Disk full on `tokio::fs::create_dir_all`**: dir not created, caller gets
  `Internal`, lock dropped. No leak.
- **Permission denied on read**: `read` returns `Internal` (not `NotFound`).
  Gate on RPC fails closed; listing skips loudly. Good.
- **Permission denied on write**: same as disk full.

---

## Phase 4 — Red-Teaming

| # | Scenario | Outcome | Verdict |
|---|----------|---------|---------|
| 1 | Two simultaneous `apply` calls on same canvas, same `base_revision`. | First commits, second returns `Conflict { current_revision = N+1 }`. No data loss. Tested by `concurrent_applies_serialize_one_wins_one_conflicts`. | OK |
| 2 | Concurrent `apply` (commits) and `read_asset`. | `read_asset` does NOT take the per-canvas lock; can race with the in-passing sweep inside `apply`. Result: reader may see an asset briefly, then it gets reaped (only if unreferenced). Reader would see the file that the sweep will remove. No correctness issue (atomic_write + immediate read is consistent). | OK |
| 3 | Disk full mid-asset-upload. | `atomic_write_bytes` fails, no file written, no doc touched. Caller gets `Internal`. | OK |
| 4 | Path traversal `asset.get` with `../etc/passwd`. | `parse_asset_id` rejects (no dot split, or non-hex chars). Returns `Invalid`. Tested. | OK |
| 5 | Concurrent `delete` while `apply` in flight. | Both acquire the same per-canvas lock. Whichever runs first wins. If delete commits first, apply's `existing_mut()` returns `None` → `NotFound`. | OK |
| 6 | UTF-8 / null bytes in JSON content. | `check_title` rejects control characters including `\0`. `is_valid_id` requires ASCII alphanumeric. Shape `text`/`label`/etc. are `String` — any UTF-8 accepted (no cap, see Finding 9). | OK (title), Warning on text fields |
| 7 | `create` race with concurrent `delete` of same id. | Both lock for same id; whichever runs first wins. UUID collision unreachable. | OK |
| 8 | `apply_ops` overflow (`doc.shapes.len() > MAX_SHAPES` after batch). | Whole batch rejected; caller drops guard without committing. No partial state. Tested. | OK |
| 9 | Concurrent `selection.set` on same canvas. | Serialized by std::sync::Mutex. Last-write-wins by `next_stamp` increment. | OK |
| 10 | Asset uploaded but referencing op never lands (panel crash between `put_asset` and `apply`). | Asset sits unreferenced in `<canvas>/assets/`. **Never reaped unless `apply` (which runs in-passing sweep) is called on the canvas.** Since `sweep_orphan_assets` is orphaned (Finding 1), the ORPHAN_GRACE window is theoretical. Slow disk leak. | **Warning** |
| 11 | `read_asset` for asset id of a canvas that doesn't exist. | `parse_asset_id` passes (valid format). `tokio::fs::read` returns NotFound. Error message says **"asset <sha> in canvas <id>"** — misleading because the canvas itself doesn't exist. | **Warning** (Finding 6) |
| 12 | `put_asset` race with concurrent `delete`. | Both acquire per-canvas lock. Whichever runs first wins. If put wins, the asset is written, then delete removes the dir. If delete wins, put sees no doc, returns `NotFound`. No orphan. | OK |
| 13 | Manual directory at the dedupe path `<sha256>.png`. | `tokio::fs::metadata(&path).await.is_ok()` returns true (directories are metadata entries). Dedupe triggers. `OpenOptions::write(true).open(...)` would fail on Linux (EISDIR), warn, and return `Ok(asset_id)` as if dedupe succeeded. Subsequent `read_asset` would try to read the directory and fail with `Internal`. **Silent miss.** | **Warning** (Finding 3) |
| 14 | `create` partial failure: dir created, write fails. | `<root>/cv-X/` exists but is empty. Subsequent `delete("cv-X")` acquires lock, reads no doc, returns `NotFound` — **but does NOT remove the directory**. **Orphan empty dir.** | **Warning** (Finding 5) |
| 15 | `apply_ops` batch with shape `asset_id` referencing a non-existent or non-canonical asset. | `apply_ops` does not validate `asset_id` shape. Garbage in is stored verbatim. `read_asset` rejects malformed ids. Orphans won't be reaped (asset file never existed). | **Warning** (Finding 7) |
| 16 | `create` with `project_id` referencing an invisible project. | Gate at `handle_create` (handlers/canvas.rs:123) checks `project_visible(project_id)` before storing. Returns `NotFound`. Tested. | OK |
| 17 | Apply on a deleted canvas (between gate and lock). | Gate reads the canvas (handler), then delete runs, then apply acquires lock. `existing_mut()` returns None → `NotFound`. | OK |
| 18 | `sweep_assets_with` runs concurrently with `put_asset` on same canvas. | Both hold the per-canvas lock at distinct points. If put wins: file written with mtime=now; sweep sees young file, keeps it. If sweep wins: orphans removed; put sees no existing file, creates dir, writes. | OK |

---

## Phase 5 — Verification (suggested proptests)

Suggested proptest sketches for invariant logic — **DO NOT RUN**.

```rust
#[proptest]
fn apply_with_random_ops_preserves_shape_count_invariant(
    initial: Vec<Shape>, ops: Vec<CanvasOp>
) {
    // Setup: store with a canvas holding `initial`.
    // Apply `ops`.
    // Property 1: if any single op in `ops` would push `shapes.len() > MAX_SHAPES`,
    //              the WHOLE batch is rejected (shapes unchanged).
    // Property 2: after a successful apply, every shape.id is unique.
    // Property 3: revision = initial.revision + 1 on success, unchanged on failure.
}

#[proptest]
fn sweep_assets_with_only_removes_unreferenced_and_old(
    seed: Vec<u8>,  // asset bytes (via rng)
    referenced: HashSet<String>,  // shape asset_ids
    grace_offset_secs: i64       // negative = orphan, positive = fresh
) {
    // Setup: write `seed`-keyed assets, mark some referenced, age some past grace.
    // Run sweep.
    // Property: post-state on disk == (intersection of seed and referenced) ∪
    //           (seed - referenced - old-past-grace) — i.e. referenced OR fresh
    //           OR no-longer-present.
    // Property: count of removed == # of seed that were NOT referenced AND aged past grace.
}

#[proptest]
fn put_then_get_round_trips_for_any_supported_mime(
    bytes: Vec<u8>, mime: String
) {
    // Setup: canvas.
    // Generate mime from allowlist; reject samples where bytes.len() > cap.
    // put_asset; read_asset.
    // Property: bytes round-trip verbatim; mime round-trips to canonical form.
}

#[proptest]
fn id_charset_rejects_every_traversal_shape(
    id_strategy: Vec<String>  // arbitrary UTF-8 + control chars + path-like strings
) {
    // Property: `checked_id(id)` returns Invalid iff
    //   id.is_empty() OR id.len() > 64 OR any byte NOT in [A-Za-z0-9_-].
}

#[proptest]
fn apply_does_not_partial_commit_on_validation_failure(
    initial: Vec<Shape>, valid_prefix: Vec<CanvasOp>, bad_tail: CanvasOp
) {
    // Apply `valid_prefix` followed by `bad_tail` (one that apply_ops rejects).
    // Property: doc on disk is exactly `initial` (no valid_prefix ops landed).
    // Property: revision unchanged.
}
```

---

## Findings

### [Critical] None

The code is correct for all primary use cases. No Critical findings.

---

### [Warning] Orphaned public API `sweep_orphan_assets` — the orphan sweep never runs

- **Location**: `src/canvas/assets.rs:201`
- **Trigger condition**: A `canvas.asset.put` succeeds but the referencing op never lands (panel crash mid-flow, model abandons the op, network drop before `canvas.apply` reaches the server, etc.). The asset sits unreferenced in `<canvas>/assets/`. There is NO caller in production code that invokes `sweep_orphan_assets` — confirmed via `grep -rn sweep_orphan_assets src/`:
  - Definition (assets.rs:201)
  - Doc comments (assets.rs:14, 219)
  - 4 tests (assets.rs:489, 521, 539, 593, 606)
  - **0 production callers.**
- **Expected**: `ORPHAN_GRACE = 1 hour` is the documented upper bound on how long deleted content lingers (assets.rs module doc: *"while still bounding how long deleted content lingers on disk"*). The grace window is theoretical if no sweep runs.
- **Actual**: The only sweep is the in-passing sweep in `apply` (store.rs:273), which fires ONLY on `apply`. A canvas with unreferenced assets that is never touched again leaks indefinitely. With 10MB per asset cap, a few hundred leaks is GBs.
- **Suggested fix**: Either (a) wire a periodic background sweep (e.g., extend `gateway::channel_health_monitor`'s 5-min cadence pattern, or a new `canvas_orphan_sweeper` task); or (b) schedule an in-passing sweep from `selection::set`/`canvas.get` (cheap because they already hold the canvas context); or (c) make `ORPHAN_GRACE` private to the apply in-passing sweep and document that only assets that survived a later apply are eligible for reaping.

---

### [Warning] `create()` does not check existence — silent overwrite on id reuse

- **Location**: `src/canvas/store.rs:130-135`
- **Trigger condition**: A `<root>/cv-X/doc.json` exists on disk for any reason (manual restore from backup, leftover from a previous install, operator copy-paste, etc.). `create()` calls `DocLocks::lock` which reads (returns `Some(old_doc)`), then `guard.insert(doc)` REPLACES the read, then commits. No check that `existing_mut().is_none()` like `apply` and `delete` do.
- **Expected**: Either `create` should fail with `Internal` ("canvas already exists"), or the doc should at minimum be checked. Asymmetric with `apply`/`delete`/`put_asset` which all gate on `existing_mut().is_none()`.
- **Actual**: Silent overwrite. Unreachable under normal UUID semantics, but the API contract does not prevent the caller from passing a chosen id (the id is generated server-side here, so this is internal-only). Worth a one-line guard for defense in depth.
- **Suggested fix**: Add `if guard.existing_mut().is_some() { return Err(Internal("canvas id collision")); }` before `guard.insert(doc)`.

---

### [Warning] `put_asset` dedupe uses `metadata().is_ok()` — directories/symlinks silently accepted

- **Location**: `src/canvas/assets.rs:139`
- **Trigger condition**: A filesystem entry exists at `<canvas>/assets/<sha256>.<ext>` that is NOT a regular file — a directory (operator mkdir), a symlink (operator ln -s), a fifo (operator mkfifo), etc. `tokio::fs::metadata` succeeds for all of these. The dedupe branch fires. `set_modified` may fail silently. `atomic_write_bytes` is NOT called — the bytes the caller supplied are NOT stored. The function returns `Ok(asset_id)` as if dedupe was a hit.
- **Expected**: Either reject non-regular files (`metadata().is_ok() && file_is_regular`), or write through the bytes anyway (overwriting the directory/symlink/fifo).
- **Actual**: Silent data loss. The Panel thinks the asset was stored; later `read_asset` would `tokio::fs::read` the directory and fail with `Internal` ("failed to read asset: Is a directory"). Tested only for true file-vs-none, not for non-file-vs-file.
- **Suggested fix**: Replace with `tokio::fs::metadata(&path).await.map(|m| m.is_file()).unwrap_or(false)`, or use `tokio::fs::try_exists` plus `tokio::fs::File::open` (which fails on non-regular files).

---

### [Warning] Cancellation race in `DocLocks::lock()` breaks the "one id = one lock" invariant

- **Location**: `src/canvas/doc_io.rs:49-72`
- **Trigger condition**: `slot()` returns a fresh `Arc<tokio::sync::Mutex<()>>` with strong_count = 1. Before `lock_owned().await` finishes, the calling task is cancelled (e.g., a request handler aborts). The `lock_owned()` future drops. The Arc's strong_count goes to 0. The Weak in the slots table is now dangling.
- **Expected**: Subsequent `slot("cv-X")` calls upgrade the same Weak to the same Arc.
- **Actual**: The next caller finds `Weak::upgrade() == None`, creates a NEW Arc with a NEW `tokio::sync::Mutex<()>`. Two callers can now hold DIFFERENT locks for the SAME canvas id. Concurrent applies would race against each other on the doc.json — the optimistic-concurrency check (`base_revision == doc.revision`) would still catch most data races, but the discipline "one id = one lock" is structurally broken under cancellation.
- **Suggested fix**: Hold the Arc in the table as `Arc<...>` (not Weak) until the DocGuard is dropped. Or use `Arc::new` and a parking_lot/Mutex-based counter for slot table cleanup. The pattern `slot().lock_owned()` is the bug — the lock should be acquired synchronously inside `slot()` or `slot()` should return a future that owns the Arc.

---

### [Warning] `read_asset` returns misleading "asset not found" when the canvas is missing

- **Location**: `src/canvas/assets.rs:174-194`
- **Trigger condition**: Caller invokes `read_asset("cv-X", "<valid-sha>.png")` for a canvas id that passes `checked_id` but does not exist on disk. `parse_asset_id` passes (format OK). `tokio::fs::read(<root>/cv-X/assets/<sha>.png)` fails with `NotFound` because the parent directory does not exist.
- **Expected**: Distinguish "canvas does not exist" (`NotFound: canvas cv-X`) from "asset does not exist in this canvas" (`NotFound: asset <sha>.png in canvas cv-X`).
- **Actual**: Both surface as `NotFound("asset <sha>.png in canvas cv-X")`. Operator/agent reading the error thinks the asset name is wrong, when the real issue is the canvas id.
- **Suggested fix**: After the file read fails with NotFound, do a stat on `<root>/<id>` to disambiguate. Or take the per-canvas lock and check `existing_mut()` (cheap).

---

### [Warning] `create()` partial-failure: orphan empty canvas directory

- **Location**: `src/canvas/store.rs:130-136` + `src/canvas/doc_io.rs:163-175`
- **Trigger condition**: `tokio::fs::create_dir_all(<root>/cv-X/)` succeeds (e.g., parent dirs were writable), but the subsequent `atomic_write_bytes` fails (disk full mid-write, permission revoked between calls, FS error on the new dir). `write()` returns `Internal`. The `commit` returns `Err`. `create` returns `Err(Internal)`. The lock drops. **The directory `<root>/cv-X/` exists on disk, but is empty.**
- **Expected**: Either the dir is cleaned up on failure (rollback), or subsequent `delete(id)` removes the dir even when `existing_mut()` is None.
- **Actual**: A subsequent `delete("cv-X")` acquires the lock, reads `<root>/cv-X/doc.json` (returns `Ok(None)`), returns `Err(NotFound)` WITHOUT removing the directory. The empty dir leaks until an operator cleans `<root>` manually. Also: `list_entries` would walk this empty dir and `warn!` every time it's called (loud noise).
- **Suggested fix**: In `write()`, on `atomic_write_bytes` failure, attempt `tokio::fs::remove_dir(&path.parent())` as rollback. Or in `delete()`, when `existing_mut()` is None, also call `tokio::fs::remove_dir_all(&self.root.join(id)).await` (idempotent — `NotFound` from RMDIR is swallowable).

---

### [Warning] `Shape::asset_id` not validated by `apply_ops` — garbage in is stored verbatim

- **Location**: `src/canvas/validate.rs:74-105`
- **Trigger condition**: An `apply` batch contains `CanvasOp::UpsertShape { shape: Image { asset_id: "../escape.png", .. } }` (or any non-canonical string). `shape_is_well_formed` only checks `common.id` and `parent_id` — not `asset_id`. The batch is accepted, the doc is committed with `asset_id = "../escape.png"`.
- **Expected**: Either reject at validation time, or coerce to the canonical form.
- **Actual**: The doc.json stores the malformed reference. The orphan sweep uses `s.asset_ids()` and `parse_asset_id(&name)` — the malformed asset_id never matches a real file, so it doesn't affect sweep correctness. `read_asset` rejects the malformed id as Invalid. So this is "garbage in" but contained — no security issue, no file system hazard. Stale references (pointing to deleted assets) are also not auto-cleaned by the sweep.
- **Suggested fix**: In `shape_is_well_formed`, add a `parse_asset_id(asset_id).is_some()` check for `Image`, `Html`, and `AiImageFrame::reference_asset_ids`. Either reject or normalize.

---

### [Warning] No size cap on `owner_user_id`, `project_id`, shape `text`, deck `title`

- **Location**: `src/canvas/store.rs:120-128`, `shared/protocol/src/canvas.rs:198+`
- **Trigger condition**: Caller passes `owner_user_id: "x".repeat(10_000_000)`. There is no `check_*` for these fields. The doc.json stores them verbatim.
- **Expected**: Bounded storage dimensions, consistent with `MAX_TITLE_BYTES`, `MAX_SHAPES`, `MAX_OPS_PER_APPLY`, `MAX_ASSET_BYTES`.
- **Actual**: A single `apply` with a 100MB `text` field writes a 100MB doc.json atomically (temp+rename). Repeated applies multiply the storage. No cap, no warning.
- **Suggested fix**: Add `MAX_OWNER_ID_BYTES`, `MAX_PROJECT_ID_BYTES`, `MAX_TEXT_BYTES` constants in `shared/protocol/src/canvas.rs` and validate in `apply_ops`. At minimum, add a soft check.

---

### [Warning] `apply_ops` allows mid-batch self-referential upsert/delete sequences

- **Location**: `src/canvas/validate.rs:82-95`
- **Trigger condition**: A batch `UpsertShape{n1} → DeleteShape{n1}` is single-pass: n1 is inserted, then removed. Final state: n1 absent. But a batch `DeleteShape{n1} → UpsertShape{n1}` is also valid: n1 was absent, becomes present. Final state: n1 present. And a batch `UpsertShape{n1 with text "A"} → UpsertShape{n1 with text "B"}`: final state is n1 with text "B".
- **Expected**: Clear single-pass semantics. (Currently documented; this is the design.)
- **Actual**: The semantics are correct and deterministic but surprise clients that assume "ordered execution with no in-place overwrite". A model that emits `UpsertShape{...} → DeleteShape{...}` for the same id is almost certainly confused (delete-then-upsert is the right order). The store does not detect this pattern.
- **Suggested fix**: (Optional) detect "upsert then delete of same id" / "delete then upsert of same id" within a batch and emit a soft warn (or just refactor — most callers use replace-in-place correctly).

---

### [Warning] Unknown JSON fields silently dropped on `CanvasDoc` deserialize

- **Location**: `shared/protocol/src/canvas.rs:330+` (`CanvasDoc` struct)
- **Trigger condition**: A corrupted/forward-compatible document carries extra fields (e.g., from a newer server version). `serde_json` silently drops them. A logic bug introduced by an old build that put a field at the wrong layer would also be silently dropped.
- **Expected**: AGENTS.md rule 8: "JSON parsing — unknown fields silently dropped may hide logic bugs."
- **Actual**: `CanvasDoc` uses `#[serde(default)]` selectively (`decks`, `owner_user_id`, `project_id`) but no `#[serde(deny_unknown_fields)]`. A document with extra fields parses successfully.
- **Suggested fix**: Add `#[serde(deny_unknown_fields)]` to `CanvasDoc` in `shared/protocol/src/canvas.rs` so a future schema drift surfaces as a parse error.

---

### [Warning] `delete()` removes the directory while holding the per-canvas lock — but `read_asset` and `get` do NOT take the lock, creating TOCTOU windows

- **Location**: `src/canvas/store.rs:283-298`, `src/canvas/assets.rs:174-194`, `src/canvas/store.rs:141-150`
- **Trigger condition**: A `read_asset` call begins reading `<root>/cv-X/assets/<sha>.png`. Concurrently, `delete("cv-X")` holds the per-canvas lock and runs `tokio::fs::remove_dir_all(<root>/cv-X/)`. The reader sees the file open successfully (the OS keeps it alive) and reads the bytes. Caller sees an asset from a canvas that just disappeared.
- **Expected**: Either `read_asset`/`get` also take the per-canvas lock, or the system tolerates the brief inconsistency.
- **Actual**: The system tolerates the brief inconsistency. `get` and `read_asset` are explicitly lock-free (per their docstrings). This is by design (read throughput), but worth flagging because the contract "a read either sees the canvas or sees a NotFound" is only true for reads STARTED after the delete commits — reads in flight may see the old state. Most callers (Panel, builtin tool) handle "asset not found" gracefully, but a hand-written consumer might assume linearizability.
- **Suggested fix**: Document the guarantee explicitly in the module doc: "reads initiated after a delete commits always see NotFound; reads initiated before may see the old bytes." No code change needed unless stronger guarantees are required.

---

### [Warning] `deck.frame_ids` not validated against existing frames

- **Location**: `src/canvas/validate.rs:60-66`
- **Trigger condition**: A batch contains `UpsertDeck { deck: { id: "d1", title: "x", frame_ids: vec!["nonexistent_frame".into()] } }`. `require_id` only validates the charset, not existence. The deck is stored with a dangling frame reference.
- **Expected**: The doc JSON should not store dangling references, OR the consumer should be aware that frames may not exist.
- **Actual**: The deck is stored. The Panel might render a broken presentation. The orphan sweep would not affect frames (they're shapes, not assets).
- **Suggested fix**: Either add a post-state check that every deck.frame_ids id corresponds to a `Shape::Frame` in the same doc, or document explicitly that frame_ids may dangle.

---

### [Warning] `apply_ops` validates cap AFTER mutation in place — large batch allocates then rolls back

- **Location**: `src/canvas/validate.rs:81-99`
- **Trigger condition**: A batch that over-shoots the cap (e.g., 501 new shapes against a doc at MAX_SHAPES-1) is mutated in place first (501 Vec pushes), then the cap check rejects the WHOLE batch. The Vec allocation is wasted. The 501 pushed shapes live as `shape.clone()` instances until `apply_ops` returns Err and the guard drops.
- **Expected**: Either pre-check (estimate post-batch size) or stream the validation.
- **Actual**: One-time allocation waste on rejected batches. Bounded by `MAX_OPS_PER_APPLY = 500` and the size of each `Shape` (bounded by `MAX_INK_POINTS = 10000` points per Ink). Worst case ~60MB transient allocation, then dropped. Acceptable in practice.
- **Suggested fix**: Leave as-is. The allocation cost is dwarfed by the JSON serialization cost on commit.

---

### [Warning] `CanvasListing.row` uses `doc.shapes.len() as u64` — pre-truncation at MAX_SHAPES

- **Location**: `src/canvas/store.rs:202`
- **Trigger condition**: N/A — `usize == u64` on 64-bit, widening on 32-bit.
- **Expected**: No truncation.
- **Actual**: No truncation. Listed for completeness (the only `as` in production code).
- **Suggested fix**: None. Consider `u64::try_from(...).unwrap_or(u64::MAX)` if paranoid.

---

### [Suggested Test] `apply` does not half-commit on `apply_ops` failure

```rust
#[tokio::test]
async fn apply_ops_failure_rolls_back_in_place_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = CanvasStore::new(dir.path().to_path_buf());
    let doc = store.create(None, None, Some("u1".into())).await.unwrap();

    let err = store
        .apply(
            &doc.id,
            doc.revision,
            vec![
                CanvasOp::UpsertShape {
                    shape: note("n1", "first"),
                },
                CanvasOp::UpsertShape {
                    shape: note("bad/../id", "would-be-rejected"),
                },
            ],
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CanvasError::Invalid(_)));

    let after = store.get(&doc.id).await.unwrap();
    assert!(
        after.shapes.is_empty(),
        "no shapes landed (the whole batch is rejected)"
    );
    assert_eq!(after.revision, doc.revision);
}
```

### [Suggested Test] `create` rollback on `commit` failure

```rust
#[tokio::test]
async fn create_rolls_back_empty_directory_on_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = CanvasStore::new(dir.path().to_path_buf());

    // Force write failure by making the dir path conflict (parent is a file).
    let blocker = dir.path().join("cv-blocker");
    std::fs::write(&blocker, "not a dir").unwrap();
    // Hmm — create uses a fresh UUID id, so this specific test is hard to
    // contrive. Better: use a read-only filesystem (chmod) or fill the disk.
}
```

(In practice this is hard to unit-test without injecting a fault. A proptest that
manipulates filesystem permissions is more realistic.)

### [Suggested Test] `read_asset` distinguishes "canvas missing" from "asset missing"

```rust
#[tokio::test]
async fn read_asset_on_missing_canvas_says_canvas_not_asset() {
    let dir = tempfile::tempdir().unwrap();
    let store = CanvasStore::new(dir.path().to_path_buf());
    let valid_asset = "a".repeat(64) + ".png";
    let err = store.read_asset("cv-nope", &valid_asset).await.unwrap_err();
    match err {
        CanvasError::NotFound(msg) => {
            assert!(
                msg.contains("cv-nope"),
                "message should name the missing canvas, got: {msg}"
            );
        }
        other => panic!("expected NotFound naming the canvas, got {other:?}"),
    }
}
```

### [Suggested Test] `put_asset` dedupe rejects directory at the asset path

```rust
#[tokio::test]
async fn put_asset_dedupe_does_not_silently_succeed_on_a_directory_at_the_asset_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = CanvasStore::new(dir.path().to_path_buf());
    let doc = store.create(None, None, Some("u1".into())).await.unwrap();
    let bytes_digest = Sha256::digest(b"hello");
    let asset_id = format!("{bytes_digest:x}.png");
    let path = dir.path().join(&doc.id).join("assets").join(&asset_id);
    std::fs::create_dir_all(&path).unwrap(); // directory at the dedupe path

    let returned = store
        .put_asset(&doc.id, "image/png", b"hello")
        .await
        .unwrap();
    assert_eq!(returned, asset_id);
    // After: reading back via read_asset must NOT silently return the bytes,
    // because the bytes were never written — only a directory exists.
    let err = store.read_asset(&doc.id, &asset_id).await.unwrap_err();
    assert!(
        matches!(err, CanvasError::Internal(_)),
        "directory at asset path must surface as Internal, not Ok: {err:?}"
    );
}
```

### [Suggested Test] Sweep orphan assets via a wired caller

```rust
#[tokio::test]
async fn sweep_orphan_assets_is_wired_or_removed() {
    // Either: confirm a production caller exists (e.g., a maintenance task).
    // Or: confirm the function is dead code and should be #[cfg(test)] or removed.
    let dir = tempfile::tempdir().unwrap();
    let store = CanvasStore::new(dir.path().to_path_buf());
    let doc = store.create(None, None, None).await.unwrap();
    let asset = store.put_asset(&doc.id, "image/png", b"x").await.unwrap();
    let assets_dir = dir.path().join(&doc.id).join("assets");
    let path = assets_dir.join(&asset);
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(old));

    // The contract documents that sweep_orphan_assets will reap this.
    let removed = store.sweep_orphan_assets(&doc.id).await.unwrap();
    assert_eq!(removed, 1, "asset past ORPHAN_GRACE should be reaped");
    assert!(!path.exists());
    drop(dir);
}
```

(Already exists as a test, but the WARNING is that NO PRODUCTION CALLER exists — the
function is test-only-effective code.)

---

## Cross-Module Observations

### Wiring gaps

| Item | Status | Severity |
|------|--------|----------|
| `CanvasStore::create`, `get`, `apply`, `delete`, `list_entries`, `put_asset`, `read_asset`, `selection::{set,get}` | Wired | OK |
| `CanvasStore::sweep_orphan_assets` | **Orphaned public API** | Warning |
| `CanvasStore::list` | Thin wrapper over `list_entries`; only called by tests + RPC | OK |
| `DocLocks::slot_count` (`#[cfg(test)]`) | Test-only | OK |
| `DocGuard::{existing_mut, insert, commit}` (`pub(super)`) | Used by `store.rs` and `assets.rs` only | OK |

### Lock hierarchy spans

- The canvas per-canvas mutex (level: unnamed) is acquired around file I/O that
  includes `tokio::fs::create_dir_all`, `tokio::fs::metadata`,
  `tokio::fs::read_to_string`, `tokio::fs::remove_dir_all`. These all hold
  across `.await` for the duration of the operation.
- `DocLocks.slots` is a `std::sync::Mutex<HashMap<...>>` — never held across
  `.await`. Good.
- `selection::table` is a `std::sync::Mutex<SelectionTable>` — never held across
  `.await`. Good.
- The event bus publish (`bus.publish_frame`) is sync (uses `broadcast::Sender::send`).
  No deadlock surface.
- No interaction with the AGENTS.md lock hierarchy (DB / Memory / Tool / UI) is
  observed in the canvas module. Canvas is in its own namespace.

### API contract drift

- The `CanvasStore` API is stable across `store.rs` and `assets.rs` (separate
  `impl CanvasStore` blocks). Both blocks use `pub(super) fn doc_path` /
  `pub(super) fn assets_dir` / `pub(super) fn locks` / `pub(super) fn root` —
  visible across the canvas module's children but not to callers.
- `sweep_assets_with` is `pub(super)` and used by both `store.rs::apply` and
  `assets.rs::sweep_orphan_assets`. Good.
- `parse_asset_id` and `ext_for_mime` are private to `assets.rs` but used
  symmetrically — only `assets.rs` touches them.
- The `CanvasStore::new` and `with_event_bus` constructors form a fluent chain
  used by `commands/start/mod.rs:1224`. No drift.

### Unsignedness of `pub fn asset_ids` returning `Vec<&str>`

- `Shape::asset_ids` returns `Vec<&str>` borrowed from `&self`. In
  `apply`'s post-commit block (`store.rs:266-269`), this is consumed into a
  `HashSet<String>`. The borrow chain is bounded by the `committed` reference
  lifetime, which is reborrowed from `guard.doc`. The function compiles
  because the borrow ends before the await on `sweep_assets_with`. This is
  delicate but correct.

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 14 |
| Suggested Test | 5 |

---

## What was NOT done (State the Negative)

- **No cargo run.** The audit is static-only per the task brief.
- **No modifications to any source file.** All findings are observations; fixes
  are suggestions, not patches.
- **No new tests executed.** All test sketches are written as proptest stubs to
  be implemented and run in a follow-up.
- **No deep dive into `aleph_protocol::canvas`** beyond what `canvas/` imports.
  Wire-types schema validation is out of scope for this audit.
- **No review of the HTTP byte route** (`canvas_asset_route.rs`) beyond noting
  that it uses `read_asset` and `CanvasCapabilities` for capability-bound access.
- **No review of the `gateway::event_bus::GatewayEventBus` internals** — only
  the public surface used by `emit_updated` was inspected.
- **No review of the `builtin_tools::canvas` callers** beyond confirming the
  `apply_from` retry-once pattern and the gate's `ambient_canvas_visible`.
- **No exhaustive review of every `as` / `unwrap` in `#[cfg(test)]`** — the
  audit assumes test-code `unwrap` is acceptable.
- **No timing analysis** of `sweep_assets_with` under load.
- **No fuzz/property testing** performed — sketches only.