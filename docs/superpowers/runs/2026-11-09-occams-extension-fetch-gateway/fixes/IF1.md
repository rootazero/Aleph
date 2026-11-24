# Fix brief — IF1

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/IF1.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/IF1.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/IF1.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-IF1.md

## FIX THESE (17)

### IF1-02 [P1] Duplicate account, group, and topic identifiers silently replace runtime policy
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/config_resolver.rs:34-50` (`ConfigResolver::from_v2`)
- **Failure mode**: Configure duplicate `(account.id, group.chat_id)` or `(account.id, group.chat_id, topic.thread_id)` entries → later entries overwrite earlier `HashMap` values, so effective agent, access, retry, and error policy depend on declaration order without a validation error.
- **Evidence**: `let mut lookup = HashMap::new();`; `lookup.insert((account.id.clone(), group.chat_id, None), group_resolved.clone());`; `lookup.insert((account.id.clone(), group.chat_id, Some(topic.thread_id)), topic_resolved);`
- **Suggested fix**: Detect duplicate account IDs, group chat IDs, and topic thread IDs while constructing the resolver and reject the configuration before building the map; preserve one authoritative effective policy per address.
- **Confidence**: 0.98

### IF1-03 [P1] Per-group and per-topic `block_streaming` settings are accepted but never reach delivery
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/config_v2.rs:200-223` (`TelegramGroupConfig` / `TelegramTopicConfig`) and `src/gateway/interfaces/telegram/config_resolver.rs:92-150` (`merge_group` / `merge_topic`)
- **Failure mode**: Set `block_streaming: true` on a group or topic → resolver drops the field because `ResolvedConfig` has no corresponding value and both merge functions always copy `base.streaming`, so the configured chat still receives streaming output.
- **Evidence**: `pub block_streaming: Option<bool>,`; `streaming: base.streaming.clone(),` in both merge functions; `pub streaming: StreamingOptions,` with no resolved block flag.
- **Suggested fix**: Either remove the dead configuration field (if unsupported) or carry one effective block decision through `ResolvedConfig` and honor it at the emitter-selection boundary; preserve account defaults and topic-over-group precedence.
- **Confidence**: 0.99

### IF1-04 [P1] Invalid proxy configuration silently bypasses the configured network route
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/bot_instance.rs:10-38` (`create_bot_with_proxy`)
- **Failure mode**: A configured proxy is syntactically unusable or the proxy HTTP client cannot be built → the adapter logs a warning and constructs `Bot::new(token)`, causing Telegram traffic to go direct instead of failing closed on the operator-selected route.
- **Evidence**: `"Failed to build HTTP client with proxy; falling back to no proxy"`; `"Invalid proxy URL; falling back to no proxy"`; both branches return `Bot::new(token)`.
- **Suggested fix**: Return the proxy construction error from bot/account startup, or make the direct fallback an explicit opt-in; preserve the configured proxy as a routing invariant.
- **Confidence**: 0.98

### IF1-05 [P1] Approval completion reports success when the confirmation message was not delivered
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/approval.rs:129-143` (`TelegramChannelApprovalCapability::resolve_approval`)
- **Failure mode**: Telegram rejects the approval result update → the send error is discarded and `resolve_approval` returns `Ok(())`, so the approval ledger can report completion while the user never receives the decision.
- **Evidence**: `let _ = self.channel.send(update).await;`; `let _ = msg_id;`; `Ok(())`
- **Suggested fix**: Propagate the channel send error (or return a distinct delivery failure while retaining the already-made decision); preserve approval state while making user-visible delivery failure actionable.
- **Confidence**: 0.99

### IF1-06 [P1] Attachment-only sends convert total delivery failure into message ID `0` success
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/delivery.rs:359-375` (`send_message`)
- **Failure mode**: Send any voice/media-only outbound message and have Telegram reject every attachment → the loop only logs warnings, then returns `Ok(SendResult { message_id: "0" })`, allowing callers to treat a dropped user-visible message as delivered.
- **Evidence**: `if let Err(e) = result { tracing::warn!("Failed to send voice attachment: {}", e); }`; `first_msg_id = Some("0".to_string());`; `return Ok(SendResult { ... })`
- **Suggested fix**: Return the first attachment error when no attachment succeeds and only return a real Telegram message ID after a successful send; preserve partial-send reporting for mixed outcomes.
- **Confidence**: 0.99

### IF1-07 [P1] `AdminOnly` error policy is implemented as unrestricted `Once`
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/error_cooldown.rs:239-271` (`ErrorCooldown::should_send_error`)
- **Failure mode**: Delivery calls `should_send_error` for a policy configured as `AdminOnly` without an admin identity → the method returns `true` exactly like `Once`, so an ordinary chat can receive an error that the policy intended to expose only to an administrator.
- **Evidence**: `ErrorPolicyMode::AdminOnly => {`; `// AdminOnly: same as Once but only for admin users`; `true` with no identity or authorization input.
- **Suggested fix**: Move the admin authorization decision to the caller with an explicit authorized/admin argument and make this helper deny by default when that fact is absent; preserve `Once` throttling independently.
- **Confidence**: 0.97

