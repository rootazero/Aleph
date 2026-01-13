//! AI processing pipeline for AetherCore
//!
//! This module contains the main AI processing methods:
//! - process_input: Main entry point from Swift
//! - process_with_ai_first: AI-first detection mode
//! - execute_capability_and_continue: Capability execution
//! - build_enriched_payload: Context enrichment

use super::types::{CapturedContext, StorageHelper};
use super::AetherCore;
use crate::clarification::{ClarificationOption, ClarificationRequest};
use crate::error::{AetherError, AetherException, Result};
use crate::event_handler::ProcessingState;
use crate::utils::pii;
use std::sync::Arc;
use tracing::{debug, error, info};

impl AetherCore {
    // ========================================================================
    // AI PROCESSING PIPELINE
    // ========================================================================

    /// Handle processing error with user-friendly messaging
    ///
    /// This helper centralizes error handling logic for AI processing failures.
    /// It extracts user-friendly messages, logs errors, notifies the event handler,
    /// and returns an AetherException.
    ///
    /// # Arguments
    ///
    /// * `error` - The AetherError to handle
    ///
    /// # Returns
    ///
    /// AetherException::Error for UniFFI compatibility
    pub(crate) fn handle_processing_error(&self, error: &AetherError) -> AetherException {
        let friendly_message = error.user_friendly_message();
        let suggestion = error.suggestion().map(|s| s.to_string());

        error!(error = ?error, user_message = %friendly_message, "AI processing failed");

        // Notify Swift layer with detailed error
        self.event_handler.on_error(friendly_message, suggestion);
        self.event_handler.on_state_changed(ProcessingState::Error);

        AetherException::Error
    }

