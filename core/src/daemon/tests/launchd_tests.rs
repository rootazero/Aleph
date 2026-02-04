#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use crate::daemon::*;
    use crate::daemon::platforms::launchd::LaunchdService;

    #[tokio::test]
    async fn test_launchd_service_creation() {
        let service = LaunchdService::new();
        assert!(service.plist_path().to_string_lossy().contains("LaunchAgents"));
    }

    #[tokio::test]
    async fn test_launchd_generate_plist() {
        let service = LaunchdService::new();
        let config = DaemonConfig::default();
        let plist = service.generate_plist(&config).unwrap();
        assert!(plist.contains("com.aether.daemon"));
        assert!(plist.contains("RunAtLoad"));
        assert!(plist.contains("KeepAlive"));
    }
}