### IF1-09 [P1] Poll answers and reactions bypass the Telegram access policy
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/mod.rs:661-701` (`poll_handler`) and `src/gateway/interfaces/telegram/mod.rs:705-717` (`reaction_handler`)
- **Failure mode**: An unauthorized user submits a poll answer or reacts in a group → these handlers construct and enqueue `InboundMessage` directly without `AccessController::check_message` (or an equivalent router-side channel identity gate), so the event can start an agent turn despite message/callback allowlists.
- **Evidence**: Poll path directly builds `InboundMessage` and executes `let _ = inbound_tx.send(inbound);`; reaction path directly executes `if let Some(inbound) = reaction_handler::convert_reaction(...) { let _ = inbound_tx.send(inbound); }`.
- **Suggested fix**: Apply the same account-aware access classification before enqueueing these update kinds, and fail closed when an update cannot identify an authorized chat/user; preserve poll/reaction metadata and topic routing.
- **Confidence**: 0.98

### IF1-10 [P1] Stopping the channel leaves per-account health tasks running forever
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/mod.rs:745-780` (`TelegramChannel::start`) and `src/gateway/interfaces/telegram/mod.rs:787-798` (`TelegramChannel::stop`)
- **Failure mode**: Start succeeds and then `stop()` is called → `stop` signals only each polling loop, while the separately spawned health-check loop has no cancellation handle and loops forever, continuing `get_me()` calls and retaining bot/channel state after shutdown.
- **Evidence**: `tokio::spawn(async move { let mut interval = tokio::time::interval(...); loop { interval.tick().await; match ... bot.get_me() ... } });`; `stop()` only sends `shutdown_tx`.
- **Suggested fix**: Give the health task a cancellation channel/token owned by `BotInstance`, signal it from `stop` and failed startup cleanup, and join/await task termination where lifecycle semantics require it; preserve health probing while running.
- **Confidence**: 0.99

### IF1-11 [P1] Finalizing a lane holds the shared tracker mutex across Telegram I/O
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/streaming/lane_handle.rs:152-166` (`LaneHandle::finalize`)
- **Failure mode**: A lane has no preview and Telegram send is slow or hangs → `self.tracker` remains locked across `send_text_message().await`, blocking all other lanes and stream state operations for the network request.
- **Evidence**: `let mut tracker = self.tracker.lock().await;`; `let id = self.delivery.send_text_message(&text).await?;`; only afterward updates `state.final_message_id`.
- **Suggested fix**: Copy the needed lane state, release the mutex before the send, then reacquire it to commit the result; preserve serialized tracker updates and avoid publishing a message ID after a failed send.
- **Confidence**: 0.99

### IF1-12 [P1] Streaming run errors ignore the configured error policy
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/streaming/orchestrator.rs:179-185` (`StreamOrchestrator::run`)
- **Failure mode**: A run emits `StreamEvent::RunError` while the effective policy is `Silent` or `AdminOnly` → the orchestrator unconditionally writes `Error: ...` to the answer lane, exposing a user-visible error that ordinary delivery would suppress.
- **Evidence**: `StreamEvent::RunError { error, .. } => {`; `let error_msg = format!("Error: {error}");`; `let _ = answer_lane.write_chunk(&error_msg).await;` and `finalize`.
- **Suggested fix**: Apply the effective policy before writing the error (and fail closed when admin authorization is unavailable), while preserving finalization and stream termination.
- **Confidence**: 0.98

### IF1-13 [P1] Orchestrated streaming is constructed with synthetic delivery configuration
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:43-64` (`TelegramEventEmitter::new`) and `src/gateway/inbound_router/executor.rs:683-725` (`try_create_telegram_emitter`)
- **Failure mode**: A group/topic uses non-default error, retry, link-preview, or cooldown settings and orchestrated streaming is selected → the emitter constructs `ResolvedConfig` with account ID `"telegram"`, empty token, default policies, and a fresh cooldown, so streaming ignores the effective per-chat delivery configuration and maintains separate failure state.
- **Evidence**: `account_id: "telegram".to_string(),`; `bot_token: String::new(),`; `error_policy: Default::default(), max_retries: 3,`; `Arc::new(ErrorCooldown::new())`; the caller passes only `streaming` into `TelegramEventEmitter::new`.
- **Suggested fix**: Resolve and pass the same account/chat/topic `ResolvedConfig` and shared `ErrorCooldown` used by normal delivery; preserve the single per-chat policy and cooldown owner.
- **Confidence**: 0.99

### IF1-14 [P1] Status reactions target Telegram message ID `1` instead of the inbound message
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:75-90` (`TelegramEventEmitter::new`)
- **Failure mode**: Status reactions are enabled for any inbound message whose ID is not `1` → the orchestrator's synthetic `InboundMessage` parses ID `1`, so every lifecycle reaction is sent to message 1 (or fails) rather than the message that started the run.
- **Evidence**: `id: MessageId::new("1"),`; `tokio::spawn(orchestrator.run(inbound));`; `run` derives `inbound_message_id` from `initial_inbound.id`.
- **Suggested fix**: Pass the actual inbound Telegram message ID into the emitter and construct the synthetic context with it; preserve status reaction behavior and avoid inventing a second message identity.
- **Confidence**: 0.99

