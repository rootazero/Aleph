//! Daemon test context

use alephcore::daemon::{DaemonEvent, DaemonEventBus, GovernorDecision, ResourceGovernor};
use tokio::sync::broadcast::Receiver;

/// Daemon test context
/// Note: Cannot derive Debug because ResourceGovernor doesn't implement Debug
#[derive(Default)]
pub struct DaemonContext {
    pub event_bus: Option<DaemonEventBus>,
    pub receivers: Vec<Receiver<DaemonEvent>>,
    pub last_events: Vec<DaemonEvent>,
    pub governor: Option<ResourceGovernor>,
    pub governor_decision: Option<Result<GovernorDecision, String>>,
    pub cli_parse_result: Option<Result<(), String>>,
}

impl std::fmt::Debug for DaemonContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonContext")
            .field("event_bus", &self.event_bus)
            .field("receivers_count", &self.receivers.len())
            .field("last_events", &self.last_events)
            .field("governor", &"<ResourceGovernor>")
            .field("governor_decision", &self.governor_decision)
            .field("cli_parse_result", &self.cli_parse_result)
            .finish()
    }
}
