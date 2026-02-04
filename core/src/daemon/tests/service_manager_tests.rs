#[cfg(test)]
mod tests {
    use crate::daemon::*;

    #[tokio::test]
    async fn test_service_manager_trait_exists() {
        // Mock implementation to test trait
        struct MockService;

        #[async_trait::async_trait]
        impl ServiceManager for MockService {
            async fn install(&self, _config: &DaemonConfig) -> Result<()> {
                Ok(())
            }

            async fn uninstall(&self) -> Result<()> {
                Ok(())
            }

            async fn start(&self) -> Result<()> {
                Ok(())
            }

            async fn stop(&self) -> Result<()> {
                Ok(())
            }

            async fn status(&self) -> Result<DaemonStatus> {
                Ok(DaemonStatus::Unknown)
            }

            async fn service_status(&self) -> Result<ServiceStatus> {
                Ok(ServiceStatus::NotInstalled)
            }
        }

        let service: Box<dyn ServiceManager> = Box::new(MockService);
        let result = service.service_status().await;
        assert!(result.is_ok());
    }
}
