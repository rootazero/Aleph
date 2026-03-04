//! Manifest Builder for POE architecture.
//!
//! This module implements the "First Principles" engine that converts raw user
//! instructions into structured `SuccessManifest` contracts.
//!
//! It uses LLM reasoning (System 2) to:
//! 1. Analyze the user's intent
//! 2. Retrieve similar successful experiences (if available)
//! 3. Define hard constraints (what MUST be true)
//! 4. Define soft metrics (what represents quality)
//!
//! ## Example
//!
//! ```rust,ignore
//! use alephcore::poe::manifest::ManifestBuilder;
//! use alephcore::providers::AiProvider;
//!
//! let builder = ManifestBuilder::new(provider)
//!     .with_experience_tracker(tracker);
//!
//! let manifest = builder.build(
//!     "Create a new Rust CLI tool called 'echo-cli'",
//!     Some("Current dir is /workspace")
//! ).await?;
//! ```

use crate::sync_primitives::Arc;

use crate::agents::thinking::ThinkLevel;
use crate::error::Result;
use crate::poe::types::SuccessManifest;
use crate::providers::AiProvider;
use crate::skill_evolution::EvolutionTracker;
use crate::utils::json_extract::extract_json_robust;
use tracing::warn;

/// System prompt for the Manifest Builder.
const MANIFEST_BUILDER_SYSTEM_PROMPT: &str = r#"You are the "First Principles" engine of an advanced AI agent.
Your goal is to translate a vague user instruction into a rigorous Success Manifest (contract).

## The Philosophy
Do not just execute. First, define what "done" looks like.
- **Hard Constraints**: Binary conditions that MUST be met. If these fail, the task failed.
- **Soft Metrics**: Quality indicators. These are weighted and contribute to a score.

## Output Format
You must output ONLY valid JSON matching this structure:

```json
{
  "task_id": "unique-id-derived-from-instruction",
  "objective": "Clear, concise statement of the goal",
  "hard_constraints": [
    {
      "type": "FileExists",
      "params": { "path": "path/to/file" }
    },
    {
      "type": "CommandPasses",
      "params": { "cmd": "cargo", "args": ["test"], "timeout_ms": 60000 }
    }
  ],
  "soft_metrics": [
    {
      "rule": {
        "type": "SemanticCheck",
        "params": {
          "target": { "type": "File", "value": "src/main.rs" },
          "prompt": "Is the code idiomatic and well-commented?",
          "passing_criteria": "Uses idiomatic Rust patterns, clear comments",
          "model_tier": "CloudSmart"
        }
      },
      "weight": 0.8,
      "threshold": 0.7
    }
  ],
  "max_attempts": 5
}
```

## Available Constraint Types

### File System
- `FileExists { path }`
- `FileNotExists { path }`
- `FileContains { path, pattern }` (pattern is regex)
- `FileNotContains { path, pattern }`
- `DirStructureMatch { root, expected }` (expected is "src/, Cargo.toml, ...")

### Execution
- `CommandPasses { cmd, args, timeout_ms }`
- `CommandOutputContains { cmd, args, pattern, timeout_ms }`

### Data
- `JsonSchemaValid { path, schema }`

### Semantic (LLM Judge)
- `SemanticCheck { target, prompt, passing_criteria, model_tier }`
  - target: `File(path)` or `Content(string)` or `CommandOutput { cmd, args }`
  - model_tier: `LocalFast`, `CloudFast`, `CloudSmart`, `CloudDeep`

## Guidelines
1. **Be Specific**: Prefer `CommandPasses` (e.g., `cargo test`) over vague semantic checks.
2. **Be Realistic**: Don't set `max_attempts` too low (default 3-5).
3. **Use Semantic Checks for Quality**: Use LLM judges for code style, documentation quality, or complex logic that regex can't catch.
4. **Context Matters**: Use the provided context to resolve relative paths.

Output ONLY JSON. No markdown."#;

/// System prompt for amending an existing manifest.
const MANIFEST_AMEND_SYSTEM_PROMPT: &str = r#"You are modifying an existing Success Manifest based on user feedback.

## Your Task
1. Understand what the user wants to add, remove, or modify
2. Output a COMPLETE updated manifest (not just the changes)
3. Preserve all existing constraints unless explicitly asked to remove them
4. Keep the same task_id and objective unless the user explicitly wants to change them

## Output Format
Output ONLY valid JSON matching the SuccessManifest structure. No markdown.
The structure is:
{
  "task_id": "string",
  "objective": "string",
  "hard_constraints": [...],
  "soft_metrics": [...],
  "max_attempts": number
}

