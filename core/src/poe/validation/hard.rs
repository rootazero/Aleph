//! Hard validation for POE architecture.
//!
//! This module implements deterministic validation checks that can be executed
//! by Rust code without requiring an LLM. These are binary pass/fail checks
//! that verify concrete conditions like file existence, content matching,
//! command execution, and JSON schema validation.

use crate::poe::types::{RuleResult, ValidationRule};
use regex::Regex;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Hard validator for deterministic validation checks.
///
/// This validator executes rules that can be evaluated without LLM involvement:
/// - File system checks (existence, content patterns)
/// - Command execution checks (exit codes, output patterns)
/// - Data validation (JSON schema)
///
/// Semantic checks are skipped (returned as passed) since they require
/// the SemanticValidator.
#[derive(Debug, Clone, Default)]
pub struct HardValidator;

impl HardValidator {
    /// Create a new HardValidator instance.
    pub fn new() -> Self {
        Self
    }

    /// Validate all rules and return results for each.
    ///
    /// Rules are validated concurrently when possible.
    pub async fn validate_all(&self, rules: &[ValidationRule]) -> Result<Vec<RuleResult>, String> {
        let mut results = Vec::with_capacity(rules.len());

        for rule in rules {
            results.push(self.validate_single(rule).await);
        }

        Ok(results)
    }

    /// Validate a single rule and return the result.
    pub async fn validate_single(&self, rule: &ValidationRule) -> RuleResult {
        match rule {
            ValidationRule::FileExists { path } => self.validate_file_exists(rule, path),

            ValidationRule::FileNotExists { path } => self.validate_file_not_exists(rule, path),

            ValidationRule::FileContains { path, pattern } => {
                self.validate_file_contains(rule, path, pattern).await
            }

            ValidationRule::FileNotContains { path, pattern } => {
                self.validate_file_not_contains(rule, path, pattern).await
            }

            ValidationRule::DirStructureMatch { root, expected } => {
                self.validate_dir_structure(rule, root, expected)
            }

            ValidationRule::CommandPasses {
                cmd,
                args,
                timeout_ms,
            } => self.validate_command_passes(rule, cmd, args, *timeout_ms).await,

            ValidationRule::CommandOutputContains {
                cmd,
                args,
                pattern,
                timeout_ms,
            } => {
                self.validate_command_output_contains(rule, cmd, args, pattern, *timeout_ms)
                    .await
            }

            ValidationRule::JsonSchemaValid { path, schema } => {
                self.validate_json_schema(rule, path, schema).await
            }

            // SemanticCheck is handled by SemanticValidator, skip here
            ValidationRule::SemanticCheck { .. } => RuleResult::pass(rule.clone()),
        }
    }

    // ========== File System Validators ==========

    /// Check if a file exists at the given path.
    fn validate_file_exists(&self, rule: &ValidationRule, path: &Path) -> RuleResult {
        if path.exists() {
            RuleResult::pass(rule.clone())
        } else {
            RuleResult::fail(rule.clone(), format!("File does not exist: {}", path.display()))
        }
    }

    /// Check if a file does NOT exist at the given path.
    fn validate_file_not_exists(&self, rule: &ValidationRule, path: &Path) -> RuleResult {
        if !path.exists() {
            RuleResult::pass(rule.clone())
        } else {
            RuleResult::fail(rule.clone(), format!("File should not exist: {}", path.display()))
        }
    }

