# Static Review — `src/artifacts/` (Aleph core)

**Module:** `src/artifacts/` (2 files, ~935 lines)
**Worktree:** `.worktrees/review-artifacts` (branch `review/artifacts`)
**Scope:** `src/artifacts/mod.rs`, `src/artifacts/store.rs`
**Reviewer tools:** static read + graph cross-reference + rust-doctor context

## Summary

The artifacts module is one of the smallest and most self-contained surfaces in the core. Its two responsibilities — (1) write-temp+rename durable blob storage scoped to a percent-encoded session directory, and (2) an `id`-addressed read path guarded by UUID canonicalisation — are implemented cleanly, with documented invariants ("traversal-safe", "injective", "sidecar-first eviction") that match the code.

The core security boundary — `../../etc/passwd` must not be reachable through any caller-supplied string — is closed by `encode_session_key` (all non-`[A-Za-z0-9_-]` bytes escape) and `validate_id` (rejects every UUID spelling except bare hyphenated lowercase). Both are exercised by `read_rejects_ids_that_are_not_bare_uuids` and `encode_session_key_cannot_produce_traversal`.

The single actionable bug is a documentation/reality drift in `evict_overflow`: the function's doc claims "a failure here is logged", but the two per-record removals are `let _ = …` — errors are silently discarded. That violates the rust-doctor "silent error discard" rule and is P1 because the underlying filesystem problem (full disk, permission revocation, IO error) is exactly what an operator needs to see.

The other findings are P2/P3 design notes or cross-module deferrals matching the aggregate.md `#[non_exhaustive]` pattern. No P0.

## Findings

#### [P1] silent-error-discard — `evict_overflow` swallows per-record removal errors

- **File:** `src/artifacts/store.rs:307-308`
- **Rule:** silent-error-discard (Reliability)
- **Context:** Inside `async fn evict_overflow(dir: &Path)`, after `count_sidecars`/`read_records` already warn on their own failure paths. Best-effort loop removing `<id>.json` then `<id>.bin` for every record that overflows `MAX_ARTIFACTS_PER_SESSION`.
- **Before:**
  ```rust
  for record in records.into_iter().take(overflow) {
      // Sidecar first: a partial delete leaves an invisible orphan blob, not
      // a listed record whose bytes are gone.
      let _ = fs::remove_file(dir.join(format!("{}.json", record.id))).await;
      let _ = fs::remove_file(dir.join(format!("{}.bin", record.id))).await;
  }
  ```
- **After:**
  ```rust
  for record in records.into_iter().take(overflow) {
      let json = dir.join(format!("{}.json", record.id));
      let bin = dir.join(format!("{}.bin", record.id));
      // Sidecar first: a partial delete leaves an invisible orphan blob, not
      // a listed record whose bytes are gone. Failures are logged but not
      // propagated — the blob is already durably written, and surfacing
      // them as a failed `put` would undo work that succeeded.
      if let Err(e) = fs::remove_file(&json).await {
          warn!(path = %json.display(), error = %e, "artifact eviction: could not remove sidecar");
      }
      if let Err(e) = fs::remove_file(&bin).await {
          warn!(path = %bin.display(), error = %e, "artifact eviction: could not remove blob");
      }
  }
  ```
- **Why:** The function's own doc comment says "a failure here is logged" but the code uses `let _ =`. A full disk, a revoked permission, or an IO error here is precisely the signal an operator needs — silently dropping it leaves an orphan `.bin` (invisible to `list` because its `.json` was the one that got removed) and grows the data directory unbounded, undermining the `MAX_ARTIFACTS_PER_SESSION` cap. The other failure paths in this same function already use `warn!`; this one is the only outlier.

#### [P2] api-design — `read` returns full `Vec<u8>`, forcing ranged/text callers to materialise the whole blob

- **File:** `src/artifacts/store.rs:160-176`
- **Rule:** api-design (Performance)
- **Context:** `pub async fn read(&self, session_key, id) -> Result<(ArtifactRecord, Vec<u8>), _>`. Two distinct callers consume this: `gateway/server/artifact_route.rs:354` slices a `Range: bytes=100-199` out of a potentially 50 MB payload, and `gateway/handlers/artifacts.rs:236` (`handle_read_text`) loads the full blob to truncate via `truncate_utf8`. Both would benefit from a streaming or bounded reader.
- **Before:**
  ```rust
  let bytes = read_or_not_found(&dir.join(format!("{id}.bin")), id).await?;
  Ok((record, bytes))
  ```
- **After (future — defer to dedicated change):**
  ```rust
  // Either: expose `read_range(session_key, id, start, end) -> Result<(Record, Bytes), _>`
  // backed by `tokio::fs::File::read`, so the route serves only the slice it asked for;
  // or: expose a stream-yielding `read_stream(session_key, id) -> Result<impl AsyncRead + _, _>`
  // and let axum body-stream it.
  ```
