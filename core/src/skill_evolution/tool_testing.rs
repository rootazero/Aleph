//! Tool self-testing and registration.
//!
//! This module provides:
//! 1. Self-test framework for generated tools
//! 2. Registration of tested tools into the ToolServer
//! 3. Runtime execution via subprocess (Python, Node.js)
//!
//! ## Self-Test Process
//!
//! 1. Validate tool definition schema
//! 2. Check entrypoint exists and is executable
//! 3. Run with minimal input to verify it doesn't crash
//! 4. Parse output to verify JSON response
//! 5. Mark tool as tested on success

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::dispatcher::ToolDefinition;
use crate::error::{AlephError, Result};
use crate::tools::{AlephToolDyn, AlephToolServer};

use super::tool_generator::{GeneratedToolDefinition, ToolGenerator};

// =============================================================================
// Test Report
// =============================================================================

/// Result of a self-test run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfTestReport {
    /// Tool name
    pub tool_name: String,
    /// Whether all tests passed
    pub passed: bool,
    /// Individual test results
    pub tests: Vec<TestResult>,
    /// Overall execution time in milliseconds
    pub duration_ms: u64,
    /// Timestamp when test was run
    pub tested_at: i64,
}

/// Individual test result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// Name of the test
    pub name: String,
    /// Whether this test passed
    pub passed: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Any output from the test
    pub output: Option<String>,
}

impl SelfTestReport {
    fn new(tool_name: &str) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            passed: true,
            tests: Vec::new(),
            duration_ms: 0,
            tested_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        }
    }

    fn add_pass(&mut self, name: &str, output: Option<String>) {
        self.tests.push(TestResult {
            name: name.to_string(),
            passed: true,
            error: None,
            output,
        });
    }

    fn add_fail(&mut self, name: &str, error: &str) {
        self.passed = false;
        self.tests.push(TestResult {
            name: name.to_string(),
            passed: false,
            error: Some(error.to_string()),
            output: None,
        });
    }
}

// =============================================================================
// Tool Tester
// =============================================================================

/// Tester for generated tools
pub struct ToolTester {
    /// Tool generator for accessing tool packages
    generator: ToolGenerator,
    /// Timeout for test execution in seconds
    timeout_secs: u64,
}

impl ToolTester {
    /// Create a new tester
    pub fn new(generator: ToolGenerator) -> Self {
        Self {
            generator,
            timeout_secs: 30,
        }
    }

    /// Set the timeout for test execution
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Run self-tests on a generated tool
    pub async fn run_self_test(&self, tool_name: &str) -> Result<SelfTestReport> {
        let start = std::time::Instant::now();
        let mut report = SelfTestReport::new(tool_name);

        // Load definition
        let definition = match self.generator.load_definition(tool_name) {
            Ok(def) => {
                report.add_pass("load_definition", None);
                def
            }
            Err(e) => {
                report.add_fail("load_definition", &e.to_string());
                report.duration_ms = start.elapsed().as_millis() as u64;
                return Ok(report);
            }
        };

        // Validate schema
        if let Err(e) = self.validate_schema(&definition) {
            report.add_fail("validate_schema", &e);
        } else {
            report.add_pass("validate_schema", None);
        }

        // Check entrypoint exists
        let package_dir = self.generator.get_package_dir(tool_name);
        let entrypoint_path = package_dir.join(&definition.entrypoint);
        if entrypoint_path.exists() {
            report.add_pass("entrypoint_exists", None);
        } else {
            report.add_fail(
                "entrypoint_exists",
                &format!("Entrypoint not found: {}", entrypoint_path.display()),
            );
            report.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(report);
        }

        // Run with minimal input
        let test_input = self.generate_minimal_input(&definition.input_schema);
        match self.execute_tool(&definition, &package_dir, &test_input).await {
            Ok(output) => {
                // Verify output is valid JSON
                if serde_json::from_str::<Value>(&output).is_ok() {
                    report.add_pass("execute_minimal", Some(output));
                } else {
                    report.add_fail("execute_minimal", "Output is not valid JSON");
                }
            }
            Err(e) => {
                report.add_fail("execute_minimal", &e);
            }
        }

        report.duration_ms = start.elapsed().as_millis() as u64;

        // Update the tool's tested status
        if report.passed {
            if let Err(e) = self.generator.mark_self_tested(tool_name, true) {
                warn!(error = %e, "Failed to mark tool as tested");
            }
        }

        info!(
            tool_name = %tool_name,
            passed = report.passed,
            duration_ms = report.duration_ms,
            "Self-test complete"
        );

        Ok(report)
    }

