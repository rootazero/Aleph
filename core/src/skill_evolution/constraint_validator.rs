//! Constraint Validator: Validates that soft constraints match hard constraints
//!
//! The ConstraintValidator ensures that the semantic constraints defined in
//! SuccessManifest (soft constraints) are consistent with the sandbox
//! Capabilities (hard constraints).
//!
//! This prevents situations where:
//! - Manifest says "no network" but Capabilities allow network
//! - Manifest allows file access but Capabilities don't grant it
//! - Capabilities grant permissions that Manifest doesn't explicitly allow
//!
//! # Example
//!
//! ```rust
//! use alephcore::skill_evolution::constraint_validator::ConstraintValidator;
//! use alephcore::skill_evolution::success_manifest::SuccessManifest;
//! use alephcore::exec::sandbox::capabilities::Capabilities;
//!
//! let manifest = SuccessManifest::new("test_skill", "Test skill");
//! let capabilities = Capabilities::default();
//!
//! // Validate constraints
//! match ConstraintValidator::validate(&manifest, &capabilities) {
//!     Ok(report) => {
//!         println!("Validation passed with {} warnings", report.warnings.len());
//!     }
//!     Err(mismatch) => {
//!         eprintln!("Validation failed: {:?}", mismatch);
//!     }
//! }
//! ```
//!
//! # Validation Rules
//!
//! 1. **Network**: If manifest prohibits network, capabilities must deny network
//! 2. **Filesystem Read**: Manifest-allowed paths must be granted by capabilities
//! 3. **Filesystem Write**: Manifest-allowed write paths must be granted
//! 4. **Unauthorized Permissions**: Capabilities shouldn't grant undeclared permissions
//! 5. **Process**: If manifest prohibits fork, capabilities must deny process spawn

use crate::exec::sandbox::capabilities::{
    Capabilities, FileSystemCapability, NetworkCapability,
};
use crate::skill_evolution::success_manifest::SuccessManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Constraint validator
pub struct ConstraintValidator;

impl ConstraintValidator {
    /// Validate that soft constraints (Manifest) match hard constraints (Capabilities)
    ///
    /// Returns Ok(ValidationReport) if validation succeeds (may contain warnings)
    /// Returns Err(ConstraintMismatch) if validation fails (contains errors)
    pub fn validate(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
    ) -> Result<ValidationReport, ConstraintMismatch> {
        let mut report = ValidationReport::new();

        // Rule 1: Network constraints
        Self::validate_network_constraints(manifest, capabilities, &mut report);

        // Rule 2: Filesystem read constraints
        Self::validate_filesystem_read_constraints(manifest, capabilities, &mut report);

        // Rule 3: Filesystem write constraints
        Self::validate_filesystem_write_constraints(manifest, capabilities, &mut report);

        // Rule 4: Unauthorized permissions
        Self::validate_unauthorized_permissions(manifest, capabilities, &mut report);

        // Rule 5: Process constraints
        Self::validate_process_constraints(manifest, capabilities, &mut report);

        if report.has_errors() {
            Err(ConstraintMismatch::ValidationFailed(report))
        } else {
            Ok(report)
        }
    }

    /// Validate network constraints
    fn validate_network_constraints(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
        report: &mut ValidationReport,
    ) {
        // If Manifest prohibits network, Capabilities must deny network
        if manifest.prohibits_network() {
            match &capabilities.network {
                NetworkCapability::Deny => {
                    // Good: both prohibit network
                }
                NetworkCapability::AllowDomains(domains) => {
                    report.add_error(ValidationError::NetworkMismatch {
                        manifest_rule: "Prohibit all network access".to_string(),
                        capabilities_rule: format!("Allow domains: {:?}", domains),
                        reason: "Manifest prohibits network but Capabilities allow specific domains".to_string(),
                    });
                }
                NetworkCapability::AllowAll => {
                    report.add_error(ValidationError::NetworkMismatch {
                        manifest_rule: "Prohibit all network access".to_string(),
                        capabilities_rule: "Allow all network access".to_string(),
                        reason: "Manifest prohibits network but Capabilities allow all network".to_string(),
                    });
                }
            }
        } else {
            // Manifest allows network (or doesn't explicitly prohibit)
            // This is OK - Capabilities can be more restrictive
            if matches!(capabilities.network, NetworkCapability::Deny) {
                report.add_warning(ValidationWarning::CapabilitiesMoreRestrictive {
                    aspect: "network".to_string(),
                    reason: "Manifest doesn't prohibit network but Capabilities deny it".to_string(),
                });
            }
        }
    }

