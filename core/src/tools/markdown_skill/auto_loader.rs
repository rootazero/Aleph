//! Auto-loader for Evolution-generated Markdown Skills
//!
//! Automatically generates and loads skills when Evolution Pipeline
//! detects solidification patterns.

use std::path::PathBuf;
use crate::sync_primitives::Arc;
use tracing::{info, warn};

use crate::error::Result;
use crate::skill_evolution::types::SolidificationSuggestion;
use crate::tools::{AlephToolDyn, AlephToolServer};

use super::generator::{MarkdownSkillGenerator, MarkdownSkillGeneratorConfig};
use super::loader::load_skills_from_dir;

/// Auto-loader for Evolution-generated skills
///
/// Handles the complete workflow:
/// 1. Receive SolidificationSuggestion from Evolution Pipeline
/// 2. Generate SKILL.md using MarkdownSkillGenerator
/// 3. Load skill into ToolServer for immediate availability
/// 4. Track loaded skills for management
pub struct EvolutionAutoLoader {
    /// Skill generator for creating SKILL.md files
    generator: MarkdownSkillGenerator,

    /// Tool server for registering loaded skills
    tool_server: Arc<AlephToolServer>,

    /// Track generated skill paths for later management
    generated_skills: std::sync::RwLock<Vec<PathBuf>>,
}

