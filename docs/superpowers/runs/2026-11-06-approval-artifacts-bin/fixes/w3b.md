# W3B fix report

Validation: `git diff --check` passed before the commits. Per the fix contract, no cargo, Clippy, rustfmt, tests, or builds were run.

## W3B-DC-01 [P1] Path-keyed locks do not cover aliased store roots
- verdict: fixed
- files: src/artifacts/store.rs:286-301
- change: Canonicalize the already-created session directory before looking up its process-global publication lock. If canonicalization fails, use an absolute-path fallback so lock acquisition remains infallible. Relative, absolute, and symlinked spellings of an existing directory therefore share one publication/eviction critical section.
- risk: Canonicalization adds a filesystem lookup to each `put`. If the directory disappears before canonicalization, the fallback cannot collapse symlink aliases, but subsequent writes will encounter the same disappearing-directory race; no public signature changed.

## W3B-DC-02 [P1] Directory-sync failures are still acknowledged as durable writes
- verdict: fixed
- files: src/artifacts/store.rs:539-588
- change: Return directory open/fsync results from the Unix `sync_parent_dir` implementation and propagate them through `write_atomic`. Keep the no-op best-effort implementation only on non-Unix platforms. A Unix `put` can no longer return success after the renamed directory entry failed to sync.
- risk: Unix filesystems that reject directory fsync now fail the publication and trigger existing cleanup instead of acknowledging the write. Non-Unix behaviour is unchanged; no public signature changed.

## W3B-DC-03 [P2] Reserved-name encoding strands existing non-Windows artifacts
- verdict: fixed
- files: src/artifacts/mod.rs:40-41, src/artifacts/mod.rs:75-98, src/artifacts/mod.rs:212-241
- change: Gate both the reserved-device-name import and `%` prefix on `cfg(windows)`, leaving non-Windows encoded path components unchanged. Added platform-specific regression coverage for Windows escaping and non-Windows preservation. Artifact storage and the media cache still share `encode_session_key`, so their mapping remains aligned on every platform.
- risk: Non-Windows artifacts written under a prefixed name by the affected version are not migrated or read through a fallback; pre-existing unprefixed artifacts are restored as requested. Windows behaviour is unchanged.

## W3B-DC-04 [P2] Deduplicated attachment verification reads an unbounded file
- verdict: fixed
- files: src/bin/aleph-migrate/transform.rs:7, src/bin/aleph-migrate/transform.rs:459-493
- change: Open the attachment and reject a metadata length mismatch before hashing, then compute SHA-256 through a 64 KiB read buffer instead of `fs::read`. Oversized existing targets now fail before allocation, and verification memory stays bounded even when the expected file itself is large.
- risk: A file mutated after its metadata check can now surface a hash mismatch rather than a size mismatch, while still failing closed without whole-file allocation. Hashing performs the same full-file I/O as before; no public signature changed.