    /// Validate filesystem read constraints
    fn validate_filesystem_read_constraints(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
        report: &mut ValidationReport,
    ) {
        // For each path Manifest allows reading, check if Capabilities grant access
        for read_path in manifest.allowed_read_paths() {
            let path_buf = PathBuf::from(read_path);
            let has_access = capabilities.filesystem.iter().any(|cap| match cap {
                FileSystemCapability::ReadOnly { path } => {
                    Self::path_matches(&path_buf, path)
                }
                FileSystemCapability::ReadWrite { path } => {
                    Self::path_matches(&path_buf, path)
                }
                FileSystemCapability::TempWorkspace => false,
            });

            if !has_access {
                report.add_error(ValidationError::FileSystemMismatch {
                    manifest_path: read_path.clone(),
                    operation: "read".to_string(),
                    reason: "Manifest allows reading but Capabilities don't grant access".to_string(),
                });
            }
        }
    }

    /// Validate filesystem write constraints
    fn validate_filesystem_write_constraints(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
        report: &mut ValidationReport,
    ) {
        // For each path Manifest allows writing, check if Capabilities grant write access
        for write_path in manifest.allowed_write_paths() {
            let path_buf = PathBuf::from(write_path);
            let has_write_access = capabilities.filesystem.iter().any(|cap| match cap {
                FileSystemCapability::ReadWrite { path } => {
                    Self::path_matches(&path_buf, path)
                }
                _ => false,
            });

            if !has_write_access {
                report.add_error(ValidationError::FileSystemMismatch {
                    manifest_path: write_path.clone(),
                    operation: "write".to_string(),
                    reason: "Manifest allows writing but Capabilities don't grant write access".to_string(),
                });
            }
        }
    }

    /// Validate unauthorized permissions
    fn validate_unauthorized_permissions(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
        report: &mut ValidationReport,
    ) {
        // Check if Capabilities grant permissions that Manifest doesn't explicitly allow
        for cap in &capabilities.filesystem {
            match cap {
                FileSystemCapability::ReadWrite { path } => {
                    if !manifest.allows_write_to(path) {
                        report.add_error(ValidationError::UnauthorizedPermission {
                            capability: format!("ReadWrite: {}", path.display()),
                            reason: "Capabilities grant write permission but Manifest doesn't explicitly allow it".to_string(),
                        });
                    }
                }
                FileSystemCapability::ReadOnly { path } => {
                    if !manifest.allows_read_from(path) {
                        report.add_warning(ValidationWarning::UnauthorizedReadPermission {
                            path: path.display().to_string(),
                            reason: "Capabilities grant read permission but Manifest doesn't explicitly allow it".to_string(),
                        });
                    }
                }
                FileSystemCapability::TempWorkspace => {
                    // TempWorkspace is generally OK
                }
            }
        }
    }

    /// Validate process constraints
    fn validate_process_constraints(
        manifest: &SuccessManifest,
        capabilities: &Capabilities,
        report: &mut ValidationReport,
    ) {
        // If Manifest prohibits fork, Capabilities must enforce no_fork
        if manifest.prohibited_operations.process.prohibit_fork {
            if !capabilities.process.no_fork {
                report.add_error(ValidationError::ProcessMismatch {
                    manifest_rule: "Prohibit fork/exec".to_string(),
                    capabilities_rule: "Allow fork/exec".to_string(),
                    reason: "Manifest prohibits fork but Capabilities allow it".to_string(),
                });
            }
        }
    }

    /// Check if a path pattern matches a specific path
    fn path_matches(pattern: &Path, path: &Path) -> bool {
        let pattern_str = pattern.to_string_lossy();
        let path_str = path.to_string_lossy();

        if pattern_str.ends_with("/**") {
            // Recursive match
            let prefix = &pattern_str[..pattern_str.len() - 3];
            path_str.starts_with(prefix)
        } else if pattern_str.ends_with("/*") {
            // Single-level match
            let prefix = &pattern_str[..pattern_str.len() - 1];
            if let Some(rest) = path_str.strip_prefix(prefix) {
                !rest.is_empty() && !rest.contains('/')
            } else {
                false
            }
        } else {
            // Exact match or prefix match
            path_str.starts_with(pattern_str.as_ref())
        }
    }
}

/// Validation report containing errors and warnings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationReport {
    /// Validation errors (must be fixed)
    pub errors: Vec<ValidationError>,
    /// Validation warnings (should be reviewed)
    pub warnings: Vec<ValidationWarning>,
}

impl ValidationReport {
    /// Create a new empty validation report
    pub fn new() -> Self {
        Self {
            errors: vec![],
            warnings: vec![],
        }
    }

