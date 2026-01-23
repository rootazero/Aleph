//! Agent configuration FFI methods
//!
//! Contains code execution and file operations config:
//! - agent_get_code_exec_config, agent_update_code_exec_config
//! - agent_get_file_ops_config, agent_update_file_ops_config

use crate::ffi::{AetherCore, AetherFfiError};
use tracing::info;

impl AetherCore {
    // ===== CODE EXECUTION CONFIG =====

    /// Get code execution configuration
    pub fn agent_get_code_exec_config(&self) -> crate::ffi::dispatcher_types::CodeExecConfigFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => crate::ffi::dispatcher_types::CodeExecConfigFFI::from(cfg.agent.code_exec),
            Err(_) => crate::ffi::dispatcher_types::CodeExecConfigFFI::default(),
        }
    }

    /// Update code execution configuration
    pub fn agent_update_code_exec_config(
        &self,
        config: crate::ffi::dispatcher_types::CodeExecConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update code_exec section
        full_config.agent.code_exec = config.into();

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!("Code execution configuration updated");
        Ok(())
    }

    // ===== FILE OPERATIONS CONFIG =====

    /// Get file operations configuration
    pub fn agent_get_file_ops_config(&self) -> crate::ffi::dispatcher_types::FileOpsConfigFFI {
        // Load from config file or return defaults
        match crate::config::Config::load() {
            Ok(cfg) => crate::ffi::dispatcher_types::FileOpsConfigFFI::from(cfg.agent.file_ops),
            Err(_) => crate::ffi::dispatcher_types::FileOpsConfigFFI::default(),
        }
    }

    /// Update file operations configuration
    pub fn agent_update_file_ops_config(
        &self,
        config: crate::ffi::dispatcher_types::FileOpsConfigFFI,
    ) -> Result<(), AetherFfiError> {
        // Load current config
        let mut full_config = crate::config::Config::load()
            .map_err(|e| AetherFfiError::Config(format!("Failed to load config: {}", e)))?;

        // Update file_ops section
        full_config.agent.file_ops = config.into();

        // Save to file
        full_config
            .save()
            .map_err(|e| AetherFfiError::Config(format!("Failed to save config: {}", e)))?;

        info!("File operations configuration updated");
        Ok(())
    }
}
