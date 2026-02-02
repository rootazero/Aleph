//! POE Contract Types
//!
//! This module defines types for the contract signing workflow:
//! - `PendingContract`: A contract awaiting user signature
//! - `ContractContext`: Optional context for contract generation
//! - `SignRequest`: Request to sign a contract with optional amendments

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::poe::SuccessManifest;

// ============================================================================
// Pending Contract
// ============================================================================

/// A POE contract awaiting user signature.
///
/// Created by `poe.prepare`, stored until signed via `poe.sign` or
/// rejected via `poe.reject`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingContract {
    /// Unique contract identifier
    pub contract_id: String,

    /// Original user instruction
    pub instruction: String,

    /// Generated success manifest
    pub manifest: SuccessManifest,

    /// Optional context information
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContractContext>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

impl PendingContract {
    /// Create a new pending contract.
    pub fn new(
        contract_id: impl Into<String>,
        instruction: impl Into<String>,
        manifest: SuccessManifest,
    ) -> Self {
        Self {
            contract_id: contract_id.into(),
            instruction: instruction.into(),
            manifest,
            context: None,
            created_at: Utc::now(),
        }
    }

    /// Set the contract context.
    pub fn with_context(mut self, context: ContractContext) -> Self {
        self.context = Some(context);
        self
    }
}

// ============================================================================
// Contract Context
// ============================================================================

/// Context information for contract generation.
///
/// Provides hints to the ManifestBuilder about the execution environment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContractContext {
    /// Working directory for the task
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,

    /// Related files to consider
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    /// Session key for event routing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

impl ContractContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the working directory.
    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Add related files.
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = files;
        self
    }

    /// Set the session key.
    pub fn with_session_key(mut self, key: impl Into<String>) -> Self {
        self.session_key = Some(key.into());
        self
    }

    /// Convert context to a string for ManifestBuilder.
    pub fn to_context_string(&self) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(dir) = &self.working_dir {
            parts.push(format!("Working directory: {}", dir));
        }

        if !self.files.is_empty() {
            parts.push(format!("Related files: {}", self.files.join(", ")));
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }
}

// ============================================================================
// Sign Request
// ============================================================================

/// Request to sign a pending contract.
///
/// Supports three modes:
/// 1. No modifications: Sign the contract as-is
/// 2. Natural language amendments: LLM interprets and merges changes
/// 3. JSON override: Direct manifest field overrides (advanced)
#[derive(Debug, Clone, Deserialize)]
pub struct SignRequest {
    /// Contract ID to sign
    pub contract_id: String,

    /// Natural language amendments (e.g., "also check cargo clippy")
    #[serde(default)]
    pub amendments: Option<String>,

    /// Direct manifest override (advanced mode)
    #[serde(default)]
    pub manifest_override: Option<SuccessManifest>,

    /// Whether to stream execution events
    #[serde(default = "default_true")]
    pub stream: bool,
}

fn default_true() -> bool {
    true
}

impl SignRequest {
    /// Create a simple sign request with no modifications.
    pub fn new(contract_id: impl Into<String>) -> Self {
        Self {
            contract_id: contract_id.into(),
            amendments: None,
            manifest_override: None,
            stream: true,
        }
    }

    /// Add natural language amendments.
    pub fn with_amendments(mut self, amendments: impl Into<String>) -> Self {
        self.amendments = Some(amendments.into());
        self
    }

    /// Add manifest override.
    pub fn with_override(mut self, manifest: SuccessManifest) -> Self {
        self.manifest_override = Some(manifest);
        self
    }

    /// Disable streaming.
    pub fn without_streaming(mut self) -> Self {
        self.stream = false;
        self
    }
}

// ============================================================================
// Reject Request
// ============================================================================

/// Request to reject a pending contract.
#[derive(Debug, Clone, Deserialize)]
pub struct RejectRequest {
    /// Contract ID to reject
    pub contract_id: String,

    /// Optional reason for rejection
    #[serde(default)]
    pub reason: Option<String>,
}

// ============================================================================
// Response Types
// ============================================================================

/// Result of poe.prepare request.
#[derive(Debug, Clone, Serialize)]
pub struct PrepareResult {
    /// Unique contract identifier
    pub contract_id: String,

    /// Generated success manifest
    pub manifest: SuccessManifest,

    /// Creation timestamp
    pub created_at: String,

    /// Original instruction (echoed back)
    pub instruction: String,
}

/// Result of poe.sign request.
#[derive(Debug, Clone, Serialize)]
pub struct SignResult {
    /// Task ID (from manifest)
    pub task_id: String,

    /// Session key for event subscription
    pub session_key: String,

    /// Signature timestamp
    pub signed_at: String,

    /// Final manifest after amendments
    pub final_manifest: SuccessManifest,
}

/// Result of poe.reject request.
#[derive(Debug, Clone, Serialize)]
pub struct RejectResult {
    /// Contract ID
    pub contract_id: String,

    /// Whether rejection succeeded
    pub rejected: bool,
}

/// Result of poe.pending request.
#[derive(Debug, Clone, Serialize)]
pub struct PendingResult {
    /// List of pending contracts
    pub contracts: Vec<ContractSummary>,

    /// Total count
    pub count: usize,
}

/// Summary of a pending contract (for listing).
#[derive(Debug, Clone, Serialize)]
pub struct ContractSummary {
    /// Contract ID
    pub contract_id: String,

    /// Original instruction
    pub instruction: String,

    /// Task objective
    pub objective: String,

    /// Creation timestamp
    pub created_at: String,
}

impl From<PendingContract> for ContractSummary {
    fn from(contract: PendingContract) -> Self {
        Self {
            contract_id: contract.contract_id,
            instruction: contract.instruction,
            objective: contract.manifest.objective,
            created_at: contract.created_at.to_rfc3339(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::SuccessManifest;

    #[test]
    fn test_pending_contract_creation() {
        let manifest = SuccessManifest::new("task-1", "Test objective");
        let contract = PendingContract::new("contract-123", "Do something", manifest);

        assert_eq!(contract.contract_id, "contract-123");
        assert_eq!(contract.instruction, "Do something");
        assert!(contract.context.is_none());
    }

    #[test]
    fn test_contract_context() {
        let context = ContractContext::new()
            .with_working_dir("/workspace")
            .with_files(vec!["src/main.rs".into(), "Cargo.toml".into()]);

        let context_str = context.to_context_string().unwrap();
        assert!(context_str.contains("/workspace"));
        assert!(context_str.contains("src/main.rs"));
    }

    #[test]
    fn test_sign_request() {
        let request = SignRequest::new("contract-123")
            .with_amendments("also check clippy")
            .without_streaming();

        assert_eq!(request.contract_id, "contract-123");
        assert_eq!(request.amendments, Some("also check clippy".into()));
        assert!(!request.stream);
    }

    #[test]
    fn test_contract_summary_from() {
        let manifest = SuccessManifest::new("task-1", "Test objective");
        let contract = PendingContract::new("contract-123", "Do something", manifest);
        let summary: ContractSummary = contract.into();

        assert_eq!(summary.contract_id, "contract-123");
        assert_eq!(summary.objective, "Test objective");
    }
}
