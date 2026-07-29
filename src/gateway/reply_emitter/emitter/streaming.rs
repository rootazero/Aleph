use crate::sync_primitives::Ordering;
use async_trait::async_trait;
use tracing::{debug, warn};

use super::super::sanitize::sanitize_llm_output;
use super::{NativeStreamState, ReplyEmitter};
use crate::gateway::channel::OutboundMessage;
use crate::gateway::event_emitter::{EventEmitError, EventEmitter, StreamEvent};
use crate::gateway::streaming::StreamAction;

#[async_trait]
impl EventEmitter for ReplyEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        match event {
            StreamEvent::ResponseChunk {
                delta,
                is_final,
                is_intermediate,
                ..
            } => {
                if is_intermediate {
                    if delta.is_empty() {
                        // Intermediate boundary marker from DeltaSink: the LLM
                        // finished one iteration (tool use) and will continue.
                        if self.config.stream_enabled {
                            // Voice mode: streaming was skipped, buffer holds
                            // the text for RunComplete to convert to TTS.
                            // Do NOT finalize or clear — just reset controller.
                            if self.should_voice().await {
                                let mut ctrl = self.streaming.lock().await;
                                ctrl.reset();
                            } else {
                                // Streaming mode: the text was already delivered via
                                // real-time edits.  Finalize the current streamed
                                // message (perform any pending edit), then reset the
                                // controller so the next iteration starts a fresh
                                // message.  Do NOT send a new standalone message —
                                // the text is already visible to the user.
                                let mut ctrl = self.streaming.lock().await;
                                match ctrl.finalize() {
                                    StreamAction::EditFinal(text) => {
                                        let text = sanitize_llm_output(&text);
                                        if let Some(msg_id) = ctrl.message_id().cloned() {
                                            let _ = self
                                                .channel_registry
                                                .edit(
                                                    &self.route.channel_id,
                                                    &self.route.conversation_id,
                                                    &msg_id,
                                                    &text,
                                                )
                                                .await;
                                        }
                                    }
                                    StreamAction::SendFinal(text) => {
                                        // Rare: text never reached initial threshold.
                                        // Send it now as a new message.
                                        drop(ctrl);
                                        self.send_to_channel(&text).await;
                                        // Re-acquire for reset below
                                        let mut ctrl = self.streaming.lock().await;
                                        ctrl.reset();
                                        let mut buffer = self.buffer.lock().await;
                                        let _ = std::mem::take(&mut *buffer);
                                        return Ok(());
                                    }
                                    _ => {}
                                }
                                ctrl.reset();
                                drop(ctrl);
                                // Clear buffer (text was already streamed)
                                let mut buffer = self.buffer.lock().await;
                                let _ = std::mem::take(&mut *buffer);
                            } // end else (non-voice)
                        } else {
                            // Non-streaming: flush buffer as standalone message
                            let mut buffer = self.buffer.lock().await;
                            let accumulated = std::mem::take(&mut *buffer);
                            drop(buffer);
                            if !accumulated.is_empty() {
                                if self.should_voice().await {
                                    self.send_as_voice(&accumulated).await;
                                } else {
                                    self.send_to_channel(&accumulated).await;
                                }
                            }
                        }
                    } else {
                        // Non-empty intermediate: send immediately as standalone message
                        if self.should_voice().await {
                            self.send_as_voice(&delta).await;
                        } else {
                            self.send_to_channel(&delta).await;
                        }
                    }
                    // Do NOT buffer — this is a separate message from the final response
                } else {
                    // React with 👀 on first non-intermediate chunk and start typing indicator
                    if !self.has_sent.load(Ordering::SeqCst) && !delta.is_empty() {
                        self.react_on_inbound("👀").await;
                        self.start_typing_indicator();
                    }

                    // Existing behavior: accumulate text into buffer
                    if !delta.is_empty() {
                        self.buffer.lock().await.push_str(&delta);
                    }

                    // Real-time streaming: push chunks to controller and act
                    // Skip streaming text when voice mode is active — buffer only,
                    // voice reply will be sent on RunComplete.
                    if self.config.stream_enabled && !delta.is_empty() && !self.should_voice().await
                    {
                        let mut ctrl = self.streaming.lock().await;
                        ctrl.push_chunk(&delta);
                        match ctrl.poll_action() {
                            StreamAction::SendInitial(text) => {
                                let mut text = sanitize_llm_output(&text).into_owned();
                                text.push_str(
                                    crate::gateway::reply_emitter::emitter::STREAMING_CURSOR,
                                );
                                let message = OutboundMessage {
                                    conversation_id: self.route.conversation_id.clone(),
                                    text,
                                    attachments: vec![],
                                    reply_to: self.route.reply_to.clone(),
                                    inline_keyboard: None,
                                    metadata: Default::default(),
                                };
                                if let Ok(result) = self
                                    .channel_registry
                                    .send(&self.route.channel_id, message)
                                    .await
                                {
                                    ctrl.record_sent(result.message_id);
                                    self.has_sent.store(true, Ordering::SeqCst);
                                    self.typing_cancel.cancel();
                                }
                            }
                            StreamAction::Edit(text) => {
                                let text = sanitize_llm_output(&text).into_owned();
                                let overflow_threshold = self.overflow_threshold();
                                if overflow_threshold > 0
                                    && text.chars().count() > overflow_threshold
                                {
                                    // Overflow: finalize current message, start new one
                                    let char_boundary = text
                                        .char_indices()
                                        .nth(overflow_threshold)
                                        .map_or(text.len(), |(i, _)| i);
                                    let (head, tail) = text.split_at(char_boundary);

                                    // Edit current message with head (clean, no cursor)
                                    if let Some(msg_id) = ctrl.message_id() {
                                        let msg_id = msg_id.clone();
                                        let _ = self
                                            .channel_registry
                                            .edit(
                                                &self.route.channel_id,
                                                &self.route.conversation_id,
                                                &msg_id,
                                                head,
                                            )
                                            .await;
                                    }

                                    // Reset controller and push overflow text back
                                    ctrl.reset();
                                    ctrl.push_chunk(tail);

                                    // Send new message with overflow + cursor
                                    let mut overflow_text = tail.to_string();
                                    overflow_text.push_str(
                                        crate::gateway::reply_emitter::emitter::STREAMING_CURSOR,
                                    );
                                    let message = OutboundMessage {
                                        conversation_id: self.route.conversation_id.clone(),
                                        text: overflow_text,
                                        attachments: vec![],
                                        reply_to: None,
                                        inline_keyboard: None,
                                        metadata: Default::default(),
                                    };
                                    if let Ok(result) = self
                                        .channel_registry
                                        .send(&self.route.channel_id, message)
                                        .await
                                    {
                                        ctrl.record_sent(result.message_id);
                                    }
                                } else {
                                    // Normal edit with cursor
                                    let mut text_with_cursor = text;
                                    text_with_cursor.push_str(
                                        crate::gateway::reply_emitter::emitter::STREAMING_CURSOR,
                                    );
                                    if let Some(msg_id) = ctrl.message_id() {
                                        let msg_id = msg_id.clone();
                                        match self
                                            .channel_registry
                                            .edit(
                                                &self.route.channel_id,
                                                &self.route.conversation_id,
                                                &msg_id,
                                                &text_with_cursor,
                                            )
                                            .await
                                        {
                                            Ok(()) => ctrl.record_edit(),
                                            // Flood control: feed the channel's
                                            // retry hint into the controller so it
                                            // widens its debounce (and falls back to
                                            // a final-only flush after repeated
                                            // strikes) instead of hammering the
                                            // throttled endpoint on the next chunk.
                                            Err(
                                                crate::gateway::channel::ChannelError::RateLimited {
                                                    retry_after_secs,
                                                },
                                            ) => ctrl.record_edit_throttled(Some(
                                                std::time::Duration::from_secs(retry_after_secs),
                                            )),
                                            Err(_) => {}
                                        }
                                    }
                                }
                            }
                            StreamAction::Wait => {}
                            _ => {}
                        }
                    }

                    // Native streaming: forward chunks in real-time
                    if !self.native_disabled.load(Ordering::SeqCst) {
                        if let Some(ref handler) = self.native_handler {
                            let accumulated = {
                                let buffer = self.buffer.lock().await;
                                sanitize_llm_output(&buffer).into_owned()
                            };

                            let mut state = self.native_stream_state.lock().await;
                            if state.is_none() && accumulated.chars().count() >= 20 {
                                // First chunk with enough content: start streaming
                                let status =
                                    crate::gateway::interfaces::msteams::types::pick_status_text();
                                match handler
                                    .stream_start(&self.route.conversation_id, status)
                                    .await
                                {
                                    Ok(stream_id) => {
                                        *state = Some(NativeStreamState {
                                            stream_id,
                                            sequence: 0,
                                            last_update: std::time::Instant::now(),
                                        });
                                    }
                                    Err(e) => {
                                        warn!("Native stream_start failed, falling back: {}", e);
                                        self.native_disabled.store(true, Ordering::SeqCst);
                                    }
                                }
                            } else if let Some(ref mut s) = *state {
                                // Throttle updates at 1500ms
                                if s.last_update.elapsed() >= std::time::Duration::from_millis(1500)
                                {
                                    s.sequence += 1;
                                    if let Err(e) = handler
                                        .stream_update(
                                            &self.route.conversation_id,
                                            &s.stream_id,
                                            &accumulated,
                                            s.sequence,
                                        )
                                        .await
                                    {
                                        warn!("Native stream_update failed: {}", e);
                                        // Don't disable — try to finalize later
                                    }
                                    s.last_update = std::time::Instant::now();
                                }
                            }
                        }
                    }

                    // Instant mode: flush on final chunk (skip if native streaming active)
                    if !self.config.stream_enabled && is_final && self.native_handler.is_none() {
                        let mut buffer = self.buffer.lock().await;
                        if !buffer.is_empty() {
                            let text = std::mem::take(&mut *buffer);
                            drop(buffer);
                            let reasoning = self.take_reasoning_buffer().await;
                            if self.should_voice().await {
                                self.send_as_voice(&text).await;
                            } else {
                                self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                                    .await;
                            }
                        }
                    }
                    // Typewriter mode: do nothing here, wait for RunComplete
                }
            }

