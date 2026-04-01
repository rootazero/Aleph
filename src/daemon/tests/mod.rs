// Core daemon tests have been migrated to BDD:
// - event_tests, cli_tests, service_manager_tests, resource_governor_tests -> core.feature
// - ipc_tests -> ipc.feature
// - launchd_tests -> launchd.feature
// See: core/tests/features/daemon/

#[cfg(target_os = "macos")]
mod integration_tests;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::*;

    #[test]
    fn test_daemon_module_exists() {
        // This test ensures the daemon module is properly declared
        let config = DaemonConfig::default();
        // After tilde expansion fix, socket_path uses actual home directory
        assert!(
            config.socket_path.ends_with(".aleph/daemon.sock"),
            "socket_path should end with .aleph/daemon.sock, got: {}",
            config.socket_path
        );
    }
}
