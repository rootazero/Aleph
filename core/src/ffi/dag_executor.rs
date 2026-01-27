//! DAG task executor for multi-step task orchestration
//!
//! This module provides DAG (Directed Acyclic Graph) based task execution
//! for multi-step task orchestration with generation task support.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::dispatcher::agent_types::{Task, TaskGraph, TaskType};
use crate::dispatcher::{
    DagScheduler, DagTaskDisplayStatus, DagTaskPlan, ExecutionCallback, TaskContext, TaskOutput,
    UserDecision, MAX_PARALLELISM,
};
use crate::dispatcher::scheduler::GraphTaskExecutor;
use crate::generation::{GenerationParams, GenerationProviderRegistry, GenerationRequest, GenerationType};

use super::prompt_helpers::response_needs_user_input;

/// Comprehensive task executor for DAG nodes
///
/// This executor handles different task types:
/// - Generation tasks (image/video/audio): calls the actual generation provider
/// - AI inference tasks: uses LLM completion
/// - Other tasks: uses LLM with task-specific prompts
pub struct DagTaskExecutor {
    provider: Arc<dyn crate::providers::AiProvider>,
    generation_registry: Arc<std::sync::RwLock<GenerationProviderRegistry>>,
}

impl DagTaskExecutor {
    pub fn new(
        provider: Arc<dyn crate::providers::AiProvider>,
        generation_registry: Arc<std::sync::RwLock<GenerationProviderRegistry>>,
    ) -> Self {
        Self {
            provider,
            generation_registry,
        }
    }

    /// Execute an image generation task
    async fn execute_image_generation(
        &self,
        task: &Task,
        image_gen: &crate::dispatcher::agent_types::ImageGenTask,
    ) -> crate::error::Result<TaskOutput> {
        info!(
            task_id = %task.id,
            provider = %image_gen.provider,
            model = %image_gen.model,
            "Executing image generation task"
        );

        // Get provider from registry (clone Arc to release lock before await)
        let gen_provider = {
            let registry = self.generation_registry.read().map_err(|e| {
                crate::error::AetherError::config(format!(
                    "Failed to read generation registry: {}",
                    e
                ))
            })?;

            registry.get(&image_gen.provider).ok_or_else(|| {
                crate::error::AetherError::config(format!(
                    "Generation provider '{}' not found in registry",
                    image_gen.provider
                ))
            })?
        };

        // Build generation request with model
        let params = GenerationParams::builder().model(&image_gen.model).build();
        let request = GenerationRequest::image(&image_gen.prompt).with_params(params);

        // Execute generation (lock is released before this point)
        let output = gen_provider.generate(request).await.map_err(|e| {
            crate::error::AetherError::provider(format!("Image generation failed: {}", e))
        })?;

        // Format result
        let result = if let Some(url) = output.data.as_url() {
            format!("✓ 图像生成成功: {}", url)
        } else if let Some(path) = output.data.as_local_path() {
            format!("✓ 图像已保存到: {}", path)
        } else {
            "✓ 图像生成成功".to_string()
        };

        info!(task_id = %task.id, "Image generation completed");
        Ok(TaskOutput::text(result))
    }

    /// Execute a video generation task
    async fn execute_video_generation(
        &self,
        task: &Task,
        video_gen: &crate::dispatcher::agent_types::VideoGenTask,
    ) -> crate::error::Result<TaskOutput> {
        info!(
            task_id = %task.id,
            provider = %video_gen.provider,
            model = %video_gen.model,
            "Executing video generation task"
        );

        // Get provider from registry (clone Arc to release lock before await)
        let gen_provider = {
            let registry = self.generation_registry.read().map_err(|e| {
                crate::error::AetherError::config(format!(
                    "Failed to read generation registry: {}",
                    e
                ))
            })?;

            registry.get(&video_gen.provider).ok_or_else(|| {
                crate::error::AetherError::config(format!(
                    "Generation provider '{}' not found in registry",
                    video_gen.provider
                ))
            })?
        };

        // Build generation request with model
        let params = GenerationParams::builder().model(&video_gen.model).build();
        let request = GenerationRequest::video(&video_gen.prompt).with_params(params);

        // Execute generation (lock is released before this point)
        let output = gen_provider.generate(request).await.map_err(|e| {
            crate::error::AetherError::video(format!("Video generation failed: {}", e))
        })?;

        // Format result
        let result = if let Some(url) = output.data.as_url() {
            format!("✓ 视频生成成功: {}", url)
        } else if let Some(path) = output.data.as_local_path() {
            format!("✓ 视频已保存到: {}", path)
        } else {
            "✓ 视频生成成功".to_string()
        };

        info!(task_id = %task.id, "Video generation completed");
        Ok(TaskOutput::text(result))
    }