### IF1-15 [P1] Media can overtake final streamed text
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:102-117` (`TelegramEventEmitter::emit`)
- **Failure mode**: `RunComplete` is sent into the bounded channel but the spawned orchestrator has not yet processed/finalized it → `deliver_run_media()` runs immediately, allowing attachments to arrive before the final text edit and producing observable output reordering.
- **Evidence**: `let forwarded = self.event_tx.send(event).await...;`; `if run_ended { self.media.deliver_run_media().await; }`; no acknowledgement from the orchestrator that `finalize` completed.
- **Suggested fix**: Acknowledge completion only after the orchestrator finalizes the answer, then drain media, or have one ordered owner perform both operations; preserve the no-double-text media leg.
- **Confidence**: 0.98

### IF1-16 [P1] Multi-account outbound routing always selects the first account fallback
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/mod.rs:801-820` (`TelegramChannel::send`)
- **Failure mode**: Configure multiple accounts, with a chat explicitly mapped only under a later account → every account has an account-level `(chat_id=0)` resolver fallback, so `find` accepts the first instance before checking the later explicit mapping and sends through the wrong bot.
- **Evidence**: `let instance = self.bot_instances.iter().find(|inst| { self.config_resolver.resolve(&inst.account_id, chat_id_i64, thread_id).is_some() });`; `resolve` falls through to `.lookup.get(&(account_id.to_string(), 0, None))`.
- **Suggested fix**: Resolve the most-specific chat/topic mapping across accounts before applying account fallbacks, or require an explicit account route for ambiguous multi-account sends; preserve deterministic single-account fallback behavior.
- **Confidence**: 0.99

### IF1-17 [P1] Multi-account approval capability is permanently bound to the first bot
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/telegram/mod.rs:1020-1055` (`TelegramChannel::approval_capability`)
- **Failure mode**: An approval belongs to a conversation served by a later Telegram account → the capability clones only `self.bot_instances.first()` and constructs a one-account channel, so the approval card/result is delivered through the first bot and its access policy.
- **Evidence**: `let first = self.bot_instances.first()?;`; `bot_instances: vec![instance],`; `access_by_account: HashMap::from([(first.account_id.clone(), first_access.clone())])`.
- **Suggested fix**: Bind approval delivery to the account encoded by the pending conversation/route, or expose a per-account capability lookup; preserve the authorization check and approval callback contract.
- **Confidence**: 0.98

### IF1-18 [P2] Concurrent `Once` error checks are not atomic
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/telegram/error_cooldown.rs:248-257` (`ErrorCooldown::should_send_error`)
- **Failure mode**: Two concurrent failures for the same `scope_key` both execute the `get` before either `insert` → both return `true` and both emit an error despite `Once` promising one error per 60 seconds.
- **Evidence**: `if let Some(last) = self.error_policy_cooldowns.get(scope_key) { ... }`; followed by `self.error_policy_cooldowns.insert(scope_key.to_owned(), now);`.
- **Suggested fix**: Use an atomic entry/update operation or a small lock around the check-and-record pair; preserve per-scope 60-second expiry and avoid serializing unrelated scopes.
- **Confidence**: 0.97

### IF1-19 [P2] Failed first streaming preview consumes the throttle window
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/telegram/streaming/lane_handle.rs:83-100` (`LaneHandle::write_chunk`)
- **Failure mode**: The first preview send fails → `last_update` was set before the network call but `preview_message_id` remains `None`, so subsequent deltas are withheld until the debounce interval and the eventual retry may send a much larger accumulated message instead of retrying promptly.
- **Evidence**: `state.last_update = std::time::Instant::now();`; `let id = self.delivery.send_text_message(&text).await?;`; `state.preview_message_id = Some(id)` only after success.
- **Suggested fix**: Commit the throttle timestamp only after a successful first send, or explicitly clear/reset it on failure; preserve first-preview coalescing after successful delivery.
- **Confidence**: 0.95


## DO NOT FIX (3) — refuted / deferred by adjudication or routing

- IF1-01: ### IF1-01 [P1] Empty-account construction panics instead of producing the documented deny-all channel
- IF1-08: ### IF1-08 [P1] Boot diagnostic `getUpdates` consumes an update without dispatching it
- IF1-20: ### IF1-20 [P2] Offset persistence failure causes restart reprocessing after an acknowledged update
