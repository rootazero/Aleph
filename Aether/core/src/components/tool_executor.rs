//! Tool executor component - executes tools with retry logic.
//!
//! Subscribes to: ToolCallRequested
//! Publishes: ToolCallStarted, ToolCallCompleted, ToolCallFailed, ToolCallRetrying

/// Retry policy for tool execution
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 1000,
            max_delay_ms: 10000,
        }
    }
}

/// Tool Executor - executes tools with retry and error handling
pub struct ToolExecutor;
