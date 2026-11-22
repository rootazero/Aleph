# Fix review — IF1

| ID | Verdict | Comment |
|---|---|---|
| IF1-01 | PARTIAL | The diff improves outbound account selection with “prefer an exact chat (or chat+topic) match first,” but `approval_capability` still clones only `self.bot_instances.first()` and therefore can retain the wrong bot for a multi-account approval. |
| IF1-02 | PARTIAL | The diff advances the tracker after `handler.dispatch(...)` and removes diagnostic advancement, but `let _ = handler.dispatch(...)` ignores dispatch failure and still advances the update, so failed handling can be durably acknowledged. |
| IF1-03 | PARTIAL | The diff changes messages to `format!("{}_{}", msg.chat.id.0, msg.id.0)`, but leaves poll IDs as `format!("poll_{}", q.poll_id)` and reaction IDs unchanged; voter/chat and reaction-change collisions therefore remain. |
| IF1-04 | FIXED | The diff removes the synthetic `"default"` controller with `if !config_v2.accounts.is_empty()` and adds `empty_accounts_reaches_start_config_error_without_constructor_panic`, proving empty configurations reach `start()` without constructing an invalid controller. |
| IF1-05 | FIXED | The diff replaces `instance.resolved_config.clone()` with a resolver lookup using `resolve(&instance.account_id, chat_id_i64, thread_id)`, so delivery uses the effective chat/topic configuration. |
| IF1-06 | NOT_FIXED | The adapter change explicitly clamps local access to the account default, and the diff contains no Core account/chat/topic policy-resolution change; a permissive group override remains rejected by the original Core policy reduction. |
| IF1-07 | FIXED | The diff now tracks `first_msg_id: Option<i64>`, returns the first successful provider ID, and returns `Err(err)` when every attachment fails. |
| IF1-08 | PARTIAL | The diff removes hardcoded streaming configuration and supplies `delivery.config.streaming`, but it does not address long-stream edits/sends being converted to success by `is_benign_edit_error` or implement chunked streaming finalization. |
| IF1-09 | FIXED | The diff awaits the orchestrator task and maps its result to `EventEmitError::EventBus`, so terminal Telegram finalization failures reach the producer. |
| IF1-10 | FIXED | The diff treats both terminal events as terminal and drains media after awaiting completion; the new `run_error_drains_pending_media_before_terminating` test covers the RunError path. |
| IF1-11 | FIXED | The diff changes the emitter constructor to accept `TelegramDelivery` and removes the synthetic `Bot`/`ResolvedConfig`, allowing the caller to supply the account’s live proxy-configured transport. |
| IF1-12 | FIXED | The parser now returns an error when `streaming.buffer_size == 0`, preventing `mpsc::channel(0)` from being reached. |
| IF1-13 | PARTIAL | The diff restores and resolves `block_streaming` and `LaneHandle::write_chunk` suppresses previews when disabled, but the executor/emitter-selection path is not changed to select the effective group/topic configuration, so the orchestrator can still be selected despite a disabled effective setting. |
| IF1-14 | FIXED | The diff removes the hardcoded `LinkPreviewMode::Enabled` configuration and constructs delivery from `TelegramDelivery`, with `final_text_settles_before_media_and_keeps_disabled_link_previews` asserting the disabled API option. |
| IF1-15 | FIXED | The diff removes `MessageId::new("1")` and passes `route.inbound_message_id` into `StreamOrchestrator::run`, with the reaction test asserting message ID `987`. |
| IF1-16 | FIXED | The diff awaits `task.await` before `self.media.deliver_run_media().await`, so final text settles before media is drained. |
| IF1-17 | FIXED | The diff removes `singles.push(att)` and sends each non-image immediately after flushing the preceding album, preserving mixed attachment order. |
| IF1-18 | FIXED | The diff adds `health_shutdown_tx`, cancels the health task in `stop()`, and calls `self.bot_instances.clear()`, preventing leaked health tasks and stale instance selection. |
| IF1-19 | FIXED | The diff calls `reasoning_lane.flush().await` and `reasoning_lane.finalize_streamed().await` for enabled nonempty reasoning content during RunComplete. |
| IF1-20 | PARTIAL | The diff does not modify `streaming/status_controller.rs`, so the no-op decision still compares only reaction state and does not notice a changed visible emoji in the same state. |

