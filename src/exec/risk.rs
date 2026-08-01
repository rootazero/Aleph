//! Risk level for command execution.
//!
//! Four-tier traffic-light protocol. Only `Blocked` / `Danger` are produced
//! today (by [`crate::exec::SecurityKernel::assess_custom`] over the user's
//! `[security]` custom patterns); `Safe` / `Caution` remain part of the ordered
//! scale. The catastrophic built-in floor lives in
//! [`crate::sandbox::command_policy`], not here.

/// Risk level for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Safe: Read-only operations, silent pass
    Safe,
    /// Caution: Resource consumption, network requests - allowed but logged
    Caution,
    /// Danger: Destructive operations - requires explicit approval
    Danger,
    /// Blocked: Absolutely forbidden - immediate rejection
    Blocked,
}

impl RiskLevel {
    /// Check if this risk level should be blocked
    #[must_use]
    pub const fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::Safe < RiskLevel::Caution);
        assert!(RiskLevel::Caution < RiskLevel::Danger);
        assert!(RiskLevel::Danger < RiskLevel::Blocked);
    }

    #[test]
    fn risk_level_predicates() {
        assert!(RiskLevel::Blocked.is_blocked());
        assert!(!RiskLevel::Danger.is_blocked());
    }
}
