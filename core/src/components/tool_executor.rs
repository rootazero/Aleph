//! Tool executor component - executes tools with retry logic.
//!
//! Subscribes to: ToolCallRequested
//! Publishes: ToolCallStarted, ToolCallCompleted, ToolCallFailed, ToolCallRetrying

use async_trait::async_trait;

use crate::dispatcher::DEFAULT_MAX_RETRIES;
use crate::event::{
    AetherEvent, ErrorKind, EventContext, EventHandler, EventType, HandlerError, TokenUsage,
    ToolCallError, ToolCallRequest, ToolCallResult, ToolCallRetry, ToolCallStarted,
};

// ============================================================================
// Tool Retry Policy
// ============================================================================

/// Retry policy for tool execution
///
/// This is distinct from `crate::config::RetryPolicy` which is for HTTP/network operations.
/// ToolRetryPolicy uses `ErrorKind` to determine retryable errors in tool execution context.
#[derive(Debug, Clone)]
pub struct ToolRetryPolicy {
    /// Maximum number of retry attempts (not including the initial attempt)
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (cap for exponential growth)
    pub max_delay_ms: u64,
    /// Error kinds that are retryable
    pub retryable_errors: Vec<ErrorKind>,
}

impl Default for ToolRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay_ms: 1000,
            max_delay_ms: 30000,
            retryable_errors: vec![
                ErrorKind::Timeout,
                ErrorKind::RateLimit,
                ErrorKind::ServiceUnavailable,
            ],
        }
    }
}

impl ToolRetryPolicy {
    /// Create a new retry policy with custom settings
    pub fn new(max_retries: u32, base_delay_ms: u64, max_delay_ms: u64) -> Self {
        Self {
            max_retries,
            base_delay_ms,
            max_delay_ms,
            retryable_errors: vec![
                ErrorKind::Timeout,
                ErrorKind::RateLimit,
                ErrorKind::ServiceUnavailable,
            ],
        }
    }

    /// Add a retryable error kind
    pub fn with_retryable_error(mut self, error_kind: ErrorKind) -> Self {
        if !self.retryable_errors.contains(&error_kind) {
            self.retryable_errors.push(error_kind);
        }
        self
    }
}

// ============================================================================
// Tool Execution Result (internal)
// ============================================================================

/// Internal result type for tool execution
enum ToolExecutionResult {
    /// Tool executed successfully
    Success { output: String },
    /// Tool execution failed
    Failure {
        error: String,
        error_kind: ErrorKind,
    },
}

// ============================================================================
// ToolExecutor Component
// ============================================================================

/// Tool Executor - executes tools with retry and error handling
///
/// This component:
/// - Subscribes to ToolCallRequested events
/// - Executes tools with exponential backoff retry logic
/// - Publishes ToolCallStarted, ToolCallCompleted, ToolCallFailed, ToolCallRetrying events
pub struct ToolExecutor {
    /// Retry policy configuration
    retry_policy: ToolRetryPolicy,
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor {
    /// Create a new ToolExecutor with default retry policy
    pub fn new() -> Self {
        Self {
            retry_policy: ToolRetryPolicy::default(),
        }
    }

    /// Create a ToolExecutor with a custom retry policy
    pub fn with_retry_policy(retry_policy: ToolRetryPolicy) -> Self {
        Self { retry_policy }
    }

    /// Calculate delay for exponential backoff
    ///
    /// delay = base_delay_ms * 2^(attempt-1), capped at max_delay_ms
    /// attempt is 1-based (first retry is attempt 1)
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }

        // Calculate 2^(attempt-1)
        let multiplier = 1u64 << (attempt - 1).min(31); // Prevent overflow

        // Calculate delay with overflow protection
        let delay = self.retry_policy.base_delay_ms.saturating_mul(multiplier);

