//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - File-based persistence with optimistic locking
//! - Async approval manager for RPC integration

pub mod allowed_decisions;
pub mod allowlist;
pub mod analysis;
pub mod approval;
pub mod bridge;
pub mod config;
pub mod decision;
pub mod kernel;
pub mod leak_detector;
pub mod manager;
pub mod masker;
pub mod parser;
pub mod risk;
pub mod sanitize;
pub mod secret_patterns;
pub mod socket;
pub mod storage;

pub use allowlist::match_allowlist;
pub use analysis::CommandAnalysis;
pub use bridge::ApprovalBridge;
pub use config::ExecApprovalsFile;
pub use decision::ApprovalRequest;
pub use kernel::SecurityKernel;
pub use manager::{ExecApprovalManager, PendingApproval};
pub use masker::SecretMasker;
pub use parser::analyze_shell_command;
pub use risk::{RiskLevel, BLOCKED_PATTERNS};
pub use sanitize::has_invisible_chars;
pub use socket::ApprovalDecisionType;
pub use storage::{ConfigWithHash, ExecApprovalsStorage, StorageError};
