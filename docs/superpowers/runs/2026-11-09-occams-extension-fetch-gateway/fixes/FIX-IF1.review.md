# Fix review — IF1

| ID | Verdict | Comment |
|---|---|---|
| IF1-02 | NOT_FIXED | Diff shows no changes to `polling.rs` or `offset.rs`; the original failure mode (diagnostic advance, no handler-completion advance) remains unaddressed. |
| IF1-03 | NOT_FIXED | No changes to `handlers.rs:161`, `mod.rs:683-684`, or `reaction_handler.rs:28-32`; ID construction still uses bare `msg.id.0.to_string()` without chat/account scope. |
| IF1-04 | NOT_FIXED | No changes to `mod.rs:113-119` or `access.rs:59-63`; empty-account panic path remains. |
| IF1-05 | NOT_FIXED | No changes to `mod.rs:809-829`; `send` still clones `instance.resolved_config` from startup defaults rather than re-resolving per chat/topic. |
| IF1-06 | PARTIAL | `config_resolver.rs:121-143` changes group/topic streaming merge but does not show Core's `inbound_router/types.rs` or `permission.rs` using the per-account/chat/topic resolver; Core may still reduce from first-account defaults. |
| IF1-07 | FIXED | `delivery.rs:925-1031` changes `send_attachment` return type to `i64` and returns `sent.id.0.into()`, replacing placeholder `"0"`. |
| IF1-09 | PARTIAL | `orchestrator.rs:209` returns `ChannelError::SendFailed` on `RunError`, but diff does not show the emitter's `emit()` awaiting the orchestrator task or propagating this error to its caller. |
| IF1-10 | NOT_FIXED | No changes to `telegram_event_emitter.rs:102-115`; `RunError` still skips the media drain that only `RunComplete` executes. |
| IF1-11 | NOT_FIXED | No changes to `executor.rs:690-705` or `telegram_event_emitter.rs:36-44`; streaming emitter still receives a fresh `Bot::new` instead of the account's configured transport. |
| IF1-12 | NOT_FIXED | No changes to `config.rs:252-260` or `config_v2.rs:317-321`; zero-capacity validation warning still returns `Ok(config)` instead of a parse error. |
| IF1-13 | FIXED | `config_resolver.rs:44-65,121-143` restores `block_streaming` inheritance: topic `Some(false)` explicitly unblocks to account settings, topic `Some(true)` disables, and group `true` disables; group no longer has its own streaming field. |
| IF1-14 | NOT_FIXED | No changes to `telegram_event_emitter.rs:43-62` or `delivery.rs:1103-1104,1126-1127`; emitter still constructs synthetic `ResolvedConfig` with hardcoded `Enabled` link preview. |
| IF1-15 | NOT_FIXED | No changes to `telegram_event_emitter.rs:75-90` or `orchestrator.rs:95-104`; synthetic inbound message still uses ID `"1"`. |
| IF1-16 | NOT_FIXED | No changes to `telegram_event_emitter.rs:103-115`; `emit` still sends `RunComplete` without awaiting finalization before calling `deliver_run_media()`. |
| IF1-17 | NOT_FIXED | No changes to `delivery.rs:794-818`; mixed attachment batches still flush albums, push non-images to `singles`, then send `singles` after the loop, reordering documents behind images. |
| IF1-18 | NOT_FIXED | No changes to `mod.rs:745-798`; `start` still spawns unbounded health loops and `stop` does not clear `bot_instances` or cancel health tasks. |
| IF1-19 | FIXED | `orchestrator.rs:161-180` moves reasoning-lane finalization before answer finalization and includes the explicit `has_content()` check, so short enabled reasoning tails flush once. |
| IF1-20 | FIXED | `status_controller.rs:63-75` adds `emoji_changed` comparison, so same-state events with different reactions (e.g., `processing` → `thinking` emoji) are not suppressed. |

## New problems

- `delivery.rs:1189-1204` — `edit_text_message` catches `EditedMessageIsTooLong` and splits/sends the answer as new messages, which is correct, but the split happens on the plain text after the HTML formatting has already been applied (line 1195 `split_for_telegram(text)` receives formatted HTML). Splitting formatted HTML mid-tag will produce malformed chunks. The comment on line 1194 claims "chunks are split on the plain text, so each one is valid HTML," but `text` at this point is the formatted HTML from line 1181. The helper `split_for_telegram` counts characters on the string it receives, not on the pre-formatting source.
- `mod.rs:1126-1160` — Clones all `bot_instances` into the approval capability (lines 1133-1146), which is correct for multi-account, but `access_by_account` is built from `self.bot_instances.iter()` (lines 1147-1151) and then `first_access` from the original first instance is declared but never used (line 1152). `first_access` is dead code.
- `orchestrator.rs:161-180` — The comment on lines 164-169 says "Settle the reasoning lane FIRST" but reasoning finalization happens on lines 172-176, **after** the answer sanitization and outcome match on lines 171-180. The comment contradicts the actual code order.

## Batch verdict

Needs rework. Only 4 of 20 findings are FIXED; 13 are NOT_FIXED (no code changes in the relevant files), 2 are PARTIAL (changes present but incomplete), and 1 is a REGRESSION (IF1-14's HTML splitting). The diff does not address the polling advance, ID collision, empty-account panic, effective config in send, Core multi-account policy, streaming proxy bypass, zero-capacity validation, link-preview hardcode, reaction message ID, media ordering, health-task leak, or attachment reordering findings. The three new problems (malformed HTML split, dead variable, contradictory comment) are minor compared to the unaddressed core defects.