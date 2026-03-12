//! Markdown Skill Generator - Evolution Loop Integration
//!
//! Generates SKILL.md files from Evolution Loop solidification suggestions.
//! This enables automatic skill creation from detected usage patterns.

use std::path::PathBuf;
use tracing::{debug, info};

use crate::error::Result;

/// Metrics for a skill pattern (previously in skill_evolution module).
#[derive(Debug, Clone)]
pub struct SkillMetrics {
    pub skill_id: String,
    pub total_executions: usize,
    pub successful_executions: usize,
    pub avg_duration_ms: f64,
    pub avg_satisfaction: Option<f64>,
    pub failure_rate: f64,
    pub first_used: i64,
    pub last_used: i64,
    pub context_frequency: std::collections::HashMap<String, usize>,
}

impl SkillMetrics {
    /// Success rate (0.0 - 1.0)
    pub fn success_rate(&self) -> f64 {
        if self.total_executions == 0 {
            return 0.0;
        }
        self.successful_executions as f64 / self.total_executions as f64
    }
}

/// Suggestion for solidifying a detected usage pattern into a skill.
#[derive(Debug, Clone)]
pub struct SolidificationSuggestion {
    pub pattern_id: String,
    pub suggested_name: String,
    pub suggested_description: String,
    pub confidence: f32,
    pub instructions_preview: String,
    pub sample_contexts: Vec<String>,
    pub metrics: SkillMetrics,
}

use super::spec::{
    AlephExtensions, AlephSkillSpec, ConfirmationMode, EvolutionMeta,
    InputHint, NetworkMode, RequiresSpec, SandboxMode, SecuritySpec, SkillMetadata,
};

/// Configuration for Markdown Skill generation
#[derive(Debug, Clone)]
pub struct MarkdownSkillGeneratorConfig {
    /// Output directory for generated skills (default: ~/.aleph/skills/generated)
    pub output_dir: PathBuf,

    /// Default sandbox mode for generated skills
    pub default_sandbox: SandboxMode,

    /// Default confirmation mode
    pub default_confirmation: ConfirmationMode,

    /// Whether to generate input hints from instructions
    pub generate_input_hints: bool,
}

impl Default for MarkdownSkillGeneratorConfig {
    fn default() -> Self {
        let output_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("skills")
            .join("generated");

        Self {
            output_dir,
            default_sandbox: SandboxMode::Host,
            default_confirmation: ConfirmationMode::Write,
            generate_input_hints: true,
        }
    }
}

/// Generator for Markdown-based skills from evolution suggestions
pub struct MarkdownSkillGenerator {
    config: MarkdownSkillGeneratorConfig,
}

impl MarkdownSkillGenerator {
    /// Create a new generator with default configuration
    pub fn new() -> Self {
        Self {
            config: MarkdownSkillGeneratorConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: MarkdownSkillGeneratorConfig) -> Self {
        Self { config }
    }

    /// Generate a SKILL.md file from a solidification suggestion
    ///
    /// Returns the path to the generated file
    pub fn generate(&self, suggestion: &SolidificationSuggestion) -> Result<PathBuf> {
        let skill_name = to_skill_name(&suggestion.suggested_name);
        let skill_dir = self.config.output_dir.join(&skill_name);

        // Create skill directory
        std::fs::create_dir_all(&skill_dir)?;

        // Generate spec
        let spec = self.generate_spec(suggestion);

        // Generate markdown content
        let markdown_content = self.generate_markdown_content(suggestion);

        // Combine frontmatter + content
        let skill_md = self.generate_skill_md(&spec, &markdown_content)?;

        // Write to SKILL.md
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, skill_md)?;

        info!(
            skill_name = %skill_name,
            confidence = suggestion.confidence,
            path = %skill_path.display(),
            "Generated Markdown skill from evolution pattern"
        );

        Ok(skill_path)
    }

