//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop), the `StreamCallback`
//! adapter that bridges `LoopCallback` to Gateway `StreamEvent`s, the
//! `ExecutionAdapter` trait implementation, and background memory persistence.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use std::sync::atomic::{AtomicBool, Ordering};

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, MessageRole};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::media::{MediaItem, PendingMedia, MAX_MEDIA_PER_RUN};
use crate::gateway::streaming_sink::StreamingDeltaSink;

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

// ============================================================================
// Agent loop execution
// ============================================================================

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Run the agent loop (think->act two-step, Claude Code-inspired).
    ///
    /// Uses the flat `LoopToolRegistry` and single-layer `SafetyGuard`.
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
        cancel_token: CancellationToken,
    ) -> Result<String, ExecutionError> {
        use crate::agent_loop::model_behaviors::{load_model_behavior, protocol_to_behavior};
        use crate::agent_loop::{
            adapters::build_registry_from_tools, provider_bridge::AiProviderBridge, AgentLoop,
            LoopConfig, PromptBuilder, SafetyGuard,
        };

        info!(run_id = run_id, "Starting agent loop (think->act)");

        // Write workspace-scoped output paths to tool context handle
        if let Some(tc_handle) = self.tool_registry.tool_context_handle() {
            let workspace_path = agent.workspace();
            match crate::tools::ToolContext::from_workspace(workspace_path) {
                Ok(ctx) => {
                    let mut tc = tc_handle.write().await;
                    *tc = ctx;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create ToolContext from workspace {}: {}",
                        workspace_path.display(),
                        e
                    );
                }
            }
        }

        // === Pre-compute values reusable across retry attempts ===

        // Build tool registry inputs (filtered by agent whitelist)
        let allowed_tools: Vec<crate::dispatcher::UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();

        let default_working_dir = Some(agent.workspace().to_string_lossy().to_string());

        // Subagent tool factories (cloneable Arc closures)
        let sub_allowed_tools: Vec<_> = allowed_tools
            .iter()
            .filter(|t| t.name != "subagent")
            .cloned()
            .collect();

        let sub_tool_registry_ref = self.tool_registry.clone();
        let sub_working_dir_ref = default_working_dir.clone();
        let sub_tool_factory: crate::agent_loop::subagent_tool::ToolRegistryFactory = Arc::new({
            let sub_tool_registry = sub_tool_registry_ref.clone();
            let sub_allowed = sub_allowed_tools.clone();
            let sub_dir = sub_working_dir_ref.clone();
            move || {
                build_registry_from_tools(sub_tool_registry.clone(), &sub_allowed, sub_dir.clone())
            }
        });

        let sub_safety_factory: crate::agent_loop::subagent_tool::SafetyGuardFactory = Arc::new({
            let global_perms = self.global_tool_permissions.clone();
            let agent_perms_clone = agent.config().tool_permissions();
            move || SafetyGuard::from_permissions(&global_perms, &agent_perms_clone)
        });

        // Resolve soul for prompt building (constant across retries)
        let identity_resolver = crate::thinker::identity::IdentityResolver::with_defaults();
        let resolved_soul = identity_resolver.resolve();

        // Pre-fetch eligible skills snapshot (constant across retries)
        let eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>> =
            if let Ok(ext_manager) = crate::gateway::handlers::plugins::get_extension_manager() {
                if ext_manager.is_loaded().await {
                    let snapshot = ext_manager.skill_system().current_snapshot().await;
                    if !snapshot.eligible_manifests.is_empty() {
                        Some(snapshot.eligible_manifests)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

        // Agent config values
        let agent_perms = agent.config().tool_permissions();
        let max_loops = agent.config().max_loops as usize;
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);

        // Load conversation history from session (for multi-turn context)
        // Compression time excluded from agent's timeout budget
        let before_compress = tokio::time::Instant::now();
        let history = if let Some(ref sc) = self.session_compactor {
            sc.prepare_history(
                &agent,
                &request.session_key,
                &request.input,
                token_budget as u64,
            )
            .await
        } else {
            build_loop_history(&agent, &request.session_key, &request.input).await
        };
        let compress_elapsed = before_compress.elapsed();
        if !compress_elapsed.is_zero() {
            *deadline.lock().await += compress_elapsed;
        }

        // Pre-process multimodal attachments (constant across retries)
        let multimodal_messages: Option<Vec<crate::providers::message::UnifiedMessage>> =
            if let (false, Some(media_processor)) = (
                request.attachments.is_empty(),
                self.media_processor.as_ref(),
            ) {
                let supports_vision = true;
                let media_blocks = media_processor
                    .process(
                        &request.attachments,
                        supports_vision,
                        &request.session_key.to_key_string(),
                        run_id,
                    )
                    .await;

                let mut content = vec![crate::providers::message::ContentBlock::Text {
                    text: request.input.clone(),
                }];
                content.extend(media_blocks);

                let has_images = content
                    .iter()
                    .any(|b| matches!(b, crate::providers::message::ContentBlock::Image { .. }));
                let has_transcripts = content.iter().any(|b| {
                    if let crate::providers::message::ContentBlock::Text { text } = b {
                        text.starts_with("[Voice message transcript]")
                    } else {
                        false
                    }
                });
                tracing::info!(
                    target: "multimodal",
                    probe = "P5_inject",
                    run_id = %request.run_id,
                    content_blocks = content.len(),
                    has_images = has_images,
                    has_transcripts = has_transcripts,
                    "Multimodal UnifiedMessage built"
                );

                let mut msgs = history.clone();
                msgs.push(crate::providers::message::UnifiedMessage::user_with_content(content));
                Some(msgs)
            } else {
                None
            };

        // === Retry loop: resolve provider → build agent loop → run ===
        // On transient provider failure, report degraded, re-resolve, and retry
        // with a different provider. Max 3 attempts to prevent infinite loops.
        const MAX_FALLBACK_ATTEMPTS: usize = 3;
        let mut attempt = 0usize;

        loop {
            attempt += 1;

            // Resolve model with health-aware fallback
            let resolved = self
                .provider_registry
                .resolve_with_fallback(&agent.config().model, &agent.config().fallback_models)
                .map_err(|e| ExecutionError::Failed(e.to_string()))?;

            if resolved.is_fallback {
                info!(
                    run_id = run_id,
                    attempt = attempt,
                    original = %resolved.original_model,
                    fallback_provider = %resolved.provider_name,
                    fallback_model = %resolved.model,
                    "Using fallback model"
                );
            }

            // Emit ModelResolved so the Panel can show fallback indicators
            let _ = emitter
                .emit(StreamEvent::ModelResolved {
                    run_id: run_id.to_string(),
                    model_info: crate::providers::health::ModelInfo {
                        model: resolved.model.clone(),
                        provider: resolved.provider_name.clone(),
                        is_fallback: resolved.is_fallback,
                        original_model: if resolved.is_fallback {
                            Some(resolved.original_model.clone())
                        } else {
                            None
                        },
                    },
                })
                .await;

            let provider = self
                .provider_registry
                .get(&resolved.provider_name)
                .unwrap_or_else(|| self.provider_registry.default_provider());

            // Resolve model behavior: config override > protocol auto-mapping
            let behavior_content = {
                let behavior_name = provider
                    .model_behavior_override()
                    .or_else(|| protocol_to_behavior(&provider.protocol().to_string()));
                let content = match behavior_name {
                    Some(name) => load_model_behavior(name).await,
                    None => None,
                };
                info!(
                    run_id = run_id,
                    protocol = %provider.protocol(),
                    behavior_name = ?behavior_name,
                    loaded = content.is_some(),
                    "Model behavior resolved"
                );
                content
            };

            // Build bridge with resolved provider
            let bridge = if resolved.model.is_empty() {
                AiProviderBridge::new(provider.clone())
            } else {
                AiProviderBridge::new(provider).with_model(resolved.model.clone())
            };

            // Build tool registry from UnifiedTool list
            let mut tool_registry = build_registry_from_tools(
                self.tool_registry.clone(),
                &allowed_tools,
                default_working_dir.clone(),
            );

            // Register subagent tool
            {
                use crate::agent_loop::subagent_tool::SubagentTool;
                use crate::agent_loop::chain_context::ChainContext;
                let sub_provider = self.provider_registry.default_provider();
                tool_registry.register(Box::new(SubagentTool::new(
                    sub_provider,
                    sub_tool_factory.clone(),
                    sub_safety_factory.clone(),
                    ChainContext::new(),
                )));
            }

            debug!(
                run_id = run_id,
                attempt = attempt,
                tool_count = tool_registry.len(),
                "Agent loop: built tool registry"
            );

            // Build prompt builder
            let prompt_builder = if resolved_soul.is_empty() {
                PromptBuilder::new()
            } else {
                PromptBuilder::from_soul(&resolved_soul)
            };
            let prompt_builder = if let Some(ref skills) = eligible_skills {
                prompt_builder.with_eligible_skills(skills.clone())
            } else {
                prompt_builder
            };
            let prompt_builder = if let Some(ref content) = behavior_content {
                prompt_builder.with_model_behavior(content)
            } else {
                prompt_builder
            };

            // Safety guard from merged global + agent permissions
            let safety = SafetyGuard::from_permissions(&self.global_tool_permissions, &agent_perms);

            let loop_config = LoopConfig {
                max_iterations: max_loops,
                token_budget,
            };

            // Real-time streaming: ProviderDelta → EventEmitter
            let has_emitted_text = Arc::new(AtomicBool::new(false));
            let streaming_sink = StreamingDeltaSink::new(
                run_id.to_string(),
                emitter.clone()
                    as Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync>,
                has_emitted_text.clone(),
            );

            // Create the agent loop
            // Build ContextBudget fresh for each retry attempt
            let context_budget = self.session_compactor.as_ref().map(|sc| {
                let config = crate::agent_loop::ContextBudgetConfig {
                    token_budget: token_budget as u64,
                    warning_threshold: 0.70,
                    critical_threshold: 0.85,
                    token_estimate_ratio: sc.config().token_estimate_ratio,
                    fresh_tail_count: sc.config().fresh_tail_count,
                    circuit_breaker_max: 3,
                    diminishing_window: 4,
                    diminishing_threshold: 500,
                };
                crate::agent_loop::ContextBudget::new(&config)
            });

            let agent_loop = AgentLoop::new(
                bridge,
                tool_registry,
                prompt_builder,
                safety,
                loop_config,
                cancel_token.clone(),
            )
            .with_delta_sink(Box::new(streaming_sink))
            .with_context_budget(context_budget);

            // Create a streaming callback
            let mut callback = StreamCallback::new(
                emitter.clone(),
                run_id.to_string(),
                request.pending_media.clone(),
                true,
                has_emitted_text,
            );

            // Run the agent loop with history
            let loop_result = if let Some(ref messages) = multimodal_messages {
                agent_loop
                    .run_with_history_messages(messages.clone(), &mut callback)
                    .await
            } else {
                agent_loop
                    .run_with_history(&request.input, history.clone(), &mut callback)
                    .await
            };

            match loop_result {
                Ok(result) => {
                    // Report success for health tracking
                    self.provider_registry
                        .report_outcome(&resolved.provider_name, Ok(()));

                    // Cleanup media if attachments were processed
                    if multimodal_messages.is_some() {
                        if let Some(media_processor) = self.media_processor.as_ref() {
                            media_processor.cleanup(&request.session_key.to_key_string());
                        }
                    }

                    info!(
                        run_id = run_id,
                        iterations = result.iterations,
                        tool_calls = result.tool_calls_made,
                        tokens = result.total_tokens,
                        hit_limit = result.hit_limit,
                        "Agent loop completed"
                    );
                    let response = if result.hit_limit
                        && result
                            .final_text
                            .as_ref()
                            .map(|t| t.is_empty())
                            .unwrap_or(true)
                    {
                        warn!(
                            run_id = run_id,
                            iterations = result.iterations,
                            tool_calls = result.tool_calls_made,
                            "Agent hit iteration/token limit without producing a response"
                        );
                        let locale = crate::gateway::i18n::Locale::from_config(
                            request.metadata.get("locale").map(|s| s.as_str()),
                        );
                        crate::gateway::i18n::t(
                            crate::gateway::i18n::Msg::ErrLoopExhausted {
                                iterations: result.iterations,
                                tool_calls: result.tool_calls_made,
                            },
                            locale,
                        )
                    } else {
                        result.final_text.unwrap_or_default()
                    };
                    return Ok(response);
                }
                Err(e) => {
                    // Classify the error for health tracking and retry decisions.
                    // Errors may arrive as AlephError (from process()) or as plain
                    // anyhow::Error (from stream_raw's map_err). Try both paths.
                    let mut is_retryable = false;

                    if let Some(aleph_err) = e.downcast_ref::<crate::error::AlephError>() {
                        let provider_err: Option<crate::providers::health::ProviderError> =
                            aleph_err.into();
                        if let Some(pe) = provider_err {
                            is_retryable = true;
                            self.provider_registry
                                .report_outcome(&resolved.provider_name, Err(pe));
                        }
                    } else {
                        // stream_raw wraps errors as anyhow strings — classify via message
                        let msg = e.to_string();
                        let is_network = msg.contains("Network error")
                            || msg.contains("error sending request")
                            || msg.contains("connection")
                            || msg.contains("dns")
                            || msg.contains("timed out");
                        let is_auth = msg.contains("401")
                            || msg.contains("403")
                            || msg.contains("Unauthorized");
                        let is_server =
                            msg.contains("500") || msg.contains("502") || msg.contains("503");

                        if is_network || is_server {
                            is_retryable = true;
                            self.provider_registry.report_outcome(
                                &resolved.provider_name,
                                Err(crate::providers::health::ProviderError::Transient(
                                    crate::providers::health::TransientError::ConnectionFailed,
                                )),
                            );
                        } else if is_auth {
                            is_retryable = true;
                            self.provider_registry.report_outcome(
                                &resolved.provider_name,
                                Err(crate::providers::health::ProviderError::Permanent(
                                    crate::providers::health::PermanentError::AuthFailed,
                                )),
                            );
                        }
                    }

                    // Retry with fallback on any provider-classified error (transient or permanent)
                    // Transient: rate limit, 5xx, timeout — provider might recover
                    // Permanent: auth failed, model not found — this provider is dead, try another
                    if is_retryable && attempt < MAX_FALLBACK_ATTEMPTS {
                        // Check if a different provider is available
                        match self.provider_registry.resolve_with_fallback(
                            &agent.config().model,
                            &agent.config().fallback_models,
                        ) {
                            Ok(new_resolved)
                                if new_resolved.provider_name != resolved.provider_name =>
                            {
                                warn!(
                                    run_id = run_id,
                                    attempt = attempt,
                                    failed_provider = %resolved.provider_name,
                                    next_provider = %new_resolved.provider_name,
                                    error = %e,
                                    "Provider failed with transient error, retrying with fallback"
                                );
                                continue; // retry with new provider
                            }
                            _ => {
                                // No different fallback available, give up
                                error!(run_id = run_id, error = %e, "Agent loop failed, no alternative provider available");
                            }
                        }
                    } else {
                        error!(run_id = run_id, error = %e, attempt = attempt, "Agent loop failed");
                    }

                    // Cleanup media on failure too
                    if multimodal_messages.is_some() {
                        if let Some(media_processor) = self.media_processor.as_ref() {
                            media_processor.cleanup(&request.session_key.to_key_string());
                        }
                    }

                    return Err(ExecutionError::Failed(e.to_string()));
                }
            }
        }
    }
}

