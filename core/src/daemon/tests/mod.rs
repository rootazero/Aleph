// Core daemon tests have been migrated to BDD:
// - event_tests, cli_tests, service_manager_tests, resource_governor_tests -> core.feature
// - ipc_tests -> ipc.feature
// - launchd_tests -> launchd.feature
// See: core/tests/features/daemon/

mod perception_integration;
#[cfg(target_os = "macos")]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_daemon_module_exists() {
        // This test ensures the daemon module is properly declared
        let config = DaemonConfig::default();
        assert_eq!(config.socket_path, "~/.aleph/daemon.sock");
    }
}
