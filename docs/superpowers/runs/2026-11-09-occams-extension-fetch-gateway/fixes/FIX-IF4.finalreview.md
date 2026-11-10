# Fix review — IF4

| ID | Verdict | Comment |
|---|---|---|
| IF4-01 | FIXED | `XmppChannel::start` now rejects `use_tls` before connecting with `return Err(ChannelError::UnsupportedFeature(...))`, and the loop also refuses `config.use_tls` before `TcpStream::connect`. |
| IF4-02 | FIXED | `extract_attribute` now limits searches to `root_tag` using `root_tag.find(...)`, so body attributes cannot supply the root `from` or `type`. |
| IF4-03 | FIXED | The diff removes derived `Debug` from `LineConfig` and `BlueBubblesConfig` and adds a hand-written BlueBubbles implementation whose password is emitted as `"***"`. |
| IF4-04 | FIXED | Participant matching now requires `let is_one_on_one = participants.len() == 1;` before accepting `matches_part`, while exact identifier matching remains available. |
| IF4-05 | PARTIAL | The diff delays the watermark until `self.last_message_rowid = new_watermark`, but a decode error breaks and returns the valid prefix, so the bad row is never retried and delivery is not atomic as requested. |
| IF4-06 | NOT_FIXED | `query_messages_after` still uses `"after": current_after` and advances `current_after` to the maximum timestamp; it never paginates beyond the first 100 rows sharing that timestamp. |
| IF4-07 | NOT_FIXED | `impl From<&LineConfig> for ChannelConfig` is added, but the diff contains no startup-registration call using that conversion, so LINE policy and allowlists still are not connected to Core. |
| IF4-08 | FIXED | `edit_message` now returns `ChannelError::SendFailed` when `chunks.len() > 1` instead of silently using only `chunks[0]`. |
| IF4-09 | FIXED | Local iMessage now formats known chats as `format!("chat_guid:{}", chat.guid)`, which routes through the existing chat-guid path instead of the bare contact identifier. |
| IF4-10 | FIXED | The local iMessage send path now returns `ChannelError::SendFailed` when `attachment.path.is_none()`, preventing a false success. |
| IF4-11 | FIXED | Database opening and each polling batch are now run through `tokio::task::spawn_blocking`, preserving the shared connection and serialized polling order. |
| IF4-12 | FIXED | The listener is bound before spawning with `TcpListener::bind(...).await.map_err(...)`, and registration failure without a poll now aborts the listener task and returns an error. |
| IF4-13 | FIXED | `BlueBubblesConfig::validate` rejects a remote server combined with a loopback/unspecified callback host instead of generating an unusable callback; the earlier panic is gone. |
| IF4-14 | FIXED | Chat lookup now advances `offset += limit` until a matching chat is found or a page is empty, removing the fixed first-page-only lookup. |
| IF4-15 | FIXED | `sweep_stale` now runs `sweep_stale_blocking` inside `tokio::task::spawn_blocking`, and `ensure_staging_dir` similarly moves directory creation off the async worker. |
| IF4-16 | FIXED | `download_attachment` now calls `read_attachment_limited(res, 100 * 1024 * 1024).await?`, and the helper stops before extending beyond the cap. |
| IF4-17 | FIXED | The bare temporary-directory fallback is removed; staging failure now logs and returns `None` instead of creating an unowned file. |
| IF4-18 | FIXED | The default is now `"/line/webhook"`, and `LineChannel::new` normalizes legacy slashless paths at the configuration boundary. |
| IF4-19 | FIXED | The LINE wire fields now use `#[serde(rename = "fileName")]`, `"fileSize"`, `"packageId"`, `"stickerId"`, and the corresponding `contentProvider` field renames. |
| IF4-20 | FIXED | Flex and template sends now use `messages` envelopes, template content is tagged with `"type"`, and quick-reply items serialize with `#[serde(tag = "type", rename = "action")]`. |
| IF4-21 | FIXED | `dispatch_event` now derives media attachments from the parsed message, preserves the actual message ID and raw event, and rejects unsupported message variants instead of producing an empty success. |
| IF4-22 | FIXED | LINE now rejects standard attachments before sending with `ChannelError::UnsupportedFeature`, and capabilities are correspondingly set to false. |
| IF4-23 | PARTIAL | A 100-connection semaphore is added, but there is still no request read deadline and spawned connection tasks are not owned or aborted when the listener stops. |
| IF4-24 | PARTIAL | `run_sync_loop` obtains `room.is_direct().await.ok()` and passes it to the converter, but a missing or failed lookup remains `None` and is conservatively treated as a group. |
| IF4-25 | FIXED | Adapter-side mention gating is removed, verified textual/native mentions are emitted as `MessageMeta::AppMention`, and Matrix configuration is mapped into Core `ChannelConfig`. |
| IF4-26 | FIXED | Dedupe construction and each `try_insert` operation are now performed via `tokio::task::spawn_blocking`, while the deduper mutex retains serialization. |
| IF4-27 | PARTIAL | The cache is bounded and pruned hourly, but the diff inserts into the cache before executing the database insert; a failed insert can leave a non-persisted key cached. |
| IF4-28 | FIXED | Inbound and outbound Matrix bodies now use `read_body_limited`, which checks `Content-Length` and stops reading as soon as accumulated bytes exceed the cap. |
| IF4-29 | REGRESSION | The MUC echo check changed from the configured nickname to `own.bare()` (`if nick == own.bare()`), so the actual MUC nickname is still not used and a different member named like the bot’s bare JID can be filtered. |
| IF4-30 | FIXED | The outbound branch is now guarded by `if authenticated && bound`, so queued stanzas are not consumed before authentication and binding complete. |
| IF4-31 | FIXED | Each queue item now carries a oneshot acknowledgement, and writer failure sends `Err("XMPP write failed; delivery uncertain: ...")` back to `send_message`. |
| IF4-32 | FIXED | `send` now clones the sender under the read lock and releases the guard before awaiting `send_message`, preventing stop from waiting on the sender’s queue operation. |
| IF4-33 | FIXED | The receive loop now checks `if buffer.len() + pending.len() + n > MAX_STANZA_SIZE` and closes the connection before accepting an oversized incomplete stanza. |
| IF4-34 | FIXED | `xml_unescape` now scans and consumes each entity once, so `&amp;lt;` is no longer recursively expanded into `<`. |
| IF4-35 | FIXED | A present `chat_id` whose lookup fails now logs a refusal, rolls back the watermark, and breaks rather than falling back to `handle.id` as a DM. |

