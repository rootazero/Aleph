# Module: utils

## Summary
- Path: `src/utils/` (13 files, ~2,516 lines)
- Issues found: 0 high-confidence

## Reviewers
- Security / Logic / Architecture / Quality

## High-Confidence Issues
None.

## Per-perspective findings

### Security
- `instance_lock.rs` uses `fs2::FileExt::try_lock_exclusive` — correct cross-platform primitive. Held via `Drop` on `File`.
- Sidecar PID file (`aleph.lock.pid`) deliberately kept unlocked so a contender can read holder info on every platform, including Windows where `LockFileEx` blocks all reads.
- `path_within.rs` uses lexical `Component`-level normalization + `strip_prefix` for path-containment checks. No filesystem touch → no symlink TOCTOU. Prefix confusion (`/skills/demo` vs `/skills/demonstration`) is correctly rejected.
- `atomic_io.rs`, `atomic_write.rs` use the temp-file + rename pattern for atomic writes.
- `vault_io.rs` wraps the secret-store I/O with proper error propagation.

### Logic
- `parse_holder` at `instance_lock.rs:179-187` uses `unwrap_or(-1)` (default) — safe, no panic path.
- `no_window.rs` is a tiny `#[cfg(windows)]` extension — no-op off Windows, matches the documented purpose without platform API pollution (R1 fine, since the trait is utility-only and the Windows path is documented).
- All `.unwrap()` / `.expect()` calls verified to be in `#[cfg(test)]` blocks (json_extract.rs:215+, paths.rs:591+, atomic_io.rs:76+, etc.).
- `OneOrMany<T>` is a tagged enum with a proper `iter()` API and serde `untagged` — DRY replacement for a deprecated dependency.

### Architecture (R1-R10)
- **R1**: only platform-touched code is `no_window.rs` `#[cfg(windows)]` blocks (`std::os::windows::process::CommandExt`, `creation_flags`) — this is the correct shape: a thin cfg-gated platform-specific behavior inside a utility trait, the trait is no-op elsewhere, and the Windows-internal value is constant. Documented as `Win32`-specific.
- **R3**: no heavy deps. Uses standard `std` + `fs2` + `tempfile` (test-only) + `serde`.
- **R4, R8, R9, R10**: utilities are pure / no business logic, no LLM bypass, no hidden configurability.

### Quality
- File sizes lean: `one_or_many.rs` 148 LOC, `path_within.rs` 58 LOC, `no_window.rs` 51 LOC, `vault_io.rs` 88 LOC — single responsibility.
- `path_within.rs` test for "prefix confusion" exactly guards the common mistake that would have been a security bug if missed.
- `instance_lock.rs:53-67` provides `rewrite_holder_pid` after `fork()` — a sharp edge explicitly handled with documentation.
- Public API minimal — utilities expose only the relevant functions, no `pub` sprawl.

## Production-grade patterns observed
- Cross-platform `Win32` constant `CREATE_NO_WINDOW = 0x0800_0000` documented with its MS Learn link.
- Atomic write primitives paired with sidecar-PID atomic writing for race-free `fork()` semantics.
- Path safety: lexical-only containment check avoids the symlink TOCTOU class entirely (vs. filesystem-touching alternatives).

## Conclusion
`src/utils/` is clean, minimal, and matches every project redline (the Windows-specific trait extension is properly `#[cfg(gated)]` and documented). No changes required.
