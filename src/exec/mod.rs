//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Quote-aware shell command parsing
//! - Risk classification and blocked-pattern hard filters
//! - Async approval manager for RPC integration
//!
//! Approval grants come in three tiers, and only two of them are in-memory:
//! `AllowOnce` (a single execution) and `AllowSession` (the rest of the
//! session) live only in memory, while the `AllowAlways` tier is persisted to
//! `~/.aleph/approval-grants.json` and survives a restart until it is revoked.
//! See [`allowed_decisions::with_persistent`] for the set that carries it and
//! [`crate::sandbox::exec_approval::grants`] for the store that persists it.

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
pub use leak_detector::LeakDetector;
pub use manager::{ExecApprovalManager, ExecApprovalRecord, PendingApproval};
pub use masker::SecretMasker;
pub use parser::analyze_shell_command;
pub use risk::RiskLevel;
pub use socket::ApprovalDecisionType;