        // Cap at max_delay_ms
        delay.min(self.retry_policy.max_delay_ms)
    }

    /// Check if an error kind is retryable
    pub fn is_retryable(&self, error_kind: ErrorKind) -> bool {
        self.retry_policy.retryable_errors.contains(&error_kind)
    }

    /// Execute tool once (stub implementation)
    ///
    /// In a real implementation, this would delegate to ToolRegistry.
    /// For now, it returns a stub success response or simulates errors.
    async fn execute_once(
        &self,
        request: &ToolCallRequest,
        ctx: &EventContext,
    ) -> ToolExecutionResult {
        // Check abort signal
        if ctx.is_aborted() {
            return ToolExecutionResult::Failure {
                error: "Execution aborted by user".to_string(),
                error_kind: ErrorKind::Aborted,
            };
        }

        // Stub implementation - in real implementation this would call ToolRegistry
        // For now, return a success response with the tool name and parameters
        let output = serde_json::json!({
            "status": "success",
            "tool": request.tool,
            "message": format!("Tool '{}' executed successfully (stub)", request.tool),
            "parameters_received": request.parameters
        });

        ToolExecutionResult::Success {
            output: output.to_string(),
        }
    }

    /// Execute tool with retry logic
    ///
    /// Retries on retryable errors with exponential backoff.
    /// Publishes ToolCallRetrying events before each retry.
    async fn execute_with_retry(
        &self,
        request: &ToolCallRequest,
        call_id: &str,
        ctx: &EventContext,
    ) -> Result<(String, u32), (String, ErrorKind, u32)> {
        let mut attempt = 0u32;
        let max_attempts = self.retry_policy.max_retries + 1; // +1 for initial attempt

        loop {
            attempt += 1;

            // Check abort signal before each attempt
            if ctx.is_aborted() {
                return Err((
                    "Execution aborted by user".to_string(),
                    ErrorKind::Aborted,
                    attempt,
                ));
            }

            // Execute the tool
            let result = self.execute_once(request, ctx).await;

            match result {
                ToolExecutionResult::Success { output } => {
                    return Ok((output, attempt));
                }
                ToolExecutionResult::Failure { error, error_kind } => {
                    // Check if we should retry
                    let can_retry = self.is_retryable(error_kind) && attempt < max_attempts;

                    if !can_retry {
                        return Err((error, error_kind, attempt));
                    }

                    // Calculate delay for this retry
                    let delay_ms = self.calculate_delay(attempt);

                    // Publish ToolCallRetrying event
                    let retry_event = AetherEvent::ToolCallRetrying(ToolCallRetry {
                        call_id: call_id.to_string(),
                        attempt,
                        delay_ms,
                        reason: Some(error.clone()),
                    });
                    ctx.bus.publish(retry_event).await;

                    // Sleep for the calculated delay
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

                    // Check abort signal after sleep
                    if ctx.is_aborted() {
                        return Err((
                            "Execution aborted by user".to_string(),
                            ErrorKind::Aborted,
                            attempt,
                        ));
                    }
                }
            }
        }
    }
}

// ============================================================================
// EventHandler Implementation
// ============================================================================