## Important
- If the user asks to "add" something, ADD it to existing constraints
- If the user asks to "remove" something, REMOVE it from existing constraints
- If the user asks to "change" something, MODIFY the specific constraint
- Always output the COMPLETE manifest, not just changes"#;

/// Builder for generating SuccessManifests from instructions.
pub struct ManifestBuilder {
    /// AI provider for generating the manifest
    provider: Arc<dyn AiProvider>,
    /// Optional tracker for retrieving similar experiences
    tracker: Option<Arc<EvolutionTracker>>,
}

impl ManifestBuilder {
    /// Create a new ManifestBuilder.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider,
            tracker: None,
        }
    }

    /// Attach an evolution tracker to enable experience-based learning.
    pub fn with_experience_tracker(mut self, tracker: Arc<EvolutionTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }

    /// Build a SuccessManifest from a user instruction.
    ///
    /// This method:
    /// 1. Searches for similar past experiences (if tracker available)
    /// 2. Constructs a prompt with instruction, context, and experiences
    /// 3. Calls the LLM to generate the manifest JSON
    /// 4. Parses and validates the result
    pub async fn build(&self, instruction: &str, context: Option<&str>) -> Result<SuccessManifest> {
        // 1. Retrieve similar experiences
        let experiences_context = self.retrieve_relevant_experiences(instruction).await;

        // 2. Build the prompt
        let prompt = self.build_prompt(instruction, context, &experiences_context);

        // 3. Call LLM
        // We use CloudSmart (e.g., GPT-4o) for this planning task as it requires strong reasoning
        let response = self
            .provider
            .process_with_thinking(
                &prompt,
                Some(MANIFEST_BUILDER_SYSTEM_PROMPT),
                ThinkLevel::Low,
            )
            .await?;

        // 4. Parse response
        self.parse_manifest(&response)
    }

    /// Retrieve relevant experiences from the evolution tracker.
    async fn retrieve_relevant_experiences(&self, instruction: &str) -> String {
        if let Some(tracker) = &self.tracker {
            // Generate a pattern ID from the instruction (simple heuristic for now)
            // In a real implementation, we'd use vector search
            let pattern_id = self.generate_pattern_id(instruction);
            
            // Try to get metrics for this pattern
            if let Ok(Some(metrics)) = tracker.get_metrics(&pattern_id) {
                if metrics.successful_executions > 0 {
                    return format!(
                        "## Similar Experiences\n\nI have successfully completed similar tasks (pattern: {}) {} times.\nSuccess rate: {:.1}%.",
                        pattern_id,
                        metrics.successful_executions,
                        metrics.success_rate() * 100.0
                    );
                }
            }
        }
        
        String::new()
    }

    /// Generate a simple pattern ID from instruction (consistent with Crystallizer).
    fn generate_pattern_id(&self, instruction: &str) -> String {
        // This duplicates logic from Crystallizer to avoid circular deps or public exposure
        // Ideally this should be in a shared utility
        let lowercase = instruction.to_lowercase();
        let keywords: Vec<String> = lowercase
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 3)
            .take(3)
            .map(String::from)
            .collect();
            
        if keywords.is_empty() {
            "poe-generic-task".to_string()
        } else {
            format!("poe-{}", keywords.join("-"))
        }
    }

    /// Build the full prompt for the LLM.
    fn build_prompt(
        &self, 
        instruction: &str, 
        context: Option<&str>, 
        experiences: &str
    ) -> String {
        let context_section = if let Some(ctx) = context {
            format!("## Context\n{}\n", ctx)
        } else {
            String::new()
        };

        format!(
            "## User Instruction\n\n{}

{}
{}
Generate a Success Manifest for this task.",
            instruction,
            context_section,
            experiences
        )
    }

    /// Parse the JSON response from the LLM.
    fn parse_manifest(&self, response: &str) -> Result<SuccessManifest> {
        let json_value = match extract_json_robust(response) {
            Some(v) => v,
            None => {
                warn!("No JSON found in manifest response, constructing minimal manifest from instruction");
                // Return a minimal manifest — objective is the first line of the response
                let objective = response.lines().next().unwrap_or("Unknown task").trim();
                return Ok(SuccessManifest::new("fallback-task", objective));
            }
        };

        serde_json::from_value(json_value).map_err(|e| {
            warn!("Failed to parse manifest JSON: {}, constructing minimal manifest", e);
            crate::error::AlephError::other(format!(
                "Failed to parse generated manifest: {}. Raw: {}",
                e,
                truncate_string(response, 200)
            ))
        })
    }

    // ========================================================================
    // Amendment Methods (Contract Signing Workflow)
    // ========================================================================

    /// Amend an existing manifest based on natural language feedback.
    ///
    /// Uses LLM to interpret the user's amendment request and merge it
    /// with the existing manifest.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let amended = builder.amend(
    ///     &existing_manifest,
    ///     "also check that cargo clippy passes"
    /// ).await?;
    /// ```
    pub async fn amend(
        &self,
        current: &SuccessManifest,
        amendment: &str,
    ) -> Result<SuccessManifest> {
        // 1. Serialize current manifest
        let current_json = serde_json::to_string_pretty(current)
            .map_err(|e| crate::error::AlephError::other(
                format!("Failed to serialize manifest: {}", e)
            ))?;

        // 2. Build the prompt
        let user_prompt = format!(
            "## Current Manifest\n\n```json\n{}\n```\n\n## Amendment Request\n\n{}\n\nPlease output the updated manifest.",
            current_json,
            amendment
        );

        // 3. Call LLM
        let response = self
            .provider
            .process_with_thinking(
                &user_prompt,
                Some(MANIFEST_AMEND_SYSTEM_PROMPT),
                ThinkLevel::Low,
            )
            .await?;

        // 4. Parse response
        self.parse_manifest(&response)
    }

    /// Merge a manifest override with the current manifest.
    ///
    /// This is a pure Rust operation (no LLM) for advanced users who
    /// provide JSON overrides directly.
    ///
    /// # Merge Strategy
    ///
    /// - `task_id`: Override wins if non-empty
    /// - `objective`: Override wins if non-empty
    /// - `hard_constraints`: **Appended** (not replaced)
    /// - `soft_metrics`: **Appended** (not replaced)
    /// - `max_attempts`: Override wins if not default (5)
    /// - `rollback_snapshot`: Override wins if Some
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let merged = ManifestBuilder::merge_override(&current, &override_manifest);
    /// ```
    pub fn merge_override(
        current: &SuccessManifest,
        override_manifest: &SuccessManifest,
    ) -> SuccessManifest {
        SuccessManifest {
            task_id: if override_manifest.task_id.is_empty() {
                current.task_id.clone()
            } else {
                override_manifest.task_id.clone()
            },
            objective: if override_manifest.objective.is_empty() {
                current.objective.clone()
            } else {
                override_manifest.objective.clone()
            },
            // Constraints are APPENDED (not replaced) to prevent accidental removal
            hard_constraints: [
                current.hard_constraints.clone(),
                override_manifest.hard_constraints.clone(),
            ].concat(),
            soft_metrics: [
                current.soft_metrics.clone(),
                override_manifest.soft_metrics.clone(),
            ].concat(),
            // Use override's max_attempts only if it's not the default
            max_attempts: if override_manifest.max_attempts == 5 {
                current.max_attempts
            } else {
                override_manifest.max_attempts
            },
            // Override's rollback_snapshot takes precedence if set
            rollback_snapshot: override_manifest
                .rollback_snapshot
                .clone()
                .or_else(|| current.rollback_snapshot.clone()),
        }
    }
}