- **Why:** The store's API forces every caller to allocate `MAX_ARTIFACT_BYTES` (50 MB) even when the response is a 4 KB range or a 16 KB text preview. This is an architectural shape, not a one-line fix — defer to a dedicated change with measurable benchmarks. **In scope of this PR: no change.**

#### [P2] data-integrity — `read` does not verify `bytes.len() == record.size`

- **File:** `src/artifacts/store.rs:174`
- **Rule:** data-integrity (Security)
- **Context:** The sidecar declares `size: u64`; the `.bin` is read separately. If anything replaces the `.bin` out-of-band (operator copy, filesystem corruption, symlink planted at `<id>.bin`), `bytes.len()` and `record.size` silently disagree. `serve_artifact` uses `bytes.len()` for the wire `Content-Length` while `handle_list` exposes `record.size` to clients — they drift without any signal.
- **Before:**
  ```rust
  let bytes = read_or_not_found(&dir.join(format!("{id}.bin")), id).await?;
  Ok((record, bytes))
  ```
- **After (defer — defensive enhancement, not a bug fix):**
  ```rust
  let bytes = read_or_not_found(&dir.join(format!("{id}.bin")), id).await?;
  if bytes.len() as u64 != record.size {
      warn!(
          id = %id,
          declared = record.size,
          actual = bytes.len(),
          "artifact blob size disagrees with sidecar; refusing to serve"
      );
      return Err(ArtifactError::NotFound(id.to_string()));
  }
  Ok((record, bytes))
  ```
- **Why:** Hardening against out-of-band tampering. Requires a product decision (refuse vs warn-and-serve) and touches every caller. **Defer.**

#### [P2] concurrency — `evict_overflow` races under concurrent `put` in the same session

- **File:** `src/artifacts/store.rs:274-310`
- **Rule:** async-correctness (Concurrency)
- **Context:** Two simultaneous `put` calls in the same session each call `evict_overflow`, which is `&Path`-state only with no per-session guard. Each independently walks the directory, sorts, and removes the `overflow` oldest.
- **Behaviour:** The design is **race-safe** in observable state — both callers deterministically pick the same oldest records, `let _ = fs::remove_file` swallows the second-writer's `NotFound`, and "sidecar first" ensures partial deletes are invisible to `list`. The only consequence is mild over-eviction (both may evict when only one was needed), which is harmless. Traced concretely: worst case is ~200 records evicted instead of 100, ending up below the cap. The invariant holds.
- **Why P2 (defer):** Adding per-session serialisation would force every call to acquire a `Mutex`, and every other caller (`read`, `list`, `purge_session`) shares the same session directory — the lock would have to live at the store level, not per-call, which is an architectural change. The current "best-effort, benignly over-eager" trade is documented in the doc comment and the `take(overflow)` line. **Defer.**

#### [P2] cross-module — `ArtifactOrigin` / `ArtifactError` lack `#[non_exhaustive]`

- **File:** `src/artifacts/mod.rs:79, 117`
- **Rule:** architecture (API)
- **Context:** Two public enums in the public lib API. Both are matched exhaustively downstream (`gateway/server/artifact_route.rs:241` and the drift-guard test in `mod.rs:191-208`).
- **Why P2 (defer):** Matches the `shared::58-enum` pattern explicitly deferred in `review-results/aggregate.md`. Adding the attribute now breaks every downstream exhaustive `match`; the fix is a single attribute per enum but the audit of every match site is crate-spanning. **Defer to the dedicated change noted in aggregate.md.**

#### [P3] style — `write_atomic` `let _ = fs::remove_file(&tmp)` after failed rename

- **File:** `src/artifacts/store.rs:321`
- **Rule:** silent-error-discard (Reliability)
- **Context:** Inside the rename-failure arm of `write_atomic`. If the rename fails, we try to delete the sibling `.tmp` and propagate the rename error either way.
- **Why P3 (skip):** The next `put` of the same id calls `fs::write(&tmp, ...)` which truncates, so the stale `.tmp` is overwritten on the very next write regardless. The cleanup is a niceness, not a correctness invariant — `let _ =` here is defensible. Documented intent: the rename error is the primary signal, cleanup is opportunistic. **No change.**

#### [P3] latent — orphan `.bin` files accumulate on partial-eviction crash

- **File:** `src/artifacts/store.rs:306-309` (interaction with `count_sidecars`)
- **Rule:** resource-safety (Reliability)
- **Context:** Eviction removes `.json` first, then `.bin`. A process crash between the two leaves a `.bin` whose `.json` is gone — invisible to `list` (counts only `.json`), never addressed by `read` (which requires the `.json`). Over many crash-recovery cycles, these orphans grow without bound, since `MAX_ARTIFACTS_PER_SESSION` is enforced by `.json` count.
- **Why P3 (note):** Bounded in practice (orphan rate ≤ process-crash rate × put rate) and entirely invisible to users. A sweep could be added at boot — `for entry in read_dir { if ext == "bin" && !json.exists { remove } }` — but the symptom is "data dir grows slowly over years", not a security or correctness bug. **No change.**

