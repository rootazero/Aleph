use crate::daemon::{
    DaemonConfig, DaemonError, DaemonStatus, Result, ServiceManager, ServiceStatus,
};
use async_trait::async_trait;
use plist::{Dictionary, Integer, Value};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::process::Command;

const LAUNCHD_LABEL: &str = "com.aether.daemon";

pub struct LaunchdService {
    plist_path: PathBuf,
}

impl LaunchdService {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let plist_path = PathBuf::from(format!(
            "{}/Library/LaunchAgents/{}.plist",
            home, LAUNCHD_LABEL
        ));

        Self { plist_path }
    }

    pub fn plist_path(&self) -> &Path {
        &self.plist_path
    }

    /// Generate launchd plist content
    pub fn generate_plist(&self, config: &DaemonConfig) -> Result<String> {
        let mut dict = Dictionary::new();

        // Label
        dict.insert(
            "Label".to_string(),
            Value::String(LAUNCHD_LABEL.to_string()),
        );

        // Program arguments
        let program_args = vec![
            Value::String(config.binary_path.to_string_lossy().to_string()),
            Value::String("daemon".to_string()),
            Value::String("run".to_string()),
        ];
        dict.insert("ProgramArguments".to_string(), Value::Array(program_args));

        // Run at load
        dict.insert("RunAtLoad".to_string(), Value::Boolean(true));

        // Keep alive
        dict.insert("KeepAlive".to_string(), Value::Boolean(true));

        // Standard output/error
        let log_dir = config.log_dir.to_string_lossy().to_string();
        dict.insert(
            "StandardOutPath".to_string(),
            Value::String(format!("{}/daemon.log", log_dir)),
        );
        dict.insert(
            "StandardErrorPath".to_string(),
            Value::String(format!("{}/daemon-error.log", log_dir)),
        );

        // Process type (background daemon)
        dict.insert(
            "ProcessType".to_string(),
            Value::String("Background".to_string()),
        );

        // Nice value (priority)
        dict.insert("Nice".to_string(), Value::Integer(Integer::from(config.nice_value)));

        // Resource limits
        let mut soft_limits = Dictionary::new();
        soft_limits.insert(
            "MemoryLimit".to_string(),
            Value::Integer(Integer::from(config.soft_mem_limit as i64)),
        );
        dict.insert(
            "SoftResourceLimits".to_string(),
            Value::Dictionary(soft_limits),
        );

        let mut hard_limits = Dictionary::new();
        hard_limits.insert(
            "MemoryLimit".to_string(),
            Value::Integer(Integer::from(config.hard_mem_limit as i64)),
        );
        dict.insert(
            "HardResourceLimits".to_string(),
            Value::Dictionary(hard_limits),
        );

        // Serialize to XML plist
        let plist_value = Value::Dictionary(dict);
        let mut buf = Vec::new();
        plist::to_writer_xml(&mut buf, &plist_value)
            .map_err(|e| DaemonError::Config(format!("Failed to generate plist: {}", e)))?;

        String::from_utf8(buf)
            .map_err(|e| DaemonError::Config(format!("Invalid UTF-8 in plist: {}", e)))
    }

    /// Check if launchd service is loaded
    async fn is_loaded(&self) -> Result<bool> {
        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()
            .await?;

        Ok(output.status.success())
    }
}

#[async_trait]
impl ServiceManager for LaunchdService {
    async fn install(&self, config: &DaemonConfig) -> Result<()> {
        // Ensure log directory exists
        fs::create_dir_all(&config.log_dir).await?;

        // Generate plist content
        let plist_content = self.generate_plist(config)?;

        // Ensure LaunchAgents directory exists
        if let Some(parent) = self.plist_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write plist file
        fs::write(&self.plist_path, plist_content).await?;

        tracing::info!(
            "Installed launchd service at {}",
            self.plist_path.display()
        );

        Ok(())
    }

    async fn uninstall(&self) -> Result<()> {
        // Stop service if running
        if self.is_loaded().await? {
            self.stop().await?;
        }

        // Remove plist file
        if self.plist_path.exists() {
            fs::remove_file(&self.plist_path).await?;
            tracing::info!("Removed launchd plist at {}", self.plist_path.display());
        }

        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if !self.plist_path.exists() {
            return Err(DaemonError::ServiceError(
                "Service not installed. Run 'aether daemon install' first.".to_string(),
            ));
        }

        // Load service
        let output = Command::new("launchctl")
            .args(["load", self.plist_path.to_str().unwrap()])
            .output()
            .await?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(DaemonError::ServiceError(format!(
                "Failed to start service: {}",
                error
            )));
        }

        tracing::info!("Started launchd service {}", LAUNCHD_LABEL);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if !self.is_loaded().await? {
            return Ok(()); // Already stopped
        }

        // Unload service
        let output = Command::new("launchctl")
            .args(["unload", self.plist_path.to_str().unwrap()])
            .output()
            .await?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(DaemonError::ServiceError(format!(
                "Failed to stop service: {}",
                error
            )));
        }

        tracing::info!("Stopped launchd service {}", LAUNCHD_LABEL);
        Ok(())
    }

    async fn status(&self) -> Result<DaemonStatus> {
        if !self.is_loaded().await? {
            return Ok(DaemonStatus::Stopped);
        }

        // Check if process is actually running
        let output = Command::new("launchctl")
            .args(["list", LAUNCHD_LABEL])
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse PID from output (line like: "PID    Status  Label")
            if stdout.contains("PID") || stdout.contains(LAUNCHD_LABEL) {
                return Ok(DaemonStatus::Running);
            }
        }

        Ok(DaemonStatus::Unknown)
    }

    async fn service_status(&self) -> Result<ServiceStatus> {
        if self.plist_path.exists() {
            Ok(ServiceStatus::Installed)
        } else {
            Ok(ServiceStatus::NotInstalled)
        }
    }
}