/// Build loop history from the agent's session, excluding the current user input.
async fn build_loop_history(
    agent: &AgentInstance,
    session_key: &crate::gateway::router::SessionKey,
    current_input: &str,
) -> Vec<crate::providers::message::UnifiedMessage> {
    use crate::providers::message::UnifiedMessage;

    let session_history = agent.get_history(session_key, Some(50)).await;
    let mut msgs: Vec<UnifiedMessage> = Vec::new();

    // Skip the last message if it's the current user input we just stored
    let history_slice = if session_history
        .last()
        .map(|m| m.role == MessageRole::User && m.content == current_input)
        .unwrap_or(false)
    {
        // safe: last() returned Some, so len() >= 1
        &session_history[..session_history.len().saturating_sub(1)]
    } else {
        &session_history
    };

    for msg in history_slice {
        match msg.role {
            MessageRole::User => msgs.push(UnifiedMessage::user(msg.content.clone())),
            MessageRole::Assistant => msgs.push(UnifiedMessage::assistant(msg.content.clone())),
            _ => {}
        }
    }
    msgs
}

// ============================================================================
// StreamCallback — bridges LoopCallback to Gateway StreamEvents
// ============================================================================

/// Callback adapter that bridges AgentLoop events to Gateway StreamEvents.
pub(super) struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    seq: u64,
    chunk_index: u32,
    pending_media: PendingMedia,
    /// True when a StreamingDeltaSink is active for this run.
    /// When true, text tokens that were already delivered via DeltaSink are skipped.
    streaming_active: bool,
    /// Shared flag set by StreamingDeltaSink after each token delivery.
    /// StreamCallback swaps it to false and skips the duplicate on_text call.
    has_emitted_text: Arc<AtomicBool>,
}