#[async_trait]
impl EventHandler for ToolExecutor {
    fn name(&self) -> &'static str {
        "ToolExecutor"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallRequested]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Only handle ToolCallRequested events
        let request = match event {
            AetherEvent::ToolCallRequested(req) => req,
            _ => return Ok(vec![]),
        };

        // Generate a unique call ID
        let call_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now().timestamp_millis();

        // Publish ToolCallStarted event
        let started_event = AetherEvent::ToolCallStarted(ToolCallStarted {
            call_id: call_id.clone(),
            tool: request.tool.clone(),
            input: request.parameters.clone(),
            timestamp: started_at,
            session_id: None,
        });
        ctx.bus.publish(started_event).await;

        // Execute with retry
        let result = self.execute_with_retry(request, &call_id, ctx).await;

        let completed_at = chrono::Utc::now().timestamp_millis();

        match result {
            Ok((output, _attempts)) => {
                // Return ToolCallCompleted event
                Ok(vec![AetherEvent::ToolCallCompleted(ToolCallResult {
                    call_id,
                    tool: request.tool.clone(),
                    input: request.parameters.clone(),
                    output,
                    started_at,
                    completed_at,
                    token_usage: TokenUsage::default(),
                    session_id: None,
                })])
            }
            Err((error, error_kind, attempts)) => {
                // Return ToolCallFailed event
                Ok(vec![AetherEvent::ToolCallFailed(ToolCallError {
                    call_id,
                    tool: request.tool.clone(),
                    error,
                    error_kind,
                    is_retryable: self.is_retryable(error_kind),
                    attempts,
                    session_id: None,
                })])
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBus;

    // ========================================================================
    // ToolRetryPolicy Tests
    // ========================================================================

    #[test]
    fn test_retry_policy_default() {
        let policy = ToolRetryPolicy::default();

        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.base_delay_ms, 1000);
        assert_eq!(policy.max_delay_ms, 30000);
        assert!(policy.retryable_errors.contains(&ErrorKind::Timeout));
        assert!(policy.retryable_errors.contains(&ErrorKind::RateLimit));
        assert!(policy
            .retryable_errors
            .contains(&ErrorKind::ServiceUnavailable));
        assert!(!policy.retryable_errors.contains(&ErrorKind::NotFound));
        assert!(!policy.retryable_errors.contains(&ErrorKind::InvalidInput));
    }

    #[test]
    fn test_retry_policy_custom() {
        let policy = ToolRetryPolicy::new(5, 500, 60000);

        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.base_delay_ms, 500);
        assert_eq!(policy.max_delay_ms, 60000);
    }

    #[test]
    fn test_retry_policy_with_retryable_error() {
        let policy = ToolRetryPolicy::default().with_retryable_error(ErrorKind::ExecutionFailed);

        assert!(policy
            .retryable_errors
            .contains(&ErrorKind::ExecutionFailed));

        // Adding same error twice should not duplicate
        let policy2 = policy
            .clone()
            .with_retryable_error(ErrorKind::ExecutionFailed);
        let count = policy2
            .retryable_errors
            .iter()
            .filter(|e| **e == ErrorKind::ExecutionFailed)
            .count();
        assert_eq!(count, 1);
    }

    // ========================================================================
    // Calculate Delay Tests
    // ========================================================================

    #[test]
    fn test_calculate_delay() {
        let executor = ToolExecutor::new();

        // attempt 0 should return 0
        assert_eq!(executor.calculate_delay(0), 0);

        // attempt 1: base_delay * 2^0 = 1000 * 1 = 1000
        assert_eq!(executor.calculate_delay(1), 1000);

        // attempt 2: base_delay * 2^1 = 1000 * 2 = 2000
        assert_eq!(executor.calculate_delay(2), 2000);

        // attempt 3: base_delay * 2^2 = 1000 * 4 = 4000
        assert_eq!(executor.calculate_delay(3), 4000);

        // attempt 4: base_delay * 2^3 = 1000 * 8 = 8000
        assert_eq!(executor.calculate_delay(4), 8000);

        // attempt 5: base_delay * 2^4 = 1000 * 16 = 16000
        assert_eq!(executor.calculate_delay(5), 16000);

        // attempt 6: base_delay * 2^5 = 1000 * 32 = 32000, but capped at 30000
        assert_eq!(executor.calculate_delay(6), 30000);

        // Very high attempt should be capped at max_delay_ms
        assert_eq!(executor.calculate_delay(100), 30000);
    }

    #[test]
    fn test_calculate_delay_custom_policy() {
        let policy = ToolRetryPolicy::new(5, 500, 10000);
        let executor = ToolExecutor::with_retry_policy(policy);

        // attempt 1: 500 * 1 = 500
        assert_eq!(executor.calculate_delay(1), 500);

        // attempt 2: 500 * 2 = 1000
        assert_eq!(executor.calculate_delay(2), 1000);

        // attempt 3: 500 * 4 = 2000
        assert_eq!(executor.calculate_delay(3), 2000);

        // attempt 5: 500 * 16 = 8000
        assert_eq!(executor.calculate_delay(5), 8000);

        // attempt 6: 500 * 32 = 16000, capped at 10000
        assert_eq!(executor.calculate_delay(6), 10000);
    }

    // ========================================================================
    // Is Retryable Tests
    // ========================================================================

    #[test]
    fn test_is_retryable() {
        let executor = ToolExecutor::new();

        // Retryable errors
        assert!(executor.is_retryable(ErrorKind::Timeout));
        assert!(executor.is_retryable(ErrorKind::RateLimit));
        assert!(executor.is_retryable(ErrorKind::ServiceUnavailable));

        // Non-retryable errors
        assert!(!executor.is_retryable(ErrorKind::NotFound));
        assert!(!executor.is_retryable(ErrorKind::InvalidInput));
        assert!(!executor.is_retryable(ErrorKind::PermissionDenied));
        assert!(!executor.is_retryable(ErrorKind::ExecutionFailed));
        assert!(!executor.is_retryable(ErrorKind::Aborted));
    }

    #[test]
    fn test_is_retryable_custom_policy() {
        let policy = ToolRetryPolicy::default().with_retryable_error(ErrorKind::ExecutionFailed);
        let executor = ToolExecutor::with_retry_policy(policy);

        // Custom retryable error
        assert!(executor.is_retryable(ErrorKind::ExecutionFailed));

        // Default retryable errors still work
        assert!(executor.is_retryable(ErrorKind::Timeout));
    }

    // ========================================================================
    // ToolExecutor Construction Tests
    // ========================================================================

    #[test]
    fn test_tool_executor_new() {
        let executor = ToolExecutor::new();

        // Should have default policy
        assert_eq!(executor.retry_policy.max_retries, 3);
        assert_eq!(executor.retry_policy.base_delay_ms, 1000);
        assert_eq!(executor.retry_policy.max_delay_ms, 30000);
    }

    #[test]
    fn test_tool_executor_with_retry_policy() {
        let policy = ToolRetryPolicy::new(10, 100, 5000);
        let executor = ToolExecutor::with_retry_policy(policy);

        assert_eq!(executor.retry_policy.max_retries, 10);
        assert_eq!(executor.retry_policy.base_delay_ms, 100);
        assert_eq!(executor.retry_policy.max_delay_ms, 5000);
    }

    #[test]
    fn test_tool_executor_default() {
        let executor = ToolExecutor::default();

        assert_eq!(executor.retry_policy.max_retries, 3);
    }

    // ========================================================================
    // EventHandler Implementation Tests
    // ========================================================================

    #[test]
    fn test_handler_name() {
        let executor = ToolExecutor::new();
        assert_eq!(executor.name(), "ToolExecutor");
    }

    #[test]
    fn test_handler_subscriptions() {
        let executor = ToolExecutor::new();
        let subs = executor.subscriptions();

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0], EventType::ToolCallRequested);
    }

    #[tokio::test]
    async fn test_handler_ignores_other_events() {
        use crate::event::StopReason;

        let executor = ToolExecutor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // LoopStop event should be ignored
        let event = AetherEvent::LoopStop(StopReason::Completed);
        let result = executor.handle(&event, &ctx).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_handler_executes_tool_successfully() {
        let executor = ToolExecutor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let request = ToolCallRequest {
            tool: "test_tool".to_string(),
            parameters: serde_json::json!({"query": "test"}),
            plan_step_id: None,
        };

        let event = AetherEvent::ToolCallRequested(request);
        let result = executor.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AetherEvent::ToolCallCompleted(_)));

        if let AetherEvent::ToolCallCompleted(completed) = &result[0] {
            assert_eq!(completed.tool, "test_tool");
            assert!(completed.output.contains("test_tool"));
            assert!(completed.completed_at >= completed.started_at);
        }
    }

    #[tokio::test]
    async fn test_handler_publishes_started_event() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        // Subscribe to events
        let mut subscriber = bus.subscribe();

        let request = ToolCallRequest {
            tool: "test_tool".to_string(),
            parameters: serde_json::json!({}),
            plan_step_id: None,
        };

        let event = AetherEvent::ToolCallRequested(request);

        // Handle in background so we can receive events
        let handle = tokio::spawn({
            let executor = ToolExecutor::new();
            let ctx = ctx.clone();
            async move { executor.handle(&event, &ctx).await }
        });

        // Wait a bit for the started event
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // The ToolCallStarted event should have been published
        let received =
            tokio::time::timeout(tokio::time::Duration::from_millis(100), subscriber.recv()).await;

        assert!(received.is_ok());
        if let Ok(Ok(timestamped)) = received {
            assert!(matches!(timestamped.event, AetherEvent::ToolCallStarted(_)));
        }

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_handler_respects_abort_signal() {
        let executor = ToolExecutor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        // Signal abort
        ctx.abort();

        let request = ToolCallRequest {
            tool: "test_tool".to_string(),
            parameters: serde_json::json!({}),
            plan_step_id: None,
        };

        let event = AetherEvent::ToolCallRequested(request);
        let result = executor.handle(&event, &ctx).await.unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], AetherEvent::ToolCallFailed(_)));

        if let AetherEvent::ToolCallFailed(error) = &result[0] {
            assert_eq!(error.error_kind, ErrorKind::Aborted);
            assert!(!error.is_retryable);
        }
    }

    // ========================================================================
    // Execute Once Tests (Stub)
    // ========================================================================

    #[tokio::test]
    async fn test_execute_once_stub_success() {
        let executor = ToolExecutor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        let request = ToolCallRequest {
            tool: "search".to_string(),
            parameters: serde_json::json!({"query": "rust programming"}),
            plan_step_id: None,
        };

        let result = executor.execute_once(&request, &ctx).await;

        match result {
            ToolExecutionResult::Success { output } => {
                assert!(output.contains("search"));
                assert!(output.contains("success"));
            }
            ToolExecutionResult::Failure { .. } => {
                panic!("Expected success, got failure");
            }
        }
    }

    #[tokio::test]
    async fn test_execute_once_aborted() {
        let executor = ToolExecutor::new();
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        ctx.abort();

        let request = ToolCallRequest {
            tool: "test".to_string(),
            parameters: serde_json::json!({}),
            plan_step_id: None,
        };

        let result = executor.execute_once(&request, &ctx).await;

        match result {
            ToolExecutionResult::Failure { error_kind, .. } => {
                assert_eq!(error_kind, ErrorKind::Aborted);
            }
            ToolExecutionResult::Success { .. } => {
                panic!("Expected failure due to abort");
            }
        }
    }
}
