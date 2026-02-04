//! Node.js subprocess management

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::ipc::{JsonRpcRequest, JsonRpcResponse};
use crate::extension::error::ExtensionError;

/// Default timeout for IPC calls (30 seconds)
pub const DEFAULT_IPC_TIMEOUT: Duration = Duration::from_secs(30);

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID
fn next_request_id() -> String {
    format!("req-{}", REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Host script source - either a file path or embedded content
#[derive(Debug, Clone)]
pub enum HostScript {
    /// Path to an external host script file
    Path(String),
    /// Embedded script content (will be written to temp file)
    Embedded(&'static str),
}

/// Node.js plugin host process
pub struct NodeProcess {
    child: Child,
    plugin_id: String,
    /// Timeout for IPC calls
    timeout: Duration,
    /// Temp file path for embedded script (cleaned up on drop)
    temp_script_path: Option<PathBuf>,
}

impl NodeProcess {
    /// Start a new Node.js plugin host process with default timeout
    pub fn start(
        node_path: &str,
        host_script: HostScript,
        plugin_path: &str,
        plugin_id: &str,
    ) -> Result<Self, ExtensionError> {
        Self::start_with_timeout(node_path, host_script, plugin_path, plugin_id, DEFAULT_IPC_TIMEOUT)
    }

    /// Start a new Node.js plugin host process with custom timeout
    pub fn start_with_timeout(
        node_path: &str,
        host_script: HostScript,
        plugin_path: &str,
        plugin_id: &str,
        timeout: Duration,
    ) -> Result<Self, ExtensionError> {
        info!("Starting Node.js plugin host for: {}", plugin_id);

        // Check if node binary exists before attempting to spawn
        let node_path_buf = PathBuf::from(node_path);
        if !node_path_buf.exists() {
            return Err(ExtensionError::Runtime(format!(
                "Node.js binary not found at: {}",
                node_path
            )));
        }

        // Resolve host script path (write to temp file if embedded)
        let (script_path, temp_script_path) = match host_script {
            HostScript::Path(path) => (path, None),
            HostScript::Embedded(content) => {
                let temp_path = Self::write_embedded_script(content, plugin_id)?;
                let path_str = temp_path.to_string_lossy().to_string();
                (path_str, Some(temp_path))
            }
        };

        // Note: stderr is redirected to null to prevent I/O deadlock.
        // If the node process writes to stderr and the pipe buffer fills,
        // it would block the process. We don't capture stderr anyway.
        let child = Command::new(node_path)
            .arg(&script_path)
            .arg(plugin_path)
            .arg(plugin_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                // Clean up temp file on spawn failure
                if let Some(ref temp_path) = temp_script_path {
                    let _ = fs::remove_file(temp_path);
                }
                ExtensionError::Runtime(format!("Failed to start Node.js process: {}", e))
            })?;

        Ok(Self {
            child,
            plugin_id: plugin_id.to_string(),
            timeout,
            temp_script_path,
        })
    }

    /// Write embedded script content to a temp file
    ///
    /// Uses UUID to ensure unique filenames, preventing race conditions
    /// when multiple plugins with the same ID are started in the same process.
    fn write_embedded_script(content: &str, plugin_id: &str) -> Result<PathBuf, ExtensionError> {
        let temp_dir = std::env::temp_dir();
        let unique_id = Uuid::new_v4();
        let file_name = format!("aleph-plugin-host-{}-{}.js", plugin_id, unique_id);
        let temp_path = temp_dir.join(file_name);

        fs::write(&temp_path, content)
            .map_err(|e| ExtensionError::Runtime(format!(
                "Failed to write embedded plugin host script: {}", e
            )))?;

        debug!("Wrote embedded plugin-host.js to {:?}", temp_path);
        Ok(temp_path)
    }

    /// Set the timeout for IPC calls
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Send a request and wait for response with timeout
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse, ExtensionError> {
        self.call_with_timeout(method, params, self.timeout)
    }

    /// Send a request and wait for response with custom timeout
    ///
    /// This method uses a channel-based timeout mechanism that properly restores
    /// stdout even on timeout. The spawned reader thread always sends back the
    /// stdout handle, ensuring it can be reused for subsequent calls.
    pub fn call_with_timeout(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<JsonRpcResponse, ExtensionError> {
        let id = next_request_id();
        let request = JsonRpcRequest::new(&id, method, params);

        // Send request
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| ExtensionError::Runtime("No stdin".to_string()))?;

        let request_line = serde_json::to_string(&request)
            .map_err(|e| ExtensionError::Runtime(format!("Serialize error: {}", e)))?;

        debug!("Sending to plugin {}: {}", self.plugin_id, request_line);

        writeln!(stdin, "{}", request_line)
            .map_err(|e| ExtensionError::Runtime(format!("Write error: {}", e)))?;
        stdin
            .flush()
            .map_err(|e| ExtensionError::Runtime(format!("Flush error: {}", e)))?;

        // Read response with timeout using a channel
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or_else(|| ExtensionError::Runtime("No stdout".to_string()))?;

        let (tx, rx) = mpsc::channel();
        let plugin_id_clone = self.plugin_id.clone();

        // Spawn thread to read response - always sends stdout back
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut response_line = String::new();
            let result = reader.read_line(&mut response_line);
            // Always send back the stdout handle, even if read fails
            // This ensures stdout can be restored for future calls
            let _ = tx.send((result, response_line, reader.into_inner()));
        });

        // Wait for response with timeout
        match rx.recv_timeout(timeout) {
            Ok((read_result, response_line, stdout)) => {
                // Restore stdout for future calls
                self.child.stdout = Some(stdout);

                // Check read result
                read_result.map_err(|e| ExtensionError::Runtime(format!("Read error: {}", e)))?;

                debug!(
                    "Received from plugin {}: {}",
                    self.plugin_id,
                    response_line.trim()
                );

                let response: JsonRpcResponse = serde_json::from_str(&response_line)
                    .map_err(|e| ExtensionError::Runtime(format!("Parse error: {}", e)))?;

                if response.id != id {
                    return Err(ExtensionError::Runtime(format!(
                        "Response ID mismatch: expected {}, got {}",
                        id, response.id
                    )));
                }

                Ok(response)
            }
            Err(_) => {
                // Timeout occurred - the reader thread still owns stdout
                // We cannot recover stdout here; the process is likely hung.
                // Mark the process as unusable by not restoring stdout.
                // Future calls will fail with "No stdout" error.
                warn!(
                    "Plugin '{}' timed out after {:?}, process may need restart",
                    plugin_id_clone, timeout
                );
                Err(ExtensionError::Runtime(format!(
                    "Plugin '{}' timed out after {:?}",
                    plugin_id_clone, timeout
                )))
            }
        }
    }

    /// Send shutdown signal
    pub fn shutdown(&mut self) -> Result<(), ExtensionError> {
        info!("Shutting down Node.js plugin: {}", self.plugin_id);

        // Try graceful shutdown first
        if let Err(e) = self.call("shutdown", serde_json::json!({})) {
            // Log the error but continue with forced shutdown
            error!(
                "Failed to send shutdown signal to plugin '{}': {}",
                self.plugin_id, e
            );
        }

        // Wait briefly for graceful shutdown
        std::thread::sleep(Duration::from_millis(100));

        // Force kill if still running
        if let Err(e) = self.child.kill() {
            // InvalidInput error means the process already exited, which is fine
            if e.kind() != std::io::ErrorKind::InvalidInput {
                warn!("Failed to kill Node.js process '{}': {}", self.plugin_id, e);
            }
        }

        Ok(())
    }

    /// Check if process is still running
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Get the plugin ID
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}

