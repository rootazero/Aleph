//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol
//! - File-based persistence with optimistic locking
//! - Async approval manager for RPC integration

pub mod allowlist;
pub mod analysis;
pub mod approval;
pub mod bridge;
pub mod config;
pub mod decision;
pub mod forwarder;
#[cfg(unix)]
pub mod ipc;
pub mod kernel;
pub mod leak_detector;
pub mod manager;
pub mod masker;
pub mod parser;
pub mod risk;
pub mod sandbox;
pub mod sanitize;
pub mod socket;
pub mod storage;

pub use allowlist::match_allowlist;
pub use analysis::CommandAnalysis;
pub use bridge::ApprovalBridge;
pub use config::{ExecApprovalsFile, ExecAsk, ExecSecurity, ResolvedExecConfig};
pub use decision::{decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext};
pub use forwarder::{ExecApprovalForwarder, ForwardMode, ForwardTarget};
#[cfg(unix)]
pub use ipc::{IpcError, IpcServer};
pub use kernel::SecurityKernel;
pub use manager::{ExecApprovalManager, PendingApproval};
pub use masker::SecretMasker;
pub use parser::analyze_shell_command;
pub use risk::{RiskLevel, BLOCKED_PATTERNS};
pub use sanitize::has_invisible_chars;
pub use socket::ApprovalDecisionType;
pub use storage::{ConfigWithHash, ExecApprovalsStorage, StorageError};