    /// Execute an audio generation task
    async fn execute_audio_generation(
        &self,
        task: &Task,
        audio_gen: &crate::dispatcher::agent_types::AudioGenTask,
    ) -> crate::error::Result<TaskOutput> {
        info!(
            task_id = %task.id,
            provider = %audio_gen.provider,
            model = %audio_gen.model,
            "Executing audio generation task"
        );

        // Get provider from registry and determine type (release lock before await)
        let (gen_provider, gen_type) = {
            let registry = self.generation_registry.read().map_err(|e| {
                crate::error::AetherError::config(format!(
                    "Failed to read generation registry: {}",
                    e
                ))
            })?;

            let provider = registry.get(&audio_gen.provider).ok_or_else(|| {
                crate::error::AetherError::config(format!(
                    "Generation provider '{}' not found in registry",
                    audio_gen.provider
                ))
            })?;

            // Determine if this is speech or audio based on provider capabilities
            let gen_type = if provider.supports(GenerationType::Speech) {
                GenerationType::Speech
            } else {
                GenerationType::Audio
            };

            (provider, gen_type)
        };

        // Build generation request with model
        let params = GenerationParams::builder().model(&audio_gen.model).build();
        let request = if gen_type == GenerationType::Speech {
            GenerationRequest::speech(&audio_gen.prompt).with_params(params)
        } else {
            GenerationRequest::audio(&audio_gen.prompt).with_params(params)
        };

        // Execute generation (lock is released before this point)
        let output = gen_provider.generate(request).await.map_err(|e| {
            crate::error::AetherError::provider(format!("Audio generation failed: {}", e))
        })?;

        // Format result
        let result = if let Some(url) = output.data.as_url() {
            format!("✓ 音频生成成功: {}", url)
        } else if let Some(path) = output.data.as_local_path() {
            format!("✓ 音频已保存到: {}", path)
        } else {
            "✓ 音频生成成功".to_string()
        };

        info!(task_id = %task.id, "Audio generation completed");
        Ok(TaskOutput::text(result))
    }

    /// Execute a generic LLM task
    async fn execute_llm_task(
        &self,
        task: &Task,
        context: &str,
    ) -> crate::error::Result<TaskOutput> {
        // Truncate context if too large
        const MAX_CONTEXT_CHARS: usize = 50_000;
        let context_to_use = if context.len() > MAX_CONTEXT_CHARS {
            warn!(
                task_id = %task.id,
                actual_len = context.len(),
                "Context too large, truncating to {} chars",
                MAX_CONTEXT_CHARS
            );
            // Safe UTF-8 truncation
            context.chars().take(MAX_CONTEXT_CHARS).collect()
        } else {
            context.to_string()
        };

        let prompt = format!(
            "{}\n\n请执行以下任务:\n任务: {}\n描述: {}\n\n请直接给出结果。",
            context_to_use,
            task.name,
            task.description.as_deref().unwrap_or("无"),
        );

        let response = self.provider.process(&prompt, None).await?;

        // Check if LLM response indicates missing required input
        if response_needs_user_input(&response) {
            warn!(
                task_id = %task.id,
                task_name = %task.name,
                "LLM response indicates missing required input"
            );
            return Err(crate::error::AetherError::MissingInput {
                task_id: task.id.clone(),
                task_name: task.name.clone(),
                message: response,
            });
        }

        Ok(TaskOutput::text(response))
    }
}

