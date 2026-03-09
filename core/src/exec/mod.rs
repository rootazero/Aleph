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
pub mod socket;
pub mod storage;

pub use allowlist::match_allowlist;
pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use bridge::{ApprovalBridge, SentApprovalMessage};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
pub use decision::{
    decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext, DEFAULT_SAFE_BINS,
};
pub use forwarder::{
    ApprovalMessage, ExecApprovalForwarder, ForwardMode, ForwardTarget, ForwarderConfig,
    ForwarderEvent,
};
#[cfg(unix)]
pub use ipc::{IpcClient, IpcConnection, IpcError, IpcMessage, IpcServer, PendingInfo};
pub use kernel::{RiskAssessment, SecurityKernel};
pub use manager::{ExecApprovalManager, ExecApprovalRecord, PendingApproval};
pub use masker::SecretMasker;
pub use parser::{analyze_shell_command, tokenize_segment};
pub use risk::{RiskLevel, BLOCKED_PATTERNS, DANGER_PATTERNS, SAFE_PATTERNS};
// Sandbox re-exports will be enabled as types are implemented
// pub use sandbox::{
//     Capabilities, EnvironmentCapability, ExecutionStatus, FallbackPolicy, FileSystemCapability,
//     NetworkCapability, ProcessCapability, ProfileGenerator, SandboxAdapter, SandboxAuditLog,
//     SandboxCommand, SandboxManager, SandboxProfile, SandboxViolation,
// };
pub use socket::{ApprovalDecisionType, ApprovalRequestPayload, SegmentInfo, SocketMessage};
pub use storage::{ConfigWithHash, ExecApprovalsStorage, StorageError};
