//! Agent manager execution (rig-core path)

use crate::agents::RigAgentManager;
use crate::core::MediaAttachment;
use crate::ffi::AetherEventHandler;
use crate::rig_tools::file_ops::{clear_written_files, take_written_files};
use crate::rig_tools::set_tool_progress_handler;
use rig::completion::Message;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::memory::store_memory_after_response;
use super::progress_callback::FfiToolProgressAdapter;

/// Execute input using RigAgentManager (existing code path)
#[allow(clippy::too_many_arguments)]
pub fn execute_with_agent_manager(
    runtime: &tokio::runtime::Handle,
    processed_input: &str,
    config: &crate::agents::RigAgentConfig,
    tool_server_handle: rig::tool::server::ToolServerHandle,
    registered_tools: Arc<RwLock<Vec<String>>>,
    conversation_histories: &Arc<RwLock<HashMap<String, Vec<Message>>>>,
    topic_id: &Option<String>,
    attachments: Option<&[MediaAttachment]>,
    op_token: &CancellationToken,
    handler: &Arc<dyn AetherEventHandler>,
    memory_config: &crate::config::MemoryConfig,
    memory_path: &Option<String>,
    input_for_memory: &str,
    app_context: &Option<String>,
    window_title: &Option<String>,
) {
    // Clear any previously tracked written files for this session
    clear_written_files();

    // Set up tool progress callback for streaming updates
    let progress_adapter = Arc::new(FfiToolProgressAdapter::new(handler.clone()));
    set_tool_progress_handler(Some(progress_adapter));

    // Create manager with shared ToolServerHandle (all tools persist across calls)
    let manager =
        RigAgentManager::with_shared_handle(config.clone(), tool_server_handle, registered_tools);

    // Stream initial progress to show the skill is being executed
    let initial_chunk = "\n**[开始]** 正在执行任务...\n".to_string();
    info!(chunk_len = initial_chunk.len(), "Sending initial stream chunk");
    handler.on_stream_chunk(initial_chunk);

    // Get or create conversation history for this topic
    let topic_key = topic_id
        .clone()
        .unwrap_or_else(|| "single-turn".to_string());
    let mut history = {
        let histories = conversation_histories.read().unwrap();
        histories.get(&topic_key).cloned().unwrap_or_default()
    };
    let history_len_before = history.len();

    let result = runtime.block_on(async {
        tokio::select! {
            biased;

            // Check for cancellation first (biased mode)
            _ = op_token.cancelled() => {
                Err(crate::error::AetherError::cancelled())
            }

            // Process with conversation history and attachments for multi-turn + multimodal support
            result = manager.process_with_history_and_attachments(processed_input, &mut history, attachments) => {
                result
            }
        }
    });

    // Update conversation history after processing
    // rig-core's with_history() mutates the history to add the current exchange
    if history.len() > history_len_before {
        let mut histories = conversation_histories.write().unwrap();
        histories.insert(topic_key.clone(), history);
        debug!(topic_id = %topic_key, "Conversation history updated");
    }

    // Clear tool progress callback after execution completes
    set_tool_progress_handler(None);

    match result {
        Ok(response) => {
            // Collect files written during tool execution
            let written_files = take_written_files();
            let final_content = if written_files.is_empty() {
                response.content.clone()
            } else {
                // Append file markers that Swift can parse
                // Format: \n\n[GENERATED_FILES]\nfile:///path/to/file1\nfile:///path/to/file2\n[/GENERATED_FILES]
                let file_urls: Vec<String> = written_files
                    .iter()
                    .map(|f| format!("file://{}", f.path.display()))
                    .collect();
                info!(
                    file_count = written_files.len(),
                    files = ?file_urls,
                    "Appending generated files to response"
                );
                format!(
                    "{}\n\n[GENERATED_FILES]\n{}\n[/GENERATED_FILES]",
                    response.content,
                    file_urls.join("\n")
                )
            };

            // Store memory if enabled
            if memory_config.enabled {
                if let Some(ref db_path) = memory_path {
                    let store_result = runtime.block_on(async {
                        store_memory_after_response(
                            db_path,
                            memory_config,
                            input_for_memory,
                            &response.content,
                            app_context.as_deref(),
                            window_title.as_deref(),
                            topic_id.as_deref(),
                        )
                        .await
                    });

                    match store_result {
                        Ok(memory_id) => {
                            info!(memory_id = %memory_id, "Memory stored successfully");
                            handler.on_memory_stored();
                        }
                        Err(e) => {
                            warn!(error = %e, "Failed to store memory (non-blocking)");
                        }
                    }
                }
            }

            // Stream completion message
            handler.on_stream_chunk("\n**[完成]** 任务执行成功\n".to_string());

            // If tokio::select! returned the result branch, the operation completed successfully
            handler.on_complete(final_content);
        }
        Err(e) => {
            // Check if the error is due to cancellation
            if op_token.is_cancelled() {
                handler.on_stream_chunk("\n**[取消]** 操作已取消\n".to_string());
                handler.on_error("Operation cancelled".to_string());
            } else {
                error!(error = %e, "Processing failed");
                handler.on_stream_chunk(format!("\n**[错误]** {}\n", e));
                handler.on_error(e.to_string());
            }
        }
    }
}
