//! Reactive reflection component for immediate failure response
//!
//! This module implements "pain learning" - the system's ability to respond
//! immediately to failures by analyzing root causes and generating corrective
//! behavioral anchors.

use super::types::{AnchorScope, AnchorSource, BehavioralAnchor};
use super::AnchorStore;
use crate::error::AlephError;
use crate::memory::store::MemoryBackend;
use crate::providers::AiProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// Signals that trigger reactive reflection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureSignal {
    /// Task execution failed with an error
    ExecutionError {
        task_id: String,
        error: String,
        context: HashMap<String, String>,
    },

    /// User provided negative feedback
    NegativeFeedback {
        session_id: String,
        user_message: String,
        previous_response: String,
    },

    /// Success manifest validation failed
    ManifestValidationFailed {
        task_id: String,
        manifest: String,
        actual_result: String,
    },

    /// User asked follow-up question indicating intent drift
    IntentDrift {
        task_id: String,
        original_intent: String,
        followup_question: String,
        time_gap_seconds: u64,
    },

    /// Same failure pattern repeated multiple times
    RepeatedFailure {
        pattern_hash: String,
        failure_count: u32,
        recent_attempts: Vec<String>,
    },
}

/// Snapshot of failure context for analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureSnapshot {
    /// Unique identifier for this failure experience
    pub experience_id: String,

    /// Original user intent that led to this failure
    pub intent: String,

    /// Execution trace (tool calls, intermediate results)
    pub execution_trace: Vec<String>,

    /// Where in the execution the failure occurred
    pub failure_point: String,

    /// Error message or failure description
    pub error_message: String,

    /// Environment context (OS, language versions, etc.)
    pub environment: HashMap<String, String>,
}

/// Root cause analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCause {
    /// Human-readable root cause description
    pub root_cause: String,

    /// Whether this failure was preventable
    pub preventable: bool,

    /// Suggested behavioral rule to prevent recurrence
    pub suggested_rule: String,
}

/// Result of reactive reflection
#[derive(Debug, Clone)]
pub struct ReflectionResult {
    /// Generated behavioral anchor
    pub anchor: BehavioralAnchor,

    /// Failure snapshot for future reference
    pub snapshot: FailureSnapshot,

    /// Whether the task should be retried with the new anchor
    pub should_retry: bool,
}

/// Placeholder for LLM configuration
/// TODO: Replace with actual LLMConfig from providers module
#[derive(Debug, Clone)]
pub struct LLMConfig {
    pub model: String,
    pub temperature: f32,
}

impl Default for LLMConfig {
    fn default() -> Self {
        Self {
            model: "claude-3-5-sonnet-20241022".to_string(),
            temperature: 0.7,
        }
    }
}

/// Reactive reflector for immediate failure response
///
/// This component analyzes failures in real-time and generates corrective
/// behavioral anchors with high priority (100) and confidence (0.8).
pub struct ReactiveReflector {
    _db: MemoryBackend,
    anchor_store: Arc<RwLock<AnchorStore>>,
    _llm_config: LLMConfig,
    provider: Arc<dyn AiProvider>,
}

impl ReactiveReflector {
    /// Create a new reactive reflector
    ///
    /// # Arguments
    ///
    /// * `db` - Memory backend for storing failure experiences
    /// * `anchor_store` - Store for persisting behavioral anchors
    /// * `llm_config` - LLM configuration for root cause analysis
    /// * `provider` - AI provider for LLM calls
    pub fn new(
        db: MemoryBackend,
        anchor_store: Arc<RwLock<AnchorStore>>,
        llm_config: LLMConfig,
        provider: Arc<dyn AiProvider>,
    ) -> Self {
        Self {
            _db: db,
            anchor_store,
            _llm_config: llm_config,
            provider,
        }
    }