    /// Validate the tool's input schema
    fn validate_schema(&self, definition: &GeneratedToolDefinition) -> std::result::Result<(), String> {
        let schema = &definition.input_schema;

        // Must be an object type
        if schema.get("type").and_then(|t| t.as_str()) != Some("object") {
            return Err("Schema must be of type 'object'".to_string());
        }

        // Must have properties
        if !schema.get("properties").map(|p| p.is_object()).unwrap_or(false) {
            return Err("Schema must have 'properties' object".to_string());
        }

        Ok(())
    }

    /// Generate minimal input that satisfies the schema
    fn generate_minimal_input(&self, schema: &Value) -> Value {
        let mut input = serde_json::Map::new();

        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            let required = schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            for (key, prop) in properties {
                if required.contains(&key.as_str()) {
                    // Generate a minimal value based on type
                    let value = match prop.get("type").and_then(|t| t.as_str()) {
                        Some("string") => Value::String("test".to_string()),
                        Some("number") | Some("integer") => Value::Number(0.into()),
                        Some("boolean") => Value::Bool(false),
                        Some("array") => Value::Array(vec![]),
                        Some("object") => Value::Object(serde_json::Map::new()),
                        _ => Value::String("test".to_string()),
                    };
                    input.insert(key.clone(), value);
                }
            }
        }

        // Ensure at least one input for non-empty schemas
        if input.is_empty() {
            input.insert("input".to_string(), Value::String("test".to_string()));
        }

