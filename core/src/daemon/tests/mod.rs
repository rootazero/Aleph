mod service_manager_tests;
#[cfg(target_os = "macos")]
mod launchd_tests;
mod resource_governor_tests;
mod ipc_tests;
mod cli_tests;
#[cfg(target_os = "macos")]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn test_daemon_module_exists() {
        // This test ensures the daemon module is properly declared
        let config = DaemonConfig::default();
        assert_eq!(config.socket_path, "~/.aether/daemon.sock");
    }
}
