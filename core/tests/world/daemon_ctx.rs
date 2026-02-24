//! Daemon test context
#![allow(dead_code)]

use alephcore::daemon::{
    DaemonConfig, DaemonEvent, DaemonEventBus, GovernorDecision, IpcServer, JsonRpcRequest,
    ResourceGovernor,
};
use alephcore::{
    ProactiveDispatcher, ProactiveDispatcherConfig, WorldModel, WorldModelConfig,
};
#[cfg(target_os = "macos")]
use alephcore::daemon::platforms::launchd::LaunchdService;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;
use tokio::task::JoinHandle;

/// Daemon test context
/// Note: Cannot derive Debug because ResourceGovernor doesn't implement Debug
#[derive(Default)]
pub struct DaemonContext {
    pub event_bus: Option<DaemonEventBus>,
    pub arc_event_bus: Option<Arc<DaemonEventBus>>,
    pub receivers: Vec<Receiver<DaemonEvent>>,
    pub last_events: Vec<DaemonEvent>,
    pub governor: Option<ResourceGovernor>,
    pub governor_decision: Option<Result<GovernorDecision, String>>,
    pub cli_parse_result: Option<Result<(), String>>,

    // IPC context
    pub ipc_server: Option<IpcServer>,
    pub socket_path: Option<String>,
    pub json_rpc_request: Option<JsonRpcRequest>,
    pub json_rpc_json: Option<String>,

    // Launchd context (macOS)
    #[cfg(target_os = "macos")]
    pub launchd_service: Option<LaunchdService>,
    #[cfg(target_os = "macos")]
    pub plist_content: Option<String>,
    pub daemon_config: Option<DaemonConfig>,

    // WorldModel context
    pub worldmodel: Option<Arc<WorldModel>>,
    pub worldmodel_config: Option<WorldModelConfig>,
    pub worldmodel_handle: Option<JoinHandle<()>>,

    // Dispatcher context
    pub dispatcher: Option<Arc<ProactiveDispatcher>>,
    pub dispatcher_config: Option<ProactiveDispatcherConfig>,
    pub dispatcher_handle: Option<JoinHandle<()>>,

    // Persistence testing
    pub persistence_temp_dir: Option<TempDir>,
    pub persistence_state_path: Option<PathBuf>,

    // Test state
    pub received_event: Option<DaemonEvent>,
}


impl std::fmt::Debug for DaemonContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("DaemonContext");
        d.field("event_bus", &self.event_bus)
            .field("arc_event_bus", &self.arc_event_bus.is_some())
            .field("receivers_count", &self.receivers.len())
            .field("last_events", &self.last_events)
            .field("governor", &"<ResourceGovernor>")
            .field("governor_decision", &self.governor_decision)
            .field("cli_parse_result", &self.cli_parse_result)
            .field("socket_path", &self.socket_path)
            .field("ipc_server", &self.ipc_server.as_ref().map(|s| s.socket_path()))
            .field("json_rpc_request", &self.json_rpc_request)
            .field("daemon_config", &self.daemon_config)
            .field("worldmodel", &self.worldmodel.is_some())
            .field("dispatcher", &self.dispatcher.is_some())
            .field("persistence_state_path", &self.persistence_state_path);

        #[cfg(target_os = "macos")]
        d.field("launchd_service", &self.launchd_service.as_ref().map(|_| "<LaunchdService>"))
            .field("plist_content", &self.plist_content.as_ref().map(|_| "<plist>"));

        d.finish()
    }
}
