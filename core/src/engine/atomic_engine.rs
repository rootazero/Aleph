use crate::engine::{AtomicAction, AtomicExecutor, ReflexLayer};
use crate::error::AlephError;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::future::Future;
use tokio::sync::RwLock;

/// Main atomic engine that orchestrates L1/L2/L3 routing and self-healing execution
pub struct AtomicEngine {
    /// Fast reflex routing layer (L1/L2)
    reflex: Arc<RwLock<ReflexLayer>>,
    /// Atomic operation executor
    executor: AtomicExecutor,
    /// Maximum retry attempts for self-healing
    max_retries: usize,
    /// Working directory for relative path resolution
    working_dir: PathBuf,
}

/// Execution result with optional feedback for self-healing
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Whether execution succeeded
    pub success: bool,
    /// Output or error message
    pub message: String,
    /// Optional suggested fix for failed operations
    pub suggested_fix: Option<AtomicAction>,
}

impl AtomicEngine {
    /// Create a new atomic engine
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            reflex: Arc::new(RwLock::new(ReflexLayer::with_default_rules())),
            executor: AtomicExecutor::new(working_dir.clone()),
            max_retries: 3,
            working_dir,
        }
    }

    /// Execute an atomic action with self-healing
    pub async fn execute(&self, action: AtomicAction) -> Result<ExecutionResult, AlephError> {
        self.execute_with_retries(action, 0).await
    }

    /// Execute with retry logic for self-healing
    fn execute_with_retries<'a>(
        &'a self,
        action: AtomicAction,
        retry_count: usize,
    ) -> Pin<Box<dyn Future<Output = Result<ExecutionResult, AlephError>> + Send + 'a>> {
        Box::pin(async move {
            // Try to execute the action
            let result = self.executor.execute(&action).await;

            match result {
                Ok(atomic_result) => Ok(ExecutionResult {
                    success: atomic_result.success,
                    message: atomic_result.output,
                    suggested_fix: None,
                }),
                Err(e) if retry_count < self.max_retries => {
                    // Attempt self-healing
                    if let Some(fix) = self.suggest_fix(&action, &e).await {
                        tracing::warn!(
                            "Execution failed (attempt {}/{}), trying fix: {:?}",
                            retry_count + 1,
                            self.max_retries,
                            fix
                        );
                        // Execute the fix
                        let fix_result = self.executor.execute(&fix).await;
                        if fix_result.is_ok() {
                            // Retry original action after fix
                            return self.execute_with_retries(action, retry_count + 1).await;
                        }
                    }
                    // No fix available or fix failed
                    Ok(ExecutionResult {
                        success: false,
                        message: e.to_string(),
                        suggested_fix: None,
                    })
                }
                Err(e) => Ok(ExecutionResult {
                    success: false,
                    message: e.to_string(),
                    suggested_fix: None,
                }),
            }
        })
    }

    /// Suggest a fix for a failed action
    async fn suggest_fix(
        &self,
        action: &AtomicAction,
        error: &AlephError,
    ) -> Option<AtomicAction> {
        match action {
            AtomicAction::Write { path, .. } => {
                // If write failed due to missing directory, try creating it
                if error.to_string().contains("No such file or directory") {
                    if let Some(parent) = PathBuf::from(path).parent() {
                        return Some(AtomicAction::Bash {
                            command: format!("mkdir -p {}", parent.display()),
                            cwd: None,
                        });
                    }
                }
            }
            AtomicAction::Read { path, .. } => {
                // If read failed due to missing file, suggest checking if it exists
                if error.to_string().contains("No such file or directory") {
                    return Some(AtomicAction::Bash {
                        command: format!("test -f {}", path),
                        cwd: None,
                    });
                }
            }
            _ => {}
        }
        None
    }

    /// Route a user query through L1/L2/L3 layers
    pub async fn route_query(&self, query: &str) -> RoutingResult {
        let reflex = self.reflex.read().await;

        // Get stats before routing
        let stats_before = reflex.stats();

        // Try L1/L2 reflex routing (fast path)
        if let Some(action) = reflex.try_reflex(query) {
            // Get stats after routing to determine which layer was used
            let stats_after = reflex.stats();

            let layer = if stats_after.l1_hits > stats_before.l1_hits {
                RoutingLayer::L1
            } else if stats_after.l2_hits > stats_before.l2_hits {
                RoutingLayer::L2
            } else {
                // Shouldn't happen, but default to L2
                RoutingLayer::L2
            };

            return RoutingResult {
                layer,
                action: Some(action),
                latency_ms: 0.0, // Will be measured by caller
            };
        }

        // L3: Requires LLM reasoning (fallback)
        RoutingResult {
            layer: RoutingLayer::L3,
            action: None,
            latency_ms: 0.0,
        }
    }

    /// Learn from successful L3 routing to populate L1 cache
    pub async fn learn_from_success(&self, query: String, action: AtomicAction) {
        let reflex = self.reflex.read().await;
        reflex.learn_from_success(&query, action);
    }

    /// Get routing statistics
    pub async fn get_stats(&self) -> RoutingStats {
        let reflex = self.reflex.read().await;
        let stats = reflex.stats();

        RoutingStats {
            l1_hits: stats.l1_hits as usize,
            l2_hits: stats.l2_hits as usize,
            l3_fallbacks: stats.l3_fallbacks as usize,
            total_queries: stats.total() as usize,
        }
    }
}

