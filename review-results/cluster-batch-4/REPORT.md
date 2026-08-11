# Review Report — Batch 4 (Node-side commands: file + dispatch table)

**Scope:** `src/cluster/node_file_cmd.rs` (323 LOC), `src/cluster/node_runtime.rs` (272 LOC)
**Date:** 2026-08-11
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-cluster` (branch `review/cluster`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 3 |
| Low      | 3 |

The single High finding is the only one that crosses a security boundary
(file write can escape its containment on a workspace root that itself sits
on a symlinked path). Existing tests cover happy paths but none exercise the
adversarial symlink-at-root case.

---

## Findings

### [HIGH] src/cluster/node_file_cmd.rs:36 — `resolve_in_jail` follows symlinks in `workspace_dir` itself, so a node whose workspace is replaced by a symlink before `create_dir_all` can write outside the intended jail

**Category:** Security / Path traversal
**Confidence:** High

**Description:**
```rust
async fn resolve_in_jail(path: &str, workspace_dir: &Path) -> Result<PathBuf, String> {
    tokio::fs::create_dir_all(workspace_dir).await...?;
    let root = tokio::fs::canonicalize(workspace_dir).await...?;
    let resolved = check_and_resolve_path(Path::new(path), &get_denied_paths(), Some(&root))...?;
    if !resolved.starts_with(&root) {
        return Err("path escapes node workspace".to_string());
    }
    Ok(resolved)
}
```

`tokio::fs::canonicalize` resolves symlinks at every component. If an attacker
who has write access to the parent of `workspace_dir` (or who can race the
service to delete and re-create `workspace_dir` as a symlink) replaces
`workspace_dir` with `workspace_dir -> /etc`, then `root` resolves to
`/etc`. The subsequent `starts_with(&root)` check passes because `/etc/x`
starts with `/etc`. `check_and_resolve_path` has its own deny-list but the
deny-list is abouts `/etc/shadow`, not the whole subtree.

This is a **node-side** attack (the attacker controls the node host, not the
center), but the contract is "node workspace is a jail, and bytes go directly
to the host filesystem" — the threat model assumes the workspace path itself
is trustworthy.

**Fix:** explicitly `std::fs::metadata` `workspace_dir` once, ensure
`file_type().is_dir()` and not a symlink, and reject if symlinked. Use
`fs::symlink_metadata` (does NOT follow symlinks) rather than
`fs::metadata` for the check.

---

### [MEDIUM] src/cluster/node_runtime.rs:155 — `with_bash` adds a single command without `file.read`/`file.write`; `register_file_commands` is an opt-in step, so the typical node runtime ships without file commands unless both are wired

**Category:** Quality / API ergonomics
**Confidence:** High

**Description:**
`CommandTable::new()` → `with_bash(...)` → `register_file_commands(ws)`.
The two-step pattern is documented, but a casual reader may believe
`with_bash` is the only entry point and conclude that nodes cannot do file
transfer. Add a one-line `with_bash_and_files(bash, session, ws)` convenience
constructor; or update the doc to point at `register_file_commands` from
`with_bash`'s doc-comment. Either fix is one-line.

---

### [MEDIUM] src/cluster/node_file_cmd.rs:91 — `FileWriteCommand::run` opens with `.create(true).truncate(true)` plus `.create_new(true)` when overwrite=false; this combination has well-defined behaviour but the comment is wrong (truncate is ignored when create_new is set)

**Category:** Quality / Documentation
**Confidence:** High

**Description:**
```rust
let mut opts = tokio::fs::OpenOptions::new();
opts.write(true).create(true).truncate(true);
if !overwrite {
    opts.create_new(true);
}
```
When `overwrite=false`, both `create(true)` and `create_new(true)` are set.
`create_new(true)` implies `create(true)` and rejects existing files; the
`truncate(true)` is a no-op in that case. The comment above the block is
fine, but the option set looks redundant. Cosmetic; no fix beyond tightening
the docstring.

---

### [MEDIUM] src/cluster/node_file_cmd.rs:148 — `FileReadCommand` reads the entire file into memory (`tokio::fs::read`), defeating the size cap's purpose on adversarial inputs

**Category:** Logic / Memory
**Confidence:** Medium-High

**Description:**
The size cap is enforced twice (via `metadata.len()` and via `bytes.len()`),
but `tokio::fs::read` allocates the full buffer before the second check
runs. An adversarial connect frame with a `sha256` matching a real-but-large
file at a known path can be used to OOM the node process — the center is
willing to spend `MAX_FILE_BYTES + ε` of memory per `file.read` call. With
`MAX_FILE_BYTES = 8 MiB`, an attacker can drive allocation growth 8 MiB per
RPC. Trivial mitigation:

```rust
let file = tokio::fs::File::open(&src).await?;
let mut buf = Vec::with_capacity(std::cmp::min(size, MAX_FILE_BYTES) as usize);
file.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut buf).await?;
if buf.len() > MAX_FILE_BYTES { return Err(...); }
```

Use `File::take(MAX_FILE_BYTES as u64 + 1)` so the kernel only delivers
`MAX_FILE_BYTES + 1` bytes into the buffer regardless of the file's actual
size.

---

### [LOW] src/cluster/node_runtime.rs:103 — `BashNodeCommand::run` uses `BashExecTool::call_json(args)`; no explicit timeout is enforced at this layer

**Category:** Quality / Defense in depth
**Confidence:** Medium

**Description:** the timeout is set inside `BashExecTool` via sandbox
configuration. If a node-side operator misconfigures the sandbox
(no_timeout = true), `bash` calls through the reverse RPC can run forever.
This is acceptable as long as the reverse-RPC `call` timeout
(`node_invoke`'s `timeout_ms`) is honored — and it is (ReverseRpcChannel
returns `Timeout`). Document the layering in `BashNodeCommand::run`'s
docstring.

---

### [LOW] src/cluster/node_runtime.rs:50 — `descriptors()` sorts by `name.cmp` (byte order); for non-ASCII command names this is unintuitive but not buggy

**Category:** Documentation
**Confidence:** High

**Description:** all production nodes ship ASCII command names today, so this
is theoretical. Note in the docstring that the sort is `str::cmp` not
locale-aware.

---

### [LOW] src/cluster/node_file_cmd.rs:3 — `MAX_FILE_BYTES = 8 * 1024 * 1024` is hardcoded; a fleet operator has no way to lower it without recompiling

**Category:** Configurability
**Confidence:** Medium

**Description:** the size cap is a security boundary; per-node configurability
(e.g. via the `CommandTable` constructor) would let a low-power node ship
with `MAX_FILE_BYTES = 1 MiB`. Future enhancement, no fix in this batch.

---

## Files reviewed (cross-referenced, not in findings scope)

- `src/builtin_tools/file_ops.rs` — `check_and_resolve_path`, `get_denied_paths`.
  Read-only cross-reference. The deny-list is consulted by `resolve_in_jail`.
- `src/builtin_tools/file_ops.rs::check_and_resolve_path` — confirmed it does
  not enforce containment (only resolves relative paths against a base); the
  `starts_with(&root)` check in `resolve_in_jail` is the actual jail gate.

## Clean areas

- SHA-256 verification of `file.write` is correct and tested.
- Base64 size cap (`max_b64_len`) is tight (4/3 + 4 padding).
- `dispatch`'s allowlist authority is correct (only `tool.call` method, only
  registered tools).
- `BashNodeCommand` uses `SESSION_ID.scope` to bind the bash sandbox session
  correctly.
- Existing tests cover round-trip, oversize, sha mismatch, traversal,
  overwrite semantics, missing, oversize read.