//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod allowlist;
pub mod analysis;
pub mod config;
pub mod parser;

pub use allowlist::match_allowlist;
pub use analysis::{CommandAnalysis, CommandResolution, CommandSegment};
pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    ResolvedExecConfig, SocketConfig,
};
pub use parser::{analyze_shell_command, tokenize_segment};