    /// Add an error to the report
    pub fn add_error(&mut self, error: ValidationError) {
        self.errors.push(error);
    }

    /// Add a warning to the report
    pub fn add_warning(&mut self, warning: ValidationWarning) {
        self.warnings.push(warning);
    }

    /// Check if the report has any errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Check if the report has any warnings
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Get the total number of issues (errors + warnings)
    pub fn issue_count(&self) -> usize {
        self.errors.len() + self.warnings.len()
    }
}

impl Default for ValidationReport {
    fn default() -> Self {
        Self::new()
    }
}

/// Validation error
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValidationError {
    /// Network constraint mismatch
    NetworkMismatch {
        manifest_rule: String,
        capabilities_rule: String,
        reason: String,
    },
    /// Filesystem constraint mismatch
    FileSystemMismatch {
        manifest_path: String,
        operation: String,
        reason: String,
    },
    /// Unauthorized permission granted
    UnauthorizedPermission {
        capability: String,
        reason: String,
    },
    /// Process constraint mismatch
    ProcessMismatch {
        manifest_rule: String,
        capabilities_rule: String,
        reason: String,
    },
}

/// Validation warning
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValidationWarning {
    /// Capabilities are more restrictive than Manifest
    CapabilitiesMoreRestrictive {
        aspect: String,
        reason: String,
    },
    /// Unauthorized read permission (less severe than write)
    UnauthorizedReadPermission {
        path: String,
        reason: String,
    },
}

/// Constraint mismatch error
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConstraintMismatch {
    /// Validation failed with errors
    ValidationFailed(ValidationReport),
}

impl std::fmt::Display for ConstraintMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintMismatch::ValidationFailed(report) => {
                write!(f, "Constraint validation failed with {} error(s)", report.errors.len())
            }
        }
    }
}

impl std::error::Error for ConstraintMismatch {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::success_manifest::{
        AllowedOperations, DataProcessing, FileSystemOperations, FileSystemRestrictions,
        NetworkRestrictions, ProcessRestrictions, ProhibitedOperations, ScriptExecution,
    };

    #[test]
    fn test_network_constraint_match() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.prohibited_operations.network.prohibit_all = true;

        let mut capabilities = Capabilities::default();
        capabilities.network = NetworkCapability::Deny;

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_ok());
        let report = result.unwrap();
        assert!(!report.has_errors());
    }

    #[test]
    fn test_network_constraint_mismatch() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.prohibited_operations.network.prohibit_all = true;

        let mut capabilities = Capabilities::default();
        capabilities.network = NetworkCapability::AllowAll;

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_err());
        if let Err(ConstraintMismatch::ValidationFailed(report)) = result {
            assert!(report.has_errors());
            assert_eq!(report.errors.len(), 1);
        }
    }

    #[test]
    fn test_filesystem_read_constraint_match() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.allowed_operations.filesystem.read_paths = vec![
            "/data/input/**".to_string(),
        ];

        let mut capabilities = Capabilities::default();
        capabilities.filesystem = vec![
            FileSystemCapability::ReadOnly {
                path: PathBuf::from("/data/input"),
            },
        ];

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_ok());
    }

    #[test]
    fn test_filesystem_write_constraint_mismatch() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.allowed_operations.filesystem.write_paths = vec![
            "/data/output/**".to_string(),
        ];

        let capabilities = Capabilities::default();
        // Default capabilities don't grant write access to /data/output

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_err());
        if let Err(ConstraintMismatch::ValidationFailed(report)) = result {
            assert!(report.has_errors());
        }
    }

    #[test]
    fn test_unauthorized_write_permission() {
        let manifest = SuccessManifest::new("test", "test");
        // Manifest doesn't allow any write paths

        let mut capabilities = Capabilities::default();
        capabilities.filesystem = vec![
            FileSystemCapability::ReadWrite {
                path: PathBuf::from("/data/output"),
            },
        ];

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_err());
        if let Err(ConstraintMismatch::ValidationFailed(report)) = result {
            assert!(report.has_errors());
        }
    }

    #[test]
    fn test_process_constraint_match() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.prohibited_operations.process.prohibit_fork = true;

        let mut capabilities = Capabilities::default();
        capabilities.process.no_fork = true;

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_constraint_mismatch() {
        let mut manifest = SuccessManifest::new("test", "test");
        manifest.prohibited_operations.process.prohibit_fork = true;

        let mut capabilities = Capabilities::default();
        capabilities.process.no_fork = false;

        let result = ConstraintValidator::validate(&manifest, &capabilities);
        assert!(result.is_err());
    }
}
