# Fix review — IF1

| ID | Verdict | Comment |
|---|---|---|
| IF1-02 | PARTIAL | The new `poll_updates` advances only after `handler.dispatch(...)`, but `OffsetTracker::for_account` creates a different key from the account’s later `advance(..., "polling")`, and `initialize("polling")` is not the account-specific initialization used by `for_account`; therefore durable per-account progress and one-time initialization are not actually connected. |
| IF1-05 | FIXED | `TelegramChannel::send` now resolves `self.config_resolver.resolve(&instance.account_id, chat_id_i64, thread_id)` instead of using `instance.resolved_config.clone()`, so topic/group-specific delivery configuration is used. |
| IF1-06 | PARTIAL | The diff adds account stamping and `inbound_config`, but the shown Core policy construction is unchanged; `inbound_config` resolves the adapter-side effective config while Core still appears to receive only the first account’s policy, so the two authorization reductions are not proven identical. |
| IF1-04 | FIXED | The empty-account branch no longer constructs the fake `"default"` controller (`if !config_v2.accounts.is_empty()`), allowing the existing start-time configuration error path to remain reachable. |
| IF1-09 | FIXED | `TelegramEventEmitter::emit` awaits the orchestrator completion handle and converts its result into `EventEmitError::EventBus`, so final Telegram send/edit failures are observable by the producer. |
| IF1-11 | FIXED | `TelegramEventEmitter::new` now accepts a `TelegramDelivery` rather than constructing a separate `Bot`/`ResolvedConfig`; the production delivery path therefore reuses the account transport instead of `Bot::new`. |
| IF1-13 | PARTIAL | Lane writes now stop previews when `!stream_cfg.enabled`, and resolver merges group/topic disable state, but the diff does not show emitter selection enforcing the effective disabled state; a feature-enabled route can still select the orchestrator and produce final delivery behavior without proving the required non-streaming fallback. |
| IF1-12 | FIXED | `parse_telegram_channel_config` returns an error when any configured account has `streaming.buffer_size == 0`, preventing the later `mpsc::channel(0)` panic at the configuration boundary. |
| IF1-16 | FIXED | `emit` now awaits `task.await` before calling `self.media.deliver_run_media().await`, so media cannot overtake final text completion. |
| IF1-17 | REGRESSION | The replacement leaves a duplicated/braced tail: after the new loop it contains `if !image_buf.is_empty() { ... } }` followed by another `if !image_buf.is_empty()`, which is malformed/braces do not align and can fail compilation; the ordering logic itself also lacks the promised immediate-send guarantee until the diff is syntactically corrected. |
| IF1-19 | FIXED | The `RunComplete` branch explicitly flushes the reasoning lane when enabled and finalizes it when it has content, before breaking. |
| IF1-14 | FIXED | `TelegramEventEmitter::new` uses `delivery.config.streaming` and no longer constructs a hardcoded `ResolvedConfig` with `LinkPreviewMode::Enabled`, so the resolved delivery configuration remains the source of preview behavior. |
| IF1-15 | FIXED | The emitter derives `inbound_message_id` from `route.inbound_message_id` and the orchestrator receives that value instead of using the hardcoded `MessageId::new("1")`. |
| IF1-03 | PARTIAL | The supplied diff contains no identifiable change corresponding to IF1-03, so the finding cannot be considered fixed. |
| IF1-07 | PARTIAL | The supplied diff contains no identifiable change corresponding to IF1-07, so the finding cannot be considered fixed. |
| IF1-10 | PARTIAL | The supplied diff contains no identifiable change corresponding to IF1-10, so the finding cannot be considered fixed. |
| IF1-18 | PARTIAL | The supplied diff contains no identifiable change corresponding to IF1-18, so the finding cannot be considered fixed. |

## New problems

- `src/gateway/interfaces/telegram/delivery.rs:821-835` — The edited `send_attachments` contains unmatched/duplicated closing braces and a duplicated final image flush, so the file does not form a valid function body and the diff cannot compile.
- `src/gateway/interfaces/telegram/offset.rs:35-58` — `OffsetTracker::for_account` derives a per-account channel key but `run_polling_loop` calls `tracker.initialize("polling")` on that same tracker, while `advance` persists with the literal bot ID `"polling"`; the intended account-scoped durable cursor is not clearly keyed or initialized consistently.
- `src/gateway/interfaces/telegram/polling.rs:61-80` — `poll_updates` ignores the handler dispatch result (`let _ = handler.dispatch(...)`), so a handler that fails cannot prevent acknowledgement; the tracker advances even when ordered processing did not complete successfully.
- `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:57-72` — A terminal event takes the completion handle before awaiting it; if `RunComplete` is sent more than once, subsequent terminal emission returns `ChannelClosed` rather than being safely ignored or diagnosed, and the first terminal event’s `forwarded` result is combined only after media delivery.
- `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:62-70` — Media is drained even when final text delivery fails (`self.media.deliver_run_media().await` precedes `completion.and(forwarded)`), so a failed final answer can still be followed by media, violating the stated “finalization failure” semantics.

## Batch verdict

needs rework — the diff contains a likely syntax error in `delivery.rs`, several findings remain only partially addressed, and polling completion/acknowledgement semantics are not safely established; it is not safe to commit.