            StreamEvent::RunComplete { summary, .. } => {
                // RunComplete is single-sourced from the orchestrator drain
                // (helpers::run_dispatch_and_drain_classified); keep this
                // guard as defence against any future duplicate producer.
                if self.run_complete_handled.swap(true, Ordering::SeqCst) {
                    tracing::debug!(run_id = %self.run_id, "Ignoring duplicate RunComplete");
                    return Ok(());
                }

                // Stop the persistent typing indicator
                self.typing_cancel.cancel();

                // Finalize native stream if active
                if !self.native_disabled.load(Ordering::SeqCst) {
                    if let Some(ref handler) = self.native_handler {
                        let state = self.native_stream_state.lock().await.take();
                        if let Some(s) = state {
                            let text = {
                                let mut buffer = self.buffer.lock().await;
                                let raw = std::mem::take(&mut *buffer);
                                sanitize_llm_output(&raw).into_owned()
                            };
                            if !text.is_empty() {
                                let message = OutboundMessage::text(
                                    self.route.conversation_id.as_str(),
                                    &text,
                                );
                                match handler
                                    .stream_finalize(
                                        &self.route.conversation_id,
                                        &s.stream_id,
                                        message,
                                    )
                                    .await
                                {
                                    Ok(_result) => {
                                        self.has_sent.store(true, Ordering::SeqCst);
                                        // Stop typing, react success, close the media
                                        // leg — then return. This branch used to
                                        // `let _ =` the drain, silently dropping every
                                        // attachment; and because it returns early it
                                        // never reaches the cleanup at the end of
                                        // `RunComplete`, so it has to close the leg
                                        // itself. `deliver_run_media` is both halves.
                                        self.typing_cancel.cancel();
                                        self.react_on_inbound("\u{1f44d}").await;
                                        self.deliver_run_media().await;
                                        return Ok(());
                                    }
                                    Err(e) => {
                                        warn!("Native stream_finalize failed, falling back: {}", e);
                                        // Re-fill buffer for normal path
                                        *self.buffer.lock().await = text;
                                    }
                                }
                            }
                        }
                    }
                }

                // Hermes-summary-wiring: append a one-line cap notice when
                // the run did not complete cleanly. Pulls `terminate_reason`
                // off the enriched RunSummary; silent when the field is
                // missing (legacy producers) or equals "completed".
                if let Some(notice) = cap_notice_for(&summary) {
                    self.buffer.lock().await.push_str(&notice);
                }

                // Append fallback notice for non-Panel channels (Telegram, CLI, etc.).
                // The arrow is dropped when the model id is unchanged — the run
                // moved to a different *provider* serving the same id, and
                // "gpt-4o → gpt-4o" reads as a bug. Same guard the TUI and CLI
                // renderers already apply; this one lacked it because until the
                // route witness landed, nothing could set `is_fallback` truthfully
                // and the branch never actually ran.
                if let Some(info) = self.fallback_info.lock().await.take() {
                    let head = match info.original_model.as_deref() {
                        Some(orig) if orig != info.model => {
                            format!("{orig} \u{2192} {}", info.model)
                        }
                        _ => info.model.clone(),
                    };
                    let notice = format!("\n\n\u{26a1} {head} ({})", info.provider);
                    self.buffer.lock().await.push_str(&notice);
                }

                // Optional runtime-metadata footer (model · tokens · duration
                // · cost · cwd, plus an opt-in `tools` digest). Off unless
                // `gateway.runtime_footer.enabled = true` in TOML.
                if self.config.footer.enabled {
                    let model_label = self.model_label.lock().await.clone();
                    let cwd = std::env::current_dir()
                        .ok()
                        .and_then(|p| p.to_str().map(|s| s.to_string()));
                    let home = std::env::var("HOME").ok();
                    let inputs = crate::gateway::runtime_footer::RuntimeFooterInputs {
                        model: model_label.as_deref(),
                        total_tokens: Some(summary.total_tokens),
                        cwd: cwd.as_deref(),
                        duration_ms: summary.duration_ms,
                        cost_usd: summary.estimated_cost_usd,
                        tool_summaries: &summary.tool_summaries,
                    };
                    let block = crate::gateway::runtime_footer::build_footer_block(
                        &self.config.footer,
                        &inputs,
                        home.as_deref(),
                    );
                    if !block.is_empty() {
                        self.buffer.lock().await.push_str(&block);
                    }
                }

                // Flush accumulated buffer (always flush — intermediate messages
                // may have set has_sent, but the buffer holds the final response)
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                let reasoning = self.take_reasoning_buffer().await;

                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else if self.config.stream_enabled {
                        // Send explicit reasoning first if available
                        if let Some(ref r) = reasoning {
                            self.send_to_channel(&format!("🤔 {r}")).await;
                        }
                        // Finalize the streaming controller
                        let mut ctrl = self.streaming.lock().await;
                        match ctrl.finalize() {
                            StreamAction::SendFinal(final_text) => {
                                drop(ctrl);
                                if self.should_voice().await {
                                    self.send_as_voice(&final_text).await;
                                } else {
                                    self.send_to_channel(&final_text).await;
                                }
                            }
                            StreamAction::EditFinal(final_text) => {
                                let final_text = sanitize_llm_output(&final_text);
                                let msg_id = ctrl.message_id().cloned();
                                drop(ctrl);
                                if let Some(msg_id) = msg_id {
                                    let _ = self
                                        .channel_registry
                                        .edit(
                                            &self.route.channel_id,
                                            &self.route.conversation_id,
                                            &msg_id,
                                            &final_text,
                                        )
                                        .await;
                                }
                            }
                            StreamAction::Done => {
                                drop(ctrl);
                            }
                            _ => {
                                drop(ctrl);
                            }
                        }
                        let media = self.drain_and_send_media().await;
                        self.send_media_standalone(media).await;
                    } else {
                        self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                            .await;
                    }
                }

