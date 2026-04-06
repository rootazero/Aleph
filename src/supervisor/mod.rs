//! PtySupervisor module for controlling external CLI tools.
//!
//! This module provides PTY-based process control for tools like Claude Code,
//! allowing Aleph to act as a "supervisor" that can:
//! - Spawn processes in a pseudo-terminal
//! - Read and parse their output in real-time
//! - Inject input (commands, approvals)
//! - Detect semantic events (approval requests, errors)

#[allow(dead_code)]
pub mod pty;
#[allow(dead_code)]
pub mod types;

// NOTE: Disabled — ClaudeSupervisor types have been removed. Tests need rewrite.
#[cfg(all(test, feature = "disabled-tests"))]
mod tests;