    /// Handle a failure signal and generate corrective anchor
    ///
    /// # Arguments
    ///
    /// * `signal` - The failure signal to analyze
    ///
    /// # Returns
    ///
    /// * `Result<ReflectionResult>` - Reflection result with anchor and snapshot
    ///
    pub fn handle_failure(&self, signal: FailureSignal) -> Result<ReflectionResult, AlephError> {
        // Step 1: Create failure snapshot
        let snapshot = self.create_failure_snapshot(&signal)?;

        // Step 2: Analyze root cause
        let root_cause = self.analyze_root_cause(&snapshot)?;

        // Step 3: Generate corrective anchor
        let anchor = self.generate_corrective_anchor(&root_cause)?;

        // Step 4: Persist anchor to store
        {
            let mut store = self.anchor_store.write().map_err(|e| {
                AlephError::config(format!("Failed to acquire anchor store lock: {}", e))
            })?;
            store.add(anchor.clone()).map_err(|e| {
                AlephError::config(format!("Failed to persist anchor: {}", e))
            })?;
        }

        // Step 5: Determine if retry is appropriate
        let should_retry = root_cause.preventable && matches!(signal, FailureSignal::ExecutionError { .. });

        Ok(ReflectionResult {
            anchor,
            snapshot,
            should_retry,
        })
    }

    /// Create a failure snapshot from a signal
    ///
    /// # Arguments
    ///
    /// * `signal` - The failure signal to snapshot
    ///
    /// # Returns
    ///
    /// * `Result<FailureSnapshot>` - Captured failure context
    pub fn create_failure_snapshot(
        &self,
        signal: &FailureSignal,
    ) -> Result<FailureSnapshot, AlephError> {
        let experience_id = Uuid::new_v4().to_string();

        let (intent, failure_point, error_message, execution_trace, context) = match signal {
            FailureSignal::ExecutionError {
                task_id,
                error,
                context,
            } => (
                format!("Execute task {}", task_id),
                "task_execution".to_string(),
                error.clone(),
                vec![format!("Task ID: {}", task_id)],
                context.clone(),
            ),
            FailureSignal::NegativeFeedback {
                session_id,
                user_message,
                previous_response,
            } => (
                user_message.clone(),
                "user_feedback".to_string(),
                format!("User dissatisfied with response: {}", previous_response),
                vec![format!("Session ID: {}", session_id)],
                HashMap::new(),
            ),
            FailureSignal::ManifestValidationFailed {
                task_id,
                manifest,
                actual_result,
            } => (
                format!("Validate task {} against manifest", task_id),
                "manifest_validation".to_string(),
                format!("Expected: {}, Got: {}", manifest, actual_result),
                vec![format!("Task ID: {}", task_id)],
                HashMap::new(),
            ),
            FailureSignal::IntentDrift {
                task_id,
                original_intent,
                followup_question,
                time_gap_seconds,
            } => (
                original_intent.clone(),
                "intent_drift".to_string(),
                format!(
                    "User asked follow-up after {}s: {}",
                    time_gap_seconds, followup_question
                ),
                vec![format!("Task ID: {}", task_id)],
                HashMap::new(),
            ),
            FailureSignal::RepeatedFailure {
                pattern_hash,
                failure_count,
                recent_attempts,
            } => (
                format!("Repeated failure pattern {}", pattern_hash),
                "repeated_failure".to_string(),
                format!("Failed {} times", failure_count),
                recent_attempts.clone(),
                HashMap::new(),
            ),
        };

        Ok(FailureSnapshot {
            experience_id,
            intent,
            execution_trace,
            failure_point,
            error_message,
            environment: context,
        })
    }

