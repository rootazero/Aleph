//! Critic agent for proactive excellence learning
//!
//! This module implements "excellence learning" - the system's ability to scan
//! successful but mediocre task executions during idle time and generate
//! optimization suggestions.

use super::types::{AnchorScope, AnchorSource, BehavioralAnchor};
use super::AnchorStore;
use crate::error::AlephError;
use crate::poe::crystallization::experience::Experience;
use crate::memory::store::MemoryBackend;
use crate::providers::AiProvider;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::{Arc, RwLock};
use uuid::Uuid;

/// Configuration for critic scanning behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticScanConfig {
    /// Minimum idle time in seconds before triggering scan
    pub min_idle_seconds: u64,

    /// Number of experiences to scan per batch
    pub batch_size: usize,

    /// Efficiency threshold (0.0-1.0) for triggering criticism
    /// Experiences below this threshold are candidates for optimization
    pub efficiency_threshold: f32,

    /// Redundant operations threshold
    /// Number of redundant operations before suggesting optimization
    pub redundancy_threshold: usize,
}

impl Default for CriticScanConfig {
    fn default() -> Self {
        Self {
            min_idle_seconds: 300,    // 5 minutes
            batch_size: 10,            // Scan 10 experiences per batch
            efficiency_threshold: 0.7, // Below 70% efficiency triggers criticism
            redundancy_threshold: 3,   // 3+ redundant operations triggers optimization
        }
    }
}

/// Placeholder for task step representation
/// TODO: Replace with actual TaskStep from dispatcher module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    /// Tool name
    pub tool: String,

    /// Tool parameters (JSON)
    pub parameters: String,

    /// Result (JSON)
    pub result: Option<String>,
}

/// Analysis of a task execution chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainAnalysis {
    /// Number of redundant file reads detected
    pub redundant_reads: usize,

    /// List of unnecessary steps identified
    pub unnecessary_steps: Vec<String>,

    /// List of missing optimizations
    pub missing_optimizations: Vec<String>,

    /// Overall efficiency score (0.0-1.0)
    pub efficiency_score: f32,
}

/// Critic report with optimization suggestions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticReport {
    /// Experience ID being criticized
    pub experience_id: String,

    /// Chain analysis results
    pub analysis: ChainAnalysis,

    /// Suggested behavioral anchor for optimization
    pub suggested_anchor: BehavioralAnchor,

    /// Confidence in this suggestion (0.0-1.0)
    pub confidence: f32,
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

/// Critic agent for proactive excellence learning
///
/// This component scans successful but mediocre task executions during idle time
/// and generates optimization suggestions with medium priority (50) and lower
/// confidence (0.6) compared to reactive anchors.
pub struct CriticAgent {
    _db: MemoryBackend,
    _anchor_store: Arc<RwLock<AnchorStore>>,
    _scan_config: CriticScanConfig,
    _llm_config: LLMConfig,
    provider: Arc<dyn AiProvider>,
}

impl CriticAgent {
    /// Create a new critic agent
    ///
    /// # Arguments
    ///
    /// * `db` - Memory backend for querying experiences
    /// * `anchor_store` - Store for persisting behavioral anchors
    /// * `scan_config` - Configuration for scanning behavior
    /// * `llm_config` - LLM configuration for analysis
    /// * `provider` - AI provider for LLM calls
    pub fn new(
        db: MemoryBackend,
        anchor_store: Arc<RwLock<AnchorStore>>,
        scan_config: CriticScanConfig,
        llm_config: LLMConfig,
        provider: Arc<dyn AiProvider>,
    ) -> Self {
        Self {
            _db: db,
            _anchor_store: anchor_store,
            _scan_config: scan_config,
            _llm_config: llm_config,
            provider,
        }
    }

    /// Scan for improvement opportunities
    ///
    /// Queries the database for successful but mediocre experiences and generates
    /// optimization suggestions.
    ///
    /// # Returns
    ///
    /// * `Result<Vec<CriticReport>>` - List of critic reports with suggestions
    pub fn scan_for_improvements(&self) -> Result<Vec<CriticReport>, AlephError> {
        // TODO: Query database for successful but mediocre experiences
        // For now, return empty list as this requires database integration
        Ok(Vec::new())
    }

