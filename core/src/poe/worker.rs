//! Worker abstraction for POE execution.
//!
//! This module defines the Worker trait and its implementations:
//! - `Worker`: Async trait for executing instructions with snapshot/restore
//! - `StateSnapshot`: Captures workspace state for rollback
//! - `AgentLoopWorker`: Real implementation that integrates with AgentLoop
//! - `MockWorker`: Test implementation with configurable behavior

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

use crate::agent_loop::{
    Action, ActionExecutor, ActionResult, AgentLoop, CompressorTrait, GuardViolation,
    LoopCallback, LoopConfig, LoopResult, LoopState, RequestContext, RunContext, ThinkerTrait, Thinking,
};
use crate::agent_loop::decision::QuestionGroup;
use crate::dispatcher::UnifiedTool;
use crate::error::Result;
use crate::poe::types::{Artifact, ChangeType, StepLog, WorkerOutput};

// ============================================================================
// StateSnapshot
// ============================================================================

/// A snapshot of workspace state that can be used for rollback.
///
/// StateSnapshot captures the state of the workspace at a point in time,
/// allowing the orchestrator to restore to a known good state if execution
/// fails or needs to be retried with a different approach.
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    /// When this snapshot was taken
    pub timestamp: DateTime<Utc>,

    /// Root directory of the workspace
    pub workspace: PathBuf,

    /// List of files and their content hashes at snapshot time.
    /// The hash is SHA-256 of file contents.
    pub file_hashes: Vec<(PathBuf, String)>,
}

impl StateSnapshot {
    /// Create a new empty snapshot.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            timestamp: Utc::now(),
            workspace,
            file_hashes: Vec::new(),
        }
    }

    /// Create a snapshot with the given file hashes.
    pub fn with_files(workspace: PathBuf, file_hashes: Vec<(PathBuf, String)>) -> Self {
        Self {
            timestamp: Utc::now(),
            workspace,
            file_hashes,
        }
    }

    /// Check if a file is tracked in this snapshot.
    pub fn contains_file(&self, path: &PathBuf) -> bool {
        self.file_hashes.iter().any(|(p, _)| p == path)
    }

    /// Get the hash of a specific file, if tracked.
    pub fn get_file_hash(&self, path: &PathBuf) -> Option<&str> {
        self.file_hashes
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, hash)| hash.as_str())
    }

    /// Get the number of tracked files.
    pub fn file_count(&self) -> usize {
        self.file_hashes.len()
    }
}

// ============================================================================
// Worker Trait
// ============================================================================

/// Trait for workers that execute instructions in the POE framework.
///
/// Workers are responsible for:
/// 1. Executing natural language instructions (via AI agent loops)
/// 2. Supporting abort/cancellation
/// 3. Creating and restoring snapshots for rollback
///
/// The Worker trait is designed to be implemented by different backends:
/// - `AgentLoopWorker`: Integrates with the Aleph agent loop
/// - `MockWorker`: For testing POE orchestration logic
#[async_trait]
pub trait Worker: Send + Sync {
    /// Execute an instruction, optionally with feedback from a previous failure.
    ///
    /// # Arguments
    /// * `instruction` - Natural language instruction to execute
    /// * `previous_failure` - Optional feedback from a previous failed attempt,
    ///   which the worker can use to adjust its approach
    ///
    /// # Returns
    /// * `Ok(WorkerOutput)` - Execution completed (may have succeeded or failed)
    /// * `Err(_)` - Execution could not be attempted (infrastructure error)
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput>;

    /// Abort the current execution.
    ///
    /// This should interrupt any ongoing work and return as quickly as possible.
    /// The worker may be in an inconsistent state after abort.
    async fn abort(&self) -> Result<()>;

    /// Take a snapshot of current workspace state.
    ///
    /// The snapshot can be used to restore the workspace to this point
    /// if a subsequent operation fails and needs to be rolled back.
    async fn snapshot(&self) -> Result<StateSnapshot>;

