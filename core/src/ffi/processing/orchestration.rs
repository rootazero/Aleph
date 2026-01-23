//! Main orchestration logic for processing with agent loop

use crate::agents::rig::ChatMessage;
use crate::agents::RigAgentConfig;
use crate::command::CommandParser;
use crate::config::{GenerationConfig, RoutingRuleConfig};
use crate::core::MediaAttachment;
use crate::dispatcher::{AnalysisResult, TaskAnalyzer};
use crate::ffi::dag_executor::run_dag_execution;
use crate::ffi::prompt_helpers::extract_attachment_text;
use crate::ffi::provider_factory::create_provider_from_config;
use crate::ffi::AetherEventHandler;
use crate::generation::GenerationProviderRegistry;
use crate::intent::{IntentRouter, RouteResult};
use crate::skills::SkillsRegistry;
use crate::tools::AetherToolServerHandle;
use crate::utils::paths::get_skills_dir;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::agent_loop::run_agent_loop;
use super::direct_route::handle_direct_route;

/// Process input using the new Agent Loop architecture
///
/// This function implements the new observe-think-act-feedback loop:
/// - L0-L2: Fast routing via IntentRouter (slash commands, patterns, context)
/// - Agent Loop: LLM-based thinking for complex tasks
///
/// # Architecture Flow
///
/// ```text
/// User Input
///     |
/// IntentRouter (L0-L2)
///     +-- DirectRoute -> Execute immediately (slash commands, etc.)
///     +-- NeedsThinking -> Agent Loop
///                             |
///                         +-------------------------+
///                         | Guards -> Compress ->   |
///                         | Think -> Decide ->      |
///                         | Execute -> Feedback     |
///                         | (repeat until done)     |
///                         +-------------------------+
/// ```
#[allow(clippy::too_many_arguments)]
pub fn process_with_agent_loop(
    runtime: &tokio::runtime::Handle,
    input: &str,
    app_context: &Option<String>,
    window_title: &Option<String>,
    config: &RigAgentConfig,
    tool_server_handle: AetherToolServerHandle,
    registered_tools: Arc<RwLock<Vec<String>>>,
    conversation_histories: &Arc<RwLock<HashMap<String, Vec<ChatMessage>>>>,
    topic_id: &Option<String>,
    attachments: Option<&[MediaAttachment]>,
    op_token: &CancellationToken,
    handler: &Arc<dyn AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    generation_config: &GenerationConfig,
    routing_rules: &[RoutingRuleConfig],
    generation_registry: &Arc<RwLock<GenerationProviderRegistry>>,
) {
    // ================================================================
    // Step 0: Set up working directory for this session/topic
    // ================================================================
    // If topic_id is provided, create and use a topic-specific output directory
    // This organizes outputs by conversation/session for better file management
    if let Some(tid) = topic_id {
        if let Ok(output_dir) = crate::utils::paths::get_output_dir() {
            let topic_dir = output_dir.join(tid);
            // Create topic directory if it doesn't exist
            if std::fs::create_dir_all(&topic_dir).is_ok() {
                info!(topic_dir = %topic_dir.display(), topic_id = %tid, "Setting session working directory");
                crate::rig_tools::file_ops::set_working_dir(Some(topic_dir));
            }
        }
    } else {
        // Clear any previous working directory for single-turn mode
        crate::rig_tools::file_ops::set_working_dir(None);
    }

    // ================================================================
    // Step 1: Build IntentRouter with dynamic command sources
    // ================================================================
    let mut command_parser = CommandParser::new();

    // Load skills registry
    if let Ok(skills_dir) = get_skills_dir() {
        let registry = SkillsRegistry::new(skills_dir);
        if registry.load_all().is_ok() {
            command_parser = command_parser.with_skills_registry(Arc::new(registry));
        }
    }

    // Add routing rules for custom commands
    command_parser = command_parser.with_routing_rules(routing_rules.to_vec());

    let router = IntentRouter::new().with_command_parser(Arc::new(command_parser));

    // ================================================================
    // Step 2: Route the input (L0-L2)
    // ================================================================
    let route_result = router.route(input, None);

    match route_result {
        RouteResult::DirectRoute(info) => {
            // Direct execution - skip Agent Loop
            info!(
                layer = ?info.layer,
                latency_us = info.latency_us,
                "Direct route - skipping Agent Loop"
            );

            handle_direct_route(
                runtime,
                input,
                info.mode,
                config,
                tool_server_handle,
                registered_tools,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                generation_config,
                generation_registry,
                attachments,
            );
        }

        RouteResult::NeedsThinking(ctx) => {
            // Needs thinking - analyze task complexity first
            info!(
                category_hint = ?ctx.category_hint,
                bias_execute = ctx.bias_execute,
                latency_us = ctx.latency_us,
                "Needs thinking - analyzing task complexity"
            );

            // Create provider for TaskAnalyzer
            let provider = match create_provider_from_config(config) {
                Ok(p) => p,
                Err(e) => {
                    error!(error = %e, "Failed to create provider for analysis");
                    handler.on_error(format!("Provider error: {}", e));
                    return;
                }
            };

            let analyzer =
                TaskAnalyzer::with_generation_config(provider.clone(), generation_config.clone());

            // Analyze input
            let analysis_result = runtime.block_on(async { analyzer.analyze(input).await });

            match analysis_result {
                Ok(AnalysisResult::SingleStep { intent }) => {
                    info!(intent = %intent, "Single-step task - using Agent Loop");
                    run_agent_loop(
                        runtime,
                        input,
                        ctx,
                        config,
                        tool_server_handle,
                        registered_tools,
                        op_token,
                        handler,
                        memory_config,
                        memory_path,
                        input_for_memory,
                        app_context,
                        window_title,
                        generation_config,
                        attachments,
                        conversation_histories,
                        topic_id,
                        generation_registry,
                    );
                }
                Ok(AnalysisResult::MultiStep {
                    task_graph,
                    requires_confirmation,
                }) => {
                    info!(
                        tasks = task_graph.tasks.len(),
                        requires_confirmation,
                        "Multi-step task - using DAG scheduler"
                    );

                    // Build input with attachment content for DAG context
                    let dag_input =
                        if let Some(attachment_text) = extract_attachment_text(attachments) {
                            info!(
                                attachment_len = attachment_text.len(),
                                "Including attachment text in DAG context"
                            );
                            format!("{}\n\n{}", input, attachment_text)
                        } else {
                            input.to_string()
                        };

                    run_dag_execution(
                        runtime,
                        task_graph,
                        requires_confirmation,
                        provider,
                        op_token,
                        handler,
                        &dag_input,
                        generation_registry,
                        None, // Use default max_task_retries from SchedulerConfig
                    );
                }
                Err(e) => {
                    warn!(error = %e, "Task analysis failed, falling back to Agent Loop");
                    run_agent_loop(
                        runtime,
                        input,
                        ctx,
                        config,
                        tool_server_handle,
                        registered_tools,
                        op_token,
                        handler,
                        memory_config,
                        memory_path,
                        input_for_memory,
                        app_context,
                        window_title,
                        generation_config,
                        attachments,
                        conversation_histories,
                        topic_id,
                        generation_registry,
                    );
                }
            }
        }
    }
}
