# W3B brief — artifacts + migrate fixes (wave 3)

Contract: `../BRIEF_FIX.md` (same directory). Worktree: /home/zou/data/workspace/Aleph-wt-fix-wave3b (branch fix-wave3b).
Report: `/home/zou/data/workspace/Aleph-wt-fix-wave3b/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w3b.md` (uncommitted, one section per ID).

## W3B-DC-01 [P1] Path-keyed locks do not cover aliased store roots
- location: src/artifacts/store.rs (publication lock registry, `slot`/`publication_lock`)
- kind: partial-fix
- finding: the publication lock is keyed by the directory path as written, so two different spellings of the same directory (relative vs absolute, or a symlinked path) get different locks and can race on the same directory.
- suggested-fix: key the lock registry by a canonicalized directory identity (e.g. `std::fs::canonicalize` when the directory exists, falling back to the absolute path), so aliases share one lock.

## W3B-DC-02 [P1] Directory-sync failures are still acknowledged as durable writes
- location: src/artifacts/store.rs:544 and 564-580 (HEAD)
- kind: swallowed-error
- finding: `write_atomic` always returns success after the rename because `sync_parent_dir` only logs open/fsync failures. A directory fsync error therefore still lets `put` return `Ok` without the directory-entry durability the fix promised; a crash can lose the acknowledged sidecar or blob name.
- suggested-fix: propagate directory-sync failures on platforms where the fsync is supported, while keeping the existing best-effort behaviour only for platforms where directory fsync is genuinely unsupported (isolate that exception to the unsupported platform, not to all platforms).

## W3B-DC-03 [P2] Reserved-name encoding strands existing non-Windows artifacts
- location: src/artifacts/mod.rs:84-92 (HEAD)
- kind: api-break
- finding: the Windows device-name workaround changes the on-disk directory mapping on EVERY platform. Existing Unix artifacts for a session key such as `CON` were validly stored under `CON`, but the upgraded code now reads and writes `%CON`, making the old artifacts invisible. There is no migration and no fallback.
- suggested-fix: apply the reserved-name prefix only on Windows (`#[cfg(windows)]`), or provide an explicit fallback that still reads the pre-existing unprefixed directory on other platforms. Preserve the shared artifact/media-cache mapping in either case.

## W3B-DC-04 [P2] Deduplicated attachment verification reads an unbounded file
- location: src/bin/aleph-migrate/transform.rs:460-478 (HEAD)
- kind: partial-fix
- finding: the new integrity check calls `fs::read` BEFORE checking the length. If the expected content-addressed target already exists but is corrupt or was replaced with a very large file, verification allocates the entire file and can abort the migration through OOM instead of returning the intended size/hash mismatch.
- suggested-fix: check the metadata length first and compute the SHA-256 with a streaming reader (or use a reader bounded to `expected_size + 1`) so an oversized target is rejected without unbounded allocation.

Scope: src/artifacts/mod.rs, src/artifacts/store.rs, src/bin/aleph-migrate/transform.rs only.
Read the current HEAD code before editing — wave-1 and wave-2 fixes are already merged.
