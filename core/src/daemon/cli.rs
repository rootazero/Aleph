use crate::daemon::{
    create_service_manager, DaemonConfig, DaemonEventBus, PerceptionConfig, Result,
    WatcherRegistry,
};
use crate::daemon::perception::watchers::{FSEventWatcher, ProcessWatcher, SystemStateWatcher, TimeWatcher};
use clap::{Parser, Subcommand};
use crate::sync_primitives::Arc;
use tracing::{error, info};

#[derive(Debug, Parser)]
#[command(name = "daemon")]
#[command(about = "Manage Aleph daemon service")]
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
        info!("Installing Aleph daemon service...");

        let service = create_service_manager()?;
        let config = DaemonConfig::default();

        service.install(&config).await?;

        info!("✓ Daemon service installed successfully");
        info!("  Run 'aleph daemon start' to start the service");

        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        info!("Uninstalling Aleph daemon service...");

        let service = create_service_manager()?;
        service.uninstall().await?;

        info!("✓ Daemon service uninstalled successfully");

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        info!("Starting Aleph daemon service...");

        let service = create_service_manager()?;
        service.start().await?;

        info!("✓ Daemon service started successfully");

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("Stopping Aleph daemon service...");

        let service = create_service_manager()?;
        service.stop().await?;

        info!("✓ Daemon service stopped successfully");

        Ok(())
    }

    async fn status(&self) -> Result<()> {
        let service = create_service_manager()?;

        let service_status = service.service_status().await?;
        let daemon_status = service.status().await?;

        info!("Aleph Daemon Status:");
        info!("  Service: {:?}", service_status);
        info!("  Daemon:  {:?}", daemon_status);

        Ok(())
    }

    async fn run(&self) -> Result<()> {
        use crate::daemon::ipc::IpcServer;
        use crate::daemon::dispatcher::{Dispatcher, DispatcherConfig};
        use crate::daemon::worldmodel::{WorldModel, WorldModelConfig};

        info!("Starting Aleph daemon with Perception Layer, WorldModel, and Dispatcher...");

        // 1. Load configurations
        let config = DaemonConfig::default();
        let mut perception_config = PerceptionConfig::load()?;
        perception_config.expand_paths()?;

        // 2. Create EventBus
        let event_bus = Arc::new(DaemonEventBus::new(1000));

        // 3. Create and register Watchers (Perception Layer)
        let mut registry = WatcherRegistry::new();

        if perception_config.enabled {
            if perception_config.process.enabled {
                registry.register(Arc::new(ProcessWatcher::new(
                    perception_config.process.clone(),
                )));
            }

            if perception_config.filesystem.enabled {
                registry.register(Arc::new(FSEventWatcher::new(
                    perception_config.filesystem.clone(),
                )));
            }

            if perception_config.time.enabled {
                registry.register(Arc::new(TimeWatcher::new(
                    perception_config.time.clone(),
                )));
            }

            if perception_config.system.enabled {
                registry.register(Arc::new(SystemStateWatcher::new(
                    perception_config.system.clone(),
                )));
            }

            info!("Registered {} watchers", registry.watcher_count());

            // 4. Start all Watchers
            registry.start_all(event_bus.clone()).await?;
            info!("All watchers started");
        } else {
            info!("Perception layer disabled in configuration");
        }

        // 5. Create and start WorldModel (Phase 3)
        info!("Initializing WorldModel...");
        let worldmodel_config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(worldmodel_config, event_bus.clone()).await?);

        let worldmodel_clone = worldmodel.clone();
        let worldmodel_handle = tokio::spawn(async move {
            if let Err(e) = worldmodel_clone.run().await {
                error!("WorldModel error: {}", e);
            }
        });
        info!("WorldModel started");

        // 6. Create and start Dispatcher (Phase 4)
        info!("Initializing Dispatcher...");
        let dispatcher_config = DispatcherConfig::default();
        let dispatcher = Dispatcher::new(dispatcher_config, worldmodel.clone(), event_bus.clone());

        let dispatcher_clone = dispatcher.clone();
        let dispatcher_handle = tokio::spawn(async move {
            if let Err(e) = dispatcher_clone.run().await {
                error!("Dispatcher error: {}", e);
            }
        });
        info!("Dispatcher started");

        // 7. Start IPC Server
        let server = IpcServer::new(config.socket_path.clone());
        let server_handle = tokio::spawn(async move {
            if let Err(e) = server.start().await {
                error!("IPC server error: {}", e);
            }
        });

        // 8. Wait for Ctrl+C
        tokio::signal::ctrl_c().await?;

        // 9. Graceful shutdown
        info!("Shutting down daemon...");
        registry.shutdown_all().await?;
        worldmodel_handle.abort();
        dispatcher_handle.abort();
        server_handle.abort();
        info!("Daemon stopped");

        Ok(())
    }
}
