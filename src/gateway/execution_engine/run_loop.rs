//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop), the `StreamCallback`
//! adapter that bridges `LoopCallback` to Gateway `StreamEvent`s, the
//! `ExecutionAdapter` trait implementation, and background memory persistence.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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

#[derive(Clone)]
struct ExtensionSkillDiscoverySource {
    skill_system: crate::skill::SkillSystem,
}

impl ExtensionSkillDiscoverySource {
    fn new(skill_system: crate::skill::SkillSystem) -> Self {
        Self { skill_system }
    }
}

impl crate::agent_loop::SkillDiscoverySource for ExtensionSkillDiscoverySource {
    fn discover(
        &self,
    ) -> Pin<Box<dyn Future<Output = Vec<crate::agent_loop::SkillInfo>> + Send + '_>> {
        let skill_system = self.skill_system.clone();
        Box::pin(async move {
            let snapshot = skill_system.current_snapshot().await;
            snapshot
                .eligible_manifests
                .into_iter()
                .map(|manifest| crate::agent_loop::SkillInfo {
                    name: manifest.name().to_string(),
                    description: manifest.description().to_string(),
                    schema: None,
                })
                .collect()
        })
    }
}

fn plugin_tool_to_unified_tool(
    tool: crate::extension::ToolRegistration,
) -> crate::dispatcher::UnifiedTool {
    let mut unified = crate::dispatcher::UnifiedTool::new(
        format!("plugin:{}:{}", tool.plugin_id, tool.name),
        &tool.name,
        &tool.description,
        crate::dispatcher::ToolSource::Plugin {
            plugin_id: tool.plugin_id.clone(),
        },
    );
    unified.parameters_schema = Some(tool.parameters);
    unified
}

fn active_plugin_tools_for_agent(
    extension_manager: &crate::extension::ExtensionManager,
    agent: &crate::gateway::agent_instance::AgentInstance,
) -> Vec<crate::dispatcher::UnifiedTool> {
    extension_manager
        .active_plugin_tools_snapshot()
        .into_iter()
        .filter(|tool| agent.is_tool_allowed(&tool.name))
        .map(plugin_tool_to_unified_tool)
        .collect()
}

#[derive(Clone)]
struct ExtensionToolRefreshSource<R: ToolRegistry + 'static> {
    extension_manager: Arc<crate::extension::ExtensionManager>,
    tool_registry: Arc<R>,
    agent: Arc<AgentInstance>,
    base_tools: Vec<crate::dispatcher::UnifiedTool>,
    default_working_dir: Option<String>,
    last_seen_revision: Arc<Mutex<u64>>,
}

impl<R: ToolRegistry + 'static> ExtensionToolRefreshSource<R> {
    fn new(
        extension_manager: Arc<crate::extension::ExtensionManager>,
        tool_registry: Arc<R>,
        agent: Arc<AgentInstance>,
        base_tools: Vec<crate::dispatcher::UnifiedTool>,
        default_working_dir: Option<String>,
    ) -> Self {
        Self {
            last_seen_revision: Arc::new(Mutex::new(extension_manager.plugin_tool_revision())),
            extension_manager,
            tool_registry,
            agent,
            base_tools,
            default_working_dir,
        }
    }

    fn merged_tools(&self) -> Vec<crate::dispatcher::UnifiedTool> {
        let mut tools = self.base_tools.clone();
        tools.extend(active_plugin_tools_for_agent(
            &self.extension_manager,
            &self.agent,
        ));
        tools
    }
}

impl<R: ToolRegistry + 'static> crate::agent_loop::ToolRefreshSource
    for ExtensionToolRefreshSource<R>
{
    fn poll_changes(&self) -> bool {
        let current_revision = self.extension_manager.plugin_tool_revision();
        let mut last_seen = self
            .last_seen_revision
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if current_revision != *last_seen {
            *last_seen = current_revision;
            return true;
        }

        false
    }

    fn fetch_tools(&self) -> Vec<Box<dyn crate::agent_loop::LoopTool>> {
        crate::agent_loop::adapters::build_tool_adapters_from_tools(
            self.tool_registry.clone(),
            &self.merged_tools(),
            self.default_working_dir.clone(),
        )
    }
}