/// Routing result from L1/L2/L3 layers
#[derive(Debug, Clone)]
pub struct RoutingResult {
    /// Which layer handled the routing
    pub layer: RoutingLayer,
    /// The routed action (None if L3 fallback needed)
    pub action: Option<AtomicAction>,
    /// Routing latency in milliseconds
    pub latency_ms: f64,
}

/// Routing layer identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingLayer {
    /// L1: Exact match cache (<10ms)
    L1,
    /// L2: Keyword routing (<50ms)
    L2,
    /// L3: LLM reasoning (1-3s)
    L3,
}

/// Routing statistics
#[derive(Debug, Clone)]
pub struct RoutingStats {
    /// L1 cache hits
    pub l1_hits: usize,
    /// L2 keyword matches
    pub l2_hits: usize,
    /// L3 fallbacks
    pub l3_fallbacks: usize,
    /// Total queries
    pub total_queries: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Patch, WriteMode};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_execute_read() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "Hello, World!").unwrap();

        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());
        let result = engine
            .execute(AtomicAction::Read {
                path: test_file.to_str().unwrap().to_string(),
                range: None,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_execute_write() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");

        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());
        let result = engine
            .execute(AtomicAction::Write {
                path: test_file.to_str().unwrap().to_string(),
                content: "Test content".to_string(),
                mode: WriteMode::Overwrite,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(std::fs::read_to_string(&test_file).unwrap(), "Test content");
    }

    #[tokio::test]
    async fn test_execute_edit() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "Line 1\nLine 2\nLine 3\n").unwrap();

        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());
        let patch = Patch::new(2, 2, "Line 2".to_string(), "Modified Line 2".to_string()).unwrap();
        let result = engine
            .execute(AtomicAction::Edit {
                path: test_file.to_str().unwrap().to_string(),
                patches: vec![patch],
            })
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert!(content.contains("Modified Line 2"));
    }

    #[tokio::test]
    async fn test_execute_bash() {
        let temp_dir = TempDir::new().unwrap();
        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());

        let result = engine
            .execute(AtomicAction::Bash {
                command: "echo 'Hello from bash'".to_string(),
                cwd: None,
            })
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.message.contains("Hello from bash"));
    }

    #[tokio::test]
    async fn test_self_healing_mkdir() {
        let temp_dir = TempDir::new().unwrap();
        let nested_file = temp_dir.path().join("nested/dir/test.txt");

        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());
        let result = engine
            .execute(AtomicAction::Write {
                path: nested_file.to_str().unwrap().to_string(),
                content: "Test".to_string(),
                mode: WriteMode::Overwrite,
            })
            .await
            .unwrap();

        // Should succeed after self-healing creates the directory
        assert!(result.success);
        assert!(nested_file.exists());
    }

    #[tokio::test]
    async fn test_routing_l1_exact_match() {
        let temp_dir = TempDir::new().unwrap();
        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());

        // First, learn a custom command
        let query = "my custom command".to_string();
        let action = AtomicAction::Bash {
            command: "echo test".to_string(),
            cwd: None,
        };
        engine.learn_from_success(query.clone(), action).await;

        // Now it should route via L1
        let result = engine.route_query(&query).await;
        assert_eq!(result.layer, RoutingLayer::L1);
        assert!(result.action.is_some());
    }

    #[tokio::test]
    async fn test_routing_l2_keyword() {
        let temp_dir = TempDir::new().unwrap();
        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());

        // "git status" should match L2 keyword rule
        let result = engine.route_query("git status").await;
        assert_eq!(result.layer, RoutingLayer::L2);
        assert!(result.action.is_some());
    }

    #[tokio::test]
    async fn test_routing_l3_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());

        let result = engine
            .route_query("analyze the performance of my application")
            .await;
        assert_eq!(result.layer, RoutingLayer::L3);
        assert!(result.action.is_none());
    }

    #[tokio::test]
    async fn test_learn_from_success() {
        let temp_dir = TempDir::new().unwrap();
        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());

        let query = "custom command".to_string();
        let action = AtomicAction::Bash {
            command: "echo test".to_string(),
            cwd: None,
        };

        // Learn from success
        engine.learn_from_success(query.clone(), action.clone()).await;

        // Should now route via L1
        let result = engine.route_query(&query).await;
        assert_eq!(result.layer, RoutingLayer::L1);
        assert!(result.action.is_some());
    }

    #[tokio::test]
    async fn test_get_stats() {
        let temp_dir = TempDir::new().unwrap();
        let engine = AtomicEngine::new(temp_dir.path().to_path_buf());

        // Trigger some routing
        engine.route_query("git status").await; // L2
        engine.route_query("git log").await; // L2
        engine.route_query("complex query that needs reasoning").await; // L3

        let stats = engine.get_stats().await;
        assert_eq!(stats.l2_hits, 2);
        assert_eq!(stats.l3_fallbacks, 1);
        assert_eq!(stats.total_queries, 3);
    }
}