impl EvolutionAutoLoader {
    /// Create a new auto-loader with default configuration
    pub fn new(tool_server: Arc<AlephToolServer>) -> Self {
        Self {
            generator: MarkdownSkillGenerator::new(),
            tool_server,
            generated_skills: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// Create with custom generator configuration
    pub fn with_config(
        tool_server: Arc<AlephToolServer>,
        config: MarkdownSkillGeneratorConfig,
    ) -> Self {
        Self {
            generator: MarkdownSkillGenerator::with_config(config),
            tool_server,
            generated_skills: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// Auto-load a skill from a solidification suggestion
    ///
    /// Returns the number of tools successfully loaded (0 or 1)
    pub async fn load_from_suggestion(
        &self,
        suggestion: &SolidificationSuggestion,
    ) -> Result<usize> {
        info!(
            pattern_id = %suggestion.pattern_id,
            suggested_name = %suggestion.suggested_name,
            confidence = suggestion.confidence,
            "Auto-loading skill from Evolution suggestion"
        );

        // Phase 1: Generate SKILL.md
        let skill_path = match self.generator.generate(suggestion) {
            Ok(path) => path,
            Err(e) => {
                warn!(
                    error = %e,
                    pattern_id = %suggestion.pattern_id,
                    "Failed to generate SKILL.md from suggestion"
                );
                return Err(e);
            }
        };

        // Track generated skill
        let skill_dir = skill_path
            .parent()
            .ok_or_else(|| crate::error::AlephError::Other {
                message: "Invalid skill path".to_string(),
                suggestion: None,
            })?
            .to_path_buf();

        self.generated_skills
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(skill_dir.clone());

        // Phase 2: Load skill into ToolServer
        let tools: Vec<super::MarkdownCliTool> = load_skills_from_dir(&skill_dir).await;

        if tools.is_empty() {
            warn!(
                skill_path = %skill_path.display(),
                "Generated SKILL.md but failed to load as tool"
            );
            return Ok(0);
        }

        // Phase 3: Register in ToolServer
        let mut loaded_count = 0;
        for tool in tools {
            let tool_name = tool.name().to_string();
            let update_info = self.tool_server.replace_tool(tool).await;

            info!(
                tool_name = %tool_name,
                was_replaced = update_info.was_replaced,
                confidence = suggestion.confidence,
                "Auto-loaded Evolution skill into ToolServer"
            );

            loaded_count += 1;
        }

        Ok(loaded_count)
    }

    /// Load multiple suggestions in batch
    pub async fn load_batch(
        &self,
        suggestions: &[SolidificationSuggestion],
    ) -> Result<BatchLoadResult> {
        let mut loaded = 0;
        let mut failed = 0;

        for suggestion in suggestions {
            match self.load_from_suggestion(suggestion).await {
                Ok(count) => loaded += count,
                Err(e) => {
                    warn!(
                        error = %e,
                        pattern_id = %suggestion.pattern_id,
                        "Failed to auto-load suggestion"
                    );
                    failed += 1;
                }
            }
        }

        Ok(BatchLoadResult {
            total: suggestions.len(),
            loaded,
            failed,
        })
    }

    /// Get the list of generated skill directories
    pub fn get_generated_skills(&self) -> Vec<PathBuf> {
        self.generated_skills.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Clear the generated skills tracking list
    pub fn clear_tracking(&self) {
        self.generated_skills.write().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Result of batch loading multiple suggestions
#[derive(Debug, Clone)]
pub struct BatchLoadResult {
    /// Total number of suggestions processed
    pub total: usize,

    /// Number of skills successfully loaded
    pub loaded: usize,

    /// Number of suggestions that failed
    pub failed: usize,
}

impl BatchLoadResult {
    /// Check if all suggestions were loaded successfully
    pub fn all_succeeded(&self) -> bool {
        self.failed == 0 && self.loaded == self.total
    }

    /// Check if any suggestions were loaded
    pub fn any_succeeded(&self) -> bool {
        self.loaded > 0
    }

    /// Get success rate (0.0 - 1.0)
    pub fn success_rate(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.loaded as f32 / self.total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::SkillMetrics;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "test-pattern".to_string(),
            suggested_name: "Test Skill".to_string(),
            suggested_description: "A test skill for auto-loading".to_string(),
            confidence: 0.85,
            instructions_preview: "Use echo to print messages".to_string(),
            sample_contexts: vec!["echo hello".to_string()],
            metrics: SkillMetrics {
                skill_id: "test-pattern".to_string(),
                total_executions: 10,
                successful_executions: 9,
                avg_duration_ms: 100.0,
                avg_satisfaction: Some(0.9),
                failure_rate: 0.1,
                first_used: 0,
                last_used: 1000,
                context_frequency: HashMap::new(),
            },
        }
    }

    #[tokio::test]
    async fn test_auto_loader_creation() {
        let tool_server = Arc::new(AlephToolServer::new());
        let loader = EvolutionAutoLoader::new(tool_server);

        assert_eq!(loader.get_generated_skills().len(), 0);
    }

    #[tokio::test]
    async fn test_load_from_suggestion() {
        let temp_dir = TempDir::new().unwrap();
        let tool_server = Arc::new(AlephToolServer::new());

        let config = MarkdownSkillGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let loader = EvolutionAutoLoader::with_config(tool_server.clone(), config);
        let suggestion = create_test_suggestion();

        // Load skill
        let result = loader.load_from_suggestion(&suggestion).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);

        // Verify skill is in ToolServer
        assert!(tool_server.has_tool("test-skill").await);

        // Verify tracking
        let generated = loader.get_generated_skills();
        assert_eq!(generated.len(), 1);
    }

    #[tokio::test]
    async fn test_batch_load() {
        let temp_dir = TempDir::new().unwrap();
        let tool_server = Arc::new(AlephToolServer::new());

        let config = MarkdownSkillGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let loader = EvolutionAutoLoader::with_config(tool_server, config);

        let suggestions = vec![
            create_test_suggestion(),
            SolidificationSuggestion {
                pattern_id: "pattern-2".to_string(),
                suggested_name: "Second Skill".to_string(),
                ..create_test_suggestion()
            },
        ];

        let result = loader.load_batch(&suggestions).await.unwrap();
        assert_eq!(result.total, 2);
        assert_eq!(result.loaded, 2);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());
        assert_eq!(result.success_rate(), 1.0);
    }

    #[tokio::test]
    async fn test_clear_tracking() {
        let temp_dir = TempDir::new().unwrap();
        let tool_server = Arc::new(AlephToolServer::new());

        let config = MarkdownSkillGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let loader = EvolutionAutoLoader::with_config(tool_server, config);
        let suggestion = create_test_suggestion();

        loader.load_from_suggestion(&suggestion).await.unwrap();
        assert_eq!(loader.get_generated_skills().len(), 1);

        loader.clear_tracking();
        assert_eq!(loader.get_generated_skills().len(), 0);
    }
}
