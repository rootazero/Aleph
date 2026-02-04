// core/src/providers/protocols/loader.rs

//! Protocol loader for YAML-based protocols

use crate::error::{AetherError, Result};
use crate::providers::protocols::{ConfigurableProtocol, ProtocolDefinition, ProtocolRegistry};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// Protocol loader manages loading protocols from YAML files
pub struct ProtocolLoader;

impl ProtocolLoader {
    /// Load a protocol from YAML file
    pub async fn load_from_file(path: &Path) -> Result<()> {
        // Read YAML file
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            AetherError::invalid_config(format!("Failed to read protocol file {:?}: {}", path, e))
        })?;

        // Parse as ProtocolDefinition
        let def: ProtocolDefinition = serde_yaml::from_str(&content).map_err(|e| {
            AetherError::invalid_config(format!("Failed to parse protocol YAML {:?}: {}", path, e))
        })?;

        // Create ConfigurableProtocol
        let protocol = ConfigurableProtocol::new(def.clone(), reqwest::Client::new())?;

        // Register in ProtocolRegistry
        ProtocolRegistry::global().register(def.name.clone(), Arc::new(protocol))?;

        info!(
            protocol_name = %def.name,
            path = ?path,
            "Successfully loaded protocol from file"
        );

        Ok(())
    }

    /// Load all protocols from directory
    pub async fn load_from_dir(dir: &Path) -> Result<()> {
        // Check if directory exists
        if !dir.exists() {
            return Err(AetherError::invalid_config(format!(
                "Protocol directory does not exist: {:?}",
                dir
            )));
        }

        if !dir.is_dir() {
            return Err(AetherError::invalid_config(format!(
                "Path is not a directory: {:?}",
                dir
            )));
        }

        // Read directory entries
        let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| {
            AetherError::invalid_config(format!("Failed to read directory {:?}: {}", dir, e))
        })?;

        let mut loaded_count = 0;
        let mut error_count = 0;

        // Process each entry
        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            AetherError::invalid_config(format!("Failed to read directory entry: {}", e))
        })? {
            let path = entry.path();

            // Check if it's a YAML file
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "yaml" || ext == "yml" {
                        // Attempt to load the file, but don't fail the entire directory load on error
                        match Self::load_from_file(&path).await {
                            Ok(()) => {
                                loaded_count += 1;
                            }
                            Err(e) => {
                                error!(
                                    path = ?path,
                                    error = %e,
                                    "Failed to load protocol file, continuing with other files"
                                );
                                error_count += 1;
                            }
                        }
                    }
                }
            }
        }

        info!(
            dir = ?dir,
            loaded = loaded_count,
            errors = error_count,
            "Finished loading protocols from directory"
        );

        Ok(())
    }

    /// Start hot reload watcher for ~/.aether/protocols
    pub fn start_watching() -> Result<Option<RecommendedWatcher>> {
        // Get ~/.aether/protocols path
        let home = std::env::var("HOME").map_err(|_| {
            AetherError::invalid_config("HOME environment variable not set".to_string())
        })?;
        let protocols_dir = PathBuf::from(home).join(".aether").join("protocols");

        // Check if directory exists
        if !protocols_dir.exists() {
            info!(
                dir = ?protocols_dir,
                "Protocols directory does not exist, skipping hot reload"
            );
            return Ok(None);
        }

        info!(
            dir = ?protocols_dir,
            "Starting hot reload watcher for protocols directory"
        );

        Self::start_watching_dir(&protocols_dir)
    }

    /// Start watching a specific directory for protocol file changes
    fn start_watching_dir(dir: &Path) -> Result<Option<RecommendedWatcher>> {
        let dir = dir.to_path_buf();
        let dir_for_closure = dir.clone();

        // Create watcher with 2-second poll interval
        let config = Config::default().with_poll_interval(Duration::from_secs(2));

        let mut watcher = RecommendedWatcher::new(
            move |event_result| {
                if let Ok(event) = event_result {
                    Self::handle_fs_event(event, &dir_for_closure);
                }
            },
            config,
        )
        .map_err(|e| AetherError::invalid_config(format!("Failed to create watcher: {}", e)))?;

        // Watch directory non-recursively
        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(|e| {
                AetherError::invalid_config(format!("Failed to watch directory {:?}: {}", dir, e))
            })?;

        info!(
            dir = ?dir,
            "Successfully started watching protocols directory"
        );

        Ok(Some(watcher))
    }

    /// Handle file system event
    fn handle_fs_event(event: Event, dir: &Path) {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                // Handle Create and Modify events
                for path in event.paths {
                    // Check if it's a YAML file
                    if let Some(ext) = path.extension() {
                        if ext == "yaml" || ext == "yml" {
                            if path.starts_with(dir) {
                                info!(
                                    path = ?path,
                                    event = ?event.kind,
                                    "Protocol file changed, reloading"
                                );
                                Self::reload_protocol(&path);
                            }
                        }
                    }
                }
            }
            EventKind::Remove(_) => {
                // Handle Remove events
                for path in event.paths {
                    // Check if it's a YAML file
                    if let Some(ext) = path.extension() {
                        if ext == "yaml" || ext == "yml" {
                            if path.starts_with(dir) {
                                info!(
                                    path = ?path,
                                    "Protocol file removed, unregistering"
                                );
                                Self::unregister_protocol(&path);
                            }
                        }
                    }
                }
            }
            _ => {
                // Ignore other events
            }
        }
    }

    /// Reload a protocol from file
    fn reload_protocol(path: &Path) {
        let path = path.to_path_buf();
        // Spawn async task to reload protocol
        tokio::spawn(async move {
            if let Err(e) = Self::load_from_file(&path).await {
                error!(
                    path = ?path,
                    error = %e,
                    "Failed to reload protocol"
                );
            }
        });
    }

    /// Unregister a protocol based on file path
    fn unregister_protocol(path: &Path) {
        // Extract protocol name from filename (without extension)
        if let Some(file_stem) = path.file_stem() {
            if let Some(name) = file_stem.to_str() {
                info!(
                    protocol_name = %name,
                    path = ?path,
                    "Unregistering protocol"
                );
                ProtocolRegistry::global().unregister(name);
            } else {
                warn!(
                    path = ?path,
                    "Failed to extract protocol name from file path"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_from_file() {
        // Register built-in protocols first (needed for 'extends')
        ProtocolRegistry::global().register_builtin();

        // Create a temporary YAML file
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test-protocol.yaml");

        let yaml_content = r#"
name: test-openai
extends: openai
base_url: https://api.test.com
differences:
  auth:
    header: X-API-Key
    prefix: "Bearer "
"#;

        tokio::fs::write(&file_path, yaml_content)
            .await
            .expect("Failed to write test YAML file");

        // Load the protocol
        ProtocolLoader::load_from_file(&file_path)
            .await
            .expect("Should load protocol from file");

        // Verify it's in the registry
        let protocol = ProtocolRegistry::global().get("test-openai");
        assert!(protocol.is_some(), "Protocol should be registered");
        assert_eq!(protocol.unwrap().name(), "test-openai");
    }

    #[tokio::test]
    async fn test_load_from_file_custom_protocol() {
        // Create a temporary YAML file with custom protocol
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("custom-protocol.yaml");

        let yaml_content = r#"
name: custom-api
base_url: https://api.custom.com
custom:
  auth:
    type: header
    header: Authorization
    prefix: "Bearer "
  endpoints:
    chat: /v1/chat
  request_template: '{"model": "{{config.model}}", "messages": [{"role": "user", "content": "{{input}}"}]}'
  response_mapping:
    content: "$.choices[0].message.content"
"#;

        tokio::fs::write(&file_path, yaml_content)
            .await
            .expect("Failed to write test YAML file");

        // Load the protocol
        ProtocolLoader::load_from_file(&file_path)
            .await
            .expect("Should load custom protocol from file");

        // Verify it's in the registry
        let protocol = ProtocolRegistry::global().get("custom-api");
        assert!(protocol.is_some(), "Custom protocol should be registered");
        assert_eq!(protocol.unwrap().name(), "custom-api");
    }

    #[tokio::test]
    async fn test_load_from_dir() {
        // Register built-in protocols first
        ProtocolRegistry::global().register_builtin();

        // Create a temporary directory with multiple YAML files
        let temp_dir = TempDir::new().unwrap();

        // Create first protocol file
        let file1 = temp_dir.path().join("protocol1.yaml");
        tokio::fs::write(
            &file1,
            r#"
name: dir-test-1
extends: openai
base_url: https://api.test1.com
"#,
        )
        .await
        .unwrap();

        // Create second protocol file
        let file2 = temp_dir.path().join("protocol2.yaml");
        tokio::fs::write(
            &file2,
            r#"
name: dir-test-2
extends: anthropic
base_url: https://api.test2.com
"#,
        )
        .await
        .unwrap();

        // Create a non-YAML file (should be ignored)
        let file3 = temp_dir.path().join("readme.txt");
        tokio::fs::write(&file3, "This is not a YAML file").await.unwrap();

        // Load all protocols from directory
        ProtocolLoader::load_from_dir(temp_dir.path())
            .await
            .expect("Should load protocols from directory");

        // Verify both protocols are registered
        assert!(
            ProtocolRegistry::global().get("dir-test-1").is_some(),
            "First protocol should be registered"
        );
        assert!(
            ProtocolRegistry::global().get("dir-test-2").is_some(),
            "Second protocol should be registered"
        );
    }

    #[tokio::test]
    async fn test_load_from_dir_with_errors() {
        // Create a temporary directory with valid and invalid files
        let temp_dir = TempDir::new().unwrap();

        // Create a valid protocol file
        let valid_file = temp_dir.path().join("valid.yaml");
        tokio::fs::write(
            &valid_file,
            r#"
name: valid-protocol
extends: openai
base_url: https://api.valid.com
"#,
        )
        .await
        .unwrap();

        // Create an invalid YAML file (should log error but not fail)
        let invalid_file = temp_dir.path().join("invalid.yaml");
        tokio::fs::write(&invalid_file, "invalid: yaml: content: [[[").await.unwrap();

        // Load from directory (should succeed for valid file, log error for invalid)
        let result = ProtocolLoader::load_from_dir(temp_dir.path()).await;
        assert!(result.is_ok(), "Should succeed despite invalid file");

        // Verify valid protocol was loaded
        assert!(
            ProtocolRegistry::global().get("valid-protocol").is_some(),
            "Valid protocol should be registered"
        );
    }

    #[tokio::test]
    async fn test_load_from_nonexistent_dir() {
        let nonexistent_dir = Path::new("/nonexistent/directory");
        let result = ProtocolLoader::load_from_dir(nonexistent_dir).await;
        assert!(result.is_err(), "Should fail for nonexistent directory");
    }

    #[tokio::test]
    async fn test_hot_reload() {
        // Register built-in protocols first
        ProtocolRegistry::global().register_builtin();

        // Create a temporary directory
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("hot-reload-test.yaml");

        // Write initial protocol
        let initial_yaml = r#"
name: hot-reload-test
extends: openai
base_url: https://api.initial.com
"#;
        tokio::fs::write(&file_path, initial_yaml)
            .await
            .expect("Failed to write initial YAML file");

        // Load initial protocol
        ProtocolLoader::load_from_file(&file_path)
            .await
            .expect("Should load initial protocol");

        // Verify initial protocol is loaded
        assert!(
            ProtocolRegistry::global().get("hot-reload-test").is_some(),
            "Initial protocol should be registered"
        );

        // Start watching the directory
        let _watcher = ProtocolLoader::start_watching_dir(temp_dir.path())
            .expect("Should start watching")
            .expect("Watcher should be created");

        // Modify the file
        let modified_yaml = r#"
name: hot-reload-test
extends: openai
base_url: https://api.modified.com
differences:
  auth:
    header: X-Modified-Key
"#;
        tokio::fs::write(&file_path, modified_yaml)
            .await
            .expect("Failed to write modified YAML file");

        // Wait for file system event to be processed
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Note: Since hot reload is async and runs in a separate task,
        // we can't easily verify the reload in this test without more complex
        // synchronization. The test mainly verifies that:
        // 1. Watcher can be created
        // 2. File modifications don't crash
        // 3. Logs are generated (check manually when running tests)
        //
        // In a real application, the watcher must be kept alive by the caller.
        info!("Hot reload test completed - check logs for reload events");
    }
}