// ============================================================================
// Agent loop execution
// ============================================================================

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Run the agent loop (think->act two-step, Claude Code-inspired).
    ///
    /// Uses the flat `LoopToolRegistry` and single-layer `SafetyGuard`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
        trace_task_id: Option<String>,
        cancel_token: CancellationToken,
    ) -> Result<String, ExecutionError> {
        use crate::agent_loop::model_behaviors::{load_model_behavior, protocol_to_behavior};
        use crate::agent_loop::{
            adapters::build_registry_from_tools, provider_bridge::AiProviderBridge, AgentLoop,
            LoopConfig, SafetyGuard,
        };
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

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

        let default_working_dir = Some(agent.workspace().to_string_lossy().to_string());

        // Resolve soul for prompt building (constant across retries)
        let identity_resolver = crate::thinker::identity::IdentityResolver::with_defaults();
        let resolved_soul = identity_resolver.resolve();

        let extension_manager: Option<Arc<crate::extension::ExtensionManager>> =
            crate::gateway::handlers::plugins::get_extension_manager()
                .ok()
                .map(Arc::clone);

        if let Some(ext_manager) = extension_manager.as_ref() {
            if let Err(e) = ext_manager.ensure_loaded().await {
                tracing::warn!("Failed to ensure extension manager is loaded: {}", e);
            }
        }

        // Pre-fetch extension-backed runtime helpers (constant across retries)
        let eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>> =
            if let Some(ext_manager) = extension_manager.as_ref() {
                let snapshot = ext_manager.skill_system().current_snapshot().await;
                if !snapshot.eligible_manifests.is_empty() {
                    Some(snapshot.eligible_manifests)
                } else {
                    None
                }
            } else {
                None
            };
        let hook_executor = if let Some(ext_manager) = extension_manager.as_ref() {
            let snapshot = ext_manager.hook_executor_snapshot().await;
            (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
        } else {
            None
        };
        let skill_system = extension_manager
            .as_ref()
            .map(|ext_manager| ext_manager.skill_system().clone());

        // Build tool registry inputs (filtered by agent whitelist).
        // Static builtins come from engine startup; plugin tools are resolved
        // from the live extension snapshot so hot-reload can refresh them.
        let base_allowed_tools: Vec<crate::dispatcher::UnifiedTool> = self
            .tools
            .iter()
            .filter(|t| !matches!(t.source, crate::dispatcher::ToolSource::Plugin { .. }))
            .filter(|t| agent.is_tool_allowed(&t.name))
            .cloned()
            .collect();
        let mut allowed_tools = base_allowed_tools.clone();
        if let Some(ext_manager) = extension_manager.as_ref() {
            allowed_tools.extend(active_plugin_tools_for_agent(ext_manager, &agent));
        } else {
            allowed_tools.extend(
                self.tools
                    .iter()
                    .filter(|t| matches!(t.source, crate::dispatcher::ToolSource::Plugin { .. }))
                    .filter(|t| agent.is_tool_allowed(&t.name))
                    .cloned(),
            );
        }

        // Subagent tool factories (cloneable Arc closures)
        let sub_allowed_tools: Vec<_> = allowed_tools
            .iter()
            .filter(|t| t.name != "subagent")
            .cloned()
            .collect();

        let sub_tool_registry_ref = self.tool_registry.clone();
        let sub_working_dir_ref = default_working_dir.clone();
        let sub_tool_factory: crate::agent_loop::agent_runtime::ToolRegistryFactory = Arc::new({
            let sub_tool_registry = sub_tool_registry_ref.clone();
            let sub_allowed = sub_allowed_tools.clone();
            let sub_dir = sub_working_dir_ref.clone();
            move || {
                build_registry_from_tools(sub_tool_registry.clone(), &sub_allowed, sub_dir.clone())
            }
        });

        let sub_safety_factory: crate::agent_loop::agent_runtime::SafetyGuardFactory = Arc::new({
            let global_perms = self.global_tool_permissions.clone();
            let agent_perms_clone = agent.config().tool_permissions();
            move || SafetyGuard::from_permissions(&global_perms, &agent_perms_clone)
        });

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
        let callback_state = Arc::new(StreamCallbackState::new(trace_task_id.and_then(
            |task_id| {
                self.state_database
                    .as_ref()
                    .map(|db| Arc::new(TracePersistence::new(db.clone(), task_id)))
            },
        )));

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
            let _behavior_content = {
                let behavior_name = provider
                    .model_behavior_override()
                    .or_else(|| protocol_to_behavior(provider.protocol()));
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
                AiProviderBridge::new(provider.clone()).with_model(resolved.model.clone())
            };

            // Build tool registry from UnifiedTool list
            let mut tool_registry = build_registry_from_tools(
                self.tool_registry.clone(),
                &allowed_tools,
                default_working_dir.clone(),
            );

            // Create a shared chain context for this run — both the SubagentTool
            // and the AgentLoop must share the same chain_id so that subagent
            // invocations produce consistent end-to-end chain tracing.
            let run_chain = crate::agent_loop::chain_context::ChainContext::new();

            // Shared prompt snapshot for fork path — written once by AgentLoop,
            // read by SubagentTool when spawning sub-agents.
            let shared_snapshot: crate::agent_loop::agent_runtime::SharedSnapshot =
                Arc::new(std::sync::RwLock::new(None));

            // Register subagent tool
            {
                use crate::agent_loop::background_tracker::BackgroundAgentTracker;
                use crate::agent_loop::subagent_tool::SubagentTool;
                use crate::agents::AgentRegistry;
                let sub_provider = self.provider_registry.default_provider();
                let agent_registry = Arc::new(AgentRegistry::with_builtins());
                let background_tracker = Arc::new(BackgroundAgentTracker::new());
                let mut subagent_tool = SubagentTool::new(
                    sub_provider,
                    sub_tool_factory.clone(),
                    sub_safety_factory.clone(),
                    run_chain.clone(),
                    agent_registry,
                    background_tracker,
                );
                subagent_tool = subagent_tool.with_shared_snapshot(shared_snapshot.clone());
                if let Some(ref mgr) = self.teammate_manager {
                    subagent_tool = subagent_tool.with_teammate_manager(mgr.clone());
                }
                if let Some(ref router) = self.message_router {
                    subagent_tool = subagent_tool.with_message_router(router.clone());
                }
                if let Some(ref inbox) = self.inbox {
                    subagent_tool = subagent_tool.with_inbox(inbox.clone());
                }
                tool_registry.register(Box::new(subagent_tool));
            }

            debug!(
                run_id = run_id,
                attempt = attempt,
                tool_count = tool_registry.len(),
                "Agent loop: built tool registry"
            );

            // Build prompt builder via thinker pipeline.
            // Pipeline layers handle soul, skills, tools, etc. during assembly.
            let mut config = PromptConfig::default();
            if let Some(custom_instructions) = agent.config().system_prompt.as_deref() {
                config.custom_instructions = Some(custom_instructions.to_string());
            }
            if let Some(ref skills) = eligible_skills {
                config.eligible_skills = Some(skills.clone());
            }
            let prompt_builder = if resolved_soul.is_empty() {
                PromptBuilder::new(config)
            } else {
                PromptBuilder::new(config).with_soul(resolved_soul.clone())
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

            // Build session context for environment info injection
            let session_ctx = crate::agent_loop::SessionContext {
                os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                shell: std::env::var("SHELL").unwrap_or_default(),
                cwd: agent.workspace().to_string_lossy().to_string(),
                git_branch: detect_git_branch(agent.workspace()).await,
                language: String::new(),
            };
            let context_compactor = crate::agent_loop::ContextCompactor::new(
                provider.clone(),
                crate::agent_loop::CompactorConfig::default(),
            );
            let stop_hooks: Vec<Box<dyn crate::agent_loop::StopHookHandler>> =
                if agent.id() == "coder" {
                    vec![Box::new(crate::agent_loop::VerifyStopHook::new(
                        agent.id().to_string(),
                        crate::agent_loop::VerifyStopHookConfig {
                            trigger_for: vec!["coder".to_string()],
                            min_iterations: 3,
                        },
                    ))]
                } else {
                    Vec::new()
                };
            let tool_refresh = extension_manager.as_ref().map(|ext_manager| {
                Arc::new(ExtensionToolRefreshSource::new(
                    Arc::clone(ext_manager),
                    self.tool_registry.clone(),
                    agent.clone(),
                    base_allowed_tools.clone(),
                    default_working_dir.clone(),
                )) as Arc<dyn crate::agent_loop::ToolRefreshSource>
            });

            let mut agent_loop = AgentLoop::new(
                bridge,
                tool_registry,
                prompt_builder,
                safety,
                loop_config,
                cancel_token.clone(),
            )
            .with_chain(run_chain)
            .with_delta_sink(Box::new(streaming_sink))
            .with_context_budget(context_budget)
            .with_session_context(session_ctx)
            .with_context_compactor(context_compactor)
            .with_summary_provider(self.provider_registry.default_provider())
            .with_stop_hooks(stop_hooks)
            .with_shared_snapshot(shared_snapshot);

            if let Some(skill_system) = skill_system.as_ref() {
                agent_loop =
                    agent_loop.with_skill_prefetcher(crate::agent_loop::SkillPrefetcher::new(
                        Arc::new(ExtensionSkillDiscoverySource::new(skill_system.clone())),
                        Duration::from_secs(30),
                    ));
            }
            if let Some(hooks) = hook_executor.as_ref() {
                agent_loop = agent_loop.with_hook_executor(Arc::clone(hooks));
            }
            if let Some(tool_refresh) = tool_refresh {
                agent_loop = agent_loop.with_tool_refresh(tool_refresh);
            }

            // Create a streaming callback
            let mut callback = StreamCallback::new(
                emitter.clone(),
                run_id.to_string(),
                request.pending_media.clone(),
                true,
                has_emitted_text,
                callback_state.clone(),
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
            callback.flush_trace_persistence().await;

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
                        // Rate limit errors should NOT trigger retries — retrying
                        // amplifies the problem and can exhaust provider quotas.
                        let is_rate_limited =
                            matches!(aleph_err, crate::error::AlephError::RateLimitError { .. });
                        let provider_err: Option<crate::providers::health::ProviderError> =
                            aleph_err.into();
                        if let Some(pe) = provider_err {
                            if !is_rate_limited {
                                is_retryable = true;
                            }
                            self.provider_registry
                                .report_outcome(&resolved.provider_name, Err(pe));
                        }
                    } else {
                        // stream_raw wraps errors as anyhow strings — classify via message.
                        // Use word-boundary-aware matching to avoid false positives
                        // (e.g. "4500" should NOT match status code "500").
                        let msg = e.to_string();
                        let is_rate_limit = msg.contains("429")
                            || msg.contains("rate limit")
                            || msg.contains("Rate limit");
                        let is_network = msg.contains("Network error")
                            || msg.contains("error sending request")
                            || msg.contains("connection")
                            || msg.contains("dns")
                            || msg.contains("timed out");
                        let is_auth = msg.contains("401")
                            || msg.contains("403")
                            || msg.contains("Unauthorized");
                        // Match HTTP status codes with boundary awareness:
                        // look for "(500)", "(502)", "(503)" patterns from error formatting,
                        // or status code at word boundaries to avoid matching "4500" etc.
                        let is_server = contains_http_status(&msg, 500)
                            || contains_http_status(&msg, 502)
                            || contains_http_status(&msg, 503);

                        if is_rate_limit {
                            // Rate limit is NOT retryable — don't amplify the problem
                            self.provider_registry.report_outcome(
                                &resolved.provider_name,
                                Err(crate::providers::health::ProviderError::Transient(
                                    crate::providers::health::TransientError::RateLimited {
                                        retry_after: None,
                                    },
                                )),
                            );
                        } else if is_network || is_server {
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

/// Check if a message contains an HTTP status code with word boundaries.
///
/// Matches patterns like "(500)", "500 ", " 500" to avoid false positives
/// where the status code digits appear inside larger numbers (e.g. "4500").
fn contains_http_status(msg: &str, code: u16) -> bool {
    let code_str = code.to_string();
    // Find all occurrences and check boundaries
    let mut search_from = 0;
    while let Some(pos) = msg[search_from..].find(&code_str) {
        let abs_pos = search_from + pos;
        let before_ok = abs_pos == 0 || !msg.as_bytes()[abs_pos - 1].is_ascii_digit();
        let after_pos = abs_pos + code_str.len();
        let after_ok = after_pos >= msg.len() || !msg.as_bytes()[after_pos].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        search_from = abs_pos + code_str.len();
    }
    false
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

struct TracePersistence {
    db: Arc<crate::resilience::StateDatabase>,
    task_id: String,
    next_step_index: AtomicU32,
    pending_writes: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl TracePersistence {
    fn new(db: Arc<crate::resilience::StateDatabase>, task_id: String) -> Self {
        Self {
            db,
            task_id,
            next_step_index: AtomicU32::new(0),
            pending_writes: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, event: &crate::agent_loop::LoopTraceEvent) {
        let step_index = self.next_step_index.fetch_add(1, Ordering::Relaxed);
        let db = self.db.clone();
        let task_id = self.task_id.clone();
        let trace_event: aleph_protocol::AgentTraceEvent = event.clone().into();

        let handle = tokio::spawn(async move {
            let trace = crate::resilience::TaskTrace::new(task_id.clone(), step_index, trace_event);
            if let Err(error) = db.insert_trace(&trace).await {
                tracing::warn!(
                    task_id = %task_id,
                    step_index,
                    error = %error,
                    "Failed to persist task trace"
                );
            }
        });

        self.pending_writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    async fn flush(&self) {
        let handles = {
            let mut pending = self
                .pending_writes
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *pending)
        };

        for handle in handles {
            if let Err(error) = handle.await {
                tracing::warn!(error = %error, "Task trace persistence task failed");
            }
        }
    }
}

struct StreamCallbackState {
    seq: AtomicU64,
    chunk_index: AtomicU32,
    trace_persistence: Option<Arc<TracePersistence>>,
}

impl StreamCallbackState {
    fn new(trace_persistence: Option<Arc<TracePersistence>>) -> Self {
        Self {
            seq: AtomicU64::new(0),
            chunk_index: AtomicU32::new(0),
            trace_persistence,
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn next_chunk_index(&self) -> u32 {
        self.chunk_index.fetch_add(1, Ordering::SeqCst)
    }

    fn persist_trace(&self, event: &crate::agent_loop::LoopTraceEvent) {
        if let Some(trace_persistence) = self.trace_persistence.as_ref() {
            trace_persistence.record(event);
        }
    }

    async fn flush_trace_persistence(&self) {
        if let Some(trace_persistence) = self.trace_persistence.as_ref() {
            trace_persistence.flush().await;
        }
    }
}

/// Callback adapter that bridges AgentLoop events to Gateway StreamEvents.
pub(super) struct StreamCallback<E: EventEmitter + Send + Sync + 'static> {
    emitter: Arc<E>,
    run_id: String,
    pending_media: PendingMedia,
    /// True when a StreamingDeltaSink is active for this run.
    /// When true, text tokens that were already delivered via DeltaSink are skipped.
    streaming_active: bool,
    /// Shared flag set by StreamingDeltaSink after each token delivery.
    /// StreamCallback swaps it to false and skips the duplicate on_text call.
    has_emitted_text: Arc<AtomicBool>,
    shared: Arc<StreamCallbackState>,
}

impl<E: EventEmitter + Send + Sync + 'static> StreamCallback<E> {
    fn new(
        emitter: Arc<E>,
        run_id: String,
        pending_media: PendingMedia,
        streaming_active: bool,
        has_emitted_text: Arc<AtomicBool>,
        shared: Arc<StreamCallbackState>,
    ) -> Self {
        Self {
            emitter,
            run_id,
            pending_media,
            streaming_active,
            has_emitted_text,
            shared,
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.shared.next_seq()
    }

    fn next_chunk_index(&self) -> u32 {
        self.shared.next_chunk_index()
    }

    fn emit_async(&self, event: StreamEvent) {
        let emitter = self.emitter.clone();
        tokio::spawn(async move {
            if let Err(e) = emitter.emit(event).await {
                tracing::warn!(error = %e, "StreamCallback: emit failed");
            }
        });
    }

    async fn flush_trace_persistence(&self) {
        self.shared.flush_trace_persistence().await;
    }
}

impl<E: EventEmitter + Send + Sync + 'static> crate::agent_loop::LoopCallback
    for StreamCallback<E>
{
    fn on_trace(&mut self, event: &crate::agent_loop::LoopTraceEvent) {
        self.shared.persist_trace(event);

        let trace_event = StreamEvent::AgentTrace {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            event: event.clone(),
        };
        self.emit_async(trace_event);

        match event {
            crate::agent_loop::LoopTraceEvent::TextEmitted { stream, text, .. } => match stream {
                crate::agent_loop::LoopTraceTextKind::Final => self.on_text(text),
                crate::agent_loop::LoopTraceTextKind::Intermediate => {
                    self.on_intermediate_text(text)
                }
            },
            crate::agent_loop::LoopTraceEvent::ToolCallStarted { call, .. } => {
                self.on_tool_call_start(call)
            }
            crate::agent_loop::LoopTraceEvent::ToolCallCompleted { call, result, .. } => {
                self.on_tool_call_done(call, result)
            }
            crate::agent_loop::LoopTraceEvent::ToolSummary { summary, .. } => {
                self.on_tool_summary(summary)
            }
            crate::agent_loop::LoopTraceEvent::TurnStarted { .. }
            | crate::agent_loop::LoopTraceEvent::TurnStateEntered { .. }
            | crate::agent_loop::LoopTraceEvent::TurnCompleted { .. }
            | crate::agent_loop::LoopTraceEvent::SessionCompleted { .. } => {}
        }
    }

    fn on_text(&mut self, text: &str) {
        // If streaming is active and DeltaSink already delivered this text token-by-token,
        // skip to avoid duplication. System-generated notices (truncation warning at
        // loop_core.rs:495) were never sent through DeltaSink, so has_emitted_text
        // will be false for them — they pass through normally.
        if self.streaming_active && self.has_emitted_text.swap(false, Ordering::Acquire) {
            return;
        }
        let chunk_index = self.next_chunk_index();

        let event = StreamEvent::ResponseChunk {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            delta: text.to_string(),
            content: text.to_string(),
            full_text: String::new(),
            chunk_index,
            is_final: false,
            is_intermediate: false,
        };
        self.emit_async(event);
    }

    fn on_intermediate_text(&mut self, text: &str) {
        // When streaming is active, DeltaSink already delivered the text tokens
        // and its boundary marker triggers emitters (ReplyEmitter, GatewayEventEmitter)
        // to flush their accumulated buffer as a standalone intermediate message.
        // Skip here to avoid duplicate intermediate messages on channels.
        if self.streaming_active {
            return;
        }
        let chunk_index = self.next_chunk_index();

        let event = StreamEvent::ResponseChunk {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            delta: text.to_string(),
            content: text.to_string(),
            full_text: String::new(),
            chunk_index,
            is_final: false,
            is_intermediate: true,
        };
        self.emit_async(event);
    }

    fn on_tool_call_start(&mut self, event: &crate::agent_loop::ToolCallStartEvent) {
        let stream_event = StreamEvent::ToolStart {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            tool_name: event.tool_name.clone(),
            tool_id: event.tool_id.clone(),
            params: event.input.clone(),
        };
        self.emit_async(stream_event);
    }

    fn on_tool_call_done(
        &mut self,
        event: &crate::agent_loop::ToolCallEndEvent,
        result: &crate::agent_loop::ToolResult,
    ) {
        // Extract _media from raw Value before serialization
        match result {
            crate::agent_loop::ToolResult::Success { output }
            | crate::agent_loop::ToolResult::SuccessAndStopLoop { output } => {
                if let Some(media_val) = output.get("_media") {
                    if let Ok(items) = serde_json::from_value::<Vec<MediaItem>>(media_val.clone()) {
                        tracing::info!(
                            tool = %event.tool_name,
                            count = items.len(),
                            urls = ?items.iter().map(|i| &i.url).collect::<Vec<_>>(),
                            "Extracted _media from tool output"
                        );
                        let mut pending =
                            self.pending_media.lock().unwrap_or_else(|e| e.into_inner());
                        let remaining = MAX_MEDIA_PER_RUN.saturating_sub(pending.len());
                        if remaining < items.len() {
                            tracing::warn!(
                                tool = %event.tool_name,
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
        let tool_result = match result {
            crate::agent_loop::ToolResult::Success { output }
            | crate::agent_loop::ToolResult::SuccessAndStopLoop { output } => {
                EmitterToolResult::success(output.to_string())
            }
            crate::agent_loop::ToolResult::Error { error, .. } => {
                EmitterToolResult::error(error.clone())
            }
        };
        let stream_event = StreamEvent::ToolEnd {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            tool_id: event.tool_id.clone(),
            result: tool_result,
            duration_ms: event.duration_ms,
        };
        self.emit_async(stream_event);
    }

    fn on_tool_summary(&mut self, summary: &str) {
        let event = StreamEvent::ReasoningBlock {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            step_type: crate::gateway::event_emitter::ReasoningStepType::Observation,
            label: "Tool Summary".to_string(),
            content: summary.to_string(),
            confidence: None,
            is_final: false,
        };
        self.emit_async(event);
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

/// Detect current git branch from workspace path.
async fn detect_git_branch(workspace: &std::path::Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace)
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            None
        } else {
            Some(branch)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::{LoopCallback, LoopTraceEvent};
    use crate::resilience::{AgentTask, RiskLevel};

    #[tokio::test]
    async fn stream_callback_persists_agent_trace_events() {
        let db = Arc::new(crate::resilience::StateDatabase::in_memory().unwrap());
        db.insert_agent_task(&AgentTask::new(
            "run-1",
            "session-1",
            "coder",
            "persist trace",
            RiskLevel::High,
        ))
        .await
        .unwrap();
        db.update_task_status("run-1", crate::resilience::TaskStatus::Running)
            .await
            .unwrap();

        let shared = Arc::new(StreamCallbackState::new(Some(Arc::new(
            TracePersistence::new(db.clone(), "run-1".to_string()),
        ))));
        let emitter = Arc::new(crate::gateway::NoOpEventEmitter::new());
        let mut callback = StreamCallback::new(
            emitter,
            "run-1".to_string(),
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            false,
            Arc::new(AtomicBool::new(false)),
            shared,
        );

        callback.on_trace(&LoopTraceEvent::TurnStarted { iteration: 1 });
        callback.flush_trace_persistence().await;

        let traces = db.get_traces_by_task("run-1").await.unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].event.kind(), "turn_started");
        assert_eq!(
            traces[0].event,
            aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 1 }
        );
    }
}
