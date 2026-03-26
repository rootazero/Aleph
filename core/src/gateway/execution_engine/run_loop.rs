//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop), the `StreamCallback`
//! adapter that bridges `LoopCallback` to Gateway `StreamEvent`s, the
//! `ExecutionAdapter` trait implementation, and background memory persistence.

use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use std::sync::atomic::{AtomicBool, Ordering};

use crate::sync_primitives::Arc;

use crate::gateway::media::{MediaItem, PendingMedia, MAX_MEDIA_PER_RUN};
use crate::gateway::streaming_sink::StreamingDeltaSink;
use super::{ExecutionError, RunRequest, RunStatus};
use crate::gateway::agent_instance::{AgentInstance, MessageRole};
use crate::gateway::event_emitter::{DynEventEmitter, EventEmitter, StreamEvent};
use crate::gateway::execution_adapter::ExecutionAdapter;

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
    ) -> Result<String, ExecutionError> {
        use crate::agent_loop::{
            AgentLoop, PromptBuilder, SafetyGuard, LoopConfig,
            adapters::build_registry_from_tools,
            provider_bridge::AiProviderBridge,
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
                    tracing::warn!("Failed to create ToolContext from workspace {}: {}", workspace_path.display(), e);
                }
            }
        }

        // Get provider
        // TODO(P3): Use call_with_fallback() from thinker::fallback when the registry
        // has fallbacks configured. This requires making AiProviderBridge registry-aware
        // so it can retry with fallback providers on transient errors.
        let provider = self.provider_registry.default_provider();
        let bridge = AiProviderBridge::new(provider);

        // Build tool registry from UnifiedTool list (filtered by agent whitelist)
        let allowed_tools: Vec<crate::dispatcher::UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();

        let default_working_dir = Some(agent.workspace().to_string_lossy().to_string());
        let mut tool_registry = build_registry_from_tools(
            self.tool_registry.clone(),
            &allowed_tools,
            default_working_dir.clone(),
        );

        // Register subagent tool — runs a sub-AgentLoop with same tools minus "subagent"
        {
            use crate::agent_loop::subagent_tool::{SubagentTool, ToolRegistryFactory, SafetyGuardFactory};

            let sub_provider = self.provider_registry.default_provider();
            let sub_tool_registry = self.tool_registry.clone();
            let sub_allowed_tools: Vec<_> = allowed_tools
                .iter()
                .filter(|t| t.name != "subagent")
                .cloned()
                .collect();
            let sub_working_dir = default_working_dir.clone();

            let factory: ToolRegistryFactory = Arc::new(move || {
                build_registry_from_tools(
                    sub_tool_registry.clone(),
                    &sub_allowed_tools,
                    sub_working_dir.clone(),
                )
            });

            let global_perms = self.global_tool_permissions.clone();
            let agent_perms_clone = agent.config().tool_permissions();
            let safety_factory: SafetyGuardFactory = Arc::new(move || {
                SafetyGuard::from_permissions(&global_perms, &agent_perms_clone)
            });

            tool_registry.register(Box::new(SubagentTool::new(sub_provider, factory, safety_factory)));
        }

        debug!(
            run_id = run_id,
            tool_count = tool_registry.len(),
            "Agent loop: built tool registry"
        );

        // Resolve soul for prompt building
        let identity_resolver = crate::thinker::identity::IdentityResolver::with_defaults();
        let resolved_soul = identity_resolver.resolve();
        let prompt_builder = if resolved_soul.is_empty() {
            PromptBuilder::new()
        } else {
            PromptBuilder::from_soul(&resolved_soul)
        };

        // Populate eligible skills from SkillSystem for scope filtering
        let prompt_builder = if let Ok(ext_manager) =
            crate::gateway::handlers::plugins::get_extension_manager()
        {
            if ext_manager.is_loaded().await {
                let snapshot = ext_manager.skill_system().current_snapshot().await;
                if !snapshot.eligible_manifests.is_empty() {
                    prompt_builder.with_eligible_skills(snapshot.eligible_manifests)
                } else {
                    prompt_builder
                }
            } else {
                prompt_builder
            }
        } else {
            prompt_builder
        };

        // Safety guard from merged global + agent permissions
        let agent_perms = agent.config().tool_permissions();
        let safety = SafetyGuard::from_permissions(&self.global_tool_permissions, &agent_perms);

        // Config from agent
        let max_loops = agent.config().max_loops as usize;
        let token_budget = agent.config().max_tokens.unwrap_or(500_000);
        let loop_config = LoopConfig {
            max_iterations: max_loops,
            token_budget,
        };

        // Build optional ToolCompactorConfig from session compactor settings
        let tool_compactor_config = self.session_compactor.as_ref().map(|sc| {
            crate::agent_loop::ToolCompactorConfig {
                token_budget: token_budget as u64,
                context_threshold: sc.config().context_threshold,
                token_estimate_ratio: sc.config().token_estimate_ratio,
                fresh_tail_count: sc.config().fresh_tail_count,
            }
        });

        // Real-time streaming: ProviderDelta → EventEmitter
        let has_emitted_text = Arc::new(AtomicBool::new(false));
        let streaming_sink = StreamingDeltaSink::new(
            run_id.to_string(),
            emitter.clone() as Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync>,
            has_emitted_text.clone(),
        );

        // Create and run the agent loop
        let agent_loop = AgentLoop::new(
            bridge,
            tool_registry,
            prompt_builder,
            safety,
            loop_config,
        )
        .with_delta_sink(Box::new(streaming_sink))
        .with_tool_compactor_config(tool_compactor_config);

        // Load conversation history from session (for multi-turn context)
        // Compression time excluded from agent's timeout budget
        let before_compress = tokio::time::Instant::now();
        let mut history = if let Some(ref sc) = self.session_compactor {
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

        // Create a streaming callback that emits events.
        // streaming_active = true: DeltaSink handles real-time token delivery;
        // StreamCallback deduplicates via has_emitted_text to avoid double-sending.
        let mut callback = StreamCallback::new(
            emitter.clone(),
            run_id.to_string(),
            request.pending_media.clone(),
            true,                      // streaming_active — DeltaSink handles text delivery
            has_emitted_text,          // shared with StreamingDeltaSink (last use, no clone)
        );

        // If attachments are present and a media processor is available,
        // process them into multimodal content blocks and use the
        // pre-built message path.
        let has_attachments = !request.attachments.is_empty() && self.media_processor.is_some();
        let loop_result = if has_attachments {
            let media_processor = self.media_processor.as_ref().unwrap();

            // TODO: Query ProviderModelInfo.supports_vision properly
            let supports_vision = true;

            let media_blocks = media_processor
                .process(&request.attachments, supports_vision, &request.session_key.to_key_string(), run_id)
                .await;

            // Build multimodal user message: text + media blocks
            let mut content = vec![crate::providers::message::ContentBlock::Text {
                text: request.input.clone(),
            }];
            content.extend(media_blocks);

            let has_images = content.iter().any(|b| matches!(b, crate::providers::message::ContentBlock::Image { .. }));
            let has_transcripts = content.iter().any(|b| {
                if let crate::providers::message::ContentBlock::Text { text } = b { text.starts_with("[Voice message transcript]") } else { false }
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

            history.push(crate::providers::message::UnifiedMessage::user_with_content(content));

            let result = agent_loop.run_with_history_messages(history, &mut callback).await;

            // Cleanup cached media files for this session
            media_processor.cleanup(&request.session_key.to_key_string());

            result
        } else {
            agent_loop.run_with_history(&request.input, history, &mut callback).await
        };

        match loop_result {
            Ok(result) => {
                info!(
                    run_id = run_id,
                    iterations = result.iterations,
                    tool_calls = result.tool_calls_made,
                    tokens = result.total_tokens,
                    hit_limit = result.hit_limit,
                    "Agent loop completed"
                );
                let response = if result.hit_limit && result.final_text.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
                    warn!(
                        run_id = run_id,
                        iterations = result.iterations,
                        tool_calls = result.tool_calls_made,
                        "Agent hit iteration/token limit without producing a response"
                    );
                    let locale = crate::gateway::i18n::Locale::from_config(
                        request.metadata.get("locale").map(|s| s.as_str())
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
                Ok(response)
            }
            Err(e) => {
                error!(run_id = run_id, error = %e, "Agent loop failed");
                Err(ExecutionError::Failed(e.to_string()))
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
    let history_slice = if session_history.last().map(|m| {
        m.role == MessageRole::User && m.content == current_input
    }).unwrap_or(false) {
        &session_history[..session_history.len() - 1]
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
            content: text.to_string(),
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
            content: text.to_string(),
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
                        let mut pending = self.pending_media.lock().unwrap_or_else(|e| e.into_inner());
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

    let context = ContextAnchor::with_session(
        session_key.clone(),
        session_key,
    );
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