#[async_trait::async_trait]
impl GraphTaskExecutor for DagTaskExecutor {
    async fn execute(&self, task: &Task, context: &str) -> crate::error::Result<TaskOutput> {
        match &task.task_type {
            TaskType::ImageGeneration(image_gen) => {
                self.execute_image_generation(task, image_gen).await
            }
            TaskType::VideoGeneration(video_gen) => {
                self.execute_video_generation(task, video_gen).await
            }
            TaskType::AudioGeneration(audio_gen) => {
                self.execute_audio_generation(task, audio_gen).await
            }
            // For all other task types, use LLM completion
            _ => self.execute_llm_task(task, context).await,
        }
    }
}

/// Adapter to convert ExecutionCallback to AetherEventHandler
///
/// This struct bridges the DAG scheduler's callback interface with
/// the FFI event handler, allowing progress updates to flow to the UI.
struct FfiExecutionCallback {
    handler: Arc<dyn crate::ffi::AetherEventHandler>,
}

impl FfiExecutionCallback {
    fn new(handler: Arc<dyn crate::ffi::AetherEventHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait::async_trait]
impl ExecutionCallback for FfiExecutionCallback {
    async fn on_plan_ready(&self, plan: &DagTaskPlan) {
        // Format plan as markdown for display
        let mut output = format!("**任务计划: {}**\n\n", plan.title);
        for (i, task) in plan.tasks.iter().enumerate() {
            let status = match task.status {
                DagTaskDisplayStatus::Pending => "○",
                DagTaskDisplayStatus::Running => "◉",
                DagTaskDisplayStatus::Completed => "✓",
                DagTaskDisplayStatus::Failed => "✗",
                DagTaskDisplayStatus::Cancelled => "⊘",
            };
            output.push_str(&format!("{} {}. {}\n", status, i + 1, task.name));
        }
        output.push_str("\n---\n\n");
        self.handler.on_stream_chunk(output);
    }

    async fn on_confirmation_required(&self, plan: &DagTaskPlan) -> UserDecision {
        use crate::ffi::plan_confirmation::store_pending_confirmation;
        use std::time::Duration;

        // Generate unique plan_id (use the plan's existing ID)
        let plan_id = plan.id.clone();

        info!(
            plan_id = %plan_id,
            task_count = plan.tasks.len(),
            "Requesting user confirmation for DAG plan"
        );

        // Store pending confirmation and get the receiver
        let receiver = store_pending_confirmation(plan_id.clone(), plan.clone());

        // Notify Swift via callback
        self.handler
            .on_plan_confirmation_required(plan_id.clone(), plan.clone());

        // Stream a message to show we're waiting for confirmation
        self.handler
            .on_stream_chunk("⏳ 等待用户确认任务计划...\n".to_string());

        // Wait for the decision with timeout
        const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(300);

        match tokio::time::timeout(CONFIRMATION_TIMEOUT, receiver).await {
            Ok(Ok(decision)) => {
                info!(plan_id = %plan_id, decision = ?decision, "Received user confirmation");
                decision
            }
            Ok(Err(_)) => {
                // Channel closed without sending - treat as cancelled
                warn!(plan_id = %plan_id, "Confirmation channel closed");
                self.handler
                    .on_stream_chunk("⚠️ 确认已取消（内部错误）\n".to_string());
                UserDecision::Cancelled
            }
            Err(_) => {
                // Timeout - treat as cancelled
                warn!(
                    plan_id = %plan_id,
                    "Confirmation timed out after {:?}",
                    CONFIRMATION_TIMEOUT
                );
                self.handler.on_stream_chunk(format!(
                    "⚠️ 确认超时（{}秒），任务已取消\n",
                    CONFIRMATION_TIMEOUT.as_secs()
                ));
                UserDecision::Cancelled
            }
        }
    }