    /// Analyze a task execution chain for inefficiencies
    ///
    /// # Arguments
    ///
    /// * `exp` - The experience to analyze
    ///
    /// # Returns
    ///
    /// * `Result<ChainAnalysis>` - Analysis results
    pub fn analyze_task_chain(&self, exp: &Experience) -> Result<ChainAnalysis, AlephError> {
        // Parse tool sequence from JSON
        let tool_sequence: Vec<TaskStep> = serde_json::from_str(&exp.tool_sequence_json)
            .unwrap_or_else(|_| Vec::new());

        // Analyze the chain
        let redundant_reads = self.count_redundant_file_reads(&tool_sequence);
        let unnecessary_steps = self.detect_unnecessary_steps(&tool_sequence);
        let missing_optimizations = self.find_missing_optimizations(&tool_sequence);
        let efficiency_score = self.calculate_efficiency(&tool_sequence);

        Ok(ChainAnalysis {
            redundant_reads,
            unnecessary_steps,
            missing_optimizations,
            efficiency_score,
        })
    }

    /// Count redundant file reads in the task chain
    ///
    /// # Arguments
    ///
    /// * `chain` - The task execution chain
    ///
    /// # Returns
    ///
    /// * `usize` - Number of redundant file reads
    fn count_redundant_file_reads(&self, chain: &[TaskStep]) -> usize {
        // TODO: Implement actual redundancy detection
        // For now, stub implementation
        let mut file_reads = std::collections::HashSet::new();
        let mut redundant_count = 0;

        for step in chain {
            if step.tool == "read_file" {
                // Extract file path from parameters (simplified)
                if !file_reads.insert(step.parameters.clone()) {
                    redundant_count += 1;
                }
            }
        }

        redundant_count
    }

    /// Detect unnecessary steps in the task chain
    ///
    /// # Arguments
    ///
    /// * `chain` - The task execution chain
    ///
    /// # Returns
    ///
    /// * `Vec<String>` - List of unnecessary step descriptions
    fn detect_unnecessary_steps(&self, chain: &[TaskStep]) -> Vec<String> {
        // TODO: Implement actual unnecessary step detection
        // For now, stub implementation
        let mut unnecessary = Vec::new();

        // Example heuristic: consecutive identical tool calls
        for i in 1..chain.len() {
            if chain[i].tool == chain[i - 1].tool && chain[i].parameters == chain[i - 1].parameters {
                unnecessary.push(format!("Duplicate {} call", chain[i].tool));
            }
        }

        unnecessary
    }

    /// Find missing optimizations in the task chain
    ///
    /// # Arguments
    ///
    /// * `chain` - The task execution chain
    ///
    /// # Returns
    ///
    /// * `Vec<String>` - List of missing optimization suggestions
    fn find_missing_optimizations(&self, chain: &[TaskStep]) -> Vec<String> {
        // TODO: Implement actual optimization detection
        // For now, stub implementation
        let mut optimizations = Vec::new();

        // Example heuristic: suggest batching for multiple similar operations
        let mut tool_counts = std::collections::HashMap::new();
        for step in chain {
            *tool_counts.entry(step.tool.clone()).or_insert(0) += 1;
        }

        for (tool, count) in tool_counts {
            if count >= 3 {
                optimizations.push(format!("Consider batching {} operations (found {})", tool, count));
            }
        }

        optimizations
    }

    /// Calculate overall efficiency score for the task chain
    ///
    /// # Arguments
    ///
    /// * `chain` - The task execution chain
    ///
    /// # Returns
    ///
    /// * `f32` - Efficiency score (0.0-1.0)
    fn calculate_efficiency(&self, chain: &[TaskStep]) -> f32 {
        // TODO: Implement actual efficiency calculation
        // For now, stub implementation based on chain length
        if chain.is_empty() {
            return 1.0;
        }

        // Simple heuristic: longer chains are less efficient
        let base_score = 1.0 - (chain.len() as f32 * 0.05).min(0.5);

        // Penalize for redundant operations
        let redundant_reads = self.count_redundant_file_reads(chain);
        let redundancy_penalty = redundant_reads as f32 * 0.1;

        (base_score - redundancy_penalty).clamp(0.0, 1.0)
    }

