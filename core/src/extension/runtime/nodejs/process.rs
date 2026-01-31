//! Node.js subprocess management

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::ipc::{JsonRpcRequest, JsonRpcResponse};
use crate::extension::error::ExtensionError;

/// Default timeout for IPC calls (30 seconds)
pub const DEFAULT_IPC_TIMEOUT: Duration = Duration::from_secs(30);

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID
fn next_request_id() -> String {
    format!("req-{}", REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Node.js plugin host process
pub struct NodeProcess {
    child: Child,
    plugin_id: String,
    /// Timeout for IPC calls
    timeout: Duration,
}

impl NodeProcess {
    /// Start a new Node.js plugin host process with default timeout
    pub fn start(
        node_path: &str,
        host_script: &str,
        plugin_path: &str,
        plugin_id: &str,
    ) -> Result<Self, ExtensionError> {
        Self::start_with_timeout(node_path, host_script, plugin_path, plugin_id, DEFAULT_IPC_TIMEOUT)
    }

    /// Start a new Node.js plugin host process with custom timeout
    pub fn start_with_timeout(
        node_path: &str,
        host_script: &str,
        plugin_path: &str,
        plugin_id: &str,
        timeout: Duration,
    ) -> Result<Self, ExtensionError> {
        info!("Starting Node.js plugin host for: {}", plugin_id);

        let child = Command::new(node_path)
            .arg(host_script)
            .arg(plugin_path)
            .arg(plugin_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ExtensionError::Runtime(format!("Failed to start Node.js process: {}", e)))?;

        Ok(Self {
            child,
            plugin_id: plugin_id.to_string(),
            timeout,
        })
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
        let plugin_id = self.plugin_id.clone();

        // Spawn thread to read response
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut response_line = String::new();
            let result = reader.read_line(&mut response_line);
            // Send both the result and the stdout back
            let _ = tx.send((result, response_line, reader.into_inner()));
        });

        // Wait for response with timeout
        let (read_result, response_line, stdout) = rx
            .recv_timeout(timeout)
            .map_err(|_| ExtensionError::Runtime(format!(
                "Plugin '{}' timed out after {:?}",
                plugin_id, timeout
            )))?;

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

    /// Send shutdown signal
    pub fn shutdown(&mut self) -> Result<(), ExtensionError> {
        info!("Shutting down Node.js plugin: {}", self.plugin_id);

        let _ = self.call("shutdown", serde_json::json!({}));

        // Wait briefly for graceful shutdown
        std::thread::sleep(Duration::from_millis(100));

        // Force kill if still running
        if let Err(e) = self.child.kill() {
            warn!("Failed to kill Node.js process: {}", e);
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
    fn test_node_process_start_invalid_node() {
        let result = NodeProcess::start(
            "/nonexistent/node",
            "/nonexistent/host.js",
            "/nonexistent/plugin",
            "test-plugin",
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
            "/nonexistent/host.js",
            "/nonexistent/plugin",
            "test-plugin",
            custom_timeout,
        );
        assert!(result.is_err());
    }
}
