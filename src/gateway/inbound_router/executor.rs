//! Agent execution for inbound messages
//!
//! Collapsed from the original two near-identical methods
//! (`execute_for_context` / `execute_for_context_with_metadata`)
//! into a single parameterized implementation.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{error, info};
use uuid::Uuid;

use crate::gateway::execution_engine::RunRequest;
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::reply_emitter::{ReplyEmitter, ReplyEmitterConfig};

use super::types::{ChannelConfig, RoutingError, SLASH_COMMAND_MODE_KEY};
use super::InboundMessageRouter;

/// Resolve the run identity a channel's inbound messages execute under:
/// the `caller_role` fed to the tool-dispatch config-tier gate, and the
/// workspace it is locked into (Layer-1 lock for Chat tier).
///
/// An **unconfigured** channel (`None` in the map) defaults to Chat
/// (`"guest"`) with no locked workspace. This default is the over-permission
/// fix — a missing config must never be treated as operator. The
/// `permission_wiring_tests` below pin exactly this.
/// Returns `(caller_role, locked_workspace, busy_input_mode_wire,
/// tool_permissions)`. The third element is the channel's busy-input policy
/// wire string (`"steer"` default / `"interrupt"` / `"queue"`), the fourth the
/// channel's tool permission override — both stamped into run metadata so the
/// execution engine dispatches without re-reading channel config.
fn channel_run_identity(
    configs: &HashMap<String, ChannelConfig>,
    channel_id: &str,
) -> (
    &'static str,
    Option<PathBuf>,
    &'static str,
    Option<crate::config::types::policies::ToolPermissionsConfig>,
) {
    let cfg = configs.get(channel_id).cloned().unwrap_or_default();
    (
        cfg.caller_role_str(),
        cfg.resolved_default_workspace(),
        cfg.busy_input_mode_wire(),
        cfg.tool_permissions,
    )
}

impl InboundMessageRouter {
    /// Execute the agent for the given context
    pub(super) async fn execute_for_context(
        &self,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        self.execute_for_context_inner(ctx, None).await
    }

    /// Execute the agent with slash command metadata
    pub(super) async fn execute_for_context_with_metadata(
        &self,
        ctx: &InboundContext,
        slash_command_mode: String,
    ) -> Result<(), RoutingError> {
        self.execute_for_context_inner(ctx, Some(slash_command_mode))
            .await
    }