    /// Process input with AI using the complete pipeline: Memory → Router → Provider → Storage
    ///
    /// This is the NEW entry point for the refactored architecture (Phase 2: Native API Separation).
    /// Swift layer handles system interactions (clipboard, hotkeys, keyboard simulation),
    /// and calls this method with pre-processed user input and captured context.
    ///
    /// Pipeline:
    /// 1. Set current context (for memory retrieval)
    /// 2. Retrieve relevant memories based on context
    /// 3. Augment prompt with memory context
    /// 4. Route to appropriate AI provider
    /// 5. Call provider.process() with augmented input
    /// 6. Store interaction for future retrieval (async, non-blocking)
    ///
    /// # Arguments
    ///
    /// * `user_input` - User input text (from Swift ClipboardManager)
    /// * `context` - Captured context (app bundle ID + window title from Swift ContextCapture)
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - AI-generated response (Swift will use KeyboardSimulator to output)
    /// * `Err(AetherException)` - Various errors
    pub fn process_input(
        &self,
        user_input: String,
        context: CapturedContext,
    ) -> std::result::Result<String, AetherException> {
        use std::time::Instant;
        let start_time = Instant::now();

        info!(
            input_length = user_input.len(),
            app = %context.app_bundle_id,
            window = ?context.window_title,
            "Processing input via new architecture (Swift → Rust)"
        );

        // Store context for memory operations
        self.set_current_context(context.clone());

        // Try the new intent routing pipeline if enabled
        if let Some(ref pipeline) = self.intent_pipeline {
            match self.process_with_pipeline(pipeline, user_input.clone(), context.clone(), start_time) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    // Log warning but fall back to AI-first mode
                    tracing::warn!(error = %e, "Pipeline processing failed, falling back to AI-first mode");
                }
            }
        }

        // AI-First Mode: AI decides if capability is needed in a single call
        // This is the fallback processing mode when pipeline is disabled or fails
        match self.process_with_ai_first(user_input.clone(), context.clone(), start_time) {
            Ok(response) => Ok(response),
            Err(e) => Err(self.handle_processing_error(&e)),
        }
    }

    /// Process input using the intent routing pipeline
    ///
    /// The pipeline provides:
    /// - Fast path via intent cache
    /// - Multi-layer routing (L1 regex, L2 semantic, L3 AI inference)
    /// - Confidence calibration
    /// - Clarification flow for missing parameters
    fn process_with_pipeline(
        &self,
        pipeline: &std::sync::Arc<crate::routing::IntentRoutingPipeline>,
        user_input: String,
        context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::routing::{PipelineResult, RoutingContext};

        info!("Processing with intent routing pipeline");

        // Build routing context
        let mut routing_ctx = RoutingContext::new(&user_input);

        // Add app context
        let app_name = context.app_bundle_id.split('.').next_back().unwrap_or("Unknown");
        routing_ctx = routing_ctx.with_app(
            Some(context.app_bundle_id.clone()),
            context.window_title.clone(),
        );
        routing_ctx.entity_hints.push(app_name.to_string());
        if let Some(ref title) = context.window_title {
            routing_ctx.entity_hints.push(title.clone());
        }

        // Process through pipeline
        let result = self.runtime.block_on(pipeline.process(routing_ctx));

        match result {
            PipelineResult::Executed { tool_name, content, .. } => {
                info!(
                    tool = %tool_name,
                    latency_ms = start_time.elapsed().as_millis(),
                    "Pipeline: Tool executed successfully"
                );
                Ok(content)
            }
            PipelineResult::ToolMatched {
                tool_name,
                parameters,
                input,
            } => {
                // Execute the matched tool using capability executor
                info!(
                    tool = %tool_name,
                    "Pipeline: Tool matched, executing capability"
                );
                self.execute_matched_tool(tool_name, parameters, input, context, start_time)
            }
            PipelineResult::GeneralChat { input, .. } => {
                // Fall back to AI-first processing for general chat
                debug!(
                    input = %input,
                    "Pipeline: No tool matched, falling back to AI-first"
                );
                Err(AetherError::other("Pipeline returned GeneralChat, use AI-first"))
            }
            PipelineResult::PendingClarification(request) => {
                // Handle clarification request
                info!(
                    session_id = %request.session_id,
                    prompt = %request.prompt,
                    "Pipeline: Clarification needed"
                );

                // Convert to ClarificationRequest for UI
                use crate::clarification::{ClarificationOption, ClarificationRequest as UiRequest};

                let ui_request = if request.suggestions.is_empty() {
                    UiRequest::text(&request.session_id, &request.prompt, None)
                } else {
                    let options: Vec<ClarificationOption> = request.suggestions.iter()
                        .map(|s| ClarificationOption::new(s, s))
                        .collect();
                    UiRequest::select(&request.session_id, &request.prompt, options)
                };

                // Notify UI
                let clarification_result = self.event_handler.on_clarification_needed(ui_request);

                if clarification_result.is_success() {
                    if let Some(value) = clarification_result.get_value() {
                        // Resume pipeline with user's clarification
                        let resume_result = self.runtime.block_on(
                            pipeline.resume_clarification(&request.session_id, &value)
                        );

                        match resume_result {
                            PipelineResult::Executed { content, .. } => Ok(content),
                            PipelineResult::ToolMatched {
                                tool_name,
                                parameters,
                                input,
                            } => {
                                self.execute_matched_tool(
                                    tool_name,
                                    parameters,
                                    input,
                                    context.clone(),
                                    start_time,
                                )
                            }
                            PipelineResult::GeneralChat { .. } => {
                                Err(AetherError::other("Pipeline returned GeneralChat after clarification"))
                            }
                            _ => Err(AetherError::other("Unexpected pipeline result after clarification"))
                        }
                    } else {
                        Err(AetherError::other("Clarification cancelled"))
                    }
                } else {
                    Err(AetherError::other("Clarification cancelled or timed out"))
                }
            }
            PipelineResult::Cancelled { reason } => {
                debug!(reason = %reason, "Pipeline: Cancelled");
                Err(AetherError::other(format!("Pipeline cancelled: {}", reason)))
            }
            PipelineResult::Skipped { reason } => {
                debug!(reason = %reason, "Pipeline: Skipped");
                Err(AetherError::other(format!("Pipeline skipped: {}", reason)))
            }
        }
    }

    /// Execute a matched tool from the intent routing pipeline.
    ///
    /// This method handles the actual execution of tools/capabilities that were
    /// matched by the pipeline. It:
    /// 1. Checks if tool is a builtin capability or native tool
    /// 2. For builtins: Maps tool names to capabilities (e.g., "youtube" -> Video)
    /// 3. For native tools: Executes via UnifiedToolExecutor
    /// 4. Calls AI provider to generate response with tool results
    fn execute_matched_tool(
        &self,
        tool_name: String,
        parameters: serde_json::Value,
        input: String,
        context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::core::tool_executor::UnifiedToolExecutor;
        use crate::payload::{Capability, ContextFormat};

        // Check if this is a builtin capability
        let capability = UnifiedToolExecutor::resolve_builtin_capability(&tool_name);

        // If not a builtin, try executing as native tool
        if capability.is_none() {
            // Map "fetch" command to "web_fetch" native tool
            let native_tool_name = if tool_name == "fetch" {
                "web_fetch".to_string()
            } else {
                tool_name.clone()
            };

            info!(tool = %native_tool_name, original = %tool_name, "Attempting native tool execution");

            // Execute native tool via NativeToolRegistry
            let registry = Arc::clone(&self.native_tool_registry);
            let args = serde_json::to_string(&parameters)?;

            let tool_result = self.runtime.block_on(async {
                registry.execute(&native_tool_name, &args).await
            });

            match tool_result {
                Ok(result) if result.is_success() => {
                    info!(
                        tool = %native_tool_name,
                        content_length = result.content.len(),
                        "Native tool executed successfully"
                    );

                    // Now make AI call with tool result
                    return self.synthesize_tool_result(
                        &native_tool_name,
                        &result.content,
                        &input,
                        context,
                        start_time,
                    );
                }
                Ok(result) => {
                    // Tool returned error
                    let error_msg = result.error.unwrap_or_else(|| "Unknown error".to_string());
                    debug!(tool = %native_tool_name, error = %error_msg, "Native tool returned error");
                    return Err(AetherError::other(format!(
                        "Tool '{}' failed: {}",
                        native_tool_name, error_msg
                    )));
                }
                Err(e) => {
                    // Tool not found or execution error
                    debug!(tool = %native_tool_name, error = %e, "Native tool execution failed, falling back to AI-first");
                    return Err(AetherError::other(format!(
                        "Unknown tool: {}, falling back to AI-first",
                        native_tool_name
                    )));
                }
            }
        }

        // It's a builtin capability
        let capability = capability.unwrap();

        info!(
            tool = %tool_name,
            capability = ?capability,
            "Executing matched tool capability"
        );

        // Update UI state
        self.event_handler
            .on_state_changed(ProcessingState::RetrievingMemory);

        // Extract query from parameters or use input
        let query = parameters
            .get("url")
            .or_else(|| parameters.get("query"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                // Try to extract URL or query from input
                // For YouTube, the input might be "/youtube https://..."
                input
                    .strip_prefix("/youtube")
                    .or_else(|| input.strip_prefix("/video"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or(input.clone())
            });

        info!(
            query = %query,
            original_input = %input,
            "Extracted query for capability execution"
        );

        // Build capabilities list - always include memory if available
        let mut capabilities = vec![capability];
        if self.memory_db.is_some() && !capabilities.contains(&Capability::Memory) {
            capabilities.push(Capability::Memory);
        }

        // Get AI provider
        let provider = self
            .get_default_provider_instance()
            .ok_or_else(|| AetherError::config("No AI provider available"))?;

        // Build enriched payload using capability executor
        let enriched_payload = self.runtime.block_on(self.build_enriched_payload(
            query.clone(),
            context.clone(),
            provider.name().to_string(),
            capabilities,
        ))?;

        // Check if we got capability results
        let has_video = enriched_payload.context.video_transcript.is_some();
        let has_search = enriched_payload.context.search_results.is_some();
        let has_memory = enriched_payload.context.memory_snippets.is_some();

        info!(
            has_video = %has_video,
            has_search = %has_search,
            has_memory = %has_memory,
            "Capability execution completed"
        );

        // Use default system prompt for tool execution
        // The prompt assembler will add capability context
        let base_prompt = "You are a helpful AI assistant.";

        // Assemble enriched prompt with capability results
        let assembler = crate::payload::PromptAssembler::new(ContextFormat::Markdown);
        let enriched_prompt = assembler.assemble_system_prompt(&base_prompt, &enriched_payload);

        info!(
            enriched_prompt_length = enriched_prompt.len(),
            "Making AI call with enriched context"
        );

        // Make AI call with enriched context
        let attachments = context.attachments.as_ref().map(|a| a.as_slice());
        let response = self.runtime.block_on(
            provider.process_with_attachments(&input, attachments, Some(&enriched_prompt)),
        )?;

        info!(
            response_length = response.len(),
            elapsed_ms = start_time.elapsed().as_millis(),
            "Pipeline: Tool execution completed"
        );

        // Store in memory asynchronously if enabled
        if self.memory_db.is_some() {
            let user_input = input.clone();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            self.runtime.spawn(async move {
                match core_clone
                    .store_interaction_memory(user_input, ai_output)
                    .await
                {
                    Ok(memory_id) => {
                        debug!(
                            memory_id = %memory_id,
                            "Pipeline tool response stored in memory"
                        );
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            "Failed to store pipeline tool response in memory"
                        );
                    }
                }
            });
        }

        // Record turn for compression scheduling
        self.record_conversation_turn();

        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(response)
    }

    /// Synthesize AI response from native tool execution result.
    ///
    /// This method takes the raw output from a native tool (like web_fetch)
    /// and makes an AI call to generate a user-friendly response.
    fn synthesize_tool_result(
        &self,
        tool_name: &str,
        tool_content: &str,
        original_input: &str,
        context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        info!(
            tool = %tool_name,
            content_length = tool_content.len(),
            "Synthesizing AI response from tool result"
        );

        // Get AI provider
        let provider = self
            .get_default_provider_instance()
            .ok_or_else(|| AetherError::config("No AI provider available"))?;

        // Build system prompt with tool result
        let system_prompt = format!(
            "You are a helpful AI assistant. The user requested help and the '{}' tool was executed to gather information.\n\n\
             ## Tool Result\n\n\
             <tool_result>\n{}\n</tool_result>\n\n\
             Based on this information, please provide a helpful response to the user's original request. \
             Summarize, explain, or answer their question using the tool output above. \
             If the tool result is in a foreign language, translate and summarize appropriately. \
             Be concise but comprehensive.",
            tool_name,
            // Truncate very long content to avoid exceeding token limits
            if tool_content.len() > 80000 {
                format!("{}...\n\n[Content truncated for length]", &tool_content[..80000])
            } else {
                tool_content.to_string()
            }
        );

        // Make AI call with tool result context
        let attachments = context.attachments.as_ref().map(|a| a.as_slice());
        let response = self.runtime.block_on(
            provider.process_with_attachments(original_input, attachments, Some(&system_prompt)),
        )?;

        info!(
            tool = %tool_name,
            response_length = response.len(),
            elapsed_ms = start_time.elapsed().as_millis(),
            "Native tool + AI synthesis completed"
        );

        // Store in memory asynchronously if enabled
        if self.memory_db.is_some() {
            let user_input = original_input.to_string();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            self.runtime.spawn(async move {
                match core_clone
                    .store_interaction_memory(user_input, ai_output)
                    .await
                {
                    Ok(memory_id) => {
                        debug!(
                            memory_id = %memory_id,
                            "Native tool response stored in memory"
                        );
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            "Failed to store native tool response in memory"
                        );
                    }
                }
            });
        }

        // Record turn for compression scheduling
        self.record_conversation_turn();

        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(response)
    }

    /// AI-First processing mode.
    ///
    /// In this mode, the AI receives information about available capabilities and decides
    /// whether to respond directly or request capability invocation via a structured JSON response.
    ///
    /// Flow:
    /// 1. Build capability declarations based on enabled features
    /// 2. Create capability-aware system prompt
    /// 3. Make single AI call
    /// 4. Parse response for capability requests
    /// 5. If capability requested, execute it and make second AI call with results
    /// 6. Return final response
    pub(crate) fn process_with_ai_first(
        &self,
        input: String,
        context: CapturedContext,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::capability::{
            AiResponse, CapabilityDeclaration, CapabilityRegistry, McpToolInfo, ResponseParser,
        };
        use crate::payload::ContextFormat;

        info!("Using AI-first detection mode");

        // SECURITY: Scrub PII (including API keys) from user input before sending to AI
        // This prevents accidental leakage of sensitive data from clipboard context
        let input = pii::scrub_pii(&input);
        debug!(
            input_length = input.len(),
            "PII scrubbing applied to user input"
        );

        // Step 1: Get router and configuration
        let router = {
            let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
            router_guard
                .as_ref()
                .map(Arc::clone)
                .ok_or(AetherError::NoProviderAvailable {
                    suggestion: Some(
                        "Configure at least one AI provider in Settings → Providers".to_string(),
                    ),
                })?
        };

        let config = self.lock_config();
        let search_enabled = config.smart_flow.intent_detection.search
            && self
                .search_registry
                .read()
                .ok()
                .and_then(|r| r.as_ref().map(|_| ()))
                .is_some();
        let video_enabled = config.smart_flow.intent_detection.video
            && config.video.as_ref().map(|v| v.enabled).unwrap_or(false);
        let memory_enabled = config.memory.enabled;
        drop(config);

        // Step 2: Get native tools from NativeToolRegistry
        // MCP tools are now handled via AgentTool infrastructure
        let mcp_tools: Option<Vec<McpToolInfo>> = {
            let definitions = self.get_native_tool_definitions();
            if definitions.is_empty() {
                None
            } else {
                Some(
                    definitions
                        .into_iter()
                        .map(|def| McpToolInfo {
                            name: def.name,
                            description: def.description,
                            input_schema: def.parameters,
                            requires_confirmation: def.requires_confirmation,
                        })
                        .collect(),
                )
            }
        };

        let mcp_tool_count = mcp_tools.as_ref().map(|t: &Vec<McpToolInfo>| t.len()).unwrap_or(0);

        // Step 3: Build capability declarations (including MCP tools)
        let registry = CapabilityRegistry::with_all_capabilities(search_enabled, video_enabled, mcp_tools);
        let capabilities: Vec<CapabilityDeclaration> = registry.all().to_vec();

        info!(
            search_enabled = search_enabled,
            video_enabled = video_enabled,
            mcp_tool_count = mcp_tool_count,
            capability_count = capabilities.len(),
            "Built capability registry for AI-first mode"
        );

        // Step 3: Route to get provider (use existing routing for provider selection)
        let routing_context = Self::build_routing_context(&context, &input);
        let routing_match = router.match_rules(&routing_context);

        let provider_name = routing_match
            .provider_name()
            .map(|s| s.to_string())
            .or_else(|| router.default_provider_name().map(|s| s.to_string()))
            .ok_or(AetherError::NoProviderAvailable {
                suggestion: Some("No default provider configured".to_string()),
            })?;

        let provider = router
            .get_provider_arc(&provider_name)
            .ok_or(AetherError::NoProviderAvailable {
                suggestion: Some(format!("Provider '{}' not found", provider_name)),
            })?;

        // Step 4: Build capability-aware system prompt
        let base_prompt = routing_match
            .assemble_prompt()
            .unwrap_or_else(|| "You are a helpful AI assistant.".to_string());

        // Get memory context if enabled
        let memory_context = if memory_enabled {
            self.get_memory_context_for_ai_first(&input, &context)?
        } else {
            None
        };

        let assembler = crate::payload::PromptAssembler::new(ContextFormat::Markdown);
        let system_prompt = assembler.build_capability_aware_prompt(
            &base_prompt,
            &capabilities,
            memory_context.as_ref(),
        );

        info!(
            provider = %provider_name,
            system_prompt_length = system_prompt.len(),
            "Making AI-first call with capability-aware prompt"
        );

        // Step 5: Notify UI and make AI call
        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        // CRITICAL: Use process_with_attachments to pass multimodal content (images, etc.)
        // to the AI provider. Without this, attachments in context would be ignored.
        let attachments = context.attachments.as_ref().map(|a| a.as_slice());
        let response = self
            .runtime
            .block_on(provider.process_with_attachments(&input, attachments, Some(&system_prompt)))?;

        // Step 6: Parse response for capability requests
        let parsed = ResponseParser::parse(&response)?;

        match parsed {
            AiResponse::Direct(text) => {
                // No capability needed - return directly
                info!(
                    response_length = text.len(),
                    elapsed_ms = start_time.elapsed().as_millis(),
                    "AI-first: Direct response (no capability invocation)"
                );

                // Notify UI about AI response
                let response_preview = if text.chars().count() > 100 {
                    let truncated: String = text.chars().take(100).collect();
                    format!("{}...", truncated)
                } else {
                    text.clone()
                };
                self.event_handler.on_ai_response_received(response_preview);

                // Store in memory asynchronously if enabled
                if self.memory_db.is_some() {
                    let user_input = input.clone();
                    let ai_output = text.clone();
                    let core_clone = self.clone_for_storage();

                    self.runtime.spawn(async move {
                        match core_clone
                            .store_interaction_memory(user_input, ai_output)
                            .await
                        {
                            Ok(memory_id) => {
                                log::debug!("[AI-first] Memory stored: {}", memory_id);
                            }
                            Err(e) => {
                                log::error!("[AI-first] Failed to store memory: {}", e);
                            }
                        }
                    });
                }

                // Record turn for compression scheduling
                self.record_conversation_turn();

                Ok(text)
            }
            AiResponse::CapabilityRequest(request) => {
                // Capability requested - execute and continue
                info!(
                    capability = %request.capability,
                    query = %request.query,
                    reasoning = ?request.reasoning,
                    "AI-first: Capability invocation requested"
                );

                self.execute_capability_and_continue(
                    request,
                    &input,
                    context,
                    provider,
                    &base_prompt,
                    start_time,
                )
            }
            AiResponse::NeedsClarification(info) => {
                // AI needs more information from user
                info!(
                    reason = %info.reason,
                    prompt = %info.prompt,
                    has_suggestions = info.has_suggestions(),
                    "AI-first: Clarification needed from user"
                );

                // Convert ClarificationInfo to ClarificationRequest for the callback
                let clarification_request = if info.has_suggestions() {
                    // If AI provided suggestions, create a Select-type request
                    let options: Vec<ClarificationOption> = info
                        .suggestions
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|s| ClarificationOption::new(s, s))
                        .collect();
                    ClarificationRequest::select(
                        &format!("ai-clarification-{}", uuid::Uuid::new_v4()),
                        &info.prompt,
                        options,
                    )
                    .with_source("ai-intent")
                } else {
                    // No suggestions - create a Text-type request
                    ClarificationRequest::text(
                        &format!("ai-clarification-{}", uuid::Uuid::new_v4()),
                        &info.prompt,
                        Some(&info.context_summary),
                    )
                    .with_source("ai-intent")
                };

                // Notify UI that clarification is needed
                let result = self
                    .event_handler
                    .on_clarification_needed(clarification_request);

                // Handle the result
                if result.is_success() {
                    if let Some(value) = result.get_value() {
                        // User provided clarification - append to original input and reprocess
                        let augmented_input = format!("{}\n\n用户补充: {}", input, value);
                        info!(
                            original_input = %input,
                            clarification = %value,
                            "Reprocessing with user clarification"
                        );
                        // Recursive call with augmented input (new start time for the clarified request)
                        return self.process_with_ai_first(
                            augmented_input,
                            context.clone(),
                            std::time::Instant::now(),
                        );
                    }
                }

                // User cancelled or timeout - return the prompt as indication
                Ok(info.prompt)
            }
        }
    }

    /// Get memory context for AI-first mode.
    fn get_memory_context_for_ai_first(
        &self,
        _input: &str,
        _context: &CapturedContext,
    ) -> Result<Option<crate::payload::AgentContext>> {
        // For MVP, we don't pre-fetch memory context
        // Memory will be included if the AI requests a capability that needs it
        // This keeps the first call lightweight
        Ok(None)
    }

    /// Execute the requested capability and continue with a second AI call.
    fn execute_capability_and_continue(
        &self,
        request: crate::capability::CapabilityRequest,
        original_input: &str,
        context: CapturedContext,
        provider: Arc<dyn crate::providers::AiProvider>,
        base_prompt: &str,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::payload::{Capability, ContextFormat};

        // Map capability ID to Capability enum
        let capability = match request.capability.as_str() {
            "search" => Capability::Search,
            "video" => Capability::Video,
            "mcp" => Capability::Mcp,
            _ => {
                return Err(AetherError::config(format!(
                    "Unknown capability: {}",
                    request.capability
                )))
            }
        };

        info!(
            capability = ?capability,
            "Executing capability from AI-first request"
        );

        // Update UI state
        if capability == Capability::Search {
            self.event_handler
                .on_state_changed(ProcessingState::RetrievingMemory); // Reusing state
        }

        // Handle MCP capability specially - execute the tool directly
        if capability == Capability::Mcp {
            return self.execute_mcp_tool_and_continue(
                request,
                original_input,
                context,
                provider,
                base_prompt,
                start_time,
            );
        }

        // Build capabilities list - always include memory if available
        let mut capabilities = vec![capability];
        if self.memory_db.is_some() && !capabilities.contains(&Capability::Memory) {
            capabilities.push(Capability::Memory);
        }

        // Determine the search query to use:
        // 1. If AI provided a specific query in parameters.query, use that (more precise)
        // 2. Otherwise fall back to the original user query
        let search_query = request
            .parameters
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| request.query.clone());

        info!(
            original_query = %request.query,
            search_query = %search_query,
            has_parameter_query = request.parameters.contains_key("query"),
            "Using search query from AI capability request"
        );

        // Build enriched payload using existing infrastructure
        let enriched_payload = self.runtime.block_on(self.build_enriched_payload(
            search_query,
            context.clone(),
            provider.name().to_string(),
            capabilities,
        ))?;

        // Assemble enriched prompt with capability results
        let assembler = crate::payload::PromptAssembler::new(ContextFormat::Markdown);
        let enriched_prompt = assembler.assemble_system_prompt(base_prompt, &enriched_payload);

        info!(
            enriched_prompt_length = enriched_prompt.len(),
            has_search_results = enriched_payload.context.search_results.is_some(),
            has_video_transcript = enriched_payload.context.video_transcript.is_some(),
            has_memory = enriched_payload.context.memory_snippets.is_some(),
            "Making second AI call with enriched context"
        );

        // Make second AI call with enriched context
        // Pass attachments for multimodal content support
        let attachments = context.attachments.as_ref().map(|a| a.as_slice());
        let response = self.runtime.block_on(
            provider.process_with_attachments(&request.query, attachments, Some(&enriched_prompt)),
        )?;

        info!(
            response_length = response.len(),
            elapsed_ms = start_time.elapsed().as_millis(),
            "AI-first: Response with capability results"
        );

        // Store in memory asynchronously if enabled
        if self.memory_db.is_some() {
            let user_input = original_input.to_string();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            self.runtime.spawn(async move {
                match core_clone
                    .store_interaction_memory(user_input, ai_output)
                    .await
                {
                    Ok(memory_id) => {
                        log::debug!(
                            "[AI-first] Capability response memory stored: {}",
                            memory_id
                        );
                    }
                    Err(e) => {
                        log::error!(
                            "[AI-first] Failed to store capability response memory: {}",
                            e
                        );
                    }
                }
            });
        }

        // Record turn for compression scheduling
        self.record_conversation_turn();

        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(response)
    }

    /// Execute MCP tool and continue with a second AI call with tool results.
    ///
    /// This method handles MCP capability requests by:
    /// 1. Extracting tool name and args from the request
    /// 2. Calling the MCP tool via McpClient
    /// 3. Building a payload with the tool results
    /// 4. Making a second AI call to interpret the results
    fn execute_mcp_tool_and_continue(
        &self,
        request: crate::capability::CapabilityRequest,
        original_input: &str,
        context: CapturedContext,
        provider: Arc<dyn crate::providers::AiProvider>,
        base_prompt: &str,
        start_time: std::time::Instant,
    ) -> Result<String> {
        use crate::payload::{ContextFormat, McpToolResult as PayloadMcpToolResult};

        // Extract tool name and args from the request
        let tool_name = request
            .parameters
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AetherError::config("MCP capability request missing 'tool' parameter"))?;

        let tool_args = request
            .parameters
            .get("args")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        info!(
            tool = %tool_name,
            args = %tool_args,
            "Executing MCP tool from AI capability request"
        );

        // Get MCP client
        let mcp_client = self.mcp_client.as_ref().ok_or_else(|| {
            AetherError::config("MCP capability requested but no MCP client available")
        })?;

        // Execute the tool
        let tool_result = self.runtime.block_on(async {
            mcp_client.call_tool(tool_name, tool_args).await
        });

        // Build the MCP tool result for the payload
        let mcp_tool_result = match tool_result {
            Ok(result) => {
                info!(
                    tool = %tool_name,
                    success = result.success,
                    "MCP tool execution completed"
                );
                PayloadMcpToolResult {
                    tool_name: tool_name.to_string(),
                    success: result.success,
                    content: result.content,
                    error: result.error,
                }
            }
            Err(e) => {
                tracing::warn!(
                    tool = %tool_name,
                    error = %e,
                    "MCP tool execution failed"
                );
                PayloadMcpToolResult {
                    tool_name: tool_name.to_string(),
                    success: false,
                    content: serde_json::json!({}),
                    error: Some(e.to_string()),
                }
            }
        };

        // Build payload with MCP tool result using ContextAnchor helper
        let anchor = crate::payload::ContextAnchor::from_captured_context(&context);

        // Get current timestamp as i64
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut payload = crate::payload::PayloadBuilder::new()
            .meta(
                crate::payload::Intent::GeneralChat,
                timestamp,
                anchor,
            )
            .config(
                provider.name().to_string(),
                vec![],
                ContextFormat::Markdown,
            )
            .user_input(request.query.clone())
            .build()
            .map_err(|e| AetherError::config(e))?;

        // Set the MCP tool result
        payload.context.mcp_tool_result = Some(mcp_tool_result);

        // Also get memory context if available
        if self.memory_db.is_some() {
            if let Ok(Some(memory_context)) = self.get_memory_context_for_ai_first(original_input, &context) {
                // get_memory_context_for_ai_first returns AgentContext, extract memory_snippets
                if let Some(snippets) = memory_context.memory_snippets {
                    payload.context.memory_snippets = Some(snippets);
                }
            }
        }

        // Assemble enriched prompt with MCP tool results
        let assembler = crate::payload::PromptAssembler::new(ContextFormat::Markdown);
        let enriched_prompt = assembler.assemble_system_prompt(base_prompt, &payload);

        info!(
            enriched_prompt_length = enriched_prompt.len(),
            has_mcp_result = payload.context.mcp_tool_result.is_some(),
            has_memory = payload.context.memory_snippets.is_some(),
            "Making second AI call with MCP tool results"
        );

        // Make second AI call with enriched context
        let attachments = context.attachments.as_ref().map(|a| a.as_slice());
        let response = self.runtime.block_on(
            provider.process_with_attachments(&request.query, attachments, Some(&enriched_prompt)),
        )?;

        info!(
            response_length = response.len(),
            elapsed_ms = start_time.elapsed().as_millis(),
            "AI-first: Response with MCP tool results"
        );

        // Store in memory asynchronously if enabled
        if self.memory_db.is_some() {
            let user_input = original_input.to_string();
            let ai_output = response.clone();
            let core_clone = self.clone_for_storage();

            self.runtime.spawn(async move {
                match core_clone
                    .store_interaction_memory(user_input, ai_output)
                    .await
                {
                    Ok(memory_id) => {
                        log::debug!(
                            "[AI-first] MCP tool response memory stored: {}",
                            memory_id
                        );
                    }
                    Err(e) => {
                        log::error!(
                            "[AI-first] Failed to store MCP tool response memory: {}",
                            e
                        );
                    }
                }
            });
        }

        // Record turn for compression scheduling
        self.record_conversation_turn();

        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(response)
    }

    /// Build routing context string from window context and clipboard content
    ///
    /// Format: `ClipboardContent\n---\n[AppName] WindowTitle`
    ///
    /// IMPORTANT: Clipboard content is placed FIRST to maintain backward compatibility
    /// with rules like `^/en` that expect content to start with a command prefix.
    pub(crate) fn build_routing_context(context: &CapturedContext, clipboard_content: &str) -> String {
        // Extract app name from bundle ID (e.g., "com.apple.Notes" → "Notes")
        let app_name = context
            .app_bundle_id
            .split('.')
            .next_back()
            .unwrap_or("Unknown");

        // Format: ClipboardContent\n---\n[AppName] WindowTitle
        // Clipboard content is FIRST to preserve backward compatibility with ^/prefix rules
        format!(
            "{}\n---\n[{}] {}",
            clipboard_content,
            app_name,
            context.window_title.as_deref().unwrap_or("")
        )
    }

    /// Build a MatchingContext for semantic detection
    ///
    /// Creates a comprehensive context object for the semantic detection system,
    /// including conversation history, app context, and time context.
    #[allow(dead_code)]
    pub(crate) fn build_matching_context(
        &self,
        input: &str,
        context: &CapturedContext,
    ) -> crate::semantic::MatchingContext {
        use crate::semantic::{AppContext, ConversationContext, MatchingContext, TimeContext};

        // Extract app name from bundle ID
        let app_name = context
            .app_bundle_id
            .split('.')
            .next_back()
            .unwrap_or("Unknown")
            .to_string();

        // Build app context
        let app_ctx = AppContext {
            bundle_id: context.app_bundle_id.clone(),
            app_name,
            window_title: context.window_title.clone(),
            attachments: Vec::new(), // TODO: Convert MediaAttachment to AttachmentType
        };

        // Build conversation context from ConversationManager
        let conversation_ctx = {
            if let Ok(manager) = self.conversation_manager.lock() {
                let session_id = manager.active_session().map(|s| s.session_id.clone());
                let turn_count = manager.turn_count();

                ConversationContext {
                    session_id,
                    turn_count,
                    previous_intents: Vec::new(), // TODO: Track intents
                    pending_params: std::collections::HashMap::new(),
                    last_response_summary: None,
                    history: Vec::new(), // TODO: Convert history
                }
            } else {
                ConversationContext::default()
            }
        };

        // Build time context
        let time_ctx = TimeContext::now();

        // Build full matching context
        MatchingContext::builder()
            .raw_input(input)
            .conversation(conversation_ctx)
            .app(app_ctx)
            .time(time_ctx)
            .build()
    }

    /// Check if semantic matching is enabled
    #[allow(dead_code)]
    pub(crate) fn is_semantic_matching_enabled(&self) -> bool {
        let router_guard = self.router.read().ok();
        router_guard
            .as_ref()
            .and_then(|r| r.as_ref())
            .map(|router| router.is_semantic_matching_enabled())
            .unwrap_or(false)
    }

    /// Clone necessary fields for async memory storage
    ///
    /// This creates a lightweight clone that can be moved into async tasks
    /// for non-blocking memory storage operations.
    pub(crate) fn clone_for_storage(&self) -> StorageHelper {
        StorageHelper {
            config: Arc::clone(&self.config),
            memory_db: self.memory_db.clone(),
            current_context: Arc::clone(&self.current_context),
        }
    }

    /// Get the default AI provider instance for memory selection and other AI tasks.
    pub(crate) fn get_default_provider_instance(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::providers::AiProvider>> {
        let config = self.lock_config();
        let default_provider_name = config.general.default_provider.clone();
        drop(config);

        // default_provider is Option<String>, extract the name if present
        if let Some(name) = default_provider_name {
            self.get_provider_by_name(&name)
        } else {
            None
        }
    }

    /// Get a provider by name from the internal provider registry.
    pub(crate) fn get_provider_by_name(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<dyn crate::providers::AiProvider>> {
        // Access the router to get providers (router uses RwLock)
        let router_guard = self.router.read().unwrap_or_else(|e| e.into_inner());
        if let Some(router) = router_guard.as_ref() {
            router.get_provider_arc(name)
        } else {
            None
        }
    }

    /// Build and enrich AgentPayload using new payload architecture
    ///
    /// This method implements the structured context protocol:
    /// 1. Creates AgentPayload using PayloadBuilder
    /// 2. Executes CapabilityExecutor to enrich context (memory, search, MCP)
    /// 3. Returns enriched payload ready for prompt assembly
    pub(crate) async fn build_enriched_payload(
        &self,
        user_input: String,
        context: CapturedContext,
        provider_name: String,
        capabilities: Vec<crate::payload::Capability>,
    ) -> Result<crate::payload::AgentPayload> {
        use crate::capability::CapabilityExecutor;
        use crate::payload::{ContextAnchor, ContextFormat, Intent, PayloadBuilder};

        // Create context anchor from captured context
        let anchor = ContextAnchor::from_captured_context(&context);

        // Get config for context format
        let context_format = ContextFormat::Markdown; // MVP uses Markdown format

        // Build initial payload
        let payload = PayloadBuilder::new()
            .meta(
                Intent::GeneralChat, // MVP uses GeneralChat intent
                chrono::Utc::now().timestamp(),
                anchor,
            )
            .config(provider_name, capabilities.clone(), context_format)
            .user_input(user_input)
            .build()
            .map_err(|e| AetherError::config(format!("Failed to build payload: {}", e)))?;

        // Get AI memory retrieval configuration
        let (use_ai_retrieval, ai_timeout_ms, ai_max_candidates, ai_fallback_count) = {
            let cfg = self.lock_config();
            (
                cfg.memory.enabled && cfg.memory.ai_retrieval_enabled,
                cfg.memory.ai_retrieval_timeout_ms,
                cfg.memory.ai_retrieval_max_candidates,
                cfg.memory.ai_retrieval_fallback_count,
            )
        };

        // Build memory exclusion set from current conversation
        let memory_exclusion_set = self.build_memory_exclusion_set();

        // Get AI provider for memory selection (if AI retrieval enabled)
        let ai_provider = if use_ai_retrieval {
            self.get_default_provider_instance()
        } else {
            None
        };

        // Execute capabilities to enrich payload
        let executor = CapabilityExecutor::new(
            self.memory_db.as_ref().map(Arc::clone),
            {
                let cfg = self.lock_config();
                Some(Arc::new(cfg.memory.clone()))
            },
            {
                // Pass SearchRegistry from persistent field (integrate-search-registry)
                let registry = self.search_registry.read().unwrap_or_else(|e| e.into_inner());
                registry.as_ref().map(Arc::clone)
            },
            {
                // Pass SearchOptions from config (integrate-search-registry)
                let cfg = self.lock_config();
                cfg.search
                    .as_ref()
                    .map(|s| crate::search::SearchOptions {
                        max_results: s.max_results,
                        timeout_seconds: s.timeout_seconds,
                        ..Default::default()
                    })
            },
            {
                // Read PII config from search.pii.enabled (integrate-search-registry)
                // Fallback to behavior.pii_scrubbing_enabled for backward compatibility
                let cfg = self.lock_config();
                cfg.search
                    .as_ref()
                    .and_then(|s| s.pii.as_ref())
                    .map(|p| p.enabled)
                    .or_else(|| cfg.behavior.as_ref().map(|b| b.pii_scrubbing_enabled))
                    .unwrap_or(false)
            },
        )
        .with_video_config({
            // Pass VideoConfig from config
            let cfg = self.lock_config();
            cfg.video.as_ref().map(|v| Arc::new(v.clone()))
        })
        // Configure AI-based memory retrieval
        .with_ai_retrieval(
            ai_provider,
            use_ai_retrieval,
            ai_timeout_ms,
            ai_max_candidates,
            ai_fallback_count,
        )
        .with_memory_exclusion_set(memory_exclusion_set);

        executor.execute_all(payload).await
    }
}
