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

// Integration tests using real PTY processes. May be flaky in constrained CI environments.
#[cfg(test)]
mod tests;