## New problems

- `src/gateway/interfaces/telegram/handlers.rs:161` — `MessageId::new(format!("{}_{}", msg.chat.id.0, msg.id.0))` makes normal inbound IDs non-numeric even though the finding explicitly notes numeric-ID consumers; it also does not include account identity, so identical chat/message IDs across Telegram accounts still collide.
- `src/gateway/interfaces/telegram/delivery.rs:358-399` — the edited attachment-only block contains stray duplicated `return Err(err);`/brace fragments after the new match, which makes the function’s control flow malformed and likely prevents compilation.
- `src/gateway/interfaces/telegram/delivery.rs:810-839` — the mixed-attachment loop now flushes an image album at 10 and again after the loop, but the diff leaves two consecutive `if !image_buf.is_empty()` blocks and mismatched indentation/braces, again making the function malformed.
- `src/gateway/interfaces/telegram/offset.rs:38-48` — `for_account` derives a per-account channel key by formatting the account ID into the key, but `initialize` and `advance` persist under `self.channel_id` rather than the derived per-account key, so account progress is not actually isolated in storage.
- `src/gateway/interfaces/telegram/polling.rs:77-82` — `let _ = handler.dispatch(...)` discards the handler result and then advances the durable tracker; a dispatch error is consequently acknowledged as completed, violating the finding’s “only after handler completion” requirement.
- `src/gateway/interfaces/telegram/access.rs:94-101` — clamping the adapter decision to the account default does not establish agreement with Core; it instead makes explicitly permissive group/topic overrides unusable at the adapter boundary, so the original failure mode remains.
- `src/gateway/interfaces/telegram/config_resolver.rs:57-66` — topic streaming configuration is overwritten with `topic_resolved.streaming = account_default.streaming.clone()`, discarding topic-level streaming options and preventing topic-specific effective configuration from reaching delivery.
- `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:54-68` — the media leg is drained only for `RunComplete` (`if run_ended`), despite the diff explicitly treating `RunError` as terminal; the new RunError test can therefore only pass if the media buffer is otherwise consumed, and the required RunError drain is absent.
- `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:63-67` — a terminal event whose `event_tx.send` fails still takes and awaits the orchestrator task, then may report `ChannelClosed` without draining media or distinguishing a send failure from delivery completion.
- `src/gateway/interfaces/telegram/streaming/lane_handle.rs:90-106` — the preview send/edit is performed while holding the tracker mutex, reintroducing the network-I/O-under-lock problem that the finding review identified; the added `drop(tracker)` calls were removed.
- `src/gateway/interfaces/telegram/streaming/orchestrator.rs:177-185` — reasoning finalization is performed after the reasoning lane flush but before answer finalization, while failures are returned as delivery errors; this can cause the answer to be omitted whenever the optional reasoning preview fails.
- `src/gateway/interfaces/telegram/config_resolver.rs:44-64` — `assert!` on duplicate account/group/topic keys turns malformed configuration into a constructor panic rather than a configuration error, which is an observable behavior regression.
- `src/gateway/interfaces/telegram/mod.rs:114-125` — empty accounts now leave `access_by_account` empty, but any later code that assumes a controller exists may fail; the provided test only proves `start()` returns an error, not that all construction paths remain safe.

## Batch verdict

Needs rework: several advertised fixes are incomplete or regress behavior, the Core policy mismatch remains, RunError media draining is absent, per-account offsets are not actually persisted under per-account keys, and the attachment/attachment-order edits contain malformed control flow that likely prevents compilation.