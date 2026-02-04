mod service_manager_tests;

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