    /// Analyze root cause of failure using LLM
    ///
    /// # Arguments
    ///
    /// * `snapshot` - The failure snapshot to analyze
    ///
    /// # Returns
    ///
    /// * `Result<RootCause>` - Root cause analysis result
    ///
    /// # Note
    ///
    /// This method uses LLM to analyze the failure and determine:
    /// - The root cause of the failure
    /// - Whether the failure was preventable
    /// - A suggested behavioral rule to prevent recurrence
    pub async fn analyze_root_cause_async(
        &self,
        snapshot: &FailureSnapshot,
    ) -> Result<RootCause, AlephError> {
        // Build prompt for LLM analysis
        let prompt = format!(
            r#"Analyze this task failure and provide root cause analysis.

**Failure Context:**
- Intent: {}
- Failure Point: {}
- Error Message: {}
- Execution Trace: {}
- Environment: {:?}

**Your Task:**
1. Identify the root cause of this failure
2. Determine if this failure was preventable
3. Suggest a behavioral rule to prevent similar failures

**Response Format (JSON):**
{{
  "root_cause": "Brief description of the root cause",
  "preventable": true/false,
  "suggested_rule": "Actionable rule to prevent recurrence"
}}

**Example:**
{{
  "root_cause": "Python version mismatch - code requires Python 3.10+ but system has 3.8",
  "preventable": true,
  "suggested_rule": "Before executing Python scripts, verify Python version meets requirements"
}}"#,
            snapshot.intent,
            snapshot.failure_point,
            snapshot.error_message,
            snapshot.execution_trace.join(" → "),
            snapshot.environment
        );

        let system_prompt = "You are an expert system debugger analyzing task failures. \
            Provide concise, actionable root cause analysis in JSON format.";

        // Call LLM
        let response = self
            .provider
            .process(&prompt, Some(system_prompt))
            .await
            .map_err(|e| AlephError::provider(format!("LLM call failed: {}", e)))?;

        // Parse JSON response
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap_or_else(|_| {
            // Fallback: extract from text if JSON parsing fails
            serde_json::json!({
                "root_cause": response.lines().next().unwrap_or("Unknown failure"),
                "preventable": true,
                "suggested_rule": "Review and retry with corrected approach"
            })
        });

        Ok(RootCause {
            root_cause: parsed["root_cause"]
                .as_str()
                .unwrap_or("Unknown failure")
                .to_string(),
            preventable: parsed["preventable"].as_bool().unwrap_or(true),
            suggested_rule: parsed["suggested_rule"]
                .as_str()
                .unwrap_or("Review and retry")
                .to_string(),
        })
    }

