//! Node.js subprocess management

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{debug, info, warn};

use super::ipc::{JsonRpcRequest, JsonRpcResponse};
use crate::extension::error::ExtensionError;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID
fn next_request_id() -> String {
    format!("req-{}", REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Node.js plugin host process
pub struct NodeProcess {
    child: Child,
    plugin_id: String,
}

impl NodeProcess {
    /// Start a new Node.js plugin host process
    pub fn start(
        node_path: &str,
        host_script: &str,
        plugin_path: &str,
        plugin_id: &str,
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
        })
    }

    /// Send a request and wait for response
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
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

        // Read response
        let stdout = self
            .child
            .stdout
            .as_mut()
            .ok_or_else(|| ExtensionError::Runtime("No stdout".to_string()))?;

        let mut reader = BufReader::new(stdout);
        let mut response_line = String::new();

        reader
            .read_line(&mut response_line)
            .map_err(|e| ExtensionError::Runtime(format!("Read error: {}", e)))?;

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
}
