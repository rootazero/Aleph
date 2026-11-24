# Fix review — IF6

| ID | Verdict | Comment |
|---|---|---|
| IF6-03 | NOT_FIXED | The diff contains no changes to `email/config.rs`, `irc/config.rs`, or `mattermost/config.rs`; the derived `Debug` on credential-bearing structs remains in place. |
| IF6-02 | NOT_FIXED | The diff contains no changes to `email/message_ops.rs`; `RFC822` fetch and unconditional `+FLAGS (\\Seen)` remain unchanged. |
| IF6-01 | NOT_FIXED | The diff contains no changes to `email/config.rs`; line 146 `sender.contains(allowed)` substring match remains. |
| IF6-05 | NOT_FIXED | The diff contains no changes to factory implementations in `irc/mod.rs`, `email/mod.rs`, `signal/mod.rs`, or `mattermost/mod.rs`; hardcoded channel IDs persist. |
| IF6-04 | NOT_FIXED | The diff contains no changes to `signal/message_ops.rs:328-346` or `irc/message_ops.rs:167-172`; timestamp-only message IDs remain. |
| IF6-06 | NOT_FIXED | The diff contains no changes to `email/message_ops.rs` markdown conversion or link replacement; unescaped HTML insertion remains. |
| IF6-09 | FIXED | Lines 64-77 thread `retry_exhausted: Arc<AtomicBool>` into monitor; line 142 sets flag on exhaustion; line 190 reads flag and maps to `Error` status instead of treating normal exit as `Disconnected`. |
| IF6-08 | NOT_FIXED | The diff contains no changes to `irc/message_ops.rs`; send still returns success after queueing (line 230), write errors only logged (line 430-433). |
| IF6-07 | NOT_FIXED | The diff contains no changes to `irc/message_ops.rs`; nick captured at line 306 remains stale after 433 handling. |
| IF6-10 | NOT_FIXED | The diff contains no changes to `email/mod.rs`, `signal/mod.rs`, or `mattermost/mod.rs`; capability declarations and send paths that ignore attachments remain. |
| IF6-11 | NOT_FIXED | The diff contains no changes to `cli.rs:294-316`; synchronous stdout operations inside async `send` remain on lines 307-308. |
| IF6-14 | NOT_FIXED | The diff contains no changes to `sms/transport.rs`; line 663 still uses stale `now` when `pinned.is_none()` after authorization wait. |
| IF6-15 | NOT_FIXED | The diff contains no changes to `email/message_ops.rs:329-356`; unbounded SEARCH/collect remains. |
| IF6-16 | NOT_FIXED | The diff contains no changes to `irc/message_ops.rs:288`; `BufReader::lines()` without per-line limit remains. |
| IF6-12 | PARTIAL | Lines 214-228 spawn a new stdin reader thread but do not stop, join, or replace the previous one; each `start` adds a detached thread competing for the same `stdin.lock()`. The finding's failure mode (restart leaves old reader consuming input) still occurs. |
| IF6-13 | NOT_FIXED | The diff contains no changes to `sms/transport.rs`; operation insertion with `outcome: None` (line 603) and indefinite retention (line 1007) remain, stranding cancelled operations. |

## New problems

- `src/gateway/interfaces/cli.rs:214-228` — Duplicate stdin reader threads on each restart. The new spawn occurs unconditionally in `start()`, while `stop()` does nothing to terminate the previous thread. Each restart leaves another detached thread blocked on `stdin.lock()`, competing for input and consuming a thread resource. This is a robustness regression introduced by the diff.

## Batch verdict

Needs complete rework. Only 1 of 16 findings (IF6-09) is fixed. The remaining 15 are untouched, and the single change to IF6-12 introduces a new resource leak without solving the original concurrency problem.