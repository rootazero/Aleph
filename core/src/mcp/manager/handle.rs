//! MCP Manager Handle
//!
//! Public API for interacting with the McpManager actor.
//!
//! The handle provides a thread-safe interface for:
//! - Server lifecycle management (add, remove, start, stop, restart)
//! - Querying server state and capabilities
//! - Aggregating tools, resources, and prompts across all servers
//! - Subscribing to manager events

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};

use super::types::{
    McpCommand, McpManagerConfig, McpManagerEvent, McpServerInfo, McpServerStatusDetail,
};
use crate::error::{AlephError, Result};
use crate::mcp::{McpClient, McpPrompt, McpResource, McpTool};

/// Handle for interacting with the MCP Manager actor
///
/// This is the public interface for the McpManager. It is cheap to clone
/// and can be shared across threads. All operations are non-blocking and
/// communicate with the actor via channels.
///
/// # Example
///
/// ```ignore
/// let handle = mcp_manager.handle();
///
/// // Add a server
/// handle.add_server(config).await?;
///
/// // List all servers
/// let servers = handle.list_servers().await?;
///
/// // Subscribe to events
/// let mut events = handle.subscribe();
/// while let Ok(event) = events.recv().await {
///     println!("Event: {:?}", event);
/// }
/// ```
#[derive(Clone)]
pub struct McpManagerHandle {
    /// Command sender for communicating with the actor
    tx: mpsc::Sender<McpCommand>,
    /// Event broadcast sender for subscribing to events
    event_tx: broadcast::Sender<McpManagerEvent>,
}

impl McpManagerHandle {
    /// Create a new handle
    ///
    /// This is typically called by the McpManager when spawning.
    pub(crate) fn new(
        tx: mpsc::Sender<McpCommand>,
        event_tx: broadcast::Sender<McpManagerEvent>,
    ) -> Self {
        Self { tx, event_tx }
    }

    // ===== Lifecycle Methods =====

