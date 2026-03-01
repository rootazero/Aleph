//! Meta-cognition integration for Agent Loop
//!
//! This module integrates the meta-cognition layer (reactive reflection,
//! anchor retrieval, and dynamic injection) into the Agent Loop's
//! observe-think-act-feedback cycle.

use crate::error::AlephError;
use crate::poe::meta_cognition::{
    AnchorRetriever, AnchorStore, BehavioralAnchor, FailureSignal, FailureSnapshot,
    ReactiveReflector, TagExtractor,
};
use crate::memory::store::MemoryBackend;
use std::collections::HashMap;
use crate::sync_primitives::{Arc, RwLock};

/// Configuration for meta-cognition integration
#[derive(Debug, Clone)]
pub struct MetaCognitionConfig {
    /// Whether meta-cognition is enabled
    pub enabled: bool,

    /// Cache size for anchor retrieval (LRU)
    pub cache_size: usize,

    /// Minimum confidence threshold for anchor injection (0.0-1.0)
    pub min_confidence: f32,

    /// Maximum number of anchors to inject per request
    pub max_anchors_per_request: usize,

    /// Whether to automatically retry tasks after reflection
    pub auto_retry_after_reflection: bool,
}

impl Default for MetaCognitionConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safe rollout
            cache_size: 100,
            min_confidence: 0.5,
            max_anchors_per_request: 5,
            auto_retry_after_reflection: false,
        }
    }
}

/// Meta-cognition integration for Agent Loop
///
/// This component bridges the meta-cognition layer with the Agent Loop,
/// providing:
/// - Reactive reflection on task failures
/// - Anchor retrieval for relevant behavioral rules
/// - Dynamic injection of anchors into system prompts
pub struct MetaCognitionIntegration {
    /// Reactive reflector for failure analysis
    reactive_reflector: Arc<ReactiveReflector>,

    /// Anchor retriever for context-based rule lookup
    anchor_retriever: Arc<RwLock<AnchorRetriever>>,

    /// Configuration
    config: MetaCognitionConfig,
}