#### [P3] perf — `evict_overflow` walks the session dir twice

- **File:** `src/artifacts/store.rs:274-310`
- **Rule:** performance (Maintainability)
- **Context:** `count_sidecars` then `read_records` each call `fs::read_dir`. On the common path (under cap) only `count_sidecars` runs; on the eviction path both run.
- **Why P3 (note):** The split exists for a reason — under cap, we want to skip the JSON parsing entirely. Combining them would either always-parse (slow) or always-walk-twice (same as today, no benefit). The current shape is correct. **No change.**

## Cross-module observations (informational, no fix)

- **Caller pattern: `ArtifactStore::default_root()` is called at 4 sites** (`gateway/server/mod.rs:875`, `gateway/handlers/session/db_handlers/modify.rs:13`, `bin/aleph-server/commands/start/builder/handlers/session.rs:200`, `gateway/execution_engine/run_loop/inner.rs:1604`), each followed by `match ArtifactStore::new(root)`. The shared `ArtifactStore::shared()` helper at `store.rs:69-83` would be the single resolution point, but each site wants its own `Arc<ArtifactStore>` to attach to handler state. Same call pattern in `archive_inbound_attachments` (`inner.rs:1604-1632`) which uses `ArtifactStore::new(root)` directly. Not a finding — just noting the repetition is intentional.
- **`gateway/handlers/artifacts.rs:236` (`handle_read_text`) reads the full blob** then truncates via `truncate_utf8`. Falls out of the P2 `read` API note above; the fix is at the store level.
- **`write_atomic` does not fsync the parent directory** after rename (POSIX durability gap on power loss). Beyond module scope; the comment is "writes go through a temp file + rename so a crash never publishes a half-written blob" — true for crash, not for power loss. Same trade as the rest of the codebase. **No change.**

## Headline counts

| File | P0 | P1 | P2 | P3 |
|------|----|----|----|----|
| `src/artifacts/mod.rs` | 0 | 0 | 1 (cross-module defer) | 0 |
| `src/artifacts/store.rs` | 0 | 1 | 2 (in-module defer) | 3 |
| **Total** | **0** | **1** | **3** | **3** |

## Findings fixed in this PR

1. **`evict_overflow` per-record removal errors** — `let _ = fs::remove_file(...).await` replaced with `if let Err(e) = ... { warn!(...) }` so that the underlying filesystem failure (full disk, permission revocation, IO error) is no longer silently dropped. The function's doc comment already promised "logged, never surfaced"; the code now matches the contract.

## Findings deferred / cross-module

1. **`read` returns full `Vec<u8>`** (P2, in-module defer). Force-feeding every caller through a 50 MB allocation for ranged reads and text previews is an API design issue. Needs benchmarks and a dedicated change.
2. **`read` does not validate `bytes.len() == record.size`** (P2, in-module defer). Defensive enhancement against out-of-band tampering. Requires a product decision (refuse vs warn-and-serve).
3. **Concurrent `evict_overflow` race** (P2, in-module defer). The current design is race-safe in observable state (over-eviction only); architectural change not justified by current behaviour.
4. **`ArtifactOrigin` / `ArtifactError` missing `#[non_exhaustive]`** (P2, cross-module defer). Matches the `shared::58-enum` pattern in `review-results/aggregate.md`. Add as part of the dedicated migration there.
5. **Orphan `.bin` accumulation** (P3, latent). Bounded by crash rate × put rate; invisible to users. Documented as informational.

## State of negative

- **Not** running `cargo check` — explicitly forbidden by the task instructions ("a unified cargo check runs after all modules finish"). Static review only.
- **Not** adding `#[non_exhaustive]` to `ArtifactOrigin`/`ArtifactError` — explicitly deferred to the cross-module migration captured in `aggregate.md`.
- **Not** adding a `read_range`/`read_stream` API — out of scope of a static review fix; needs design and benchmarks.
- **Not** adding a fsync-of-parent-dir to `write_atomic` — beyond module scope; same trade as the rest of the codebase.
- **Not** introducing per-session locking around `evict_overflow` — the current design is race-safe in observable state; the change is architectural and not justified by current behaviour.
- **Not** changing `pub` to `pub(crate)` on `ArtifactStore` or its methods — every `pub` item is consumed across `alephcore`, `aleph-protocol`, `aleph-cli`, `aleph-panel` (see `grep` results: `src/gateway/server/artifact_route.rs`, `src/gateway/handlers/artifacts.rs`, `src/builtin_tools/artifact_publish.rs`, `src/export/collect.rs`). Tightening visibility is a crate-spanning change.
- **Not** validating `chrono::Utc::now().timestamp_millis()` — `i64` overflow is ~292 million years out. Not a real concern.
- **Not** closing the `let _ = fs::remove_file(&tmp)` in `write_atomic` — the next `put` truncates the stale `.tmp` regardless, so silent cleanup here is defensible.
- **Not** running `rust-doctor . --json` / `--plan` — review is purely static per task scope.