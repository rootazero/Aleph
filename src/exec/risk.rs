//! Risk level for command execution.
//!
//! Only `Blocked` / `Danger` are produced today (by
//! [`crate::exec::SecurityKernel::assess_custom`] over the user's `[security]`
//! custom patterns). The catastrophic built-in floor lives in
//! [`crate::sandbox::command_policy`], not here.

/// Risk level for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Danger: Destructive operations - requires explicit approval
    Danger,
    /// Blocked: Absolutely forbidden - immediate rejection
    Blocked,
}
