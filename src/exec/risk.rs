//! Risk level for command execution.
//!
//! Only `Blocked` / `Danger` are produced today (by
//! [`crate::exec::SecurityKernel::assess_custom`] over the user's `[security]`
//! custom patterns). The catastrophic built-in floor lives in
//! [`crate::sandbox::command_policy`], not here.

/// Risk level for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Danger: the operator's `custom_danger` pattern matched — **advisory
    /// only**.
    ///
    /// The command still runs, with a warning; this level does NOT require
    /// approval. [`Blocked`] is the variant that vetoes.
    Danger,
    /// Blocked: Absolutely forbidden - immediate rejection
    Blocked,
}
