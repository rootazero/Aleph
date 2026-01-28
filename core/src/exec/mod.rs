//! Command execution security module.
//!
//! Provides secure shell command execution with:
//! - Three-level security model (deny/allowlist/full)
//! - Quote-aware shell command parsing
//! - Allowlist pattern matching
//! - User approval via socket protocol

pub mod config;

pub use config::{
    AgentExecConfig, AllowlistEntry, ExecAsk, ExecApprovalsFile, ExecDefaults, ExecSecurity,
    SocketConfig,
};