/// Truncate a string to a maximum length.
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

// ============================================================================ 
// Tests
// ============================================================================ 

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MockProvider;

    fn create_mock_builder(response: &str) -> ManifestBuilder {
        let provider = Arc::new(MockProvider::new(response));
        ManifestBuilder::new(provider)
    }

    #[test]
    fn test_extract_json_robust_usage() {
        // Pure JSON
        let result = extract_json_robust(r#"{"a":1}"#);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["a"], 1);

        // Markdown
        let result = extract_json_robust("```json\n{\"a\":1}\n```");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["a"], 1);

        // Surrounded text
        let result = extract_json_robust("Here is JSON:\n{\"a\":1}\nThanks");
        assert!(result.is_some());
        assert_eq!(result.unwrap()["a"], 1);
    }

    #[tokio::test]
    async fn test_build_manifest_plain_text_fallback() {
        // Mock LLM returns plain text instead of JSON
        let builder = create_mock_builder("这是一个纯文本回复，没有JSON内容");
        let manifest = builder.build("Create a new file", None).await.unwrap();

        // Should get a minimal fallback manifest
        assert_eq!(manifest.task_id, "fallback-task");
        assert!(manifest.hard_constraints.is_empty());
    }

    #[tokio::test]
    async fn test_build_manifest_success() {
        let mock_json = r#"{
            "task_id": "test-task",
            "objective": "Test objective",
            "hard_constraints": [
                {
                    "type": "FileExists",
                    "params": { "path": "test.txt" }
                }
            ],
            "soft_metrics": [],
            "max_attempts": 3
        }"#;

        let builder = create_mock_builder(mock_json);
        let manifest = builder.build("Create test.txt", None).await.unwrap();

        assert_eq!(manifest.task_id, "test-task");
        assert_eq!(manifest.objective, "Test objective");
        assert_eq!(manifest.hard_constraints.len(), 1);
    }
    
    #[tokio::test]
    async fn test_build_manifest_with_semantic_check() {
        let mock_json = r#"{
            "task_id": "code-task",
            "objective": "Write code",
            "hard_constraints": [],
            "soft_metrics": [
                {
                    "rule": {
                        "type": "SemanticCheck",
                        "params": {
                            "target": { "type": "Content", "value": "code" },
                            "prompt": "Is good?",
                            "passing_criteria": "Yes",
                            "model_tier": "CloudFast"
                        }
                    },
                    "weight": 1.0,
                    "threshold": 0.8
                }
            ]
        }"#;

        let builder = create_mock_builder(mock_json);
        let manifest = builder.build("Write good code", None).await.unwrap();

        assert_eq!(manifest.soft_metrics.len(), 1);
        let metric = &manifest.soft_metrics[0];
        assert_eq!(metric.weight, 1.0);
        assert_eq!(metric.threshold, 0.8);
    }

    #[tokio::test]
    async fn test_amend_manifest() {
        // Mock LLM returns an amended manifest
        let mock_json = r#"{
            "task_id": "test-task",
            "objective": "Test objective",
            "hard_constraints": [
                { "type": "FileExists", "params": { "path": "test.txt" } },
                { "type": "CommandPasses", "params": { "cmd": "cargo", "args": ["clippy"], "timeout_ms": 60000 } }
            ],
            "soft_metrics": [],
            "max_attempts": 3
        }"#;

        let builder = create_mock_builder(mock_json);

        // Original manifest with one constraint
        let original = SuccessManifest::new("test-task", "Test objective")
            .with_hard_constraint(crate::poe::ValidationRule::FileExists {
                path: std::path::PathBuf::from("test.txt"),
            });

        let amended = builder.amend(&original, "also run cargo clippy").await.unwrap();

        // Should have both constraints in the mock response
        assert_eq!(amended.hard_constraints.len(), 2);
    }

    #[test]
    fn test_merge_override_appends_constraints() {
        use crate::poe::ValidationRule;
        use std::path::PathBuf;

        let current = SuccessManifest::new("task-1", "Original objective")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("file1.txt"),
            });

        let override_manifest = SuccessManifest::new("", "") // Empty task_id/objective = keep current
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("file2.txt"),
            });

        let merged = ManifestBuilder::merge_override(&current, &override_manifest);

        // Should preserve original task_id and objective
        assert_eq!(merged.task_id, "task-1");
        assert_eq!(merged.objective, "Original objective");

        // Should have BOTH constraints (appended)
        assert_eq!(merged.hard_constraints.len(), 2);
    }

    #[test]
    fn test_merge_override_preserves_max_attempts() {
        let current = SuccessManifest::new("task-1", "Test")
            .with_max_attempts(10);

        // Override with default max_attempts (5) should keep current
        let override_manifest = SuccessManifest::new("", "");

        let merged = ManifestBuilder::merge_override(&current, &override_manifest);
        assert_eq!(merged.max_attempts, 10);

        // Override with explicit max_attempts should win
        let override_with_attempts = SuccessManifest::new("", "")
            .with_max_attempts(3);

        let merged2 = ManifestBuilder::merge_override(&current, &override_with_attempts);
        assert_eq!(merged2.max_attempts, 3);
    }

    #[test]
    fn test_merge_override_with_rollback_snapshot() {
        use std::path::PathBuf;

        let current = SuccessManifest::new("task-1", "Test");

        let override_manifest = SuccessManifest::new("", "")
            .with_rollback_snapshot(PathBuf::from("/snapshot/path"));

        let merged = ManifestBuilder::merge_override(&current, &override_manifest);
        assert_eq!(merged.rollback_snapshot, Some(PathBuf::from("/snapshot/path")));
    }
}
