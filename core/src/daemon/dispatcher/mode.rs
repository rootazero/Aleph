//! Dispatcher Mode
//!
//! Defines the operational mode of the Dispatcher.

use chrono::{DateTime, Utc};
use crate::daemon::worldmodel::state::PendingAction;

/// Operational mode of the Dispatcher
#[derive(Debug, Clone, PartialEq)]
pub enum DispatcherMode {
    /// Normal running mode
    Running,

    /// Reconciliation mode - waiting for user approval on high-risk actions
    Reconciling {
        /// High-risk pending actions awaiting approval
        pending_high_risk: Vec<PendingAction>,
        /// When reconciliation mode was entered
        started_at: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispatcher_mode_running() {
        let mode = DispatcherMode::Running;
        assert!(matches!(mode, DispatcherMode::Running));
    }

    #[test]
    fn test_dispatcher_mode_reconciling() {
        let mode = DispatcherMode::Reconciling {
            pending_high_risk: vec![],
            started_at: chrono::Utc::now(),
        };
        assert!(matches!(mode, DispatcherMode::Reconciling { .. }));
    }
}