    /// Add a new MCP server configuration
    ///
    /// The server will be started if `auto_start` is true in the config.
    pub async fn add_server(&self, config: McpManagerConfig) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::AddServer { config, respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))?
            .map_err(AlephError::other)
    }

    /// Remove a server by ID
    ///
    /// This will stop the server if running and remove it from the manager.
    pub async fn remove_server(&self, server_id: impl Into<String>) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::RemoveServer {
                server_id: server_id.into(),
                respond_to,
            })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))?
            .map_err(AlephError::other)
    }

    /// Restart a specific server
    ///
    /// This will stop the server (if running) and start it again.
    pub async fn restart_server(&self, server_id: impl Into<String>) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::RestartServer {
                server_id: server_id.into(),
                respond_to,
            })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))?
            .map_err(AlephError::other)
    }

    /// Start a stopped server
    ///
    /// Returns an error if the server is already running.
    pub async fn start_server(&self, server_id: impl Into<String>) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::StartServer {
                server_id: server_id.into(),
                respond_to,
            })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))?
            .map_err(AlephError::other)
    }

    /// Stop a running server
    ///
    /// Returns an error if the server is not running.
    pub async fn stop_server(&self, server_id: impl Into<String>) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::StopServer {
                server_id: server_id.into(),
                respond_to,
            })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))?
            .map_err(AlephError::other)
    }

    // ===== Query Methods =====

    /// Get the McpClient for a specific server
    ///
    /// Returns `None` if the server doesn't exist or is not running.
    pub async fn get_client(&self, server_id: impl Into<String>) -> Result<Option<Arc<McpClient>>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::GetClient {
                server_id: server_id.into(),
                respond_to,
            })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    /// List all registered servers
    ///
    /// Returns a lightweight summary of each server.
    pub async fn list_servers(&self) -> Result<Vec<McpServerInfo>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::ListServers { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    /// Get detailed status for a specific server
    ///
    /// Returns `None` if the server doesn't exist.
    pub async fn get_status(
        &self,
        server_id: impl Into<String>,
    ) -> Result<Option<McpServerStatusDetail>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::GetStatus {
                server_id: server_id.into(),
                respond_to,
            })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    // ===== Aggregation Methods (P1) =====

    /// Get aggregated tools from all healthy servers
    ///
    /// Tools are collected from all running servers and returned as a flat list.
    /// Each tool name is prefixed with the server ID to avoid conflicts.
    pub async fn aggregate_tools(&self) -> Result<Vec<McpTool>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::AggregateTools { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    /// Get aggregated resources from all healthy servers
    ///
    /// Resources are collected from all running servers and returned as a flat list.
    pub async fn aggregate_resources(&self) -> Result<Vec<McpResource>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::AggregateResources { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    /// Get aggregated prompts from all healthy servers
    ///
    /// Prompts are collected from all running servers and returned as a flat list.
    pub async fn aggregate_prompts(&self) -> Result<Vec<McpPrompt>> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::AggregatePrompts { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    // ===== Config Methods =====

    /// Reload configuration from disk
    ///
    /// This will reload the MCP server configurations and reconcile
    /// the running state with the new configuration.
    pub async fn reload_config(&self) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::ReloadConfig { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))?
            .map_err(AlephError::other)
    }

    // ===== Control Methods =====

    /// Gracefully shutdown the manager
    ///
    /// This will stop all running servers and terminate the actor.
    pub async fn shutdown(&self) -> Result<()> {
        let (respond_to, rx) = oneshot::channel();

        self.tx
            .send(McpCommand::Shutdown { respond_to })
            .await
            .map_err(|_| AlephError::channel_closed("McpManager command channel closed"))?;

        rx.await
            .map_err(|_| AlephError::channel_closed("McpManager response channel closed"))
    }

    // ===== Sampling Methods =====

    /// Set callback for handling sampling requests from MCP servers
    ///
    /// This callback will be invoked when any MCP server sends a
    /// sampling/createMessage request to use the host's LLM.
    pub async fn set_sampling_callback<F, Fut>(
        &self,
        callback: F,
    ) -> std::result::Result<(), String>
    where
        F: Fn(crate::mcp::jsonrpc::mcp::SamplingRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = crate::error::Result<crate::mcp::jsonrpc::mcp::SamplingResponse>>
            + Send
            + 'static,
    {
        let boxed: crate::mcp::sampling::SamplingCallback =
            Box::new(move |req| Box::pin(callback(req)));
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(McpCommand::SetSamplingCallback {
                callback: Arc::new(boxed),
                respond_to: tx,
            })
            .await
            .map_err(|_| "Manager not running".to_string())?;
        rx.await.map_err(|_| "Failed to set callback".to_string())
    }

    // ===== Event Subscription =====

    /// Subscribe to manager events
    ///
    /// Returns a receiver that will receive all events emitted by the manager.
    /// Events include server lifecycle changes, capability updates, and manager status.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut events = handle.subscribe();
    /// tokio::spawn(async move {
    ///     while let Ok(event) = events.recv().await {
    ///         match event {
    ///             McpManagerEvent::ServerStarted { server_id, .. } => {
    ///                 println!("Server {} started", server_id);
    ///             }
    ///             _ => {}
    ///         }
    ///     }
    /// });
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<McpManagerEvent> {
        self.event_tx.subscribe()
    }

    /// Check if the manager is still running
    ///
    /// Returns false if the command channel has been closed.
    pub fn is_running(&self) -> bool {
        !self.tx.is_closed()
    }
}

impl std::fmt::Debug for McpManagerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpManagerHandle")
            .field("is_running", &self.is_running())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_debug() {
        let (tx, _rx) = mpsc::channel(1);
        let (event_tx, _) = broadcast::channel(16);
        let handle = McpManagerHandle::new(tx, event_tx);

        let debug = format!("{:?}", handle);
        assert!(debug.contains("McpManagerHandle"));
        assert!(debug.contains("is_running"));
    }

    #[tokio::test]
    async fn test_handle_is_running() {
        let (tx, rx) = mpsc::channel(1);
        let (event_tx, _) = broadcast::channel(16);
        let handle = McpManagerHandle::new(tx, event_tx);

        assert!(handle.is_running());

        // Drop the receiver to close the channel
        drop(rx);

        // After dropping, is_running should return false
        // (though it may take a moment for the channel to be marked as closed)
        // We need to try to send something to detect the closure
        let _ = handle.tx.try_send(McpCommand::ListServers {
            respond_to: oneshot::channel().0,
        });

        // Now it should be detected as closed
        assert!(!handle.is_running());
    }

    #[test]
    fn test_handle_clone() {
        let (tx, _rx) = mpsc::channel(1);
        let (event_tx, _) = broadcast::channel(16);
        let handle = McpManagerHandle::new(tx, event_tx);

        let handle2 = handle.clone();
        assert!(handle.is_running());
        assert!(handle2.is_running());
    }

    #[tokio::test]
    async fn test_channel_closed_errors() {
        let (tx, rx) = mpsc::channel(1);
        let (event_tx, _) = broadcast::channel(16);
        let handle = McpManagerHandle::new(tx, event_tx);

        // Drop the receiver immediately
        drop(rx);

        // All methods should return ChannelClosed errors
        let result = handle.list_servers().await;
        assert!(matches!(result, Err(AlephError::ChannelClosed(_))));

        let result = handle
            .add_server(McpManagerConfig::stdio("test", "Test", "/bin/true"))
            .await;
        assert!(matches!(result, Err(AlephError::ChannelClosed(_))));
    }

    #[test]
    fn test_subscribe() {
        let (tx, _rx) = mpsc::channel(1);
        let (event_tx, _) = broadcast::channel(16);
        let handle = McpManagerHandle::new(tx, event_tx);

        // Should be able to subscribe multiple times
        let _sub1 = handle.subscribe();
        let _sub2 = handle.subscribe();
    }
}