    /// Generate a critic report with optimization suggestions using LLM
    ///
    /// # Arguments
    ///
    /// * `exp` - The experience to critique
    /// * `analysis` - The chain analysis results
    ///
    /// # Returns
    ///
    /// * `Result<CriticReport>` - Generated critic report
    ///
    /// # Note
    ///
    /// This method uses LLM to analyze the task execution and generate
    /// optimization suggestions based on the chain analysis.
    pub async fn generate_critic_report_async(
        &self,
        exp: &Experience,
        analysis: ChainAnalysis,
    ) -> Result<CriticReport, AlephError> {
        // Build prompt for LLM analysis
        let prompt = format!(
            r#"Analyze this task execution and suggest optimizations.

**Task Execution:**
- Experience ID: {}
- Pattern Hash: {}
- Efficiency Score: {:.2}
- Redundant Reads: {}
- Unnecessary Steps: {}
- Missing Optimizations: {}

**Your Task:**
Generate an optimization suggestion to improve task execution efficiency.

**Response Format (JSON):**
{{
  "optimization_type": "efficiency|redundancy|batching|caching",
  "rule_text": "Actionable optimization rule",
  "confidence": 0.0-1.0
}}

**Example:**
{{
  "optimization_type": "caching",
  "rule_text": "Cache file contents when reading the same file multiple times in a task",
  "confidence": 0.7
}}"#,
            exp.id,
            exp.pattern_hash,
            analysis.efficiency_score,
            analysis.redundant_reads,
            if analysis.unnecessary_steps.is_empty() {
                "None".to_string()
            } else {
                analysis.unnecessary_steps.join(", ")
            },
            if analysis.missing_optimizations.is_empty() {
                "None".to_string()
            } else {
                analysis.missing_optimizations.join(", ")
            }
        );

        let system_prompt = "You are an expert performance optimizer analyzing task executions. \
            Provide concise, actionable optimization suggestions in JSON format.";

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
                "optimization_type": "efficiency",
                "rule_text": response.lines().next().unwrap_or("Consider optimizing task execution"),
                "confidence": 0.6
            })
        });

        let rule_text = parsed["rule_text"]
            .as_str()
            .unwrap_or("Consider optimizing task execution")
            .to_string();
        let optimization_type = parsed["optimization_type"]
            .as_str()
            .unwrap_or("efficiency")
            .to_string();
        let confidence = parsed["confidence"].as_f64().unwrap_or(0.6) as f32;

        let anchor_id = Uuid::new_v4().to_string();
        let trigger_tags = vec!["optimization".to_string(), "proactive".to_string()];

        let suggested_anchor = BehavioralAnchor::new(
            anchor_id,
            rule_text,
            trigger_tags,
            AnchorSource::ProactiveReflection {
                pattern_hash: exp.pattern_hash.clone(),
                optimization_type,
            },
            AnchorScope::Global,
            50, // Medium priority for proactive anchors
            confidence,
        );

        Ok(CriticReport {
            experience_id: exp.id.clone(),
            analysis,
            suggested_anchor,
            confidence,
        })
    }

    /// Synchronous wrapper for generate_critic_report_async
    ///
    /// This method provides a synchronous interface for backward compatibility.
    /// It uses tokio::runtime::Handle to execute the async LLM call.
    ///
    /// # Arguments
    ///
    /// * `exp` - The experience to critique
    /// * `analysis` - The chain analysis results
    ///
    /// # Returns
    ///
    /// * `Result<CriticReport>` - Generated critic report
    pub fn generate_critic_report(
        &self,
        exp: &Experience,
        analysis: ChainAnalysis,
    ) -> Result<CriticReport, AlephError> {
        // Try to get current runtime handle, or create a new runtime
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // We're already in a tokio runtime, use block_in_place
                tokio::task::block_in_place(|| {
                    handle.block_on(self.generate_critic_report_async(exp, analysis))
                })
            }
            Err(_) => {
                // No runtime available, create a new one
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| AlephError::config(format!("Failed to create runtime: {}", e)))?;
                rt.block_on(self.generate_critic_report_async(exp, analysis))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::memory::cortex::meta_cognition::schema::initialize_schema; // Schema stays in cortex (DB concern)
    use crate::poe::crystallization::experience::ExperienceBuilder;
    use crate::memory::store::LanceMemoryBackend;
    use crate::providers::MockProvider;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn setup_test_critic() -> (CriticAgent, TempDir) {
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
            "optimization_type": "caching",
            "rule_text": "Optimize file reading by caching contents",
            "confidence": 0.7
        }"#;
        let provider = Arc::new(MockProvider::new(mock_response));

        (
            CriticAgent::new(
                db,
                anchor_store,
                CriticScanConfig::default(),
                LLMConfig::default(),
                provider,
            ),
            temp_dir,
        )
    }

    #[test]
    fn test_critic_scan_config_default() {
        let config = CriticScanConfig::default();
        assert_eq!(config.min_idle_seconds, 300);
        assert_eq!(config.batch_size, 10);
        assert_eq!(config.efficiency_threshold, 0.7);
        assert_eq!(config.redundancy_threshold, 3);
    }

    #[test]
    fn test_count_redundant_file_reads() {
        let (critic, _temp_dir) = setup_test_critic();

        let chain = vec![
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/path/to/file.txt".to_string(),
                result: Some("content".to_string()),
            },
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/path/to/file.txt".to_string(),
                result: Some("content".to_string()),
            },
            TaskStep {
                tool: "write_file".to_string(),
                parameters: "/path/to/output.txt".to_string(),
                result: Some("ok".to_string()),
            },
        ];

        let redundant_count = critic.count_redundant_file_reads(&chain);
        assert_eq!(redundant_count, 1); // Second read is redundant
    }

    #[test]
    fn test_detect_unnecessary_steps() {
        let (critic, _temp_dir) = setup_test_critic();

        let chain = vec![
            TaskStep {
                tool: "list_files".to_string(),
                parameters: "/path".to_string(),
                result: Some("[]".to_string()),
            },
            TaskStep {
                tool: "list_files".to_string(),
                parameters: "/path".to_string(),
                result: Some("[]".to_string()),
            },
        ];

        let unnecessary = critic.detect_unnecessary_steps(&chain);
        assert_eq!(unnecessary.len(), 1);
        assert!(unnecessary[0].contains("Duplicate"));
    }

    #[test]
    fn test_find_missing_optimizations() {
        let (critic, _temp_dir) = setup_test_critic();

        let chain = vec![
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/file1.txt".to_string(),
                result: Some("content1".to_string()),
            },
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/file2.txt".to_string(),
                result: Some("content2".to_string()),
            },
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/file3.txt".to_string(),
                result: Some("content3".to_string()),
            },
        ];

        let optimizations = critic.find_missing_optimizations(&chain);
        assert_eq!(optimizations.len(), 1);
        assert!(optimizations[0].contains("batching"));
    }

    #[test]
    fn test_calculate_efficiency() {
        let (critic, _temp_dir) = setup_test_critic();

        // Empty chain should have perfect efficiency
        let empty_chain: Vec<TaskStep> = vec![];
        assert_eq!(critic.calculate_efficiency(&empty_chain), 1.0);

        // Short chain should have high efficiency
        let short_chain = vec![TaskStep {
            tool: "test".to_string(),
            parameters: "{}".to_string(),
            result: None,
        }];
        let efficiency = critic.calculate_efficiency(&short_chain);
        assert!(efficiency > 0.9);
        assert!(efficiency <= 1.0);
    }

    #[test]
    fn test_analyze_task_chain() {
        let (critic, _temp_dir) = setup_test_critic();

        let tool_sequence = vec![
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/test.txt".to_string(),
                result: Some("content".to_string()),
            },
            TaskStep {
                tool: "read_file".to_string(),
                parameters: "/test.txt".to_string(),
                result: Some("content".to_string()),
            },
        ];

        let exp = ExperienceBuilder::new(
            "test-exp".to_string(),
            "test intent".to_string(),
            serde_json::to_string(&tool_sequence).unwrap(),
        )
        .pattern_hash("hash123".to_string())
        .build();

        let analysis = critic.analyze_task_chain(&exp).unwrap();

        assert_eq!(analysis.redundant_reads, 1);
        assert!(analysis.efficiency_score < 1.0);
    }

    #[test]
    fn test_generate_critic_report() {
        let (critic, _temp_dir) = setup_test_critic();

        let exp = ExperienceBuilder::new(
            "test-exp".to_string(),
            "test intent".to_string(),
            "[]".to_string(),
        )
        .pattern_hash("hash123".to_string())
        .build();

        let analysis = ChainAnalysis {
            redundant_reads: 2,
            unnecessary_steps: vec![],
            missing_optimizations: vec![],
            efficiency_score: 0.6,
        };

        let report = critic.generate_critic_report(&exp, analysis).unwrap();

        assert_eq!(report.experience_id, "test-exp");
        // Check that we got a meaningful response from the mock LLM
        assert!(!report.suggested_anchor.rule_text.is_empty());
        assert_eq!(report.suggested_anchor.priority, 50); // Medium priority
        // Confidence comes from LLM response
        assert!(report.suggested_anchor.confidence > 0.0);
        assert!(matches!(
            report.suggested_anchor.source,
            AnchorSource::ProactiveReflection { .. }
        ));
    }

    #[test]
    fn test_scan_for_improvements_empty() {
        let (critic, _temp_dir) = setup_test_critic();

        // Should return empty list for now (stub implementation)
        let reports = critic.scan_for_improvements().unwrap();
        assert_eq!(reports.len(), 0);
    }
}
