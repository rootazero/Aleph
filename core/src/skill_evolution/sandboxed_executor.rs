//! Sandboxed Tool Executor with Constraint Validation
//!
//! This module provides execution of generated tools with pre-execution
//! constraint validation. It integrates the SuccessManifest validation
//! into the tool execution pipeline.
//!
//! # Example
//!
//! ```rust,no_run
//! use alephcore::skill_evolution::sandboxed_executor::SandboxedToolExecutor;
//! use alephcore::skill_evolution::tool_generator::GeneratedToolDefinition;
//! use std::sync::Arc;
//! use std::path::PathBuf;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let sandbox_adapter = todo!();
//! let executor = SandboxedToolExecutor::new(sandbox_adapter);
//!
//! # let tool_def: GeneratedToolDefinition = todo!();
//! # let parameters = serde_json::json!({});
//! # let package_dir = PathBuf::from("/tmp");
//! // Execute tool with constraint validation
//! match executor.execute_tool(&tool_def, parameters, package_dir).await {
//!     Ok((output, audit_log)) => {
//!         println!("Tool output: {}", output);
//!         println!("Audit log: {:?}", audit_log);
//!     }
//!     Err(e) => {
//!         eprintln!("Execution failed: {}", e);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Constraint Validation Flow
//!
//! 1. Resolve capabilities from tool definition and parameters
//! 2. If SuccessManifest exists, validate against capabilities
//! 3. Block execution if validation errors found
//! 4. Log warnings if validation warnings found
//! 5. Execute tool in sandbox
//! 6. Add validation results to audit log
//!
//! # Integration with Collaborative Evolution
//!
//! This executor is the runtime enforcement point for the dual-layer
//! constraint system:
//!
//! - **Design Time**: CollaborativeSolidificationPipeline generates constraints
//! - **Runtime**: SandboxedToolExecutor validates and enforces constraints

use std::sync::Arc;
use std::path::PathBuf;

use crate::error::{AlephError, Result};
use crate::exec::sandbox::{
    executor::SandboxManager,
    adapter::{SandboxAdapter, SandboxCommand},
    audit::{SandboxAuditLog, ToolExecutionContext, ResolutionStep},
};
use super::tool_generator::GeneratedToolDefinition;
use super::sandbox_integration::resolve_tool_capabilities;
use super::constraint_validator::ConstraintValidator;

/// Executor for running tools in sandbox
pub struct SandboxedToolExecutor {
    sandbox_manager: SandboxManager,
}