    /// Unified execution implementation
    async fn execute_for_context_inner(
        &self,
        ctx: &InboundContext,
        slash_command_mode: Option<String>,
    ) -> Result<(), RoutingError> {
        // Check if execution support is configured
        let (agent_registry, execution_adapter) = match (
            self.agent_registry.as_ref(),
            self.execution_adapter.as_ref(),
        ) {
            (Some(ar), Some(ea)) => (ar.clone(), ea.clone()),
            _ => {
                info!(
                    "Would execute agent for session {} with input: {} (execution not configured)",
                    ctx.session_key.to_key_string(),
                    ctx.message.text.chars().take(100).collect::<String>()
                );
                return Ok(());
            }
        };

        // Get the agent ID from the session key
        let agent_id = ctx.session_key.agent_id();

        // Look up the agent in the registry
        let agent = agent_registry
            .get(agent_id)
            .await
            .ok_or_else(|| RoutingError::AgentNotFound(agent_id.to_string()))?;

        // Generate a unique run ID
        let run_id = Uuid::new_v4().to_string();

        // Determine voice state for this channel
        let voice_state = self
            .channel_registry
            .get_voice_state(ctx.reply_route.channel_id.as_str())
            .await;
        let voice_enabled = voice_state.is_active() || ctx.voice_reply_hint;

        // Create a ReplyEmitter config based on output_mode
        let mut reply_config = match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                let mode = cfg
                    .behavior
                    .as_ref()
                    .map_or("typewriter", |b| b.output_mode.as_str());
                let mut config = ReplyEmitterConfig::from_output_mode(mode);
                // Same config read, same derivation as the `metadata["locale"]`
                // stamp below (`general.language` -> `Locale::from_config`), so
                // the halt tag this emitter appends and the halt paragraph
                // `run_loop::inner` renders from that stamp cannot disagree
                // about which language this reply is in. Resolved here rather
                // than from the request for the ordering reason `side_answer`
                // documents: the `RunRequest` does not exist yet.
                config.locale =
                    crate::gateway::i18n::Locale::from_config(cfg.general.language.as_deref());
                config
            }
            None => ReplyEmitterConfig::default(),
        };
        reply_config.voice_enabled = voice_enabled;
        reply_config.voice_reply_hint = ctx.voice_reply_hint;
        // Pull the process-wide runtime-footer config installed at boot.
        // Defaults to disabled when never installed (tests, host-only paths).
        reply_config.footer = crate::gateway::runtime_footer::global_config();

        // A side answer arrives in the same conversation as the main run's
        // replies and — by design — does not queue behind them, so it can land
        // between two of them. Mark it here so that is legible.
        //
        // Resolved from the ONE resolver, at construction, because that is the
        // only moment this side of the wire can answer the question:
        // `execute()` stamps `BTW_METADATA_KEY` on the `RunRequest`, and that
        // request is built below and never comes back. This is the same
        // derivation `stamp_btw` makes from the same bytes
        // (`request.input == ctx.message.text`), not a second one — and it must
        // stay a call to `BtwTurn::resolve`, never a `/btw` prefix test of its
        // own. Set before the clones below so the Feishu and Telegram emitters
        // inherit it.
        //
        // `/btw promote` resolves too, and is deliberately NOT marked. The
        // badge's whole meaning is "this reply is not part of your main
        // conversation"; a promote's reply announces something ENTERING it, and
        // the crossing itself has already landed in this very transcript. That
        // question used to be deferred here — "if promotion ever delivers INTO
        // the main conversation, that delivery owes its own decision" — and
        // this is the decision, taken when promotion became a served verb
        // rather than a side turn. The predicate reads the resolver's own
        // `promote` field, never a second string test.
        reply_config.side_answer = crate::gateway::btw::BtwTurn::resolve(&ctx.message.text)
            .is_some_and(|turn| !turn.promote);

        // Reconcile the global `output_mode` preference with what this channel
        // can physically do: EditBased widens, `editing` floors. The floor is
        // the half that was missing — see `apply_channel_capabilities`.
        if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
            let ch = handle.read().await;
            reply_config.apply_channel_capabilities(ch.capabilities());
        }

        let pending_media: crate::gateway::media::PendingMedia =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

        // Detect feishu/telegram channels and optionally construct custom emitters
        let (is_feishu, is_telegram) = {
            if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
                let ch = handle.read().await;
                (
                    ch.channel_type() == "feishu",
                    ch.channel_type() == "telegram",
                )
            } else {
                (false, false)
            }
        };

        // Always attach voice deps so that mid-request voice_mode_set
        // tool calls can take effect immediately (dynamic should_voice check)
        let attach_voice = |emitter: ReplyEmitter| -> ReplyEmitter {
            if let (Some(gen_reg), Some(gen_cfg)) = (
                self.generation_registry.as_ref(),
                self.generation_config.as_ref(),
            ) {
                return emitter.with_voice(voice_state.clone(), gen_reg.clone(), gen_cfg.clone());
            }
            emitter
        };

        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            if is_feishu {
                // Try to create FeishuEventEmitter with streaming + typing
                match self
                    .try_create_feishu_emitter(
                        ctx,
                        &run_id,
                        reply_config.clone(),
                        pending_media.clone(),
                    )
                    .await
                {
                    Some(fe) => Arc::new(fe),
                    None => {
                        let re = ReplyEmitter::with_config(
                            self.channel_registry.clone(),
                            ctx.reply_route.clone(),
                            run_id.clone(),
                            reply_config,
                            pending_media.clone(),
                        );
                        Arc::new(attach_voice(re))
                    }
                }
            } else if is_telegram {
                // Capture the global output_mode switch before reply_config is
                // moved into the fallback ReplyEmitter below.
                let stream_enabled = reply_config.stream_enabled;
                // Try to create Telegram orchestrated emitter
                match self
                    .try_create_telegram_emitter(
                        ctx,
                        &run_id,
                        reply_config.clone(),
                        pending_media.clone(),
                    )
                    .await
                {
                    Some(te) => {
                        if stream_enabled {
                            Arc::new(te)
                        } else {
                            // output_mode = "instant": the orchestrated emitter
                            // streams independently of ReplyEmitter, so wrap it
                            // to buffer chunks into a single final message —
                            // keeping Telegram in sync with the global switch.
                            Arc::new(crate::gateway::event_emitter::InstantBufferingEmitter::new(
                                te,
                            ))
                        }
                    }
                    None => {
                        let re = ReplyEmitter::with_config(
                            self.channel_registry.clone(),
                            ctx.reply_route.clone(),
                            run_id.clone(),
                            reply_config,
                            pending_media.clone(),
                        );
                        Arc::new(attach_voice(re))
                    }
                }
            } else {
                let re = ReplyEmitter::with_config(
                    self.channel_registry.clone(),
                    ctx.reply_route.clone(),
                    run_id.clone(),
                    reply_config,
                    pending_media.clone(),
                );
                Arc::new(attach_voice(re))
            };

        // Build the run request metadata
        let mut metadata = HashMap::new();
        metadata.insert(
            "channel_id".to_string(),
            ctx.message.channel_id.as_str().to_string(),
        );
        metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
        // Raw (un-normalized) originating sender id — the approval "originator".
        // The channel button-approval callback compares the clicker's RAW id
        // against this, so unlike `sender_id` above it must NOT be normalized;
        // it lets a pending approval refuse resolution from anyone but the person
        // who triggered it (closes the group-chat approval bypass). Consumed via
        // `TURN_ORIGINATOR` → the channel approval bridge → the record.
        metadata.insert(
            crate::gateway::execution_engine::ORIGINATOR_USER_KEY.to_string(),
            ctx.message.sender_id.as_str().to_string(),
        );
        metadata.insert(
            "conversation_id".to_string(),
            ctx.message.conversation_id.as_str().to_string(),
        );
        if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
            let channel = handle.read().await;
            metadata.insert("platform".to_string(), channel.channel_type().to_string());
        }

        // Inject user locale for downstream i18n (run_loop, error messages)
        if let Some(ref cfg) = self.app_config {
            let cfg = cfg.read().await;
            let lang = cfg.general.language.as_deref().unwrap_or("zh");
            metadata.insert("locale".to_string(), lang.to_string());
        }
        // Stamp this channel's permission tier as the run's caller_role so the
        // tool-dispatch config gate (tools/scoped/dispatch.rs) applies uniformly
        // to external-channel messages. Unconfigured channels default to Chat
        // ("guest") — closing the prior over-permission where a missing role was
        // treated as operator. An operator opts a channel up to Config tier via
        // `permission_level = "config"` in its config block.
        let (caller_role, channel_workspace, busy_input_mode, channel_tool_permissions) =
            channel_run_identity(&self.channel_configs, ctx.message.channel_id.as_str());
        metadata.insert("caller_role".to_string(), caller_role.to_string());
        // P1 data isolation: stamp the run's owner/scope attribution from the
        // P0 sender→user link (`pairing_store`), not any task-local — channel
        // dispatch runs outside `process_request`'s task tree. An unlinked
        // peer (`None`) stamps nothing — legacy owner semantics.
        //
        // The same resolved principal is ALSO this turn's author. Scope names
        // the room, author names whoever is typing, and in a group they differ:
        // without the second stamp `run_loop::with_request_scope` falls back to
        // the session owner, so the guard audit's `actor_user` and
        // `nudges::speaker_label` both name whoever spoke FIRST in that group
        // rather than the person who just spoke. A DM is accidentally correct
        // (owner == speaker), which is why only groups were wrong.
        if let Some(user) = self
            .pairing_store
            .sender_user(ctx.message.channel_id.as_str(), &ctx.sender_normalized)
            .await
        {
            crate::scope::stamp_metadata(
                &mut metadata,
                &crate::scope::ScopeAttribution::personal(&user),
            );
            metadata.insert(
                crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
                user,
            );
        }
        // Stamp the channel's busy-input policy so the execution engine's busy
        // branch knows whether a message arriving mid-run should steer (default)
        // or interrupt the in-flight run. Absent on Panel/CLI paths → Steer.
        metadata.insert(
            crate::gateway::execution_engine::BUSY_INPUT_MODE_KEY.to_string(),
            busy_input_mode.to_string(),
        );
        // Stamp the channel's tool permission override (JSON) so `run_loop`
        // merges it as the most specific layer over global + agent
        // permissions. Only stamped when the channel configures one —
        // unconfigured channels stay byte-identical to pre-wiring metadata.
        if let Some(perms) = channel_tool_permissions {
            match serde_json::to_string(&perms) {
                Ok(json) => {
                    metadata.insert(
                        crate::gateway::execution_engine::CHANNEL_TOOL_PERMISSIONS_KEY.to_string(),
                        json,
                    );
                }
                Err(e) => error!(
                    channel_id = %ctx.message.channel_id.as_str(),
                    error = %e,
                    "Failed to serialize channel tool_permissions — channel layer skipped"
                ),
            }
        }

        let is_slash = slash_command_mode.is_some();
        if let Some(mode) = slash_command_mode {
            metadata.insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);
            // A person typed this `/foo` into the channel: the one mark a
            // `/skill`'s pre-grant requires (the engine's own safety net never
            // writes it).
            crate::gateway::execution_engine::slash_skill_scope::mark_typed(&mut metadata);
        }
        if ctx.message.is_group {
            metadata.insert("is_group".to_string(), "true".to_string());
        }
        if ctx.is_mentioned {
            metadata.insert("is_mentioned".to_string(), "true".to_string());
        }
        // Record voice mode for this turn in the session-keyed registry the
        // harness bridge reads at prompt-assembly time. Request metadata never
        // reaches `build_system_prompt`, so the old `metadata["voice_mode_active"]`
        // stamp had no reader and `VoiceModeLayer` never fired — this is the
        // wire that makes voice mode actually change agent behavior. Set on both
        // edges so disabling voice clears stale state.
        //
        // A voice turn resolves two config facts here with a single read: the
        // domain-vocabulary hint — recorded in the registry so `VoiceModeLayer`
        // can invite term-accurate transcription repair (the same
        // `[voice] vocabulary` that biases ASR, one dictionary two consumers) —
        // and the low-TTFT model pin (`[voice] llm_provider/llm_model`) so the
        // spoken reply starts faster than the global default. Empty config →
        // `None` → the run uses the global default. Only voice turns are
        // affected; text mode is untouched.
        let (voice_state, model_override) = if voice_enabled {
            let (vocabulary, model) = match &self.app_config {
                Some(cfg) => {
                    let cfg = cfg.read().await;
                    (
                        cfg.voice_local.vocabulary_hint(),
                        crate::gateway::model_override::ModelOverride::from_voice(
                            &cfg.voice_local.llm_provider,
                            &cfg.voice_local.llm_model,
                        ),
                    )
                }
                None => (None, None),
            };
            (
                Some(crate::gateway::voice::voice_mode::VoiceTurnState::new(
                    ctx.transcribed_input,
                    vocabulary,
                )),
                model,
            )
        } else {
            (None, None)
        };
        crate::gateway::voice::voice_mode::set(&ctx.session_key.to_key_string(), voice_state);

        // Channel-routed messages run in a workspace resolved by precedence:
        // a route binding's `MatchRule.workspace` (most-specific routing rule)
        // wins; otherwise the channel's Layer-1 `default_workspace` lock; a
        // Config-tier channel that sets neither falls back to the agent default.
        // Project-mode override (free workdir choice) enters via the desktop
        // Panel's `chat.send` and is gated there on Config tier.
        let effective_workspace = ctx.workspace.clone().or(channel_workspace);
        let mut request = RunRequest {
            run_id: run_id.clone(),
            input: ctx.message.text.clone(),
            session_key: ctx.session_key.clone(),
            timeout_secs: None,
            metadata,
            attachments: ctx.message.attachments.clone(),
            pending_media: pending_media.clone(),
            sandbox_override: None,
            workspace_override: effective_workspace,
            max_iterations_override: None,
            model_override,
            // Stamped from the TYPED `MessageMeta::Sms` the SMS interface
            // produced, never from the string `metadata` map above.
            telecom_public: telecom_public_from_message_meta(&ctx.message.metadata),
            telecom_authority: None,
        };

        if !request.attachments.is_empty() {
            tracing::info!(
                target: "multimodal",
                probe = "P2_resolve",
                run_id = %request.run_id,
                session_key = %request.session_key.to_key_string(),
                attachment_count = request.attachments.len(),
                "RunRequest created with attachments"
            );
        }

        let label = if is_slash {
            "slash command for agent"
        } else {
            "agent"
        };
        info!(
            "Executing {} '{}' for session {} (run_id: {})",
            label,
            agent_id,
            ctx.session_key.to_key_string(),
            run_id
        );

        // Recognise a `/btw` side question BEFORE the lane, not only inside
        // `execute()`. The lane below is registered before the engine is ever
        // entered, and to `btw::execution_session` an unstamped request is
        // indistinguishable from an ordinary one — so an unstamped side
        // question would queue on the session it was typed in and wait behind
        // the very run it is asking about. Idempotent: `execute()`'s own call
        // stamps from this same `request.input` and finds the key already
        // there. The RPC surfaces get this from `stamp_slash_mode`, which they
        // call before `spawn_queued_run` for the identical reason.
        crate::gateway::execution_engine::stamp_btw(&request.input, &mut request.metadata);

        // Busy lane is keyed by the session the run will EXECUTE on (matches
        // the engine's per-session `SessionRunRegistry` gate, which claims the
        // same key). Two sessions of the same agent get independent lanes and
        // run in parallel; only same-session messages serialize FIFO. A side
        // question executes on a derived session, so it gets its own lane —
        // that is what lets it answer while the main run keeps going, and
        // `register_run` is what picks the key rather than this call site.
        let agent_id_for_busy = request.session_key.agent_id().to_string();
        let busy_cfg = self.busy_queue_config().await;

        // Take the FIFO ticket HERE — synchronously, on the arrival path —
        // rather than inside the spawned task. Registering after the spawn made
        // lane order depend on task scheduling, so two messages sent a
        // millisecond apart could enqueue inverted, defeating the arrival-order
        // guarantee the lane exists to provide.
        let ticket = crate::gateway::busy_queue::register_run(
            &request.session_key,
            &request.metadata,
            busy_cfg.max_per_session,
            &request.run_id,
        );

        // Crash-durability write point (one of two; the Panel/CLI twin lives
        // in `busy_queue::spawn`). Only a *registered* ticket is journaled —
        // a `None` here is REJECT_NEWEST, the sender already heard no. The
        // locale rides `metadata["locale"]` (stamped above), same source the
        // outcome path below re-derives from.
        if ticket.is_some() && crate::gateway::busy_queue::durable::is_armed() {
            let locale = crate::gateway::i18n::Locale::from_config(
                request.metadata.get("locale").map(String::as_str),
            );
            crate::gateway::busy_queue::durable::record_enqueued(
                crate::gateway::busy_queue::QueuedRunPayload::from_request(&request, locale),
            );
        }

        // Spawn the delivery task (non-blocking). While the session is busy the
        // waiter parks on the lane's wake signal — fired by the engine when the
        // session's run slot frees — instead of polling, and only the front
        // ticket attempts delivery, so bursts deliver in arrival order.
        let error_channel_registry = self.channel_registry.clone();
        let error_reply_route = ctx.reply_route.clone();
        let error_app_config = self.app_config.clone();
        tokio::spawn(async move {
            use crate::gateway::busy_queue::{deliver_with_ticket, DeliveryOutcome};

            let outcome = match ticket {
                // Lane full — reject newest immediately so the sender hears
                // back now, not after the whole wait window.
                None => DeliveryOutcome::Rejected,
                // The guard is RAII: dropped on every exit — including a panic
                // inside the adapter — so a dead waiter can never leave a
                // corpse ticket wedging the lane (mirrors the engine's
                // `RunSlot` session claim).
                Some(ticket) => {
                    // `attempt` is `FnMut` and `deliver_with_ticket` calls it
                    // more than once — with the SAME `emitter`, whose
                    // `run_complete_handled` latch takes the first terminal
                    // frame and drops the rest. That is safe for exactly one
                    // reason: the only outcome that loops is
                    // `ExecutionError::AgentBusy`, which `execution_engine/gate.rs`
                    // returns *before* dispatch, so a refused attempt produces
                    // no `RunComplete` to spend the latch on. Any other outcome
                    // returns immediately. If a second retryable error is ever
                    // added here, check what it emits first.
                    let mut attempt = || {
                        execution_adapter.execute(request.clone(), agent.clone(), emitter.clone())
                    };
                    // Same frame from the channel arrival path — the shared
                    // constructor carries the bus-not-channel rule and the
                    // failure policy; see `run_queued_reporter`.
                    let mut report = crate::gateway::busy_queue::run_queued_reporter(
                        emitter.clone(),
                        request.run_id.clone(),
                        request.session_key.to_key_string(),
                    );
                    deliver_with_ticket(ticket, busy_cfg, &mut attempt, &mut report).await
                }
            };

            // Journal tombstone for the terminal outcome — the lane arms
            // (admitted/purge/cancel) cover their own cases; what remains is
            // "the attempt concluded" (gate refusal without admission) and
            // the wait deadline. `Rejected` never journaled (register failed).
            match &outcome {
                DeliveryOutcome::Executed(_) => {
                    crate::gateway::busy_queue::durable::record_settled(
                        &run_id,
                        crate::gateway::busy_queue::SettleReason::AttemptConcluded,
                    )
                }
                DeliveryOutcome::TimedOut => crate::gateway::busy_queue::durable::record_settled(
                    &run_id,
                    crate::gateway::busy_queue::SettleReason::TimedOut,
                ),
                DeliveryOutcome::Purged => crate::gateway::busy_queue::durable::record_settled(
                    &run_id,
                    crate::gateway::busy_queue::SettleReason::Purged,
                ),
                DeliveryOutcome::Rejected => {}
            }

            // An attempt that actually ran already reported its own failure to
            // this channel through the run's `ReplyEmitter` (`RunError` →
            // `send_error`). Reporting again here sent the user TWO messages
            // for one failure, so only queue-stage outcomes are ours to voice.
            if let DeliveryOutcome::Executed(Err(ref e)) = outcome {
                error!("Agent execution failed (run_id: {}): {}", run_id, e);
            }
            let Some(e) = outcome.user_error(&agent_id_for_busy) else {
                return;
            };
            error!(
                "Message never reached the agent (run_id: {}): {}",
                run_id, e
            );

            // Resolve user locale from config
            let locale = if let Some(ref cfg) = error_app_config {
                let cfg = cfg.read().await;
                crate::gateway::i18n::Locale::from_config(cfg.general.language.as_deref())
            } else {
                crate::gateway::i18n::Locale::Zh
            };

            // Routes through the SAME typed receipt the Panel/engine paths use
            // (`ExecutionError::user_receipt`) — this site used to stringify the
            // typed error and re-classify it with a second, subtly different
            // keyword table, which also echoed up to 200 chars of the raw
            // internal chain back to the channel.
            let (_code, user_msg) = e.user_receipt(locale);
            let reply = crate::gateway::channel::OutboundMessage::text(
                error_reply_route.conversation_id.as_str(),
                &user_msg,
            );
            if let Err(send_err) = error_channel_registry
                .send(&error_reply_route.channel_id, reply)
                .await
            {
                error!("Failed to send error reply: {}", send_err);
            }
        });

        Ok(())
    }

    /// Try to create a `FeishuEventEmitter` for feishu channels.
    async fn try_create_feishu_emitter(
        &self,
        ctx: &InboundContext,
        run_id: &str,
        reply_config: ReplyEmitterConfig,
        pending_media: crate::gateway::media::PendingMedia,
    ) -> Option<crate::gateway::interfaces::feishu::feishu_outbound::streaming::FeishuEventEmitter>
    {
        use crate::gateway::interfaces::feishu::api_handle;
        use crate::gateway::interfaces::feishu::feishu_outbound::streaming::FeishuEventEmitter;

        let channel_id = ctx.reply_route.channel_id.as_str();

        // Read-only projection: absence must never authenticate or start a provider.
        let (client, feishu_cfg) = api_handle::get(channel_id)?;

        let inner = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
            pending_media,
        );

        let chat_id = ctx.message.conversation_id.as_str().to_string();
        let reply_to = ctx
            .reply_route
            .reply_to
            .as_ref()
            .map(|id| id.as_str().to_string());

        Some(FeishuEventEmitter::new(
            inner,
            client,
            ctx.reply_route.clone(),
            chat_id,
            reply_to,
            feishu_cfg.streaming,
            feishu_cfg.typing_indicator,
        ))
    }

    /// Try to create a `TelegramEventEmitter` for telegram channels.
    async fn try_create_telegram_emitter(
        &self,
        ctx: &InboundContext,
        run_id: &str,
        reply_config: ReplyEmitterConfig,
        pending_media: crate::gateway::media::PendingMedia,
    ) -> Option<crate::gateway::interfaces::telegram::streaming::TelegramEventEmitter> {
        use crate::gateway::interfaces::telegram::parse_telegram_channel_config;
        use crate::gateway::interfaces::telegram::streaming::TelegramEventEmitter;

        let tg_cfg = {
            let cfg = self.app_config.as_ref()?.read().await;
            let channel_id = ctx.reply_route.channel_id.as_str();
            let raw = cfg.channels.get(channel_id)?;
            parse_telegram_channel_config(raw.clone()).ok()?
        };

        let account = tg_cfg.accounts.first()?;
        let streaming = account.streaming.clone().unwrap_or_default();

        // Only use orchestrated emitter when new streaming features are explicitly enabled
        // (P1-A: `draft_api_enabled` was CUT — `sendMessageDraft` is not a
        // generally-available Telegram API, so the orchestrator now lights
        // up purely on reasoning_lane / status_reactions.)
        if !streaming.reasoning_lane_enabled
            && streaming.status_reactions.processing.is_none()
            && streaming.status_reactions.tool_active.is_none()
            && streaming.status_reactions.complete.is_none()
        {
            return None;
        }

        let bot = teloxide::Bot::new(&account.bot_token);
        let conversation_id = ctx.message.conversation_id.as_str().to_string();

        // The orchestrated emitter takes over the wire for text but has no
        // media leg of its own — hand it a `ReplyEmitter` bound to the same
        // `pending_media` buffer the run fills, so `_media` still reaches the
        // chat under this config. Text never routes through it.
        let media = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
            pending_media,
        );

        Some(TelegramEventEmitter::new(
            bot,
            streaming,
            conversation_id,
            ctx.reply_route.clone(),
            media,
        ))
    }
}