    async fn on_task_start(&self, _task_id: &str, task_name: &str) {
        self.handler
            .on_stream_chunk(format!("\n**[开始]** {}\n", task_name));
    }

    async fn on_task_stream(&self, _task_id: &str, chunk: &str) {
        self.handler.on_stream_chunk(chunk.to_string());
    }

    async fn on_task_complete(&self, _task_id: &str, summary: &str) {
        self.handler.on_stream_chunk(format!("\n✓ {}\n", summary));
    }

    async fn on_task_retry(&self, task_id: &str, attempt: u32, error: &str) {
        self.handler.on_stream_chunk(format!(
            "\n重试 {} (第{}次): {}\n",
            task_id, attempt, error
        ));
    }

    async fn on_task_deciding(&self, task_id: &str, error: &str) {
        self.handler.on_stream_chunk(format!(
            "\n任务 {} 失败，正在决策...\n错误: {}\n",
            task_id, error
        ));
    }

    async fn on_task_failed(&self, task_id: &str, error: &str) {
        self.handler
            .on_stream_chunk(format!("\n✗ {} 失败: {}\n", task_id, error));
    }

    async fn on_all_complete(&self, summary: &str) {
        self.handler
            .on_stream_chunk(format!("\n---\n\n**执行完成**: {}\n", summary));
    }

    async fn on_cancelled(&self) {
        self.handler
            .on_stream_chunk("\n---\n\n**已取消**\n".to_string());
    }
}

/// Run DAG-based multi-step execution
///
/// This function handles multi-step task execution using the DAG scheduler.
/// It creates the necessary components (executor, callback, context) and
/// orchestrates the execution of the task graph.
///
/// # Generation Task Support
///
/// When the task graph contains generation tasks (image/video/audio), this
/// function uses the `generation_registry` to call the actual generation
/// providers instead of just asking the LLM to describe the generation.
#[allow(clippy::too_many_arguments)]
pub fn run_dag_execution(
    runtime: &tokio::runtime::Handle,
    task_graph: TaskGraph,
    _requires_confirmation: bool,
    provider: Arc<dyn crate::providers::AiProvider>,
    op_token: &CancellationToken,
    handler: &Arc<dyn crate::ffi::AetherEventHandler>,
    user_input: &str,
    generation_registry: &Arc<std::sync::RwLock<GenerationProviderRegistry>>,
    max_task_retries: Option<u32>,
) {
    // Check if already cancelled
    if op_token.is_cancelled() {
        handler.on_error("Operation cancelled".to_string());
        return;
    }

    let handler = handler.clone();
    let user_input = user_input.to_string();
    let generation_registry = Arc::clone(generation_registry);

    // Run DAG execution
    let result = runtime.block_on(async {
        // Create callback adapter
        let callback = Arc::new(FfiExecutionCallback::new(handler.clone()));

        // Create executor with generation capabilities
        let executor = Arc::new(DagTaskExecutor::new(provider, generation_registry));

        // Create context
        let context = TaskContext::new(&user_input);

        // Build scheduler config with custom retries if provided
        let scheduler_config = max_task_retries.map(|retries| crate::dispatcher::SchedulerConfig {
            max_parallelism: MAX_PARALLELISM,
            max_task_retries: retries,
        });

        // Execute graph
        DagScheduler::execute_graph(task_graph, executor, callback, context, scheduler_config).await
    });

    match result {
        Ok(exec_result) => {
            if exec_result.cancelled {
                handler.on_error("用户取消了任务执行".to_string());
                return;
            } else if !exec_result.failed_tasks.is_empty() {
                handler.on_error(format!("部分任务失败: {:?}", exec_result.failed_tasks));
                return;
            }
            // Use detailed_summary to include full task results
            let detailed = exec_result.detailed_summary();
            handler.on_complete(detailed);
        }
        Err(e) => {
            handler.on_error(format!("DAG 执行失败: {}", e));
        }
    }
}
