//! AgentLoopWorker implementation for POE execution.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use tokio::sync::{watch, RwLock};

use crate::agent_loop::{
    ActionExecutor, AgentLoop, CompressorTrait, LoopConfig, LoopResult, RequestContext,
    RunContext, ThinkerTrait,
};
use crate::dispatcher::UnifiedTool;
use crate::error::Result;
use crate::poe::types::{Artifact, StepLog, WorkerOutput};

use super::callback::PoeLoopCallback;
use super::{StateSnapshot, Worker};

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
                out.tokens_consumed = total_tokens.min(u32::MAX as usize) as u32;
                out.steps_taken = steps.min(u32::MAX as usize) as u32;
                out
            }
            LoopResult::Failed { reason, steps } => {
                let mut out = WorkerOutput::failed(reason);
                out.steps_taken = steps.min(u32::MAX as usize) as u32;
                out
            }
            LoopResult::GuardTriggered(violation) => {
                let reason = format!("Guard triggered: {}", violation.description());
                WorkerOutput::failed(reason)
            }
            LoopResult::UserAborted => {
                WorkerOutput::failed("Execution aborted by user")
            }
            LoopResult::PoeAborted { reason } => {
                WorkerOutput::failed(format!("POE aborted: {}", reason))
            }
            LoopResult::Escalated { route, context } => {
                let mut out = WorkerOutput::failed(format!(
                    "Escalated to {}: {}",
                    route.label(),
                    context.partial_result.unwrap_or_default()
                ));
                out.steps_taken = context.completed_steps.min(u32::MAX as usize) as u32;
                out
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