    /// Restore the workspace from a previous snapshot.
    ///
    /// # Arguments
    /// * `snapshot` - The snapshot to restore from
    ///
    /// # Errors
    /// Returns an error if restoration fails (e.g., files have been deleted
    /// that can't be recreated, or permissions issues).
    async fn restore(&self, snapshot: &StateSnapshot) -> Result<()>;
}

// ============================================================================
// PoeLoopCallback - Artifact Tracking Callback
// ============================================================================

/// Callback implementation for POE worker execution.
///
/// This callback tracks file artifacts created or modified during execution
/// by monitoring tool calls for file operations.
struct PoeLoopCallback {
    /// Artifacts produced during execution
    artifacts: Arc<RwLock<Vec<Artifact>>>,
    /// Execution logs
    execution_log: Arc<RwLock<Vec<StepLog>>>,
    /// Workspace root for relative path calculation
    /// Reserved for future use: converting absolute paths to workspace-relative paths
    #[allow(dead_code)]
    workspace: PathBuf,
    /// Step counter for logging
    step_counter: Arc<RwLock<u32>>,
}

impl PoeLoopCallback {
    /// Create a new PoeLoopCallback.
    fn new(
        artifacts: Arc<RwLock<Vec<Artifact>>>,
        execution_log: Arc<RwLock<Vec<StepLog>>>,
        workspace: PathBuf,
    ) -> Self {
        Self {
            artifacts,
            execution_log,
            workspace,
            step_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Extract file path from tool arguments.
    fn extract_file_path(arguments: &Value) -> Option<PathBuf> {
        // Try common argument names for file paths
        arguments
            .get("path")
            .or_else(|| arguments.get("file_path"))
            .or_else(|| arguments.get("file"))
            .or_else(|| arguments.get("target"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    /// Compute SHA-256 hash of content.
    fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Determine change type from tool name.
    fn change_type_from_tool(tool_name: &str, arguments: &Value) -> ChangeType {
        let tool_lower = tool_name.to_lowercase();

        // Check for explicit operation field
        if let Some(op) = arguments.get("operation").and_then(|v| v.as_str()) {
            let op_lower = op.to_lowercase();
            if op_lower.contains("delete") || op_lower.contains("remove") {
                return ChangeType::Deleted;
            }
            if op_lower.contains("create") || op_lower.contains("mkdir") {
                return ChangeType::Created;
            }
        }

        // Infer from tool name
        if tool_lower.contains("write") || tool_lower.contains("create") {
            ChangeType::Created
        } else if tool_lower.contains("edit") || tool_lower.contains("modify") || tool_lower.contains("update") {
            ChangeType::Modified
        } else if tool_lower.contains("delete") || tool_lower.contains("remove") {
            ChangeType::Deleted
        } else {
            ChangeType::Modified
        }
    }
}

#[async_trait]
impl LoopCallback for PoeLoopCallback {
    async fn on_loop_start(&self, _state: &LoopState) {
        // Reset step counter
        *self.step_counter.write().await = 0;
    }

    async fn on_step_start(&self, _step: usize) {}

    async fn on_thinking_start(&self, _step: usize) {}

    async fn on_thinking_done(&self, _thinking: &Thinking) {}

    async fn on_action_start(&self, _action: &Action) {}

    async fn on_action_done(&self, action: &Action, result: &ActionResult) {
        // Track file artifacts from write/edit tool calls
        if let Action::ToolCall {
            tool_name,
            arguments,
        } = action
        {
            // Check if this is a file operation tool
            let is_file_op = matches!(
                tool_name.to_lowercase().as_str(),
                "write_file" | "edit_file" | "write" | "edit" | "create_file"
                    | "file_ops" | "delete_file" | "remove_file"
            );

            if is_file_op {
                if let Some(path) = Self::extract_file_path(arguments) {
                    // Determine change type
                    let change_type = Self::change_type_from_tool(tool_name, arguments);

                    // Compute content hash if available
                    let content_hash = if let ActionResult::ToolSuccess { output, .. } = result {
                        // Try to get content from result or arguments
                        let content = arguments
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !content.is_empty() {
                            Self::compute_hash(content)
                        } else {
                            // Hash the output as fallback
                            Self::compute_hash(&output.to_string())
                        }
                    } else {
                        "unknown".to_string()
                    };

                    // Create artifact
                    let artifact = Artifact::new(path, change_type, content_hash);

                    // Add to artifacts list
                    self.artifacts.write().await.push(artifact);
                }
            }
        }

        // Log the step
        let mut step_id = self.step_counter.write().await;
        let log_entry = StepLog::new(
            *step_id,
            action.action_type(),
            result.summary(),
            0, // Duration tracked elsewhere
        );
        self.execution_log.write().await.push(log_entry);
        *step_id += 1;
    }

    async fn on_confirmation_required(&self, _tool_name: &str, _arguments: &Value) -> bool {
        // POE worker auto-confirms (the POE framework handles validation)
        true
    }

    async fn on_user_input_required(
        &self,
        _question: &str,
        _options: Option<&[String]>,
    ) -> String {
        // POE worker returns a default response
        // Real user interaction should be handled by the orchestrator
        "continue".to_string()
    }

    async fn on_user_multigroup_required(
        &self,
        _question: &str,
        _groups: &[QuestionGroup],
    ) -> String {
        "{\"default\":\"ok\"}".to_string()
    }

    async fn on_guard_triggered(&self, _violation: &GuardViolation) {}

    async fn on_complete(&self, _summary: &str) {}

    async fn on_failed(&self, _reason: &str) {}
}

// ============================================================================
// AgentLoopWorker
// ============================================================================

/// Worker implementation that integrates with the Aleph AgentLoop.
///
/// This worker executes instructions through the real AgentLoop, tracking
/// artifacts and supporting abort/cancellation via watch channels.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::poe::AgentLoopWorker;
/// use std::sync::Arc;
///
/// let worker = AgentLoopWorker::new(
///     PathBuf::from("/workspace"),
///     thinker,
///     executor,
///     compressor,
///     tools,
///     LoopConfig::default(),
/// );
///
/// let output = worker.execute("Create a new file", None).await?;
/// ```
pub struct AgentLoopWorker<T, E, C>
where
    T: ThinkerTrait + 'static,
    E: ActionExecutor + 'static,
    C: CompressorTrait + 'static,
{
    /// Workspace directory where the worker operates
    workspace: PathBuf,

    /// The thinker for LLM decisions
    thinker: Arc<T>,

    /// The action executor
    executor: Arc<E>,

    /// The context compressor
    compressor: Arc<C>,

    /// Available tools for execution
    tools: Vec<UnifiedTool>,

    /// Loop configuration
    config: LoopConfig,

    /// Abort signal sender
    abort_tx: watch::Sender<bool>,

    /// Abort signal receiver (cloned for each execution)
    abort_rx: watch::Receiver<bool>,
}

impl<T, E, C> AgentLoopWorker<T, E, C>
where
    T: ThinkerTrait + 'static,
    E: ActionExecutor + 'static,
    C: CompressorTrait + 'static,
{
    /// Create a new AgentLoopWorker with all required components.
    pub fn new(
        workspace: PathBuf,
        thinker: Arc<T>,
        executor: Arc<E>,
        compressor: Arc<C>,
        tools: Vec<UnifiedTool>,
        config: LoopConfig,
    ) -> Self {
        let (abort_tx, abort_rx) = watch::channel(false);
        Self {
            workspace,
            thinker,
            executor,
            compressor,
            tools,
            config,
            abort_tx,
            abort_rx,
        }
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Convert LoopResult to WorkerOutput.
    fn loop_result_to_output(
        &self,
        result: LoopResult,
        artifacts: Vec<Artifact>,
        execution_log: Vec<StepLog>,
    ) -> WorkerOutput {
        let mut output = match result {
            LoopResult::Completed {
                summary,
                steps,
                total_tokens,
            } => {
                let mut out = WorkerOutput::completed(summary);
                out.tokens_consumed = total_tokens as u32;
                out.steps_taken = steps as u32;
                out
            }
            LoopResult::Failed { reason, steps } => {
                let mut out = WorkerOutput::failed(reason);
                out.steps_taken = steps as u32;
                out
            }
            LoopResult::GuardTriggered(violation) => {
                let reason = format!("Guard triggered: {}", violation.description());
                WorkerOutput::failed(reason)
            }
            LoopResult::UserAborted => {
                WorkerOutput::failed("Execution aborted by user")
            }
        };

        output.artifacts = artifacts;
        output.execution_log = execution_log;
        output
    }

    /// Compute SHA-256 hash of file contents.
    async fn hash_file(path: &std::path::Path) -> std::io::Result<String> {
        let content = tokio::fs::read(path).await?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Ok(format!("{:x}", hasher.finalize()))
    }
}

#[async_trait]
impl<T, E, C> Worker for AgentLoopWorker<T, E, C>
where
    T: ThinkerTrait + 'static,
    E: ActionExecutor + 'static,
    C: CompressorTrait + 'static,
{
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        // Reset abort signal
        let _ = self.abort_tx.send(false);

        // Build prompt with optional failure context
        let prompt = match previous_failure {
            Some(feedback) => format!(
                "{}\n\n## Previous Attempt Feedback\n\nThe previous attempt failed with the following feedback:\n\n{}",
                instruction, feedback
            ),
            None => instruction.to_string(),
        };

        // Create shared storage for artifacts and logs
        let artifacts = Arc::new(RwLock::new(Vec::new()));
        let execution_log = Arc::new(RwLock::new(Vec::new()));

        // Create POE callback for artifact tracking
        let callback = PoeLoopCallback::new(
            artifacts.clone(),
            execution_log.clone(),
            self.workspace.clone(),
        );

        // Create AgentLoop with our components
        let agent_loop = AgentLoop::new(
            self.thinker.clone(),
            self.executor.clone(),
            self.compressor.clone(),
            self.config.clone(),
        );

        // Build request context with workspace
        let context = RequestContext {
            working_directory: Some(self.workspace.to_string_lossy().to_string()),
            ..Default::default()
        };

        // Create Owner identity for POE worker (internal execution)
        let identity = aleph_protocol::IdentityContext::owner(
            "poe-worker".to_string(),
            "internal".to_string(),
        );

        // Execute via AgentLoop
        let run_context = RunContext::new(
            prompt,
            context,
            self.tools.clone(),
            identity,
        )
        .with_abort_signal(self.abort_rx.clone());
        let result = agent_loop
            .run(run_context, callback)
            .await;

        // Collect artifacts and logs
        let collected_artifacts = artifacts.read().await.clone();
        let collected_logs = execution_log.read().await.clone();

        // Convert to WorkerOutput
        Ok(self.loop_result_to_output(result, collected_artifacts, collected_logs))
    }

    async fn abort(&self) -> Result<()> {
        self.abort_tx
            .send(true)
            .map_err(|_| crate::error::AlephError::other("Abort channel closed"))?;
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        let mut file_hashes = Vec::new();

        // Walk the workspace directory (limit depth to avoid huge scans)
        let walker = walkdir::WalkDir::new(&self.workspace)
            .max_depth(5)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // Skip hidden directories (like .git)
                !e.file_name()
                    .to_str()
                    .map(|s| s.starts_with('.'))
                    .unwrap_or(false)
            });

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // Skip entries we can't read
            };

            if entry.file_type().is_file() {
                match Self::hash_file(entry.path()).await {
                    Ok(hash) => {
                        file_hashes.push((entry.path().to_path_buf(), hash));
                    }
                    Err(_) => {
                        // Skip files we can't read
                        continue;
                    }
                }
            }
        }

        Ok(StateSnapshot::with_files(self.workspace.clone(), file_hashes))
    }

    async fn restore(&self, snapshot: &StateSnapshot) -> Result<()> {
        // Verify workspace matches
        if snapshot.workspace != self.workspace {
            return Err(crate::error::AlephError::other(format!(
                "Snapshot workspace {} does not match worker workspace {}",
                snapshot.workspace.display(),
                self.workspace.display()
            )));
        }

        // For now, we only verify the state matches
        // Full restoration would require storing file contents or using git
        //
        // TODO: Implement full restoration via:
        // 1. Git-based restoration (git checkout)
        // 2. Backup file storage
        // 3. Copy-on-write snapshots

        let current_snapshot = self.snapshot().await?;

        // Check for files that have been modified
        for (path, expected_hash) in &snapshot.file_hashes {
            if let Some(current_hash) = current_snapshot.get_file_hash(path) {
                if current_hash != expected_hash {
                    tracing::warn!(
                        "File {} has been modified (expected {}, got {})",
                        path.display(),
                        expected_hash,
                        current_hash
                    );
                }
            } else {
                tracing::warn!("File {} no longer exists", path.display());
            }
        }

        // Check for new files created after snapshot
        for (path, _) in &current_snapshot.file_hashes {
            if !snapshot.contains_file(path) {
                tracing::info!("New file created after snapshot: {}", path.display());
            }
        }

        Ok(())
    }
}

// ============================================================================
// PlaceholderWorker (for initial integration)
// ============================================================================

/// Placeholder worker for initial POE integration.
///
/// This worker simulates execution without actually performing any work.
/// It is used for:
/// 1. Initial Gateway integration testing
/// 2. Contract signing workflow demonstration
/// 3. Development before full AgentLoopWorker is wired
///
/// Replace with AgentLoopWorker for production use.
pub struct PlaceholderWorker {
    /// Workspace directory
    workspace: PathBuf,
    /// Counter for executions
    execution_count: std::sync::atomic::AtomicU32,
}

impl PlaceholderWorker {
    /// Create a new PlaceholderWorker.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            execution_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Create with default workspace (/tmp/poe-workspace).
    pub fn with_default_workspace() -> Self {
        Self::new(PathBuf::from("/tmp/poe-workspace"))
    }

