//! Stop-hook configuration — Phase 6b Task 10.
//!
//! Each entry runs an external shell command before the agent loop is
//! allowed to terminate. Exit-code semantics:
//! * 0 — allow stop
//! * 2 — block stop, stdout becomes the block reason
//! * other — hook error (logged, non-blocking)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One stop-hook entry from `config.toml [[stop_hooks]]`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StopHookConfig {
    /// Human-readable hook name (appears in logs and block reasons).
    pub name: String,
    /// Shell command executed via `/bin/sh -c <command>`.
    pub command: String,
    /// Per-hook timeout in seconds. Defaults to 30 if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}