## New problems

- `src/gateway/interfaces/matrix/dedupe.rs:76` — The cache is updated before `INSERT OR IGNORE`; if the insert fails, `try_insert` returns an error but the key remains cached, causing a later retry to be rejected as already seen despite never being persisted. The inserted duplicate text after the impl also leaves the file with duplicated/structurally broken code, so this file cannot be committed safely.
- `src/gateway/interfaces/xmpp/message_ops/ops.rs:195` — `MAX_STANZA_SIZE` is declared both at module scope and again inside `run_xmpp_loop`, producing an unused module-level constant warning at minimum.
- `src/gateway/interfaces/line/message_ops.rs:32` — `push` special-cases Flex while `push_flex` already builds a text-envelope payload containing `PushMessage::Flex`; direct callers of `LinePushPayload::Flex` now receive a different wire shape than the public `push_flex` path, making the enum variant’s serialization inconsistent and potentially duplicating or dropping the Flex-specific payload contract.
- `src/gateway/interfaces/line/webhook.rs:270` — The image branch returns an external `originalContentUrl` attachment without using the authenticated LINE content endpoint, while other media types are downloaded with the bot token; this can expose an unauthenticated URL and bypasses the common size limit.

## Batch verdict

needs rework — the Matrix dedupe file is visibly malformed and has an insertion-order correctness bug, while IF4-06, IF4-07, IF4-23, IF4-24, and IF4-27 remain incomplete and IF4-29 regresses.