impl<E: EventEmitter + Send + Sync + 'static> StreamCallback<E> {
    pub(super) fn new(
        emitter: Arc<E>,
        run_id: String,
        pending_media: PendingMedia,
        streaming_active: bool,
        has_emitted_text: Arc<AtomicBool>,
    ) -> Self {
        Self {
            emitter,
            run_id,
            seq: 0,
            chunk_index: 0,
            pending_media,
            streaming_active,
            has_emitted_text,
        }
    }
}

impl<E: EventEmitter + Send + Sync + 'static> crate::agent_loop::LoopCallback
    for StreamCallback<E>
{
    fn on_text(&mut self, text: &str) {
        // If streaming is active and DeltaSink already delivered this text token-by-token,
        // skip to avoid duplication. System-generated notices (truncation warning at
        // loop_core.rs:495) were never sent through DeltaSink, so has_emitted_text
        // will be false for them — they pass through normally.
        if self.streaming_active && self.has_emitted_text.swap(false, Ordering::Acquire) {
            return;
        }
        self.seq += 1;
        let chunk_index = self.chunk_index;
        self.chunk_index += 1;

        let event = StreamEvent::ResponseChunk {
            run_id: self.run_id.clone(),
            seq: self.seq,
            delta: text.to_string(),
            content: text.to_string(),
            full_text: String::new(),
            chunk_index,
            is_final: false,
            is_intermediate: false,
        };

        // Fire-and-forget emit (LoopCallback is sync, emitter is async)
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            if let Err(e) = emitter.emit(event).await {
                tracing::warn!(error = %e, "StreamCallback: emit failed");
            }
        });
    }

    fn on_intermediate_text(&mut self, text: &str) {
        // When streaming is active, DeltaSink already delivered the text tokens
        // and its boundary marker triggers emitters (ReplyEmitter, GatewayEventEmitter)
        // to flush their accumulated buffer as a standalone intermediate message.
        // Skip here to avoid duplicate intermediate messages on channels.
        if self.streaming_active {
            return;
        }
        self.seq += 1;
        let chunk_index = self.chunk_index;
        self.chunk_index += 1;

        let event = StreamEvent::ResponseChunk {
            run_id: self.run_id.clone(),
            seq: self.seq,
            delta: text.to_string(),
            content: text.to_string(),
            full_text: String::new(),
            chunk_index,
            is_final: false,
            is_intermediate: true,
        };

        // Fire-and-forget emit (LoopCallback is sync, emitter is async)
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            if let Err(e) = emitter.emit(event).await {
                tracing::warn!(error = %e, "StreamCallback: emit failed");
            }
        });
    }

    fn on_tool_start(&mut self, name: &str, input: &serde_json::Value) {
        self.seq += 1;
        let event = StreamEvent::ToolStart {
            run_id: self.run_id.clone(),
            seq: self.seq,
            tool_name: name.to_string(),
            tool_id: name.to_string(),
            params: input.clone(),
        };
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            let _ = emitter.emit(event).await;
        });
    }

    fn on_tool_done(&mut self, name: &str, result: &crate::agent_loop::ToolResult) {
        // Extract _media from raw Value before serialization
        match result {
            crate::agent_loop::ToolResult::Success { output }
            | crate::agent_loop::ToolResult::SuccessAndStopLoop { output } => {
                if let Some(media_val) = output.get("_media") {
                    if let Ok(items) = serde_json::from_value::<Vec<MediaItem>>(media_val.clone()) {
                        tracing::info!(
                            tool = %name,
                            count = items.len(),
                            urls = ?items.iter().map(|i| &i.url).collect::<Vec<_>>(),
                            "Extracted _media from tool output"
                        );
                        let mut pending =
                            self.pending_media.lock().unwrap_or_else(|e| e.into_inner());
                        let remaining = MAX_MEDIA_PER_RUN.saturating_sub(pending.len());
                        if remaining < items.len() {
                            tracing::warn!(
                                tool = %name,
                                total = items.len(),
                                accepted = remaining,
                                "Media items exceed per-run limit, dropping excess"
                            );
                        }
                        pending.extend(items.into_iter().take(remaining));
                    }
                }
            }
            _ => {}
        }

        // === Existing code below ===
        use crate::gateway::event_emitter::ToolResult as EmitterToolResult;
        self.seq += 1;
        let tool_result = match result {
            crate::agent_loop::ToolResult::Success { output }
            | crate::agent_loop::ToolResult::SuccessAndStopLoop { output } => {
                EmitterToolResult::success(output.to_string())
            }
            crate::agent_loop::ToolResult::Error { error, .. } => {
                EmitterToolResult::error(error.clone())
            }
        };
        let event = StreamEvent::ToolEnd {
            run_id: self.run_id.clone(),
            seq: self.seq,
            tool_id: name.to_string(),
            result: tool_result,
            duration_ms: 0,
        };
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            let _ = emitter.emit(event).await;
        });
    }
}

