# Fix review — IF6

| ID | Verdict | Comment |
|---|---|---|
| IF6-01 | FIXED | The diff replaces substring matching with exact mailbox matching and full domain equality: `sender == allowed` and `domain == &allowed[1..]`. |
| IF6-02 | PARTIAL | `BODY.PEEK[]` avoids premature `Seen`, but fetch errors are still discarded with `Err(e) => { ... continue; }`, so the required error propagation is missing and an incomplete batch can still proceed to acknowledgement. |
| IF6-03 | PARTIAL | Only `EmailConfig` loses derived `Debug`; the diff contains no redacted `Debug` implementation for `IrcConfig` or `MattermostConfig`, so their credentials remain exposed. |
| IF6-04 | FIXED | Signal now uses `format!("{}:{}:{}", timestamp_ms, source, conversation_id)`, and IRC adds an atomic sequence component, preventing the cited same-timestamp identity collisions. |
| IF6-05 | PARTIAL | Only IRC implements `create_with_id`; no equivalent changes appear for the Email, Signal, or Mattermost factories, so their configured IDs are still ignored. |
| IF6-06 | FIXED | The diff escapes source text with `Self::escape_html(markdown)`, protects inline code before Markdown conversion, and the new test verifies escaped text and escaped link attributes. |
| IF6-07 | FIXED | The diff changes the nickname to mutable and updates it after a successful 433 write: `nick = alt_nick;`, so subsequent filtering uses the confirmed alternate nickname. |
| IF6-08 | FIXED | A queued command now carries a completion token, and the writer reports `complete_write(token, Err(e.to_string()))` on failure before `send` returns. |
| IF6-09 | NOT_FIXED | The monitor still returns normally after setting `retry_exhausted = true`; merely dropping `inbound_tx` does not make the outer `JoinHandle<()>` report an error, so the outer status can still become `Disconnected`. |
| IF6-10 | PARTIAL | Email now advertises `attachments: false` and `max_attachment_size: 0`, but the diff contains no corresponding Signal or Mattermost capability changes or explicit rejection of nonempty attachments. |
| IF6-11 | PARTIAL | Synchronous stdout writing is moved into `spawn_blocking`, but the async worker still performs `message.text.clone()` and `config.prompt.clone()` before offloading, so the newly identified large-text copy remains. |
| IF6-12 | NOT_FIXED | The diff only removes the old stdin thread and does not add any replacement reader or retained, single-owner stdin stream, so restart can no longer deliver input at all. |
| IF6-13 | PARTIAL | The diff detects removal during authorization via `state.operations.get(&operation_id).is_none()`, but it does not make peer-await cancellation settle as Unknown or otherwise ensure every cancelled pending operation is reclaimed. |
| IF6-14 | FIXED | The unpinned production path now refreshes time after authorization and rejects expiry before peer submission via `let refreshed = Utc::now();`. |
| IF6-15 | PARTIAL | UID fetches are limited to 32, but the diff still collects a batch stream and does not establish a cumulative memory bound; additionally `fetch.body()` is obtained before the completeness check, so the oversized body may already be allocated. |
| IF6-16 | FIXED | The diff replaces `BufReader::lines()` with a reader that caps accumulation at `MAX_IRC_LINE` and discards an overlong frame. |

## New problems

- `src/gateway/interfaces/cli.rs:211-224` — Removing the stdin reader without adding a replacement makes the CLI consume no input at all, breaking its inbound-message function.
- `src/gateway/interfaces/email/message_ops.rs:353-358` — Fetch failures are logged and skipped rather than propagated; failed publication processing can continue and acknowledge other messages, contrary to IF6-02’s stop-on-failure requirement.
- `src/gateway/interfaces/email/message_ops.rs:365` — `fetch.body()` is returned before `is_complete_imap_body` is checked, so the stated per-message allocation cap cannot prevent the oversized body from being materialized in memory.
- `src/gateway/interfaces/irc/message_ops.rs:625-627` — `reader_task.abort()` is called twice and the handle is never awaited, so the reader is not confirmed terminated before reconnection.

## Batch verdict

needs rework — the CLI stdin path is broken, and IF6-02/03/05/09/10/12/13/15 remain insufficiently or incorrectly fixed.