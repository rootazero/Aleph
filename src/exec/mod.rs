//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Quote-aware shell command parsing
//! - Risk classification and blocked-pattern hard filters
//! - Async approval manager for RPC integration
//!
//! Approval grants live only in memory: `AllowOnce` for a single execution,
//! `AllowSession` for the rest of the session. There is no on-disk allowlist.

pub mod allowed_decisions;
pub mod analysis;
pub mod approval;
pub mod bridge;
pub mod decision;
pub mod kernel;
pub mod leak_detector;
pub mod manager;
pub mod masker;
pub mod parser;
pub mod risk;
pub mod secret_patterns;
pub mod socket;

pub use analysis::CommandAnalysis;
pub use bridge::ApprovalBridge;
pub use decision::{ApprovalRequest, ExecApprovalRequest};
pub use kernel::SecurityKernel;
pub use leak_detector::{LeakAction, LeakDetector, ScanResult};
pub use manager::{ExecApprovalManager, ExecApprovalRecord, PendingApproval};
pub use masker::SecretMasker;
pub use parser::analyze_shell_command;
pub use risk::RiskLevel;
pub use socket::ApprovalDecisionType;