        Value::Object(input)
    }

    /// Execute a tool with the given input
    async fn execute_tool(
        &self,
        definition: &GeneratedToolDefinition,
        package_dir: &Path,
        input: &Value,
    ) -> std::result::Result<String, String> {
        let entrypoint_path = package_dir.join(&definition.entrypoint);
        let input_json = serde_json::to_string(input).map_err(|e| e.to_string())?;

        let (cmd, args): (&str, Vec<&str>) = match definition.runtime.as_str() {
            "python" => ("python3", vec![entrypoint_path.to_str().unwrap_or("")]),
            "node" => ("node", vec![entrypoint_path.to_str().unwrap_or("")]),
            _ => return Err(format!("Unsupported runtime: {}", definition.runtime)),
        };

        debug!(
            cmd = %cmd,
            tool = %definition.name,
            "Executing tool for self-test"
        );

        let mut child = tokio::process::Command::new(cmd)
            .args(&args)
            .current_dir(package_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn process: {}", e))?;

        // Write input to stdin
        let mut stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        tokio::io::AsyncWriteExt::write_all(&mut stdin, input_json.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to stdin: {}", e))?;
        drop(stdin);

        // Wait for output with timeout
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| "Execution timed out")?
        .map_err(|e| format!("Failed to wait for process: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Process exited with error: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    }
}

// =============================================================================
// Subprocess Tool Wrapper
// =============================================================================

/// A tool wrapper that executes a generated tool via subprocess
pub struct SubprocessTool {
    definition: GeneratedToolDefinition,
    package_dir: std::path::PathBuf,
    confirmed: std::sync::atomic::AtomicBool,
}

impl SubprocessTool {
    /// Create a new subprocess tool wrapper
    pub fn new(definition: GeneratedToolDefinition, package_dir: std::path::PathBuf) -> Self {
        Self {
            definition,
            package_dir,
            confirmed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Mark the tool as confirmed for execution
    pub fn confirm(&self) {
        self.confirmed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if the tool has been confirmed
    pub fn is_confirmed(&self) -> bool {
        self.confirmed.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl AlephToolDyn for SubprocessTool {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            &self.definition.name,
            &self.definition.description,
            self.definition.input_schema.clone(),
            crate::dispatcher::ToolCategory::GeneratedSkill,
        )
    }

    fn call(
        &self,
        args: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move {
            // Check confirmation requirement
            if self.definition.requires_confirmation && !self.is_confirmed() {
                return Err(AlephError::Other {
                    message: format!(
                        "Tool '{}' requires confirmation before first use",
                        self.definition.name
                    ),
                    suggestion: Some("Call confirm() on the tool before execution".to_string()),
                });
            }

            // Check self-test
            if !self.definition.self_tested {
                return Err(AlephError::Other {
                    message: format!(
                        "Tool '{}' has not been self-tested",
                        self.definition.name
                    ),
                    suggestion: Some("Run self-test before using the tool".to_string()),
                });
            }

            let entrypoint_path = self.package_dir.join(&self.definition.entrypoint);
            let input_json = serde_json::to_string(&args).map_err(|e| AlephError::Other {
                message: format!("Failed to serialize input: {}", e),
                suggestion: None,
            })?;

            let (cmd, cmd_args): (&str, Vec<&str>) = match self.definition.runtime.as_str() {
                "python" => ("python3", vec![entrypoint_path.to_str().unwrap_or("")]),
                "node" => ("node", vec![entrypoint_path.to_str().unwrap_or("")]),
                _ => {
                    return Err(AlephError::Other {
                        message: format!("Unsupported runtime: {}", self.definition.runtime),
                        suggestion: None,
                    })
                }
            };

            let mut child = tokio::process::Command::new(cmd)
                .args(&cmd_args)
                .current_dir(&self.package_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| AlephError::Other {
                    message: format!("Failed to spawn process: {}", e),
                    suggestion: None,
                })?;

            let mut stdin = child.stdin.take().ok_or_else(|| AlephError::Other {
                message: "Failed to get stdin".to_string(),
                suggestion: None,
            })?;

            tokio::io::AsyncWriteExt::write_all(&mut stdin, input_json.as_bytes())
                .await
                .map_err(|e| AlephError::Other {
                    message: format!("Failed to write to stdin: {}", e),
                    suggestion: None,
                })?;
            drop(stdin);

            let output = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| AlephError::Other {
                message: "Tool execution timed out".to_string(),
                suggestion: None,
            })?
            .map_err(|e| AlephError::Other {
                message: format!("Failed to wait for process: {}", e),
                suggestion: None,
            })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(AlephError::Other {
                    message: format!("Tool execution failed: {}", stderr),
                    suggestion: None,
                });
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            serde_json::from_str(&stdout).map_err(|e| AlephError::Other {
                message: format!("Failed to parse tool output: {}", e),
                suggestion: None,
            })
        })
    }
}

// =============================================================================
// Tool Registrar
// =============================================================================

/// Registers generated tools into the ToolServer
pub struct ToolRegistrar {
    generator: ToolGenerator,
    tester: ToolTester,
}

impl ToolRegistrar {
    /// Create a new registrar
    pub fn new(generator: ToolGenerator) -> Self {
        let tester = ToolTester::new(generator.clone());
        Self { generator, tester }
    }

    /// Register a generated tool into the ToolServer.
    ///
    /// The tool must be self-tested before registration unless `skip_test` is true.
    pub async fn register(
        &self,
        tool_name: &str,
        server: &AlephToolServer,
        skip_test: bool,
    ) -> Result<()> {
        let definition = self.generator.load_definition(tool_name)?;

        // Run self-test if needed
        if !definition.self_tested && !skip_test {
            let report = self.tester.run_self_test(tool_name).await?;
            if !report.passed {
                return Err(AlephError::Other {
                    message: format!("Self-test failed for tool '{}'", tool_name),
                    suggestion: Some("Review the test report and fix issues".to_string()),
                });
            }
        }

        // Reload definition in case self-test updated it
        let definition = self.generator.load_definition(tool_name)?;
        let package_dir = self.generator.get_package_dir(tool_name);

        // Create subprocess tool
        let tool = SubprocessTool::new(definition, package_dir);

        // Register with server
        server.add_tool(tool).await;

        info!(tool_name = %tool_name, "Registered generated tool");
        Ok(())
    }

    /// Register all tested tools
    pub async fn register_all_tested(&self, server: &AlephToolServer) -> Result<usize> {
        let tools = self.generator.list_tools()?;
        let mut registered = 0;

        for tool in tools {
            if tool.self_tested {
                if let Err(e) = self.register(&tool.name, server, true).await {
                    warn!(tool = %tool.name, error = %e, "Failed to register tool");
                } else {
                    registered += 1;
                }
            }
        }

        Ok(registered)
    }

    /// Unregister a generated tool from the ToolServer
    pub async fn unregister(&self, tool_name: &str, server: &AlephToolServer) -> Result<()> {
        if server.remove_tool(tool_name).await {
            info!(tool_name = %tool_name, "Unregistered generated tool");
            Ok(())
        } else {
            Err(AlephError::Other {
                message: format!("Tool '{}' not found in server", tool_name),
                suggestion: None,
            })
        }
    }
}

impl Clone for ToolGenerator {
    fn clone(&self) -> Self {
        Self::with_config(super::tool_generator::ToolGeneratorConfig {
            output_dir: self.output_dir().to_path_buf(),
            runtime: "python".to_string(),
            require_confirmation: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::SkillMetrics;
    use crate::skill_evolution::SolidificationSuggestion;
    use tempfile::TempDir;

    fn create_test_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "test-pattern".to_string(),
            suggested_name: "echo-tool".to_string(),
            suggested_description: "Echo the input".to_string(),
            confidence: 0.9,
            metrics: SkillMetrics::new("test-pattern"),
            sample_contexts: vec!["echo input".to_string()],
            instructions_preview: "# Echo Tool\n\nReturn the input as-is.".to_string(),
        }
    }

    #[test]
    fn test_self_test_report() {
        let mut report = SelfTestReport::new("test-tool");
        assert!(report.passed);

        report.add_pass("test1", None);
        assert!(report.passed);

        report.add_fail("test2", "Something failed");
        assert!(!report.passed);

        assert_eq!(report.tests.len(), 2);
    }

    #[test]
    fn test_generate_minimal_input() {
        let temp_dir = TempDir::new().unwrap();
        let config = super::super::tool_generator::ToolGeneratorConfig {
            output_dir: temp_dir.path().to_path_buf(),
            runtime: "python".to_string(),
            require_confirmation: true,
        };
        let generator = ToolGenerator::with_config(config);
        let tester = ToolTester::new(generator);

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "count": { "type": "integer" }
            },
            "required": ["name"]
        });

        let input = tester.generate_minimal_input(&schema);
        assert!(input.get("name").is_some());
    }

    #[test]
    fn test_subprocess_tool_creation() {
        let definition = super::super::tool_generator::GeneratedToolDefinition {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({}),
            runtime: "python".to_string(),
            entrypoint: "entrypoint.py".to_string(),
            self_tested: true,
            requires_confirmation: false,
            generated: super::super::tool_generator::GenerationMetadata {
                pattern_id: "test".to_string(),
                confidence: 0.9,
                generated_at: 0,
                generator_version: "1.0.0".to_string(),
            },
        };

        let tool = SubprocessTool::new(definition, std::path::PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "test_tool");
        assert!(!tool.is_confirmed());

        tool.confirm();
        assert!(tool.is_confirmed());
    }
}