    /// Generate AlephSkillSpec from suggestion
    fn generate_spec(&self, suggestion: &SolidificationSuggestion) -> AlephSkillSpec {
        let skill_name = to_skill_name(&suggestion.suggested_name);

        // Extract binary requirements from instructions
        let bins = self.extract_binary_requirements(&suggestion.instructions_preview);

        // Generate input hints if enabled
        let input_hints = if self.config.generate_input_hints {
            self.extract_input_hints(&suggestion.instructions_preview)
        } else {
            Default::default()
        };

        // Build metadata
        let metadata = SkillMetadata {
            requires: RequiresSpec { bins },
            aleph: Some(AlephExtensions {
                security: SecuritySpec {
                    sandbox: self.config.default_sandbox.clone(),
                    confirmation: self.config.default_confirmation.clone(),
                    network: NetworkMode::Internet,
                },
                input_hints,
                evolution: Some(EvolutionMeta {
                    source: "auto-generated".to_string(),
                    confidence_score: suggestion.confidence as f64,
                    created_from_trace: Some(suggestion.pattern_id.clone()),
                }),
                docker: None, // Can be added later if needed
            }),
        };

        AlephSkillSpec {
            name: skill_name,
            description: suggestion.suggested_description.clone(),
            metadata,
            markdown_content: String::new(), // Will be set separately
        }
    }

    /// Generate markdown content from suggestion
    fn generate_markdown_content(&self, suggestion: &SolidificationSuggestion) -> String {
        let mut content = String::new();

        // Title
        content.push_str(&format!("# {}\n\n", suggestion.suggested_name));

        // Description
        content.push_str("## Description\n\n");
        content.push_str(&suggestion.suggested_description);
        content.push_str("\n\n");

        // Instructions
        content.push_str("## Instructions\n\n");
        content.push_str(&suggestion.instructions_preview);
        content.push_str("\n\n");

        // Examples from sample contexts
        if !suggestion.sample_contexts.is_empty() {
            content.push_str("## Examples\n\n");
            for (i, context) in suggestion.sample_contexts.iter().enumerate() {
                content.push_str(&format!("### Example {}\n\n", i + 1));
                content.push_str("```bash\n");
                content.push_str(context);
                content.push_str("\n```\n\n");
            }
        }

        // Metrics (for reference)
        content.push_str("## Metrics\n\n");
        content.push_str(&format!(
            "- Success rate: {:.1}%\n",
            suggestion.metrics.success_rate() * 100.0
        ));
        content.push_str(&format!(
            "- Total executions: {}\n",
            suggestion.metrics.total_executions
        ));
        content.push_str(&format!(
            "- Confidence: {:.1}%\n",
            suggestion.confidence * 100.0
        ));

        content
    }