    /// Get execution count.
    pub fn execution_count(&self) -> u32 {
        self.execution_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl Worker for PlaceholderWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        self.execution_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Simulate execution with placeholder output
        let mut output = WorkerOutput::completed(format!(
            "[PlaceholderWorker] Simulated execution of: {}{}",
            truncate_instruction(instruction, 100),
            previous_failure
                .map(|f| format!(" (retry after: {})", truncate_instruction(f, 50)))
                .unwrap_or_default()
        ));
        output.tokens_consumed = 50; // Simulated token usage
        output.steps_taken = 1;

        Ok(output)
    }

    async fn abort(&self) -> Result<()> {
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        Ok(())
    }
}

/// Truncate instruction for logging.
fn truncate_instruction(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

// ============================================================================
// GatewayAgentLoopWorker - Type Alias for Gateway Integration
// ============================================================================

/// Type alias for the concrete AgentLoopWorker used in Gateway.
///
/// This provides a specific instantiation of AgentLoopWorker with:
/// - `Thinker<SingleProviderRegistry>` for LLM decisions
/// - `SingleStepExecutor<BuiltinToolRegistry>` for tool execution
/// - `NoOpCompressor` for context management (compression disabled for POE)
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::poe::{GatewayAgentLoopWorker, create_gateway_worker};
/// use std::sync::Arc;
///
/// let provider = create_claude_provider_from_env()?;
/// let worker = create_gateway_worker(Arc::new(provider), PathBuf::from("/tmp/workspace"));
/// ```
pub type GatewayAgentLoopWorker = AgentLoopWorker<
    crate::thinker::Thinker<crate::thinker::SingleProviderRegistry>,
    crate::executor::SingleStepExecutor<crate::executor::BuiltinToolRegistry>,
    crate::NoOpCompressor,
>;

/// Create a GatewayAgentLoopWorker with the specified provider and workspace.
///
/// This factory function constructs all the necessary components for a POE worker:
/// - Thinker with SingleProviderRegistry for LLM calls
/// - SingleStepExecutor with BuiltinToolRegistry for tool execution
/// - NoOpCompressor (POE manages its own execution cycles)
/// - Builtin tools converted to UnifiedTool format
///
/// # Arguments
///
/// * `provider` - The AI provider for LLM calls (e.g., Claude)
/// * `workspace` - The workspace directory for file operations
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::poe::create_gateway_worker;
/// use alephcore::gateway::create_claude_provider_from_env;
/// use std::sync::Arc;
/// use std::path::PathBuf;
///
/// let provider = create_claude_provider_from_env()?;
/// let worker = create_gateway_worker(
///     Arc::new(provider),
///     PathBuf::from("/tmp/poe-workspace"),
/// );
/// ```
pub fn create_gateway_worker(
    provider: std::sync::Arc<dyn crate::providers::AiProvider>,
    workspace: PathBuf,
) -> GatewayAgentLoopWorker {
    use crate::agent_loop::LoopConfig;
    use crate::dispatcher::{ToolSource, UnifiedTool};
    use crate::executor::{BuiltinToolRegistry, SingleStepExecutor, BUILTIN_TOOL_DEFINITIONS};
    use crate::thinker::{SingleProviderRegistry, Thinker, ThinkerConfig};
    use crate::NoOpCompressor;

    // Create Thinker with single provider registry
    let registry = std::sync::Arc::new(SingleProviderRegistry::new(provider));
    let thinker = std::sync::Arc::new(Thinker::new(registry, ThinkerConfig::default()));

    // Create Executor with builtin tool registry + ExecSecurityGate
    let tool_registry = std::sync::Arc::new(BuiltinToolRegistry::new());

    // Initialize ExecApprovalManager for human-in-the-loop shell approval
    let approval_manager = std::sync::Arc::new(crate::exec::ExecApprovalManager::new());

    // Initialize platform-specific SandboxManager (macOS only)
    #[cfg(target_os = "macos")]
    let sandbox_manager = {
        use crate::exec::sandbox::{FallbackPolicy, SandboxManager};
        use crate::exec::sandbox::platforms::MacOSSandbox;
        Some(std::sync::Arc::new(
            SandboxManager::new(std::sync::Arc::new(MacOSSandbox::new()))
                .with_fallback_policy(FallbackPolicy::WarnAndExecute),
        ))
    };
    #[cfg(not(target_os = "macos"))]
    let sandbox_manager: Option<std::sync::Arc<crate::exec::sandbox::SandboxManager>> = None;

    // Create ExecSecurityGate: risk assessment + approval + sandbox + secret masking
    let exec_gate = std::sync::Arc::new(
        crate::executor::ExecSecurityGate::new(approval_manager, sandbox_manager),
    );

    let executor = std::sync::Arc::new(
        SingleStepExecutor::new(tool_registry)
            .with_exec_security_gate(exec_gate),
    );

    // Build tools list from builtin definitions
    let tools: Vec<UnifiedTool> = BUILTIN_TOOL_DEFINITIONS
        .iter()
        .map(|def| {
            UnifiedTool::new(
                format!("builtin:{}", def.name),
                def.name,
                def.description,
                ToolSource::Builtin,
            )
        })
        .collect();

    // Create the worker
    AgentLoopWorker::new(
        workspace,
        thinker,
        executor,
        std::sync::Arc::new(NoOpCompressor),
        tools,
        LoopConfig::default(),
    )
}

// ============================================================================
// MockWorker (for testing)
// ============================================================================

/// Mock worker for testing POE orchestration logic.
///
/// This worker provides configurable behavior for testing:
/// - Success/failure outcomes
/// - Token consumption per call
/// - Custom behavior via callbacks
#[cfg(test)]
pub struct MockWorker {
    /// Whether execute() should succeed or fail
    pub should_succeed: bool,

    /// Tokens to report per execute() call
    pub tokens_per_call: u32,

    /// Workspace for snapshots
    workspace: PathBuf,

    /// Counter for number of executions
    execution_count: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl MockWorker {
    /// Create a new MockWorker with default settings (succeeds, 100 tokens).
    pub fn new() -> Self {
        Self {
            should_succeed: true,
            tokens_per_call: 100,
            workspace: PathBuf::from("/tmp/mock-workspace"),
            execution_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Create a MockWorker that always fails.
    pub fn failing() -> Self {
        Self {
            should_succeed: false,
            tokens_per_call: 50,
            workspace: PathBuf::from("/tmp/mock-workspace"),
            execution_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Set whether the worker should succeed.
    pub fn with_success(mut self, success: bool) -> Self {
        self.should_succeed = success;
        self
    }

    /// Set the tokens consumed per call.
    pub fn with_tokens(mut self, tokens: u32) -> Self {
        self.tokens_per_call = tokens;
        self
    }

    /// Set the workspace path.
    pub fn with_workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace = workspace;
        self
    }

    /// Get the number of times execute() has been called.
    pub fn execution_count(&self) -> u32 {
        self.execution_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Default for MockWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[async_trait]
impl Worker for MockWorker {
    async fn execute(
        &self,
        instruction: &str,
        previous_failure: Option<&str>,
    ) -> Result<WorkerOutput> {
        // Increment execution counter
        self.execution_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        if self.should_succeed {
            let mut output = WorkerOutput::completed(format!(
                "Mock execution of: {}{}",
                instruction,
                previous_failure
                    .map(|f| format!(" (retry after: {})", f))
                    .unwrap_or_default()
            ));
            output.tokens_consumed = self.tokens_per_call;
            output.steps_taken = 1;
            Ok(output)
        } else {
            let mut output = WorkerOutput::failed(format!("Mock failure for: {}", instruction));
            output.tokens_consumed = self.tokens_per_call;
            output.steps_taken = 1;
            Ok(output)
        }
    }

    async fn abort(&self) -> Result<()> {
        // Mock abort is always successful
        Ok(())
    }

    async fn snapshot(&self) -> Result<StateSnapshot> {
        Ok(StateSnapshot::new(self.workspace.clone()))
    }

    async fn restore(&self, _snapshot: &StateSnapshot) -> Result<()> {
        // Mock restore is always successful
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_snapshot_creation() {
        let snapshot = StateSnapshot::new(PathBuf::from("/workspace"));

        assert_eq!(snapshot.workspace, PathBuf::from("/workspace"));
        assert!(snapshot.file_hashes.is_empty());
        assert_eq!(snapshot.file_count(), 0);
    }

    #[test]
    fn test_state_snapshot_with_files() {
        let files = vec![
            (PathBuf::from("foo.rs"), "abc123".to_string()),
            (PathBuf::from("bar.rs"), "def456".to_string()),
        ];

        let snapshot = StateSnapshot::with_files(PathBuf::from("/workspace"), files);

        assert_eq!(snapshot.file_count(), 2);
        assert!(snapshot.contains_file(&PathBuf::from("foo.rs")));
        assert!(!snapshot.contains_file(&PathBuf::from("baz.rs")));
        assert_eq!(
            snapshot.get_file_hash(&PathBuf::from("foo.rs")),
            Some("abc123")
        );
        assert_eq!(
            snapshot.get_file_hash(&PathBuf::from("bar.rs")),
            Some("def456")
        );
        assert_eq!(snapshot.get_file_hash(&PathBuf::from("baz.rs")), None);
    }

    #[tokio::test]
    async fn test_mock_worker_success() {
        let worker = MockWorker::new().with_tokens(200);

        let output = worker.execute("test", None).await.unwrap();

        assert!(matches!(
            output.final_state,
            crate::poe::types::WorkerState::Completed { .. }
        ));
        assert_eq!(output.tokens_consumed, 200);
        assert_eq!(worker.execution_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_worker_failure() {
        let worker = MockWorker::failing();

        let output = worker.execute("test", None).await.unwrap();

        assert!(matches!(
            output.final_state,
            crate::poe::types::WorkerState::Failed { .. }
        ));
        assert_eq!(worker.execution_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_worker_multiple_executions() {
        let worker = MockWorker::new();

        worker.execute("first", None).await.unwrap();
        worker.execute("second", None).await.unwrap();
        worker.execute("third", None).await.unwrap();

        assert_eq!(worker.execution_count(), 3);
    }

    #[tokio::test]
    async fn test_mock_worker_abort() {
        let worker = MockWorker::new();

        // Abort should always succeed
        assert!(worker.abort().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_worker_snapshot_restore() {
        let worker = MockWorker::new();

        let snapshot = worker.snapshot().await.unwrap();
        assert!(worker.restore(&snapshot).await.is_ok());
    }

    #[test]
    fn test_poe_callback_extract_file_path() {
        let args = serde_json::json!({
            "path": "/tmp/test.txt"
        });
        assert_eq!(
            PoeLoopCallback::extract_file_path(&args),
            Some(PathBuf::from("/tmp/test.txt"))
        );

        let args2 = serde_json::json!({
            "file_path": "/tmp/other.rs"
        });
        assert_eq!(
            PoeLoopCallback::extract_file_path(&args2),
            Some(PathBuf::from("/tmp/other.rs"))
        );

        let args3 = serde_json::json!({
            "unrelated": "value"
        });
        assert_eq!(PoeLoopCallback::extract_file_path(&args3), None);
    }

    #[test]
    fn test_poe_callback_change_type_from_tool() {
        let args = serde_json::json!({});

        assert!(matches!(
            PoeLoopCallback::change_type_from_tool("write_file", &args),
            ChangeType::Created
        ));
        assert!(matches!(
            PoeLoopCallback::change_type_from_tool("edit_file", &args),
            ChangeType::Modified
        ));
        assert!(matches!(
            PoeLoopCallback::change_type_from_tool("delete_file", &args),
            ChangeType::Deleted
        ));

        // With operation field
        let args_delete = serde_json::json!({
            "operation": "delete"
        });
        assert!(matches!(
            PoeLoopCallback::change_type_from_tool("file_ops", &args_delete),
            ChangeType::Deleted
        ));
    }

    #[test]
    fn test_poe_callback_compute_hash() {
        let hash = PoeLoopCallback::compute_hash("hello world");
        // SHA-256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_worker_executor_creation_with_gate() {
        // Compilation test: verifies ExecSecurityGate wiring compiles correctly
        use crate::exec::ExecApprovalManager;
        use crate::executor::{ExecSecurityGate, SingleStepExecutor, BuiltinToolRegistry};
        use std::sync::Arc;

        let tool_registry = Arc::new(BuiltinToolRegistry::new());
        let approval_manager = Arc::new(ExecApprovalManager::new());
        let gate = Arc::new(ExecSecurityGate::new(approval_manager, None));
        let _executor = SingleStepExecutor::new(tool_registry)
            .with_exec_security_gate(gate);
        // If this compiles, the wiring is correct
    }
}
