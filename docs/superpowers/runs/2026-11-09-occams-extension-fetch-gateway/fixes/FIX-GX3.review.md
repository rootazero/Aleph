# Fix review — GX3

| ID | Verdict | Comment |
|---|---|---|
| GX3-04 | NOT_FIXED | The diff changes a log message from "re-queued" to "re-delivered" (line 686), but the finding requires moving `record_enqueued` into the spawned task or wrapping it in `spawn_blocking` to eliminate blocking I/O on the arrival path. The filesystem write at `durable.rs:285` still executes synchronously before spawn. |
| GX3-01 | NOT_FIXED | No changes to `session_manager/ops/crud.rs`, `modify.rs:78` (compact_session), or any other cited locations where blocking SQLite calls occur inline. The diff only removes `self.` from a `read_recent_messages` call in `modify.rs:187`, which does not route blocking queries through `query_blocking`. |
| GX3-02 | NOT_FIXED | No changes to `session_manager/ops/identity.rs:224` (`cleanup_expired`). The finding requires adding `DELETE FROM messages_fts WHERE rowid IN (...)` before `DELETE FROM messages`. The diff does not touch this file or function. |

## New problems

- `src/gateway/session_store/file_backend/mod.rs:1436` — `format!("{:x}", Sha256::digest(...))` replaced with `hex::encode(Sha256::digest(...))`. This is a behavioral change: `format!("{:x}")` produces lowercase hex, `hex::encode` also produces lowercase hex by default, so semantically equivalent but introduces a dependency (`hex` crate) not mentioned in the findings. The change is unrelated to any finding in the batch and could introduce a regression if `hex::encode` behavior differs (e.g., in edge cases or if the `hex` crate version changes output format). The original finding made no mention of this file or hex encoding.

- `src/gateway/session_manager/ops/modify.rs:187` — `self.read_recent_messages` changed to `Self::read_recent_messages`. This is semantically equivalent (both resolve to the same inherent method) but changes the call from instance method syntax to associated function syntax for no stated reason. The change is unrelated to any finding and does not address GX3-01's blocking-I/O concern.

## Batch verdict

Needs rework. None of the three findings are addressed. GX3-04 changed a log message instead of relocating the blocking I/O. GX3-01 and GX3-02 have no relevant changes in the diff. The diff introduces unrelated changes (hex encoding, method call syntax) that were not requested and could mask the actual required fixes.