    /// Generate complete SKILL.md file
    fn generate_skill_md(&self, spec: &AlephSkillSpec, content: &str) -> Result<String> {
        let mut result = String::new();

        // Frontmatter
        result.push_str("---\n");
        result.push_str(&format!("name: {}\n", spec.name));
        result.push_str(&format!("description: \"{}\"\n", escape_yaml_string(&spec.description)));

        // Metadata
        result.push_str("metadata:\n");

        // Requires section
        if !spec.metadata.requires.bins.is_empty() {
            result.push_str("  requires:\n");
            result.push_str("    bins:\n");
            for bin in &spec.metadata.requires.bins {
                result.push_str(&format!("      - \"{}\"\n", escape_yaml_string(bin)));
            }
        }

        // Aleph extensions
        if let Some(aleph_meta) = &spec.metadata.aleph {
            result.push_str("  aleph:\n");

            // Security
            result.push_str("    security:\n");
            result.push_str(&format!("      sandbox: {}\n",
                match aleph_meta.security.sandbox {
                    SandboxMode::Host => "host",
                    SandboxMode::Docker => "docker",
                    SandboxMode::VirtualFs => "virtualfs",
                }
            ));
            result.push_str(&format!("      confirmation: {}\n",
                match aleph_meta.security.confirmation {
                    ConfirmationMode::Always => "always",
                    ConfirmationMode::Write => "write",
                    ConfirmationMode::Never => "never",
                }
            ));
            result.push_str(&format!("      network: {}\n",
                match aleph_meta.security.network {
                    NetworkMode::Internet => "internet",
                    NetworkMode::Local => "local",
                    NetworkMode::None => "none",
                }
            ));

            // Input hints
            if !aleph_meta.input_hints.is_empty() {
                result.push_str("    input_hints:\n");
                for (key, hint) in &aleph_meta.input_hints {
                    result.push_str(&format!("      {}:\n", key));
                    if let Some(hint_type) = &hint.hint_type {
                        result.push_str(&format!("        type: {}\n", hint_type));
                    }
                    if let Some(desc) = &hint.description {
                        result.push_str(&format!("        description: \"{}\"\n", escape_yaml_string(desc)));
                    }
                    if hint.optional {
                        result.push_str("        optional: true\n");
                    }
                }
            }

            // Evolution metadata
            if let Some(evolution) = &aleph_meta.evolution {
                result.push_str("    evolution:\n");
                result.push_str(&format!("      source: \"{}\"\n", escape_yaml_string(&evolution.source)));
                result.push_str(&format!("      confidence_score: {}\n", evolution.confidence_score));
                if let Some(trace_id) = &evolution.created_from_trace {
                    result.push_str(&format!("      created_from_trace: \"{}\"\n", escape_yaml_string(trace_id)));
                }
            }
        }

        result.push_str("---\n\n");

        // Markdown content
        result.push_str(content);

        Ok(result)
    }

    /// Extract binary requirements from instructions
    fn extract_binary_requirements(&self, instructions: &str) -> Vec<String> {
        let mut bins = Vec::new();

        // Simple heuristic: look for common command patterns
        let common_commands = ["git", "docker", "kubectl", "gh", "aws", "npm", "cargo"];

        for cmd in common_commands {
            if instructions.contains(cmd) {
                bins.push(cmd.to_string());
            }
        }

        debug!(bins = ?bins, "Extracted binary requirements");
        bins
    }

    /// Extract input hints from instructions
    fn extract_input_hints(&self, instructions: &str) -> std::collections::HashMap<String, InputHint> {
        use std::collections::HashMap;

        let mut hints = HashMap::new();

        // Simple heuristic: look for common parameter patterns
        // This is a basic implementation - could be enhanced with LLM extraction

        // Look for --flag patterns
        for line in instructions.lines() {
            if let Some(flag_start) = line.find("--") {
                let rest = &line[flag_start + 2..];
                if let Some(end) = rest.find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_') {
                    let flag_name = &rest[..end];
                    if !flag_name.is_empty() {
                        hints.insert(
                            flag_name.replace('-', "_"),
                            InputHint {
                                hint_type: Some("string".to_string()),
                                pattern: None,
                                values: None,
                                description: None,
                                optional: true, // Default to optional for auto-extracted
                            },
                        );
                    }
                }
            }
        }

        debug!(count = hints.len(), "Extracted input hints");
        hints
    }
}

impl Default for MarkdownSkillGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Escape a string for safe YAML embedding in double-quoted context.
///
/// Replaces backslashes and double-quotes so the value cannot break
/// out of a YAML `"..."` literal.
fn escape_yaml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Convert a suggestion name to a valid skill name (kebab-case)
fn to_skill_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_skill_name() {
        assert_eq!(to_skill_name("Quick Fix Git"), "quick-fix-git");
        assert_eq!(to_skill_name("Docker Build & Push"), "docker-build-push");
        assert_eq!(to_skill_name("search_files"), "search-files");
    }

    #[test]
    fn test_extract_binary_requirements() {
        let generator = MarkdownSkillGenerator::new();

        let instructions = "Use git to commit changes and docker to build the image";
        let bins = generator.extract_binary_requirements(instructions);

        assert!(bins.contains(&"git".to_string()));
        assert!(bins.contains(&"docker".to_string()));
    }
}
