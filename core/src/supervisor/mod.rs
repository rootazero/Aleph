//! PtySupervisor module for controlling external CLI tools.
//!
//! This module provides PTY-based process control for tools like Claude Code,
//! allowing Aether to act as a "supervisor" that can:
//! - Spawn processes in a pseudo-terminal
//! - Read and parse their output in real-time
//! - Inject input (commands, approvals)
//! - Detect semantic events (approval requests, errors)

pub mod pty;
pub mod types;

pub use pty::ClaudeSupervisor;
pub use types::{PtySize, SupervisorConfig, SupervisorError, SupervisorEvent};