// ============================================================================
// ExecutionAdapter trait implementation
// ============================================================================

/// Implement ExecutionAdapter for the ExecutionEngine.
///
/// This allows InboundMessageRouter to use ExecutionEngine via a trait object,
/// enabling routing without being generic over provider and tool registry types.
#[async_trait]
impl<P, R> ExecutionAdapter for ExecutionEngine<P, R>
where
    P: ThinkerProviderRegistry + Send + Sync + 'static,
    R: ToolRegistry + Send + Sync + 'static,
{
    async fn execute(
        &self,
        request: RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<dyn EventEmitter + Send + Sync>,
    ) -> Result<(), ExecutionError> {
        // Wrap the dyn trait object in DynEventEmitter to make it Sized,
        // then delegate to the existing generic execute method
        let wrapper = Arc::new(DynEventEmitter::new(emitter));
        ExecutionEngine::execute(self, request, agent, wrapper).await
    }

    async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
        ExecutionEngine::cancel(self, run_id).await
    }

    async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
        ExecutionEngine::get_status(self, run_id).await
    }

    async fn active_run_count(&self) -> usize {
        ExecutionEngine::active_run_count(self).await
    }
}

// ============================================================================
// Background memory persistence
// ============================================================================

/// Write a conversation turn to the memory system (Layer 1).
///
/// Runs in a background task — failures are logged but never block the caller.
pub(super) async fn write_conversation_memory(
    memory_backend: crate::memory::store::MemoryBackend,
    session_key: String,
    agent_id: String,
    user_input: String,
    ai_output: String,
) {
    use crate::memory::context::{ContextAnchor, MemoryEntry};

    let context = ContextAnchor::with_session(session_key.clone(), session_key);
    let mut entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        context,
        user_input,
        ai_output,
    );
    entry.agent = agent_id;

    use crate::memory::store::SessionStore;
    if let Err(e) = memory_backend.insert_memory(&entry).await {
        warn!("Failed to write conversation memory: {}", e);
    } else {
        debug!("Conversation memory saved to Layer 1");
    }
}