                // Fallback: if buffer was empty (race with fire-and-forget emit),
                // use final_response from summary
                if text.is_empty() {
                    if let Some(ref final_response) = summary.final_response {
                        if !final_response.is_empty() {
                            debug!(
                                "Run {} complete, sending final_response as fallback (length: {})",
                                self.run_id,
                                final_response.len()
                            );
                            if self.should_voice().await {
                                self.send_as_voice(final_response).await;
                            } else {
                                self.send_to_channel_with_reasoning(
                                    final_response,
                                    reasoning.as_deref(),
                                )
                                .await;
                            }
                        }
                    }
                }

                // React with 👍 on successful completion
                self.react_on_inbound("👍").await;

                // Last chance to deliver media, then drop the temp files. Every
                // drain above hangs off a non-empty reply, so a run that produced
                // ONLY media — the model calls `media_send` and stops — left the
                // queue full and the user with nothing. Idempotent: the buffer is
                // `mem::take`n, so a run that already drained finds it empty.
                self.deliver_run_media().await;
            }

            StreamEvent::RunError { error, .. } => {
                // Stop the persistent typing indicator
                self.typing_cancel.cancel();

                // Flush any partial response
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                let reasoning = self.take_reasoning_buffer().await;
                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else {
                        self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                            .await;
                    }
                }

                warn!("Run {} failed: {}", self.run_id, error);
                self.send_error(&error).await;

                // React with 👎 on error
                self.react_on_inbound("👎").await;
            }

            StreamEvent::AskUser { question, .. } => {
                // Drain any pending media before sending the question
                let media_attachments = self.drain_and_send_media().await;
                self.send_media_standalone(media_attachments).await;

                // Flush buffer first
                let text = {
                    let mut buffer = self.buffer.lock().await;
                    std::mem::take(&mut *buffer)
                };
                let reasoning = self.take_reasoning_buffer().await;
                if !text.is_empty() {
                    if self.should_voice().await {
                        self.send_as_voice(&text).await;
                    } else {
                        self.send_to_channel_with_reasoning(&text, reasoning.as_deref())
                            .await;
                    }
                }

                if self.should_voice().await {
                    self.send_as_voice(&question).await;
                } else {
                    self.send_to_channel(&question).await;
                }
            }

            // Accumulate explicit reasoning chunks for separate delivery
            StreamEvent::Reasoning { content, .. } => {
                if !content.is_empty() {
                    let mut buf = self.reasoning_buffer.lock().await;
                    if !buf.is_empty() && !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                    buf.push_str(&content);
                }
            }

            StreamEvent::ReasoningBlock { content, .. } => {
                if !content.is_empty() {
                    let mut buf = self.reasoning_buffer.lock().await;
                    if !buf.is_empty() {
                        buf.push_str("\n\n");
                    }
                    buf.push_str(&content);
                }
            }

            // Store fallback info for non-Panel notification
            StreamEvent::ModelResolved { model_info, .. } => {
                // Cache the resolved model name for the runtime-footer renderer.
                // Done unconditionally — the footer is opt-in, so the cost of a
                // mutex write per run is negligible vs threading a state check.
                *self.model_label.lock().await = Some(model_info.model.clone());
                if model_info.is_fallback {
                    *self.fallback_info.lock().await = Some(model_info);
                }
            }

            StreamEvent::ToolStart { tool_name, .. } => {
                self.react_on_inbound("🔧").await;
                debug!(target: "multimodal", probe = "P7_reaction", run_id = %self.run_id, tool = %tool_name, "Tool started — reaction set");
            }

            StreamEvent::ToolEnd { .. } => {
                // Tool finished — back to thinking state
                self.react_on_inbound("👀").await;
            }

            // Other events are not routed to channels. RunRetrying is a
            // transient Panel status line; channels already signal liveness
            // via typing indicators and would render it as message spam.
            StreamEvent::RunAccepted { .. }
            | StreamEvent::ToolUpdate { .. }
            | StreamEvent::AgentTrace { .. }
            | StreamEvent::UncertaintySignal { .. }
            | StreamEvent::ContextGauge { .. }
            | StreamEvent::RunRetrying { .. } => {
                debug!("Ignoring event for channel routing: {:?}", event);
            }
        }

        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Append a one-line notice when the run hit a cap. Returns `None` for a
