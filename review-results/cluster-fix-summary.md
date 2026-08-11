# Review & Fix Summary — `src/cluster`

**Date:** 2026-08-11
**Reviewer:** static (5 parallel batches, 4-perspective protocol)
**Fix branch:** `review/cluster` (worktree at `/tmp/aleph-review-cluster`)
**Final integration:** fast-forward `main` ← `review/cluster`

## Pipeline

1. Static review split into 5 parallel batches (≈2.9K LOC of production
   code, test files included).
2. 30 findings: 0 Critical / 6 High / 10 Medium / 14 Low (one High finding
   withdrawn after dependency-tree cross-check — `uuid::Uuid::new_v4()` is
   infallible in v1.x).
3. 8 fix commits on `review/cluster` (one batch commit + 7 fix commits).
   No `cargo check` mid-flight per protocol.
4. Single `cargo check -p alephcore` after fast-forward (memory-limited per
   AGENTS.md).
5. Fast-forward `main` to `review/cluster` once clean.

## Findings addressed

| Batch | ID  | Sev   | Title                                                                       | Fix commit |
|------:|-----|------:|-----------------------------------------------------------------------------|-----------:|
| 1     | B1-01 | High   | register() overwrote prev session without closing its connection          | `75c4e7549` |
| 1     | B1-02 | High   | Malformed commands/tags/version silently downgraded to empty/None          | `424de1dde` |
| 1     | B1-03 | High   | Cross-node-id conn_id collision orphaned the first session                 | `75c4e7549` |
| 1     | B1-04 | Med    | register/deregister/forget had no log breadcrumb                          | `75c4e7549` |
| 1     | B1-05 | Med    | resolve_all_by_tags used HashMap iteration order                          | `b698285ed` |
| 1     | B1-06 | Low    | NodeMatch had no Debug impl                                                | `424de1dde` |
| 1     | B1-07 | Low    | ResolveError Display had no test coverage                                  | `b698285ed` |
| 1     | B1-08 | Low    | Missing device_name fell back to "unknown" without log                     | `424de1dde` |
| 2     | B2-02 | Med    | mint_node_device fingerprint used &[..16] assuming ASCII-UUID              | `0b6f2ef52` |
| 2     | B2-03 | Med    | mint_node_device error string had no source-side warn                      | `0b6f2ef52` |
| 3     | B3-01 | High   | call() shared the same deadline for push + response — half-execute race    | `c3e1e50bf` |
| 3     | B3-02 | High   | resolve() returned true for already-handled id, masking double-resolve    | `2498a223b` |
| 3     | B3-03 | Med    | Serialization failure left a registered waiter to linger                  | `c3e1e50bf` |
| 4     | B4-01 | High   | resolve_in_jail followed symlinks in workspace_dir itself                  | `ffac995fd` |
| 4     | B4-04 | Med    | file.read allocated full file into memory before size check                | `ffac995fd` |
| 5     | B5-01 | Med    | outcome_from_str unknown arm fell through silently to Denied               | `e716321ce` |
| 5     | B5-02 | Med    | No-channel warn produced log spam on disconnect                            | `e716321ce` |

## Findings deferred (cross-file or doc-only)

| ID    | Reason                                                                                              |
|-------|-----------------------------------------------------------------------------------------------------|
| B2-04 | `deregister_node` revocation retry lives in `src/gateway/handlers/cluster.rs` (out of cluster scope). |
| B2-05 | Short-id fingerprint collision requires changing the column type (out of cluster scope).             |
| B2-06 | Deregister ambiguity payload shape change is a consumer-facing API change.                         |
| B2-07 | Cosmetic — duplicated R10 caveat in two module headers.                                             |
| B3-04 | AtomicU64 wrap is theoretical (584M years at 1/ms). Document and move on.                            |
| B3-05 | Cosmetic — `call()`'s `deadline` counts `register()` cost (microseconds).                           |
| B3-06 | `with_close` doc reference to the notify_one permit-storage guarantee.                              |
| B4-02 | `with_bash_and_files` convenience constructor — API ergonomics.                                     |
| B4-03 | `FileWriteCommand` OpenOptions redundant-create documentation.                                      |
| B4-05 | `BashNodeCommand` timeout-layering documentation.                                                   |
| B4-06 | `descriptors()` sort is `str::cmp`, not locale-aware — theoretical.                                 |
| B4-07 | `MAX_FILE_BYTES` per-node configurability.                                                          |
| B5-03 | `action.summary` size cap — producer-side enforcement.                                              |
| B5-04 | `pub(crate) use node_file_cmd::sha256_hex` consumer note.                                           |
| B5-05 | `pub(crate) use registry::normalize_node_key` visibility block split.                               |

## Withdrawn findings

| ID    | Reason                                                                                              |
|-------|-----------------------------------------------------------------------------------------------------|
| B2-01 | `uuid 1.x` exposes `pub fn new_v4() -> Uuid` (infallible). Confirmed against crate source. No panic risk. |

## Architecture compliance (cluster module)

| Redline | Status |
|---------|--------|
| **R1**  | clean — `src/cluster` does not call platform APIs. The file/jail logic is in `src/cluster/node_file_cmd.rs` (R1 explicitly allows bytes-to-host-FS on the node side). |
| **R3**  | clean — no heavy deps pulled in by this batch. |
| **R4**  | clean — interface layers (`gateway/handlers/cluster.rs`, builtin tools) are pure I/O. |
| **R7**  | clean — no LLM reasoning; all logic is table lookup + DB write. |
| **R10** | clean — does not enter `src/harness/`. |

## Categories summary

- **High**: 6 (5 fixed, 0 deferred)
- **Race / timeout accounting**: 1 (B3-01 fixed)
- **Data consistency**: 2 (B1-01, B1-03 fixed)
- **Path / symlink containment**: 1 (B4-01 fixed)
- **Resource exhaustion (file.read)**: 1 (B4-04 fixed)
- **Drift guard (string parsing)**: 2 (B3-02 primitive added, B5-01 warn)
- **Log noise / observability**: 3 (B1-04, B1-08, B5-02 fixed)
- **Defensive parsing**: 2 (B1-02, B3-03 fixed)
- **Determinism (fan-out ordering)**: 1 (B1-05 fixed)
- **Test coverage**: 1 (B1-07 fixed)
- **Doc / API ergonomics**: 13 (deferred, low priority)