    /// Synchronous wrapper for analyze_root_cause_async
    ///
    /// This method provides a synchronous interface for backward compatibility.
    /// It uses tokio::runtime::Handle to execute the async LLM call.
    ///
    /// # Arguments
    ///
    /// * `snapshot` - The failure snapshot to analyze
    ///
    /// # Returns
    ///
    /// * `Result<RootCause>` - Root cause analysis result
    pub fn analyze_root_cause(
        &self,
        snapshot: &FailureSnapshot,
    ) -> Result<RootCause, AlephError> {
        // Try to get current runtime handle, or create a new runtime
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // We're already in a tokio runtime, use block_in_place
                tokio::task::block_in_place(|| {
                    handle.block_on(self.analyze_root_cause_async(snapshot))
                })
            }
            Err(_) => {
                // No runtime available, create a new one
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| AlephError::config(format!("Failed to create runtime: {}", e)))?;
                rt.block_on(self.analyze_root_cause_async(snapshot))
            }
        }
    }

    /// Generate a corrective behavioral anchor
    ///
    /// # Arguments
    ///
    /// * `root_cause` - The root cause analysis result
    ///
    /// # Returns
    ///
    /// * `Result<BehavioralAnchor>` - Generated behavioral anchor
    pub fn generate_corrective_anchor(
        &self,
        root_cause: &RootCause,
    ) -> Result<BehavioralAnchor, AlephError> {
        let anchor_id = Uuid::new_v4().to_string();

        // Extract trigger tags from root cause
        // TODO: Improve tag extraction with NLP
        let trigger_tags = vec!["failure".to_string(), "reactive".to_string()];

        let anchor = BehavioralAnchor::new(
            anchor_id,
            root_cause.suggested_rule.clone(),
            trigger_tags,
            AnchorSource::ReactiveReflection {
                task_id: "unknown".to_string(), // TODO: Pass task_id through
                error_type: root_cause.root_cause.clone(),
            },
            AnchorScope::Global, // Apply globally for now
            100,                 // Highest priority for reactive anchors
            0.8,                 // High initial confidence
        );

        Ok(anchor)
    }

    /// Get a reference to the anchor store
    ///
    /// This is useful for external components that need to access the anchor store
    /// for operations like updating anchor confidence.
    pub fn anchor_store(&self) -> &Arc<RwLock<AnchorStore>> {
        &self.anchor_store
    }

    /// Async wrapper for handle_failure to support async contexts
    ///
    /// # Arguments
    ///
    /// * `signal` - The failure signal to analyze
    /// * `snapshot` - Pre-created failure snapshot
    ///
    /// # Returns
    ///
    /// * `Result<ReflectionResult>` - Reflection result with anchor and snapshot
    pub async fn reflect_on_failure(
        &self,
        signal: FailureSignal,
        snapshot: FailureSnapshot,
    ) -> Result<ReflectionResult, AlephError> {
        // Analyze root cause (CPU-bound, but fast enough to run inline)
        let root_cause = self.analyze_root_cause(&snapshot)?;

        // Generate corrective anchor
        let anchor = self.generate_corrective_anchor(&root_cause)?;

        // Persist anchor to store
        {
            let mut store = self.anchor_store.write().map_err(|e| {
                AlephError::config(format!("Failed to acquire anchor store lock: {}", e))
            })?;
            store.add(anchor.clone()).map_err(|e| {
                AlephError::config(format!("Failed to persist anchor: {}", e))
            })?;
        }

        // Determine if retry is appropriate
        let should_retry = root_cause.preventable && matches!(signal, FailureSignal::ExecutionError { .. });

        Ok(ReflectionResult {
            anchor,
            snapshot,
            should_retry,
        })
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::memory::cortex::meta_cognition::schema::initialize_schema;
    use crate::memory::store::LanceMemoryBackend;
    use crate::providers::MockProvider;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn setup_test_reflector() -> (ReactiveReflector, TempDir) {
        // Create in-memory database and initialize schema
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        initialize_schema(&conn).unwrap();
        let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));

        // Create temporary directory for memory backend
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("lance_db");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db: MemoryBackend = Arc::new(rt.block_on(LanceMemoryBackend::open_or_create(&db_path)).unwrap());

        // Create mock provider that returns properly formatted JSON
        let mock_response = r#"{
            "root_cause": "Task execution failed due to version mismatch",
            "preventable": true,
            "suggested_rule": "Before executing tasks, verify system requirements"
        }"#;
        let provider = Arc::new(MockProvider::new(mock_response));

        (
            ReactiveReflector::new(db, anchor_store, LLMConfig::default(), provider),
            temp_dir,
        )
    }

    #[test]
    fn test_create_failure_snapshot_execution_error() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let mut context = HashMap::new();
        context.insert("os".to_string(), "macOS".to_string());

        let signal = FailureSignal::ExecutionError {
            task_id: "task-123".to_string(),
            error: "Python version mismatch".to_string(),
            context,
        };

        let snapshot = reflector.create_failure_snapshot(&signal).unwrap();

        assert!(!snapshot.experience_id.is_empty());
        assert_eq!(snapshot.intent, "Execute task task-123");
        assert_eq!(snapshot.failure_point, "task_execution");
        assert_eq!(snapshot.error_message, "Python version mismatch");
        assert_eq!(snapshot.environment.get("os").unwrap(), "macOS");
        assert_eq!(snapshot.execution_trace.len(), 1);
    }

    #[test]
    fn test_create_failure_snapshot_negative_feedback() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let signal = FailureSignal::NegativeFeedback {
            session_id: "session-456".to_string(),
            user_message: "Find Python files".to_string(),
            previous_response: "I found 0 files".to_string(),
        };

        let snapshot = reflector.create_failure_snapshot(&signal).unwrap();

        assert_eq!(snapshot.intent, "Find Python files");
        assert_eq!(snapshot.failure_point, "user_feedback");
        assert!(snapshot
            .error_message
            .contains("User dissatisfied with response"));
    }

    #[test]
    fn test_analyze_root_cause_task_execution() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let snapshot = FailureSnapshot {
            experience_id: "exp-1".to_string(),
            intent: "Run Python script".to_string(),
            execution_trace: vec![],
            failure_point: "task_execution".to_string(),
            error_message: "Python 3.8 required, found 3.7".to_string(),
            environment: HashMap::new(),
        };

        let root_cause = reflector.analyze_root_cause(&snapshot).unwrap();

        assert!(root_cause.preventable);
        // Check that we got a meaningful response from the mock LLM
        assert!(!root_cause.root_cause.is_empty());
        assert!(!root_cause.suggested_rule.is_empty());
    }

    #[test]
    fn test_analyze_root_cause_repeated_failure() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let snapshot = FailureSnapshot {
            experience_id: "exp-2".to_string(),
            intent: "Repeated task".to_string(),
            execution_trace: vec![],
            failure_point: "repeated_failure".to_string(),
            error_message: "Failed 3 times".to_string(),
            environment: HashMap::new(),
        };

        let root_cause = reflector.analyze_root_cause(&snapshot).unwrap();

        assert!(root_cause.preventable);
        // Check that we got a meaningful response from the mock LLM
        assert!(!root_cause.root_cause.is_empty());
        assert!(!root_cause.suggested_rule.is_empty());
    }

    #[test]
    fn test_generate_corrective_anchor() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let root_cause = RootCause {
            root_cause: "Python version mismatch".to_string(),
            preventable: true,
            suggested_rule: "Always check Python version before running scripts".to_string(),
        };

        let anchor = reflector.generate_corrective_anchor(&root_cause).unwrap();

        assert!(!anchor.id.is_empty());
        assert_eq!(
            anchor.rule_text,
            "Always check Python version before running scripts"
        );
        assert_eq!(anchor.priority, 100); // Highest priority
        assert_eq!(anchor.confidence, 0.8); // High initial confidence
        assert!(matches!(
            anchor.source,
            AnchorSource::ReactiveReflection { .. }
        ));
        assert!(matches!(anchor.scope, AnchorScope::Global));
    }

    #[test]
    fn test_handle_failure_end_to_end() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let signal = FailureSignal::ExecutionError {
            task_id: "task-789".to_string(),
            error: "File not found: config.json".to_string(),
            context: HashMap::new(),
        };

        let result = reflector.handle_failure(signal).unwrap();

        // Verify anchor was generated
        assert!(!result.anchor.id.is_empty());
        assert_eq!(result.anchor.priority, 100);
        assert_eq!(result.anchor.confidence, 0.8);

        // Verify snapshot was created
        assert!(!result.snapshot.experience_id.is_empty());
        assert_eq!(result.snapshot.failure_point, "task_execution");

        // Verify should_retry is true for execution errors
        assert!(result.should_retry);

        // Verify anchor was persisted
        let store = reflector.anchor_store.read().unwrap();
        let retrieved = store.get(&result.anchor.id).unwrap();
        assert!(retrieved.is_some());
    }

    #[test]
    fn test_handle_failure_no_retry_for_feedback() {
        let (reflector, _temp_dir) = setup_test_reflector();

        let signal = FailureSignal::NegativeFeedback {
            session_id: "session-999".to_string(),
            user_message: "This is wrong".to_string(),
            previous_response: "I think it's correct".to_string(),
        };

        let result = reflector.handle_failure(signal).unwrap();

        // Should not retry for negative feedback
        assert!(!result.should_retry);
    }
}

