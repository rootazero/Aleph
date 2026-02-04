#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    #[ignore] // Run manually: cargo test --lib -- --ignored
    async fn test_daemon_full_lifecycle() {
        // This test requires sudo/admin privileges

        let service = create_service_manager().unwrap();
        let config = DaemonConfig::default();

        // 1. Install
        println!("Installing service...");
        service.install(&config).await.unwrap();

        let status = service.service_status().await.unwrap();
        assert_eq!(status, ServiceStatus::Installed);

        // 2. Start
        println!("Starting service...");
        service.start().await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let status = service.status().await.unwrap();
        assert_eq!(status, DaemonStatus::Running);

        // 3. Stop
        println!("Stopping service...");
        service.stop().await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let status = service.status().await.unwrap();
        assert_eq!(status, DaemonStatus::Stopped);

        // 4. Uninstall
        println!("Uninstalling service...");
        service.uninstall().await.unwrap();

        let status = service.service_status().await.unwrap();
        assert_eq!(status, ServiceStatus::NotInstalled);

        println!("✓ Full lifecycle test passed");
    }
}