impl MetaCognitionIntegration {
    /// Create a new meta-cognition integration
    ///
    /// # Arguments
    ///
    /// * `db` - Memory backend for storing failure experiences
    /// * `anchor_store` - Store for persisting behavioral anchors
    /// * `config` - Configuration for meta-cognition features
    // Allow arc_with_non_send_sync: ReactiveReflector and AnchorRetriever contain
    // rusqlite::Connection (not Sync) and dyn AiProvider. These are used within
    // a single-threaded context via std::sync::RwLock; Arc is used for shared
    // ownership, not cross-thread sharing.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(
        db: MemoryBackend,
        anchor_store: Arc<RwLock<AnchorStore>>,
        config: MetaCognitionConfig,
    ) -> Result<Self, AlephError> {
        use crate::poe::meta_cognition::LLMConfig;
        use crate::providers::create_mock_provider;

        let llm_config = LLMConfig::default();
        let provider = create_mock_provider();

        let reactive_reflector = Arc::new(ReactiveReflector::new(
            db.clone(),
            anchor_store.clone(),
            llm_config.clone(),
            provider,
        ));

        let anchor_retriever = Arc::new(RwLock::new(AnchorRetriever::new(
            anchor_store.clone(),
            TagExtractor::new(llm_config),
            config.cache_size,
        )));

        Ok(Self {
            reactive_reflector,
            anchor_retriever,
            config,
        })
    }

    /// Check if meta-cognition is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Handle task failure by triggering reactive reflection
    ///
    /// This method is called from the Agent Loop's feedback phase when
    /// a task fails. It analyzes the failure, generates a behavioral anchor,
    /// and optionally suggests a retry.
    ///
    /// # Arguments
    ///
    /// * `error` - The error message or failure description
    /// * `context` - Failure context (intent, execution trace, environment)
    ///
    /// # Returns
    ///
    /// * `Ok(Some(anchor))` - Reflection succeeded, anchor generated
    /// * `Ok(None)` - Reflection skipped (meta-cognition disabled)
    /// * `Err(e)` - Reflection failed (non-fatal, logged but doesn't crash loop)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use std::collections::HashMap;
    /// # use alephcore::agent_loop::meta_cognition_integration::MetaCognitionIntegration;
    /// # async fn example(integration: &MetaCognitionIntegration) -> Result<(), Box<dyn std::error::Error>> {
    /// let mut context = HashMap::new();
    /// context.insert("intent".to_string(), "Run Python script".to_string());
    /// context.insert("tool".to_string(), "shell_execute".to_string());
    ///
    /// if let Some(anchor) = integration.handle_task_failure(
    ///     "Python version mismatch",
    ///     context
    /// ).await? {
    ///     println!("Generated anchor: {}", anchor.rule_text);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn handle_task_failure(
        &self,
        error: &str,
        context: HashMap<String, String>,
    ) -> Result<Option<BehavioralAnchor>, AlephError> {
        if !self.config.enabled {
            return Ok(None);
        }

        tracing::info!(
            error = %error,
            context_keys = ?context.keys().collect::<Vec<_>>(),
            "Meta-cognition: handling task failure"
        );

        // Extract context fields
        let task_id = context
            .get("task_id")
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let intent = context.get("intent").cloned().unwrap_or_default();
        let execution_trace = context
            .get("execution_trace")
            .map(|s| vec![s.clone()])
            .unwrap_or_default();
        let failure_point = context.get("failure_point").cloned().unwrap_or_default();

        // Create failure signal
        let signal = FailureSignal::ExecutionError {
            task_id: task_id.clone(),
            error: error.to_string(),
            context: context.clone(),
        };

        // Create failure snapshot
        let snapshot = FailureSnapshot {
            experience_id: uuid::Uuid::new_v4().to_string(),
            intent,
            execution_trace,
            failure_point,
            error_message: error.to_string(),
            environment: context,
        };

        // Trigger reactive reflection
        match self
            .reactive_reflector
            .reflect_on_failure(signal, snapshot)
            .await
        {
            Ok(result) => {
                tracing::info!(
                    anchor_id = %result.anchor.id,
                    rule_text = %result.anchor.rule_text,
                    confidence = result.anchor.confidence,
                    should_retry = result.should_retry,
                    "Meta-cognition: reflection succeeded"
                );
                Ok(Some(result.anchor))
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Meta-cognition: reflection failed (non-fatal)"
                );
                Err(e)
            }
        }
    }

    /// Retrieve relevant behavioral anchors for a given intent
    ///
    /// This method is called from the Agent Loop's observe phase to fetch
    /// behavioral rules that should guide the current task execution.
    ///
    /// # Arguments
    ///
    /// * `intent` - The user's intent or request
    ///
    /// # Returns
    ///
    /// * `Ok(anchors)` - List of relevant anchors (filtered by confidence)
    /// * `Err(e)` - Retrieval failed (non-fatal)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use alephcore::agent_loop::meta_cognition_integration::MetaCognitionIntegration;
    /// # async fn example(integration: &MetaCognitionIntegration) -> Result<(), Box<dyn std::error::Error>> {
    /// let anchors = integration.retrieve_anchors_for_intent(
    ///     "Run Python script on macOS"
    /// ).await?;
    ///
    /// for anchor in anchors {
    ///     println!("Rule: {}", anchor.rule_text);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn retrieve_anchors_for_intent(
        &self,
        intent: &str,
    ) -> Result<Vec<BehavioralAnchor>, AlephError> {
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        tracing::debug!(
            intent = %intent,
            "Meta-cognition: retrieving anchors for intent"
        );

        let anchors: Vec<BehavioralAnchor> = self
            .anchor_retriever
            .write()
            .map_err(|e| AlephError::config(format!("Failed to acquire lock: {}", e)))?
            .retrieve_for_intent(intent)?;

        // Filter by confidence and limit count
        let filtered: Vec<BehavioralAnchor> = anchors
            .into_iter()
            .filter(|a| a.confidence >= self.config.min_confidence)
            .take(self.config.max_anchors_per_request)
            .collect();

        tracing::debug!(
            count = filtered.len(),
            "Meta-cognition: retrieved anchors"
        );

        Ok(filtered)
    }

    /// Inject behavioral anchors into the system prompt
    ///
    /// This method is called from the Agent Loop's think phase to augment
    /// the base system prompt with relevant behavioral rules.
    ///
    /// # Arguments
    ///
    /// * `base_prompt` - The original system prompt
    /// * `anchors` - Behavioral anchors to inject
    ///
    /// # Returns
    ///
    /// * Augmented system prompt with injected anchors
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use alephcore::agent_loop::meta_cognition_integration::MetaCognitionIntegration;
    /// # use alephcore::poe::meta_cognition::BehavioralAnchor;
    /// # fn example(integration: &MetaCognitionIntegration, anchors: Vec<BehavioralAnchor>) {
    /// let base_prompt = "You are a helpful AI assistant.";
    /// let augmented = integration.inject_into_prompt(base_prompt, &anchors);
    /// println!("{}", augmented);
    /// # }
    /// ```
    pub fn inject_into_prompt(&self, base_prompt: &str, anchors: &[BehavioralAnchor]) -> String {
        if !self.config.enabled || anchors.is_empty() {
            return base_prompt.to_string();
        }

        // Inline formatting (InjectionFormatter deprecated, replaced by PoePromptLayer)
        let anchor_section = Self::format_anchors(anchors);

        format!(
            "{}\n\n# Behavioral Anchors (Learned Rules)\n\n{}\n",
            base_prompt, anchor_section
        )
    }

    /// Format behavioral anchors for prompt injection (inlined from deprecated InjectionFormatter)
    fn format_anchors(anchors: &[BehavioralAnchor]) -> String {
        if anchors.is_empty() {
            return String::new();
        }

        let mut output = String::from("## Behavioral Guidelines\n\n");
        output.push_str("The following learned behaviors should guide your decision-making:\n\n");

        for (idx, anchor) in anchors.iter().enumerate() {
            output.push_str(&format!("{}. **{}**\n", idx + 1, anchor.rule_text));
            output.push_str(&format!("   - Priority: {}\n", anchor.priority));
            output.push_str(&format!("   - Confidence: {:.2}\n", anchor.confidence));
            output.push_str(&format!("   - Tags: {}\n", anchor.trigger_tags.join(", ")));
            output.push('\n');
        }

        output
    }

    /// Update anchor confidence based on task outcome
    ///
    /// This method should be called after task completion to update the
    /// confidence scores of anchors that were active during execution.
    ///
    /// # Arguments
    ///
    /// * `anchor_ids` - IDs of anchors that were active
    /// * `success` - Whether the task succeeded
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Confidence updated successfully
    /// * `Err(e)` - Update failed (non-fatal)
    pub async fn update_anchor_confidence(
        &self,
        anchor_ids: &[String],
        success: bool,
    ) -> Result<(), AlephError> {
        if !self.config.enabled || anchor_ids.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            anchor_count = anchor_ids.len(),
            success = success,
            "Meta-cognition: updating anchor confidence"
        );

        let mut anchor_store = self
            .reactive_reflector
            .anchor_store()
            .write()
            .map_err(|e| AlephError::config(format!("Failed to acquire lock: {}", e)))?;

        for anchor_id in anchor_ids {
            if let Ok(Some(mut anchor)) = anchor_store.get(anchor_id) {
                anchor.update_confidence(success);
                if let Err(e) = anchor_store.update(&anchor) {
                    tracing::warn!(
                        anchor_id = %anchor_id,
                        error = %e,
                        "Failed to update anchor confidence"
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::poe::meta_cognition::{AnchorScope, AnchorSource};
    use crate::memory::store::LanceMemoryBackend;
    use rusqlite::Connection;
    use tempfile::TempDir;

    #[allow(clippy::arc_with_non_send_sync)]
    async fn setup_integration() -> Result<MetaCognitionIntegration, AlephError> {
        // Create temporary directory for memory backend
        let temp_dir = TempDir::new().map_err(|e| AlephError::config(e.to_string()))?;
        let db_path = temp_dir.path().join("lance_db");
        let db: MemoryBackend = Arc::new(LanceMemoryBackend::open_or_create(&db_path)
            .await
            .map_err(|e| AlephError::config(e.to_string()))?);

        let conn = Arc::new(Connection::open_in_memory().map_err(|e| AlephError::config(e.to_string()))?);

        // Initialize schema
        crate::memory::cortex::meta_cognition::schema::initialize_schema(&conn)
            .map_err(|e| AlephError::config(e.to_string()))?;

        let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));

        let config = MetaCognitionConfig {
            enabled: true,
            cache_size: 10,
            min_confidence: 0.5,
            max_anchors_per_request: 3,
            auto_retry_after_reflection: false,
        };

        MetaCognitionIntegration::new(db, anchor_store, config)
    }

    #[tokio::test]
    async fn test_integration_creation() {
        let integration = setup_integration().await.unwrap();
        assert!(integration.is_enabled());
    }

    #[tokio::test]
    async fn test_disabled_integration_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("lance_db");
        let db: MemoryBackend = Arc::new(LanceMemoryBackend::open_or_create(&db_path).await.unwrap());
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        crate::memory::cortex::meta_cognition::schema::initialize_schema(&conn).unwrap();
        let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));

        let config = MetaCognitionConfig {
            enabled: false,
            ..Default::default()
        };

        let integration = MetaCognitionIntegration::new(db, anchor_store, config).unwrap();

        let mut context = HashMap::new();
        context.insert("intent".to_string(), "test".to_string());

        let result = integration
            .handle_task_failure("test error", context)
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_retrieve_anchors_empty_when_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("lance_db");
        let db: MemoryBackend = Arc::new(LanceMemoryBackend::open_or_create(&db_path).await.unwrap());
        let conn = Arc::new(Connection::open_in_memory().unwrap());
        crate::memory::cortex::meta_cognition::schema::initialize_schema(&conn).unwrap();
        let anchor_store = Arc::new(RwLock::new(AnchorStore::new(conn)));

        let config = MetaCognitionConfig {
            enabled: false,
            ..Default::default()
        };

        let integration = MetaCognitionIntegration::new(db, anchor_store, config).unwrap();

        let anchors = integration
            .retrieve_anchors_for_intent("test intent")
            .await
            .unwrap();

        assert!(anchors.is_empty());
    }

    #[tokio::test]
    async fn test_inject_into_prompt_no_anchors() {
        let integration = setup_integration().await.unwrap();

        let base_prompt = "You are a helpful assistant.";
        let result = integration.inject_into_prompt(base_prompt, &[]);

        assert_eq!(result, base_prompt);
    }

    #[tokio::test]
    async fn test_inject_into_prompt_with_anchors() {
        let integration = setup_integration().await.unwrap();

        let anchor = BehavioralAnchor::new(
            "test-id".to_string(),
            "Always check Python version".to_string(),
            vec!["Python".to_string()],
            AnchorSource::ManualInjection {
                author: "test".to_string(),
            },
            AnchorScope::Global,
            100,
            0.8,
        );

        let base_prompt = "You are a helpful assistant.";
        let result = integration.inject_into_prompt(base_prompt, &[anchor]);

        assert!(result.contains("Behavioral Anchors"));
        assert!(result.contains("Always check Python version"));
    }
}
