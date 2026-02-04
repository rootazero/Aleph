use crate::daemon::{create_service_manager, DaemonConfig, Result};
use clap::{Parser, Subcommand};
use tracing::{error, info};

#[derive(Debug, Parser)]
#[command(name = "daemon")]
#[command(about = "Manage Aether daemon service")]
pub struct DaemonCli {
    #[command(subcommand)]
    pub command: DaemonCommand,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Install daemon as system service
    Install,

    /// Uninstall daemon service
    Uninstall,

    /// Start daemon service
    Start,

    /// Stop daemon service
    Stop,

    /// Check daemon status
    Status,

    /// Run daemon in foreground (for development)
    Run,
}

impl DaemonCli {
    pub async fn execute(&self) -> Result<()> {
        match &self.command {
            DaemonCommand::Install => self.install().await,
            DaemonCommand::Uninstall => self.uninstall().await,
            DaemonCommand::Start => self.start().await,
            DaemonCommand::Stop => self.stop().await,
            DaemonCommand::Status => self.status().await,
            DaemonCommand::Run => self.run().await,
        }
    }

    async fn install(&self) -> Result<()> {
        info!("Installing Aether daemon service...");

        let service = create_service_manager()?;
        let config = DaemonConfig::default();

        service.install(&config).await?;

        info!("✓ Daemon service installed successfully");
        info!("  Run 'aether daemon start' to start the service");

        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        info!("Uninstalling Aether daemon service...");

        let service = create_service_manager()?;
        service.uninstall().await?;

        info!("✓ Daemon service uninstalled successfully");

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        info!("Starting Aether daemon service...");

        let service = create_service_manager()?;
        service.start().await?;

        info!("✓ Daemon service started successfully");

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping Aether daemon service...");

        let service = create_service_manager()?;
        service.stop().await?;

        info!("✓ Daemon service stopped successfully");

        Ok(())
    }

    async fn status(&self) -> Result<()> {
        let service = create_service_manager()?;

        let service_status = service.service_status().await?;
        let daemon_status = service.status().await?;

        info!("Aether Daemon Status:");
        info!("  Service: {:?}", service_status);
        info!("  Daemon:  {:?}", daemon_status);

        Ok(())
    }

    async fn run(&self) -> Result<()> {
        use crate::daemon::ipc::IpcServer;

        info!("Starting Aether daemon in foreground mode...");
        info!("Press Ctrl+C to stop");

        let config = DaemonConfig::default();
        let server = IpcServer::new(config.socket_path);

        // Start IPC server (blocks until Ctrl+C)
        server.start().await?;

        Ok(())
    }
}
