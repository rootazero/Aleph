## DC-01 [P1] Path-keyed locks do not cover aliased store roots
- location: src/artifacts/store.rs:137 and src/artifacts/store.rs:280 in HEAD
- kind: partial-fix
- finding: The publication mutex is keyed by the uncanonicalized session directory. Two `ArtifactStore` instances can address the same directory through different roots (for example, a symlink or a path containing `..`), acquire different mutexes, and still perform the concurrent eviction interleaving from B4-01; one `put` can consequently return `Ok` after the other has deleted its sidecar.
- suggested-fix: After creating the session directory, canonicalize it and use that canonical path as the publication-lock key. Keep the lock held across publication, eviction, and reconciliation as it is now.

## DC-02 [P1] Directory-sync failures are still acknowledged as durable writes
- location: src/artifacts/store.rs:544 and src/artifacts/store.rs:564-580 in HEAD
- kind: swallowed-error
- finding: `write_atomic` always returns success after rename because `sync_parent_dir` only logs open/fsync failures. On a supported Unix filesystem, a directory fsync error (or an open error such as a write/execute-only directory) therefore still lets `put` return `Ok` without the directory-entry durability required by B4-02; a crash can lose the acknowledged sidecar or blob name.
- suggested-fix: Return an `io::Result` from the supported directory-sync implementation and propagate failures through `write_atomic`/`put`. If a platform must remain best-effort, isolate that exception to the unsupported platform rather than discarding errors on Unix.

## DC-03 [P2] Reserved-name encoding strands existing non-Windows artifacts
- location: src/artifacts/mod.rs:84-92 in HEAD
- kind: api-break
- finding: The Windows device-name workaround changes the on-disk directory mapping on every platform. Existing Unix artifacts for a session key such as `CON` were validly stored under `CON`, but upgraded code now reads and writes `%CON`, making the old artifacts invisible; this is wider than B4-05's Windows-specific failure mode and has no migration or fallback.
- suggested-fix: Apply the reserved-name prefix only on Windows, or provide an explicit migration/fallback for pre-existing reserved-name directories on other platforms. Preserve the shared artifact/media-cache mapping in either case.

## DC-04 [P2] Deduplicated attachment verification reads an unbounded file
- location: src/bin/aleph-migrate/transform.rs:460-478 in HEAD
- kind: partial-fix
- finding: The new integrity check calls `fs::read` before checking length. If the expected content-addressed target already exists but is corrupt or manually replaced with a very large file, verification allocates the entire file and can abort the migration through OOM instead of returning the intended size/hash mismatch.
- suggested-fix: Check metadata length before reading and compute the SHA-256 digest with a streaming reader. Alternatively, use a reader bounded to `expected_size + 1` so an oversized target is rejected without unbounded allocation.

## Summary
- hunks-reviewed: 53
- concerns: 4
- verdict: concerns-found
- validation: static diff and source inspection only; no builds or tests run, per contract.
