//! Direct route handling for slash commands, skills, MCP, etc.

use crate::agents::RigAgentConfig;
use crate::config::GenerationConfig;
use crate::core::MediaAttachment;
use crate::ffi::AetherEventHandler;
use crate::ffi::prompt_helpers::extract_attachment_text;
use crate::ffi::tool_discovery::get_builtin_tool_descriptions;
use crate::generation::GenerationProviderRegistry;
use crate::intent::{AgentModePrompt, DirectMode, ThinkingContext};
use crate::agents::rig::ChatMessage;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::agent_loop::run_agent_loop;
use super::skill::run_skill_with_agent_loop;

/// Handle direct route cases (slash commands, skills, MCP, custom)
#[allow(clippy::too_many_arguments)]
pub fn handle_direct_route(
    runtime: &tokio::runtime::Handle,
    input: &str,
    mode: DirectMode,
    config: &RigAgentConfig,
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<RwLock<Vec<String>>>,
    op_token: &CancellationToken,
    handler: &Arc<dyn AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    generation_config: &GenerationConfig,
    generation_registry: &Arc<RwLock<GenerationProviderRegistry>>,
    attachments: Option<&[MediaAttachment]>,
) {
    // Extract attachment text if present
    let attachment_text = extract_attachment_text(attachments);
    let tool_descriptions = get_builtin_tool_descriptions(generation_config);
    let conversation_histories = Arc::new(RwLock::new(HashMap::<String, Vec<ChatMessage>>::new()));
    let topic_id = None;

    match mode {
        DirectMode::Tool(tool) => {
            info!(tool_id = %tool.tool_id, "Direct tool execution");

            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let processed_input = format!(
                "{}\n\n---\n\n请使用 {} 工具处理: {}",
                agent_prompt, tool.tool_id, tool.args
            );

            // Create a default ThinkingContext for direct execution
            let ctx = ThinkingContext {
                category_hint: None,
                bias_execute: true,
                hint_layer: None,
                latency_us: 0,
            };

            run_agent_loop(
                runtime,
                &processed_input,
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
                &None,
                generation_config,
                attachments,
                &conversation_histories,
                &topic_id,
                generation_registry,
            );
        }

        DirectMode::Skill(skill) => {
            info!(skill_id = %skill.skill_id, "Direct skill execution via AgentLoop");

            // Route skill execution through AgentLoop for full streaming support
            // This provides Claude Code CLI style progress updates
            run_skill_with_agent_loop(
                runtime,
                &skill,
                config,
                op_token,
                handler,
                memory_config,
                memory_path,
                input_for_memory,
                app_context,
                generation_config,
                generation_registry,
                attachments,
                attachment_text.as_deref(),
            );
        }

        DirectMode::Mcp(mcp) => {
            info!(server_name = %mcp.server_name, "Direct MCP execution");

            let agent_prompt = AgentModePrompt::with_tools(tool_descriptions.clone())
                .with_generation_config(generation_config)
                .generate();

            let tool_hint = if let Some(ref tool) = mcp.tool_name {
                format!(
                    "请使用 {} 工具（来自 {} 服务器）处理: {}",
                    tool, mcp.server_name, mcp.args
                )
            } else {
                format!(
                    "请使用 {} MCP 服务器的工具处理: {}",
                    mcp.server_name, mcp.args
                )
            };

            let processed_input = format!("{}\n\n---\n\n{}", agent_prompt, tool_hint);

            // Create a default ThinkingContext for direct execution
            let ctx = ThinkingContext {
                category_hint: None,
                bias_execute: true,
                hint_layer: None,
                latency_us: 0,
            };

            run_agent_loop(
                runtime,
                &processed_input,
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
                &None,
                generation_config,
                attachments,
                &conversation_histories,
                &topic_id,
                generation_registry,
            );
        }

        DirectMode::Custom(custom) => {
            info!(command_name = %custom.command_name, "Direct custom command execution");

            let prompt = custom.system_prompt.unwrap_or_else(|| {
                AgentModePrompt::with_tools(tool_descriptions.clone())
                    .with_generation_config(generation_config)
                    .generate()
            });

            let processed_input = format!("{}\n\n---\n\n用户输入: {}", prompt, input);

            // Create a default ThinkingContext for direct execution
            let ctx = ThinkingContext {
                category_hint: None,
                bias_execute: true,
                hint_layer: None,
                latency_us: 0,
            };

            run_agent_loop(
                runtime,
                &processed_input,
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
                &None,
                generation_config,
                attachments,
                &conversation_histories,
                &topic_id,
                generation_registry,
            );
        }
    }
}