/// Channel reply path of a public telecom run: one plain outbound text on the
/// inbound's own reply route. No emitter, no streaming, no owner run record.
struct ChannelPublicReplySink<'a> {
    router: &'a InboundMessageRouter,
    msg: &'a crate::gateway::channel::InboundMessage,
}

#[async_trait::async_trait]
impl crate::gateway::telecom::public_run::PublicReplySink for ChannelPublicReplySink<'_> {
    async fn send_reply(&self, text: String) -> Result<(), crate::gateway::channel::ChannelError> {
        let reply = crate::gateway::channel::OutboundMessage::text(
            self.msg.conversation_id.as_str(),
            &text,
        );
        // The driver's acknowledgment gate needs the actual send result.
        // Do not stringify transport errors here: they can contain bodies or phones.
        self.router
            .channel_registry
            .send_public_telecom(&self.msg.channel_id, reply)
            .await
            .map(|_| ())
    }

    async fn private_sms_lease(
        &self,
        ctx: &crate::gateway::telecom::TelecomPublicContext,
    ) -> Option<crate::gateway::interfaces::sms::PrivateSmsLease> {
        // Only typed SMS ingress is eligible; generic metadata cannot mint it.
        let meta = self.msg.metadata.iter().find_map(|meta| match meta {
            crate::gateway::channel::MessageMeta::Sms(meta) => Some(meta.as_ref()),
            _ => None,
        })?;
        if crate::gateway::telecom::TelecomPublicContext::from_admitted_sms(meta) != *ctx {
            return None;
        }
        let peer =
            crate::gateway::interfaces::sms::normalize_e164(self.msg.conversation_id.as_str())
                .ok()?;
        if peer != meta.peer_address {
            return None;
        }
        self.router
            .channel_registry
            .private_sms_lease(&self.msg.channel_id, ctx, &peer)
            .await
    }

    async fn send_approved_reply(
        &self,
        text: String,
        lease: &crate::gateway::interfaces::sms::PrivateSmsLease,
        cancel: &tokio_util::sync::CancellationToken,
        validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> Result<
        crate::gateway::telecom::public_run::ApprovedReplyOutcome,
        crate::gateway::channel::ChannelError,
    > {
        let reply = crate::gateway::channel::OutboundMessage::text(
            self.msg.conversation_id.as_str(),
            &text,
        );
        self.router
            .channel_registry
            .send_approved_public_telecom(&self.msg.channel_id, reply, lease, cancel, validate)
            .await
    }

    async fn send_reply_if_not_cancelled(
        &self,
        text: String,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<
        crate::gateway::telecom::public_run::PublicReplyOutcome,
        crate::gateway::channel::ChannelError,
    > {
        use crate::gateway::telecom::public_run::PublicReplyOutcome;
        let reply = crate::gateway::channel::OutboundMessage::text(
            self.msg.conversation_id.as_str(),
            &text,
        );
        self.router
            .channel_registry
            .send_public_telecom_if_not_cancelled(&self.msg.channel_id, reply, cancel)
            .await
            .map(|ack| match ack {
                Some(_) => PublicReplyOutcome::Acknowledged,
                None => PublicReplyOutcome::Cancelled,
            })
    }
}

impl InboundMessageRouter {
    /// Public telecom isolation branch (contract §3–§6). Called at the very top
    /// of `handle_message`, before any owner-state operation (route/agent
    /// resolution, session key/epoch, link access, permission, hooks). It takes
    /// the raw `InboundMessage` — no `InboundContext` is built, since that
    /// would resolve an owner session key. Runs instead of the
    /// whole owner pipeline: no hooks, HITL, slash/`/btw`/model commands, busy
    /// lane, durable journal or owner session. `/stop` cancels this binding's
    /// public run; every other text is plain caller text.
    pub(super) async fn handle_public_telecom(
        &self,
        msg: &crate::gateway::channel::InboundMessage,
        public: crate::gateway::telecom::TelecomPublicContext,
    ) -> Result<(), RoutingError> {
        let registry = crate::gateway::telecom::public_admission::PublicRunRegistry::global();
        if matches!(
            super::command_handler::classify_special_slash(&msg.text),
            Some(super::command_handler::SpecialSlash::Stop)
        ) {
            let cancelled = registry.cancel_caller(&public);
            info!("[Router] public telecom /stop (cancelled={cancelled})");
            return Ok(());
        }
        let harness = self
            .execution_adapter
            .as_ref()
            .and_then(|a| a.public_harness());
        let sink = ChannelPublicReplySink { router: self, msg };
        let disposition = crate::gateway::telecom::public_run::run_public_sms_with_private_read(
            registry,
            harness,
            crate::gateway::telecom::public_history::installed().map(|s| s.as_ref()),
            public,
            msg.text.clone(),
            &sink,
            // The trusted A/B scope is opened only after admission, inside
            // the run driver, and retired with the turn.
            self.operator_control.as_ref(),
            // Private turn only when `[telecom_private_read]` wired it.
            self.private_turn.as_ref(),
        )
        .await;
        info!("[Router] public telecom run: {disposition:?}");
        Ok(())
    }
}

#[cfg(test)]
mod public_reply_sink_tests {
    use super::*;
    use crate::gateway::channel::{
        Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo, ChannelResult,
        ChannelState, ChannelStatus, ConversationId, InboundMessage, MessageId, OutboundMessage,
        SendOperation, SendResult, UserId,
    };
    use crate::gateway::channel_registry::ChannelRegistry;
    use crate::gateway::delivery_queue::{DeliveryQueueConfig, DeliveryStore, EnqueueOutcome};
    use crate::gateway::pairing_store::SqlitePairingStore;
    use crate::gateway::routing_config::RoutingConfig;
    use crate::gateway::telecom::public_run::PublicReplySink;
    use std::sync::Mutex;

    struct SinkTransport {
        info: ChannelInfo,
        state: ChannelState,
        sent: Arc<Mutex<Vec<OutboundMessage>>>,
        fail: bool,
    }

    // Default Automatic delivery policy: the sink must isolate itself without
    // relying on the real SMS adapter's ExplicitOnly policy.
    #[async_trait::async_trait]
    impl Channel for SinkTransport {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        fn send(&self, message: OutboundMessage) -> SendOperation {
            let sent = Arc::clone(&self.sent);
            let fail = self.fail;
            Box::pin(async move {
                sent.lock().unwrap().push(message);
                if fail {
                    Err(ChannelError::RateLimited {
                        retry_after_secs: 0,
                    })
                } else {
                    Ok(SendResult {
                        message_id: MessageId::new("transport-ack"),
                        timestamp: chrono::Utc::now(),
                    })
                }
            })
        }
    }

    #[tokio::test]
    async fn actual_public_sink_preserves_route_and_result_without_replaying_owner_backlog() {
        for fail in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let store = Arc::new(
                DeliveryStore::open(
                    &temp.path().join("delivery.db"),
                    DeliveryQueueConfig::default(),
                )
                .unwrap(),
            );
            let registry = Arc::new(ChannelRegistry::new().with_delivery_store(store.clone()));
            let sent = Arc::new(Mutex::new(Vec::new()));
            let channel_id = registry
                .register(Box::new(SinkTransport {
                    info: ChannelInfo {
                        id: ChannelId::new("public-sink-transport"),
                        name: "test transport".into(),
                        channel_type: "test".into(),
                        status: ChannelStatus::Connected,
                        capabilities: ChannelCapabilities::default(),
                    },
                    state: ChannelState::new(8),
                    sent: sent.clone(),
                    fail,
                }))
                .await;
            let EnqueueOutcome::Queued(row) =
                crate::gateway::delivery_queue::enqueue_validated_legacy(
                    &registry,
                    &store,
                    channel_id.as_str(),
                    &OutboundMessage::text("original-conversation", "owner-backlog"),
                    "down",
                    0,
                )
                .await
                .unwrap()
            else {
                panic!("fixture must queue");
            };
            let router = InboundMessageRouter::new(
                registry,
                Arc::new(SqlitePairingStore::in_memory().unwrap()),
                RoutingConfig::default(),
            );
            let msg = InboundMessage {
                id: MessageId::new("inbound"),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new("original-conversation"),
                sender_id: UserId::new("caller"),
                sender_name: None,
                text: "question".into(),
                attachments: Vec::new(),
                timestamp: chrono::Utc::now(),
                reply_to: None,
                is_group: false,
                raw: None,
                metadata: Vec::new(),
            };
            let sink = ChannelPublicReplySink {
                router: &router,
                msg: &msg,
            };

            // Exercise the actual trait implementation, not a source-string
            // assertion or a duplicate helper standing in for the sink.
            let result = sink.send_reply("public-answer".into()).await;
            if fail {
                assert!(matches!(
                    result,
                    Err(ChannelError::RateLimited {
                        retry_after_secs: 0
                    })
                ));
            } else {
                assert!(result.is_ok());
            }
            {
                let messages = sent.lock().unwrap();
                assert_eq!(messages.len(), 1, "no queued replay or automatic retry");
                let reply = &messages[0];
                assert_eq!(reply.conversation_id, msg.conversation_id);
                assert_eq!(reply.text, "public-answer");
                assert!(reply.reply_to.is_none());
                assert!(reply.attachments.is_empty());
                assert!(reply.metadata.is_empty());
            }
            assert_eq!(store.len().unwrap(), 1);
            let backlog = store
                .claim_conversation(channel_id.as_str(), msg.conversation_id.as_str(), 10)
                .unwrap();
            assert_eq!(backlog.len(), 1);
            assert_eq!(backlog[0].id, row);
            assert_eq!(backlog[0].message.text, "owner-backlog");
            assert_eq!(backlog[0].attempts, 0);
            assert!(store.recent_dead_letters(10).unwrap().is_empty());

            let mut unknown = msg.clone();
            unknown.channel_id = ChannelId::new("unregistered-public-transport");
            let unknown_sink = ChannelPublicReplySink {
                router: &router,
                msg: &unknown,
            };
            assert!(matches!(
                unknown_sink.send_reply("unroutable".into()).await,
                Err(ChannelError::NotConnected(_))
            ));
            assert_eq!(sent.lock().unwrap().len(), 1);
            assert_eq!(store.len().unwrap(), 1);
            assert!(store.recent_dead_letters(10).unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn actual_public_sink_returns_cancelled_without_transport_send_when_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            DeliveryStore::open(
                &temp.path().join("delivery.db"),
                DeliveryQueueConfig::default(),
            )
            .unwrap(),
        );
        let registry = Arc::new(ChannelRegistry::new().with_delivery_store(store));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let channel_id = registry
            .register(Box::new(SinkTransport {
                info: ChannelInfo {
                    id: ChannelId::new("cancelled-public-sink-transport"),
                    name: "test transport".into(),
                    channel_type: "test".into(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(8),
                sent: sent.clone(),
                fail: false,
            }))
            .await;
        let router = InboundMessageRouter::new(
            registry,
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(),
        );
        let msg = InboundMessage {
            id: MessageId::new("cancelled-inbound"),
            channel_id,
            conversation_id: ConversationId::new("cancelled-conversation"),
            sender_id: UserId::new("caller"),
            sender_name: None,
            text: "question".into(),
            attachments: Vec::new(),
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: Vec::new(),
        };
        let sink = ChannelPublicReplySink {
            router: &router,
            msg: &msg,
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        let result = sink
            .send_reply_if_not_cancelled("public-answer".into(), &cancel)
            .await;

        assert!(matches!(
            result,
            Ok(crate::gateway::telecom::public_run::PublicReplyOutcome::Cancelled)
        ));
        assert!(sent.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod permission_wiring_tests {
    use super::channel_run_identity;
    use crate::gateway::inbound_router::types::{ChannelConfig, ChannelPermissionLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// THE security line: a channel with no registered config runs as the gated
    /// "guest" tier with no locked workspace. A regression here silently reopens
    /// the external-channel over-permission hole.
    #[test]
    fn unconfigured_channel_defaults_to_guest_with_no_workspace() {
        let empty: HashMap<String, ChannelConfig> = HashMap::new();
        // Unconfigured → guest, no workspace, the safe `steer` busy default,
        // and no channel tool-permission layer.
        assert_eq!(
            channel_run_identity(&empty, "telegram"),
            ("guest", None, "steer", None)
        );

        // Unknown id in a populated map → still the safe default.
        let mut configs = HashMap::new();
        configs.insert("slack".to_string(), ChannelConfig::default());
        assert_eq!(
            channel_run_identity(&configs, "telegram"),
            ("guest", None, "steer", None)
        );
    }

    /// A Chat-tier channel with an absolute, existing default_workspace is
    /// stamped "guest" and pinned to that workspace (Layer-1 lock).
    #[test]
    fn chat_tier_channel_locks_to_default_workspace() {
        let ws = std::env::temp_dir(); // absolute + existing
        let mut configs = HashMap::new();
        configs.insert(
            "telegram".to_string(),
            ChannelConfig {
                permission_level: ChannelPermissionLevel::Chat,
                default_workspace: Some(ws.clone()),
                ..Default::default()
            },
        );
        assert_eq!(
            channel_run_identity(&configs, "telegram"),
            ("guest", Some(ws), "steer", None)
        );
    }

    /// A channel that opts into interrupt mode is stamped `"interrupt"` (the
    /// metadata the execution engine's busy branch dispatches on), independent of
    /// its permission tier.
    #[test]
    fn interrupt_busy_mode_is_stamped() {
        use crate::gateway::execution_engine::BusyInputMode;
        let mut configs = HashMap::new();
        configs.insert(
            "ops-bot".to_string(),
            ChannelConfig {
                busy_input_mode: BusyInputMode::Interrupt,
                ..Default::default()
            },
        );
        let (_role, _ws, busy, _perms) = channel_run_identity(&configs, "ops-bot");
        assert_eq!(busy, "interrupt");
    }

    /// A channel carrying a tool-permission override surfaces it for the
    /// metadata stamp; unconfigured channels surface `None` (no stamp, no
    /// behavior change).
    #[test]
    fn channel_tool_permissions_surface_through_identity() {
        use crate::config::types::policies::ToolPermissionsConfig;
        use crate::extension::PermissionAction;

        let perms = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("bash".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let mut configs = HashMap::new();
        configs.insert(
            "group-bot".to_string(),
            ChannelConfig {
                tool_permissions: Some(perms),
                ..Default::default()
            },
        );
        let (_role, _ws, _busy, channel_perms) = channel_run_identity(&configs, "group-bot");
        let channel_perms = channel_perms.expect("configured override must surface");
        assert_eq!(channel_perms.resolve("bash"), PermissionAction::Deny);
        assert_eq!(channel_perms.resolve("read_file"), PermissionAction::Allow);
    }

    /// A Config-tier channel is stamped "operator" (Layer-2). With no
    /// default_workspace set it carries no lock (the agent default applies).
    #[test]
    fn config_tier_channel_maps_to_operator() {
        let mut configs = HashMap::new();
        configs.insert(
            "ops-bot".to_string(),
            ChannelConfig {
                permission_level: ChannelPermissionLevel::Config,
                ..Default::default()
            },
        );
        let (role, workspace, busy, _perms) = channel_run_identity(&configs, "ops-bot");
        assert_eq!(role, "operator");
        assert_eq!(workspace, None::<PathBuf>);
        // Config tier inherits the safe steer default unless explicitly opted in.
        assert_eq!(busy, "steer");
    }
}

/// Derive the public-telecom context from the TYPED `MessageMeta::Sms` the SMS
/// interface attached to an admitted inbound message. Any other channel yields
/// `None` (legacy behaviour, unchanged). This is the only production stamp
/// point of `RunRequest::telecom_public`, and the router's public-isolation
/// branch (`InboundMessageRouter::handle_public_telecom`) reads the same
/// derivation so the two can never disagree.
pub(super) fn telecom_public_from_message_meta(
    metadata: &[crate::gateway::channel::MessageMeta],
) -> Option<crate::gateway::telecom::TelecomPublicContext> {
    metadata.iter().find_map(|m| match m {
        crate::gateway::channel::MessageMeta::Sms(meta) => {
            Some(crate::gateway::telecom::TelecomPublicContext::from_admitted_sms(meta))
        }
        _ => None,
    })
}

#[cfg(test)]
mod telecom_stamp_tests {
    use super::telecom_public_from_message_meta;
    use crate::gateway::channel::MessageMeta;
    use crate::gateway::telecom::{SmsInboundMeta, TelecomChannel};
    use aleph_protocol::telecom_relay::SmsEncoding;

    fn sms_meta() -> SmsInboundMeta {
        SmsInboundMeta {
            region: "r".into(),
            binding_ref: "b".into(),
            provider_account_id: "a".into(),
            provider_message_id: "m".into(),
            envelope_event_id: "e".into(),
            connection_generation: 9,
            service_address: "+15550001111".into(),
            peer_address: "+15552223333".into(),
            encoding: SmsEncoding::Gsm7,
            segment_count: 1,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn sms_meta_stamps_public_context_and_other_channels_stay_none() {
        let stamped = telecom_public_from_message_meta(&[
            MessageMeta::AppMention,
            MessageMeta::Sms(Box::new(sms_meta())),
        ])
        .expect("typed Sms meta must stamp");
        assert_eq!(stamped.provenance().channel(), TelecomChannel::Sms);
        assert_eq!(stamped.provenance().connection_generation(), 9);

        assert!(telecom_public_from_message_meta(&[]).is_none());
        assert!(telecom_public_from_message_meta(&[MessageMeta::BotAuthored]).is_none());
    }
}

#[cfg(test)]
mod trusted_scope_lifecycle_tests {
    //! E-step: the inbound router hands its optional `OperatorControl` to
    //! the public run driver, which opens a trusted A/B scope only after
    //! admission (see `telecom::public_run` and `operator_control::ScopeGuard`
    //! tests for the lifecycle). These tests pin what remains here:
    //!
    //! 1. re-registering the same binding replaces, not stacks (E-3).
    //! 2. with no OperatorControl wired in, no scope is opened (E-4).

    use super::*;
    use crate::gateway::telecom::operator_control::OperatorControl;
    use crate::gateway::telecom::public_approval::PublicApprovalLedger;
    use crate::gateway::telecom::TelecomPublicContext;

    fn ctx_for(binding: &str) -> TelecomPublicContext {
        crate::gateway::telecom::public_context::test_support::sample_public_context(binding)
    }

    fn new_control() -> Arc<OperatorControl> {
        Arc::new(OperatorControl::new(Arc::new(PublicApprovalLedger::new())))
    }

    /// E-3: registering the same binding twice auto-cancels any
    /// outstanding approvals attached to the prior scope (per
    /// `OperatorControl::register_scope`'s contract: replace, then
    /// cancel_binding). The new scope is the only one visible to
    /// `list_pending_for_test`.
    #[test]
    fn e_reregistering_same_binding_cancels_outstanding_approvals() {
        let c = new_control();
        let ctx = ctx_for("bind-e-3");
        let first = crate::gateway::telecom::PublicApprovalScope::new(
            &ctx,
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            ctx.provenance().connection_generation(),
        );
        c.register_scope(first);
        let pending_after_first = c.list_pending_for_test();
        assert_eq!(
            pending_after_first
                .iter()
                .filter(|s| s.binding_ref() == "bind-e-3")
                .count(),
            1
        );

        // Second register on the same binding. The prior scope is
        // cancelled by the contract.
        let second = crate::gateway::telecom::PublicApprovalScope::new(
            &ctx,
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            ctx.provenance().connection_generation() + 1,
        );
        c.register_scope(second);
        let pending_after_second = c.list_pending_for_test();
        assert_eq!(
            pending_after_second
                .iter()
                .filter(|s| s.binding_ref() == "bind-e-3")
                .count(),
            1,
            "re-register must replace, not stack"
        );
        c.retire_scope("bind-e-3");
    }

    /// E-4: with no OperatorControl wired in (default
    /// `InboundMessageRouter::new`), the A/B path is closed. This is
    /// the documented "disabled" mode: the public run still happens,
    /// no scope is opened, and `retire_scope` is not called.
    #[test]
    fn e_no_operator_control_means_no_scope_opened() {
        use crate::gateway::pairing_store::SqlitePairingStore;
        use crate::gateway::routing_config::RoutingConfig;

        // We can't construct a full `InboundMessageRouter` cheaply, so
        // we instead assert via the field default: the public struct's
        // `operator_control` is `None` when no builder wired it in.
        let registry = Arc::new(crate::gateway::channel_registry::ChannelRegistry::new());
        let pairing = Arc::new(SqlitePairingStore::in_memory().expect("pairing store"));
        let router = InboundMessageRouter::new(registry, pairing, RoutingConfig::default());
        assert!(
            router.operator_control.is_none(),
            "A/B scope path must be disabled by default"
        );
    }
}
