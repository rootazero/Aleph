// Stub for LaunchdService - will be implemented in Task 3
use crate::daemon::{DaemonConfig, DaemonStatus, Result, ServiceManager, ServiceStatus};
use async_trait::async_trait;

pub struct LaunchdService;

impl LaunchdService {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ServiceManager for LaunchdService {
    async fn install(&self, _config: &DaemonConfig) -> Result<()> {
        unimplemented!("LaunchdService will be implemented in Task 3")
    }

    async fn uninstall(&self) -> Result<()> {
        unimplemented!("LaunchdService will be implemented in Task 3")
    }

    async fn start(&self) -> Result<()> {
        unimplemented!("LaunchdService will be implemented in Task 3")
    }

    async fn stop(&self) -> Result<()> {
        unimplemented!("LaunchdService will be implemented in Task 3")
    }

    async fn status(&self) -> Result<DaemonStatus> {
        unimplemented!("LaunchdService will be implemented in Task 3")
    }

    async fn service_status(&self) -> Result<ServiceStatus> {
        unimplemented!("LaunchdService will be implemented in Task 3")
    }
}