/// clean exit so the user sees just the model's reply on the happy path.
///
/// `summary.terminate_reason` is the stable string from
/// `TerminateReason::as_static_str()` (populated by P3a/P3 wiring). Missing
/// or `"completed"` → no notice; anything else → user-facing tag.
fn cap_notice_for(summary: &crate::gateway::event_emitter::RunSummary) -> Option<String> {
    let reason = summary.terminate_reason.as_deref()?;
    if reason == "completed" {
        return None;
    }
    let label = match reason {
        "hit_max_iterations" => "max iterations",
        "context_budget_exhausted" => "context budget",
        "stall_timeout" => "stalled",
        "turn_timeout" => "turn timeout",
        "consecutive_failure_cap" => "repeated failures",
        "verifier_veto" => "verifier blocked",
        "cancelled" => "cancelled",
        other => other,
    };
    Some(format!("\n\n\u{26a0}\u{fe0f} {label}"))
}

#[cfg(test)]
mod cap_notice_tests {
    use super::cap_notice_for;
    use crate::gateway::event_emitter::RunSummary;

    fn summary_with(reason: Option<&str>) -> RunSummary {
        RunSummary {
            terminate_reason: reason.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn completed_returns_none() {
        assert!(cap_notice_for(&summary_with(Some("completed"))).is_none());
    }

    #[test]
    fn missing_reason_returns_none() {
        assert!(cap_notice_for(&summary_with(None)).is_none());
    }

    #[test]
    fn hit_max_iterations_renders_label() {
        let s = cap_notice_for(&summary_with(Some("hit_max_iterations"))).expect("notice");
        assert!(s.contains("max iterations"));
        assert!(s.starts_with("\n\n"));
    }

    #[test]
    fn verifier_veto_uses_human_label() {
        let s = cap_notice_for(&summary_with(Some("verifier_veto"))).expect("notice");
        assert!(s.contains("verifier blocked"));
    }

    #[test]
    fn unknown_reason_falls_through_to_raw_string() {
        // A future TerminateReason variant lands here without code change —
        // user sees the canonical name instead of "completed" or nothing.
        let s = cap_notice_for(&summary_with(Some("new_future_cap"))).expect("notice");
        assert!(s.contains("new_future_cap"));
    }
}