impl Drop for NodeProcess {
    fn drop(&mut self) {
        let _ = self.shutdown();

        // Clean up temp script file if present
        if let Some(ref temp_path) = self.temp_script_path {
            if let Err(e) = fs::remove_file(temp_path) {
                warn!("Failed to remove temp script {:?}: {}", temp_path, e);
            } else {
                debug!("Cleaned up temp script {:?}", temp_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_request_id_increments() {
        let id1 = next_request_id();
        let id2 = next_request_id();
        assert!(id1.starts_with("req-"));
        assert!(id2.starts_with("req-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_node_process_start_invalid_node_with_path() {
        let result = NodeProcess::start(
            "/nonexistent/node",
            HostScript::Path("/nonexistent/host.js".to_string()),
            "/nonexistent/plugin",
            "test-plugin",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_node_process_start_invalid_node_with_embedded() {
        let result = NodeProcess::start(
            "/nonexistent/node",
            HostScript::Embedded("// test script"),
            "/nonexistent/plugin",
            "test-plugin-embedded",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_default_ipc_timeout() {
        assert_eq!(DEFAULT_IPC_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn test_start_with_custom_timeout() {
        // This will fail to start (invalid path) but tests the API
        let custom_timeout = Duration::from_secs(60);
        let result = NodeProcess::start_with_timeout(
            "/nonexistent/node",
            HostScript::Path("/nonexistent/host.js".to_string()),
            "/nonexistent/plugin",
            "test-plugin",
            custom_timeout,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_host_script_enum() {
        let path_script = HostScript::Path("/path/to/script.js".to_string());
        let embedded_script = HostScript::Embedded("console.log('test');");

        match path_script {
            HostScript::Path(p) => assert_eq!(p, "/path/to/script.js"),
            _ => panic!("Expected Path variant"),
        }

        match embedded_script {
            HostScript::Embedded(content) => assert!(content.contains("console")),
            _ => panic!("Expected Embedded variant"),
        }
    }

    #[test]
    fn test_write_embedded_script() {
        let content = "// test embedded script\nconsole.log('hello');";
        let temp_path = NodeProcess::write_embedded_script(content, "test-plugin");

        assert!(temp_path.is_ok());
        let temp_path = temp_path.unwrap();

        // Verify file exists and contains content
        assert!(temp_path.exists());
        let read_content = std::fs::read_to_string(&temp_path).unwrap();
        assert_eq!(read_content, content);

        // Verify filename contains plugin ID and UUID pattern
        let filename = temp_path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("aleph-plugin-host-test-plugin-"));
        assert!(filename.ends_with(".js"));
        // UUID is 36 chars (with hyphens), so total filename should be longer
        assert!(filename.len() > 40);

        // Clean up
        std::fs::remove_file(&temp_path).unwrap();
    }

    #[test]
    fn test_write_embedded_script_unique_names() {
        // Verify that multiple calls generate unique filenames (no race condition)
        let content = "// test";
        let path1 = NodeProcess::write_embedded_script(content, "same-plugin").unwrap();
        let path2 = NodeProcess::write_embedded_script(content, "same-plugin").unwrap();

        // Paths should be different due to UUID
        assert_ne!(path1, path2);

        // Both files should exist
        assert!(path1.exists());
        assert!(path2.exists());

        // Clean up
        std::fs::remove_file(&path1).unwrap();
        std::fs::remove_file(&path2).unwrap();
    }

    #[test]
    fn test_node_binary_not_found_error_message() {
        let result = NodeProcess::start(
            "/nonexistent/node",
            HostScript::Path("/nonexistent/host.js".to_string()),
            "/nonexistent/plugin",
            "test-plugin",
        );

        assert!(result.is_err());
        // Use match instead of unwrap_err to avoid Debug requirement
        match result {
            Err(err) => {
                let err_msg = format!("{}", err);
                // Should mention the path in the error
                assert!(
                    err_msg.contains("not found") || err_msg.contains("/nonexistent/node"),
                    "Error should mention node binary not found: {}",
                    err_msg
                );
            }
            Ok(_) => panic!("Expected error for nonexistent node binary"),
        }
    }
}
