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
pub mod sanitize;
pub mod sandbox;
pub mod socket;
pub mod storage;

pub use allowlist::match_allowlist;
pub use analysis::CommandAnalysis;
pub(crate) use analysis::{CommandResolution, CommandSegment};
pub use bridge::ApprovalBridge;
pub(crate) use bridge::SentApprovalMessage;
pub use config::{ExecAsk, ExecApprovalsFile, ExecSecurity, ResolvedExecConfig};
pub(crate) use config::{AgentExecConfig, AllowlistEntry, ExecDefaults, SocketConfig};
pub use decision::{decide_exec_approval, ApprovalDecision, ApprovalRequest, ExecContext};
pub(crate) use decision::DEFAULT_SAFE_BINS;
pub use forwarder::{ExecApprovalForwarder, ForwardMode, ForwardTarget};
pub(crate) use forwarder::{ApprovalMessage, ForwarderConfig, ForwarderEvent};
#[cfg(unix)]
pub use ipc::{IpcError, IpcServer};
#[cfg(unix)]
pub(crate) use ipc::{IpcClient, IpcConnection, IpcMessage, PendingInfo};
pub use kernel::SecurityKernel;
pub(crate) use kernel::RiskAssessment;
pub use manager::{ExecApprovalManager, PendingApproval};
pub(crate) use manager::ExecApprovalRecord;
pub use masker::SecretMasker;
pub use parser::analyze_shell_command;
pub(crate) use parser::tokenize_segment;
pub use risk::{RiskLevel, BLOCKED_PATTERNS};
pub(crate) use risk::{DANGER_PATTERNS, SAFE_PATTERNS};
pub use sanitize::has_invisible_chars;
pub(crate) use sanitize::sanitize_display_text;
pub use socket::ApprovalDecisionType;
pub(crate) use socket::{ApprovalRequestPayload, SegmentInfo, SocketMessage};
pub use storage::{ConfigWithHash, ExecApprovalsStorage, StorageError};