    /// Check if a file contains a specific pattern (regex).
    async fn validate_file_contains(
        &self,
        rule: &ValidationRule,
        path: &Path,
        pattern: &str,
    ) -> RuleResult {
        // First check if file exists
        if !path.exists() {
            return RuleResult::fail(
                rule.clone(),
                format!("File does not exist: {}", path.display()),
            );
        }

        // Read file contents
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Failed to read file {}: {}", path.display(), e),
                );
            }
        };

        // Compile and match regex
        let regex = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Invalid regex pattern '{}': {}", pattern, e),
                );
            }
        };

        if regex.is_match(&content) {
            RuleResult::pass(rule.clone())
        } else {
            RuleResult::fail(
                rule.clone(),
                format!(
                    "File {} does not contain pattern '{}'",
                    path.display(),
                    pattern
                ),
            )
        }
    }

    /// Check if a file does NOT contain a specific pattern (regex).
    async fn validate_file_not_contains(
        &self,
        rule: &ValidationRule,
        path: &Path,
        pattern: &str,
    ) -> RuleResult {
        // First check if file exists
        if !path.exists() {
            // If file doesn't exist, it can't contain the pattern - pass
            return RuleResult::pass(rule.clone());
        }

        // Read file contents
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Failed to read file {}: {}", path.display(), e),
                );
            }
        };

        // Compile and match regex
        let regex = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Invalid regex pattern '{}': {}", pattern, e),
                );
            }
        };

        if !regex.is_match(&content) {
            RuleResult::pass(rule.clone())
        } else {
            RuleResult::fail(
                rule.clone(),
                format!(
                    "File {} should not contain pattern '{}' but it does",
                    path.display(),
                    pattern
                ),
            )
        }
    }

    /// Validate directory structure matches expected layout.
    ///
    /// The `expected` string is a comma-separated list of paths:
    /// - "src/" means directory "src" should exist
    /// - "Cargo.toml" means file "Cargo.toml" should exist
    /// - "src/lib.rs" means file "src/lib.rs" should exist
    fn validate_dir_structure(
        &self,
        rule: &ValidationRule,
        root: &Path,
        expected: &str,
    ) -> RuleResult {
        // Check if root exists
        if !root.exists() {
            return RuleResult::fail(
                rule.clone(),
                format!("Root directory does not exist: {}", root.display()),
            );
        }

        // Parse expected structure
        let entries: Vec<&str> = expected
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        let mut missing = Vec::new();

        for entry in entries {
            let full_path = root.join(entry);

            // Check if it's a directory (ends with /) or file
            if entry.ends_with('/') {
                let dir_path = root.join(entry.trim_end_matches('/'));
                if !dir_path.is_dir() {
                    missing.push(entry.to_string());
                }
            } else if !full_path.exists() {
                missing.push(entry.to_string());
            }
        }

        if missing.is_empty() {
            RuleResult::pass(rule.clone())
        } else {
            RuleResult::fail(
                rule.clone(),
                format!(
                    "Missing entries in {}: {}",
                    root.display(),
                    missing.join(", ")
                ),
            )
        }
    }

    // ========== Command Execution Validators ==========

    /// Check if a command exits with code 0.
    async fn validate_command_passes(
        &self,
        rule: &ValidationRule,
        cmd: &str,
        args: &[String],
        timeout_ms: u64,
    ) -> RuleResult {
        match self.run_command(cmd, args, timeout_ms).await {
            Ok((exit_code, _, stderr)) => {
                if exit_code == 0 {
                    RuleResult::pass(rule.clone())
                } else {
                    RuleResult::fail(
                        rule.clone(),
                        format!(
                            "Command '{}' failed with exit code {}: {}",
                            cmd,
                            exit_code,
                            stderr.trim()
                        ),
                    )
                }
            }
            Err(e) => RuleResult::fail(rule.clone(), e),
        }
    }

    /// Check if a command's output contains a specific pattern.
    async fn validate_command_output_contains(
        &self,
        rule: &ValidationRule,
        cmd: &str,
        args: &[String],
        pattern: &str,
        timeout_ms: u64,
    ) -> RuleResult {
        // Compile regex first
        let regex = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Invalid regex pattern '{}': {}", pattern, e),
                );
            }
        };

        match self.run_command(cmd, args, timeout_ms).await {
            Ok((_, stdout, stderr)) => {
                // Check both stdout and stderr for the pattern
                let combined_output = format!("{}{}", stdout, stderr);

                if regex.is_match(&combined_output) {
                    RuleResult::pass(rule.clone())
                } else {
                    RuleResult::fail(
                        rule.clone(),
                        format!(
                            "Command '{}' output does not contain pattern '{}'",
                            cmd, pattern
                        ),
                    )
                }
            }
            Err(e) => RuleResult::fail(rule.clone(), e),
        }
    }

    /// Run a command with timeout and return (exit_code, stdout, stderr).
    async fn run_command(
        &self,
        cmd: &str,
        args: &[String],
        timeout_ms: u64,
    ) -> Result<(i32, String, String), String> {
        let mut command = Command::new(cmd);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Err(format!("Failed to spawn command '{}': {}", cmd, e));
            }
        };

        let duration = Duration::from_millis(timeout_ms);

        match timeout(duration, async {
            let output = child.wait_with_output().await;
            output
        })
        .await
        {
            Ok(Ok(output)) => {
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                Ok((exit_code, stdout, stderr))
            }
            Ok(Err(e)) => Err(format!("Command '{}' failed: {}", cmd, e)),
            Err(_) => Err(format!(
                "Command '{}' timed out after {}ms",
                cmd, timeout_ms
            )),
        }
    }

    // ========== Data Validation ==========

    /// Validate a JSON file against a basic type schema.
    ///
    /// The schema is a simplified JSON format specifying expected types:
    /// ```json
    /// {
    ///   "name": "string",
    ///   "age": "number",
    ///   "active": "boolean",
    ///   "tags": "array",
    ///   "metadata": "object"
    /// }
    /// ```
    async fn validate_json_schema(
        &self,
        rule: &ValidationRule,
        path: &Path,
        schema: &str,
    ) -> RuleResult {
        // Check if file exists
        if !path.exists() {
            return RuleResult::fail(
                rule.clone(),
                format!("JSON file does not exist: {}", path.display()),
            );
        }

        // Read and parse the JSON file
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Failed to read JSON file {}: {}", path.display(), e),
                );
            }
        };

        let json_value: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Failed to parse JSON file {}: {}", path.display(), e),
                );
            }
        };

        // Parse the schema
        let schema_value: serde_json::Value = match serde_json::from_str(schema) {
            Ok(v) => v,
            Err(e) => {
                return RuleResult::fail(
                    rule.clone(),
                    format!("Failed to parse schema: {}", e),
                );
            }
        };

        // Validate types
        match self.validate_json_types(&json_value, &schema_value) {
            Ok(()) => RuleResult::pass(rule.clone()),
            Err(errors) => RuleResult::fail(
                rule.clone(),
                format!("JSON schema validation failed: {}", errors.join("; ")),
            ),
        }
    }

    /// Recursively validate JSON types against a schema.
    fn validate_json_types(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        match schema {
            serde_json::Value::Object(schema_obj) => {
                // If value is not an object, that's an error
                let value_obj = match value.as_object() {
                    Some(o) => o,
                    None => {
                        errors.push("Expected object but found different type".to_string());
                        return Err(errors);
                    }
                };

                for (key, expected_type) in schema_obj {
                    if let Some(actual_value) = value_obj.get(key) {
                        // Check if expected_type is a string (type name) or nested object
                        if let Some(type_str) = expected_type.as_str() {
                            let type_match = match type_str {
                                "string" => actual_value.is_string(),
                                "number" => actual_value.is_number(),
                                "boolean" | "bool" => actual_value.is_boolean(),
                                "array" => actual_value.is_array(),
                                "object" => actual_value.is_object(),
                                "null" => actual_value.is_null(),
                                "any" => true,
                                _ => {
                                    errors.push(format!("Unknown type '{}' for key '{}'", type_str, key));
                                    continue;
                                }
                            };

                            if !type_match {
                                errors.push(format!(
                                    "Key '{}' expected {} but found {}",
                                    key,
                                    type_str,
                                    self.json_type_name(actual_value)
                                ));
                            }
                        } else if expected_type.is_object() {
                            // Nested object schema
                            if let Err(nested_errors) =
                                self.validate_json_types(actual_value, expected_type)
                            {
                                for err in nested_errors {
                                    errors.push(format!("In '{}': {}", key, err));
                                }
                            }
                        }
                    } else {
                        // Key is missing from the JSON
                        errors.push(format!("Missing required key '{}'", key));
                    }
                }
            }
            serde_json::Value::String(type_str) => {
                // Top-level type check
                let type_match = match type_str.as_str() {
                    "string" => value.is_string(),
                    "number" => value.is_number(),
                    "boolean" | "bool" => value.is_boolean(),
                    "array" => value.is_array(),
                    "object" => value.is_object(),
                    "null" => value.is_null(),
                    "any" => true,
                    _ => {
                        errors.push(format!("Unknown type '{}'", type_str));
                        return Err(errors);
                    }
                };

                if !type_match {
                    errors.push(format!(
                        "Expected {} but found {}",
                        type_str,
                        self.json_type_name(value)
                    ));
                }
            }
            _ => {
                errors.push("Schema must be an object or type string".to_string());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Get the type name of a JSON value.
    fn json_type_name(&self, value: &serde_json::Value) -> &'static str {
        match value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn test_file_exists() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // File doesn't exist yet
        let validator = HardValidator::new();
        let rule = ValidationRule::FileExists {
            path: file_path.clone(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
        assert!(result.error.is_some());

        // Create the file
        fs::write(&file_path, "hello").await.unwrap();

        // Now it should pass
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_file_not_exists() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        let validator = HardValidator::new();
        let rule = ValidationRule::FileNotExists {
            path: file_path.clone(),
        };

        // File doesn't exist - should pass
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);

        // Create the file
        fs::write(&file_path, "hello").await.unwrap();

        // Now it should fail
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_file_contains() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "Hello World\nThis is a test file").await.unwrap();

        let validator = HardValidator::new();

        // Test matching pattern
        let rule = ValidationRule::FileContains {
            path: file_path.clone(),
            pattern: r"Hello\s+World".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);

        // Test non-matching pattern
        let rule = ValidationRule::FileContains {
            path: file_path.clone(),
            pattern: r"Goodbye".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_file_not_contains() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "Hello World").await.unwrap();

        let validator = HardValidator::new();

        // Pattern not present - should pass
        let rule = ValidationRule::FileNotContains {
            path: file_path.clone(),
            pattern: r"Goodbye".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);

        // Pattern present - should fail
        let rule = ValidationRule::FileNotContains {
            path: file_path.clone(),
            pattern: r"Hello".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_dir_structure_match() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create structure
        fs::create_dir_all(root.join("src")).await.unwrap();
        fs::create_dir_all(root.join("tests")).await.unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").await.unwrap();
        fs::write(root.join("src/lib.rs"), "// lib").await.unwrap();

        let validator = HardValidator::new();

        // Test matching structure
        let rule = ValidationRule::DirStructureMatch {
            root: root.to_path_buf(),
            expected: "src/, tests/, Cargo.toml, src/lib.rs".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed, "Expected pass, got: {:?}", result.error);

        // Test missing structure
        let rule = ValidationRule::DirStructureMatch {
            root: root.to_path_buf(),
            expected: "src/, docs/, README.md".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
        assert!(result.error.as_ref().unwrap().contains("docs/"));
    }

    #[tokio::test]
    async fn test_command_passes() {
        let validator = HardValidator::new();

        // Test successful command
        let rule = ValidationRule::CommandPasses {
            cmd: "echo".to_string(),
            args: vec!["hello".to_string()],
            timeout_ms: 5000,
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed, "Expected pass, got: {:?}", result.error);

        // Test failing command
        let rule = ValidationRule::CommandPasses {
            cmd: "false".to_string(),
            args: vec![],
            timeout_ms: 5000,
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_command_output_contains() {
        let validator = HardValidator::new();

        // Test matching output
        let rule = ValidationRule::CommandOutputContains {
            cmd: "echo".to_string(),
            args: vec!["Hello World".to_string()],
            pattern: r"Hello\s+World".to_string(),
            timeout_ms: 5000,
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed, "Expected pass, got: {:?}", result.error);

        // Test non-matching output
        let rule = ValidationRule::CommandOutputContains {
            cmd: "echo".to_string(),
            args: vec!["Hello".to_string()],
            pattern: r"Goodbye".to_string(),
            timeout_ms: 5000,
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_json_schema_valid() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.json");

        // Create a valid JSON file
        let json_content = r#"{
            "name": "test",
            "age": 25,
            "active": true,
            "tags": ["a", "b"],
            "metadata": {"key": "value"}
        }"#;
        fs::write(&file_path, json_content).await.unwrap();

        let validator = HardValidator::new();

        // Test matching schema
        let schema = r#"{
            "name": "string",
            "age": "number",
            "active": "boolean",
            "tags": "array",
            "metadata": "object"
        }"#;
        let rule = ValidationRule::JsonSchemaValid {
            path: file_path.clone(),
            schema: schema.to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed, "Expected pass, got: {:?}", result.error);

        // Test failing schema (wrong type)
        let schema = r#"{
            "name": "number"
        }"#;
        let rule = ValidationRule::JsonSchemaValid {
            path: file_path.clone(),
            schema: schema.to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
        assert!(result.error.as_ref().unwrap().contains("expected number"));
    }

    #[tokio::test]
    async fn test_semantic_check_skipped() {
        use crate::poe::types::{JudgeTarget, ModelTier};

        let validator = HardValidator::new();

        let rule = ValidationRule::SemanticCheck {
            target: JudgeTarget::Content("test content".to_string()),
            prompt: "Is this good?".to_string(),
            passing_criteria: "Must be good".to_string(),
            model_tier: ModelTier::default(),
        };

        // Semantic checks should always pass in HardValidator
        let result = validator.validate_single(&rule).await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_validate_all() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "Hello World").await.unwrap();

        let validator = HardValidator::new();

        let rules = vec![
            ValidationRule::FileExists {
                path: file_path.clone(),
            },
            ValidationRule::FileContains {
                path: file_path.clone(),
                pattern: "Hello".to_string(),
            },
            ValidationRule::FileNotExists {
                path: dir.path().join("nonexistent.txt"),
            },
        ];

        let results = validator.validate_all(&rules).await.unwrap();
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.passed));
    }

    #[tokio::test]
    async fn test_invalid_regex_pattern() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "Hello").await.unwrap();

        let validator = HardValidator::new();

        // Invalid regex pattern
        let rule = ValidationRule::FileContains {
            path: file_path,
            pattern: "[invalid regex".to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
        assert!(result.error.as_ref().unwrap().contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_command_timeout() {
        let validator = HardValidator::new();

        // Command that would take longer than timeout
        let rule = ValidationRule::CommandPasses {
            cmd: "sleep".to_string(),
            args: vec!["10".to_string()],
            timeout_ms: 100, // 100ms timeout
        };
        let result = validator.validate_single(&rule).await;
        assert!(!result.passed);
        assert!(result.error.as_ref().unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn test_nested_json_schema() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("nested.json");

        let json_content = r#"{
            "user": {
                "name": "Alice",
                "age": 30
            },
            "active": true
        }"#;
        fs::write(&file_path, json_content).await.unwrap();

        let validator = HardValidator::new();

        // Test nested schema validation
        let schema = r#"{
            "user": {
                "name": "string",
                "age": "number"
            },
            "active": "boolean"
        }"#;
        let rule = ValidationRule::JsonSchemaValid {
            path: file_path.clone(),
            schema: schema.to_string(),
        };
        let result = validator.validate_single(&rule).await;
        assert!(result.passed, "Expected pass, got: {:?}", result.error);
    }
}