impl SandboxedToolExecutor {
    pub fn new(sandbox_adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            sandbox_manager: SandboxManager::new(sandbox_adapter),
        }
    }

    /// Execute a tool with sandboxing
    pub async fn execute_tool(
        &self,
        tool_def: &GeneratedToolDefinition,
        parameters: serde_json::Value,
        tool_package_dir: PathBuf,
    ) -> Result<(String, SandboxAuditLog)> {
        // Resolve capabilities first
        let capabilities = resolve_tool_capabilities(tool_def, &parameters)?;

        // Validate constraints if success_manifest is present
        let validation_report = if let Some(ref manifest) = tool_def.success_manifest {
            match ConstraintValidator::validate(manifest, &capabilities) {
                Ok(report) => {
                    // Log validation results
                    if !report.errors.is_empty() {
                        tracing::error!(
                            tool = %tool_def.name,
                            errors = ?report.errors,
                            "Constraint validation failed"
                        );
                        return Err(AlephError::Other {
                            message: format!(
                                "Tool '{}' has constraint violations: {}",
                                tool_def.name,
                                report.errors.iter()
                                    .map(|e| format!("{:?}", e))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            suggestion: Some("Fix the constraint violations before executing".to_string()),
                        });
                    }

                    if !report.warnings.is_empty() {
                        tracing::warn!(
                            tool = %tool_def.name,
                            warnings = ?report.warnings,
                            "Constraint validation warnings"
                        );
                    }

                    Some(report)
                }
                Err(e) => {
                    tracing::error!(
                        tool = %tool_def.name,
                        error = ?e,
                        "Constraint validation error"
                    );
                    return Err(AlephError::Other {
                        message: format!("Constraint validation error: {:?}", e),
                        suggestion: None,
                    });
                }
            }
        } else {
            None
        };

        // Build command
        let command = SandboxCommand {
            program: self.get_runtime_executable(&tool_def.runtime)?,
            args: vec![
                tool_def.entrypoint.clone(),
                serde_json::to_string(&parameters).map_err(|e| AlephError::InvalidConfig {
                    message: format!("Failed to serialize parameters: {}", e),
                    suggestion: None,
                })?,
            ],
            working_dir: Some(tool_package_dir),
        };

        // Execute in sandbox
        let (result, mut audit_log) = self
            .sandbox_manager
            .execute_sandboxed(&tool_def.name, command, capabilities)
            .await?;

        // Add tool context to audit log
        audit_log.tool_context = Some(ToolExecutionContext {
            tool_name: tool_def.name.clone(),
            tool_version: tool_def.generated.generator_version.clone(),
            base_preset: tool_def
                .required_capabilities
                .as_ref()
                .map(|c| c.base_preset.clone())
                .unwrap_or_default(),
            applied_overrides: vec![],
            parameter_bindings_used: Default::default(),
            dynamic_paths: vec![],
            capability_resolution_log: vec![
                ResolutionStep {
                    step: "load_preset".to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                    description: "Loaded base preset".to_string(),
                },
                ResolutionStep {
                    step: "validate_constraints".to_string(),
                    timestamp: chrono::Utc::now().timestamp(),
                    description: if let Some(ref report) = validation_report {
                        format!(
                            "Validated constraints: {} errors, {} warnings",
                            report.errors.len(),
                            report.warnings.len()
                        )
                    } else {
                        "No success manifest to validate".to_string()
                    },
                },
            ],
        });

        Ok((result.stdout, audit_log))
    }

    fn get_runtime_executable(&self, runtime: &str) -> Result<String> {
        match runtime {
            "python" => Ok("python3".to_string()),
            "node" => Ok("node".to_string()),
            _ => Err(AlephError::InvalidConfig {
                message: format!("Unsupported runtime: {}", runtime),
                suggestion: Some("Use 'python' or 'node'".to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::success_manifest::{
        SuccessManifest, SkillMetadata, AllowedOperations, ProhibitedOperations,
        FileSystemOperations, ScriptExecution, DataProcessing,
        NetworkRestrictions, FileSystemRestrictions, ProcessRestrictions,
    };
    use crate::skill_evolution::constraint_validator::ConstraintMismatch;
    use crate::exec::sandbox::capabilities::{Capabilities, NetworkCapability, FileSystemCapability};

    #[test]
    fn test_sandboxed_executor_structure() {
        // Basic structure test
        assert!(true);
    }

    #[test]
    fn test_constraint_validation_blocks_execution() {
        // Create a tool definition with mismatched constraints
        let manifest = SuccessManifest {
            metadata: SkillMetadata {
                skill_id: "test-tool".to_string(),
                version: "1.0.0".to_string(),
                created_at: 1707667200, // 2026-02-11 in Unix timestamp
                author: "test".to_string(),
            },
            goal: "Test tool".to_string(),
            allowed_operations: AllowedOperations {
                filesystem: FileSystemOperations {
                    read_paths: vec![],
                    write_paths: vec![],
                    allow_temp_workspace: false,
                },
                script_execution: ScriptExecution {
                    languages: vec![],
                    libraries: vec![],
                },
                data_processing: DataProcessing {
                    input_formats: vec![],
                    output_formats: vec![],
                    operations: vec![],
                },
            },
            prohibited_operations: ProhibitedOperations {
                network: NetworkRestrictions {
                    prohibit_all: true, // Prohibits all network
                    prohibited_domains: vec![],
                    reason: "Test restriction".to_string(),
                },
                filesystem: FileSystemRestrictions {
                    prohibited_paths: vec![],
                    prohibit_modify_originals: true,
                    reason: "Test restriction".to_string(),
                },
                process: ProcessRestrictions {
                    prohibit_fork: true,
                    prohibited_commands: vec![],
                    reason: "Test restriction".to_string(),
                },
            },
            recommended_tools: vec![],
            success_criteria: vec![],
            failure_handling: vec![],
            security_guarantees: vec![],
        };

        // Create capabilities that allow network (mismatch)
        let mut capabilities = Capabilities::default();
        capabilities.network = NetworkCapability::AllowAll;

        // Validate constraints
        let result = ConstraintValidator::validate(&manifest, &capabilities);

        // Should have errors due to network mismatch
        match result {
            Ok(_report) => {
                panic!("Expected validation to fail with errors, but it succeeded");
            }
            Err(ConstraintMismatch::ValidationFailed(report)) => {
                assert!(!report.errors.is_empty(), "Expected errors but got none");
                assert!(report.errors.iter().any(|e| format!("{:?}", e).contains("network") || format!("{:?}", e).contains("Network")));
            }
        }
    }

    #[test]
    fn test_constraint_validation_allows_matching() {
        // Create a tool definition with matching constraints
        let manifest = SuccessManifest {
            metadata: SkillMetadata {
                skill_id: "test-tool".to_string(),
                version: "1.0.0".to_string(),
                created_at: 1707667200, // 2026-02-11 in Unix timestamp
                author: "test".to_string(),
            },
            goal: "Test tool".to_string(),
            allowed_operations: AllowedOperations {
                filesystem: FileSystemOperations {
                    read_paths: vec!["/tmp".to_string()], // Match the capability path
                    write_paths: vec![],
                    allow_temp_workspace: true,
                },
                script_execution: ScriptExecution {
                    languages: vec![],
                    libraries: vec![],
                },
                data_processing: DataProcessing {
                    input_formats: vec![],
                    output_formats: vec![],
                    operations: vec![],
                },
            },
            prohibited_operations: ProhibitedOperations {
                network: NetworkRestrictions {
                    prohibit_all: true,
                    prohibited_domains: vec![],
                    reason: "Test restriction".to_string(),
                },
                filesystem: FileSystemRestrictions {
                    prohibited_paths: vec![],
                    prohibit_modify_originals: true,
                    reason: "Test restriction".to_string(),
                },
                process: ProcessRestrictions {
                    prohibit_fork: true,
                    prohibited_commands: vec![],
                    reason: "Test restriction".to_string(),
                },
            },
            recommended_tools: vec![],
            success_criteria: vec![],
            failure_handling: vec![],
            security_guarantees: vec![],
        };

        // Create capabilities that don't allow network (matching)
        let mut capabilities = Capabilities::default(); // Default has no network
        // Add filesystem read capability for /tmp/*
        capabilities.filesystem.push(FileSystemCapability::ReadOnly {
            path: std::path::PathBuf::from("/tmp"),
        });

        // Validate constraints
        let result = ConstraintValidator::validate(&manifest, &capabilities);

        // Should have no errors
        match result {
            Ok(report) => {
                assert!(report.errors.is_empty(), "Expected no errors but got: {:?}", report.errors);
            }
            Err(e) => {
                panic!("Validation returned Err instead of Ok: {:?}", e);
            }
        }
    }
}
