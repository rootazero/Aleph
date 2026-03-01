//! MCP Resource Manager
//!
//! Manages resources from MCP servers - files, data, and other content
//! that can be referenced and read by the AI.
//!
//! MCP resources are similar to files or data sources that servers expose
//! for reading. They can be text files, images, database records, or any
//! other content type.

use std::collections::HashMap;
use crate::sync_primitives::Arc;

use crate::error::Result;
use crate::mcp::client::McpClient;
use crate::mcp::types::McpResource;

/// Content returned when reading a resource
#[derive(Debug, Clone)]
pub enum ResourceContent {
    /// Text content (most common)
    Text(String),
    /// Binary content with MIME type
    Binary {
        /// Raw binary data
        data: Vec<u8>,
        /// MIME type (e.g., "application/octet-stream")
        mime_type: String,
    },
    /// Image content (base64 encoded by MCP servers)
    Image {
        /// Image data (decoded from base64)
        data: Vec<u8>,
        /// MIME type (e.g., "image/png", "image/jpeg")
        mime_type: String,
    },
}

impl ResourceContent {
    /// Create text content
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(content.into())
    }

    /// Create binary content
    pub fn binary(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self::Binary {
            data,
            mime_type: mime_type.into(),
        }
    }

    /// Create image content
    pub fn image(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self::Image {
            data,
            mime_type: mime_type.into(),
        }
    }

    /// Check if this is text content
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text(_))
    }

    /// Get text content if available
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// Manages MCP resources across all connected servers
///
/// The resource manager provides a unified interface for accessing resources
/// from any connected MCP server. It aggregates resources and handles
/// routing read requests to the appropriate server.
pub struct McpResourceManager {
    client: Arc<McpClient>,
}

impl McpResourceManager {
    /// Create a new resource manager
    ///
    /// # Arguments
    ///
    /// * `client` - The MCP client that manages server connections
    pub fn new(client: Arc<McpClient>) -> Self {
        Self { client }
    }

    /// List resources from a specific server
    ///
    /// # Arguments
    ///
    /// * `server` - The server name to query
    ///
    /// # Returns
    ///
    /// A list of resources available from the server
    pub async fn list(&self, server: &str) -> Result<Vec<McpResource>> {
        let all_resources = self.client.list_resources().await;
        let prefix = format!("{}:", server);

        Ok(all_resources
            .into_iter()
            .filter(|r| r.uri.starts_with(&prefix))
            .collect())
    }

    /// Read a resource by URI from a specific server
    ///
    /// # Arguments
    ///
    /// * `server` - The server name that owns the resource
    /// * `uri` - The resource URI (e.g., "file:///path/to/file")
    ///
    /// # Returns
    ///
    /// The resource content
    pub async fn read(&self, server: &str, uri: &str) -> Result<ResourceContent> {
        // Ensure URI has server prefix
        let full_uri = if uri.starts_with(&format!("{}:", server)) {
            uri.to_string()
        } else {
            format!("{}:{}", server, uri)
        };

        self.client.read_resource(&full_uri).await
    }

    /// List all resources from all connected servers
    ///
    /// Aggregates resources from all servers, returning a map from
    /// server name to its resources.
    ///
    /// # Returns
    ///
    /// A map of server names to their resource lists
    pub async fn list_all(&self) -> Result<HashMap<String, Vec<McpResource>>> {
        let mut all_resources = HashMap::new();

        let server_names = self.client.service_names().await;
        for server in server_names {
            match self.list(&server).await {
                Ok(resources) => {
                    all_resources.insert(server, resources);
                }
                Err(e) => {
                    tracing::warn!(
                        server = %server,
                        error = %e,
                        "Failed to list resources from server"
                    );
                }
            }
        }

        Ok(all_resources)
    }

    /// Subscribe to resource updates from a server
    ///
    /// # Arguments
    ///
    /// * `server` - The server name
    /// * `uri` - The resource URI to subscribe to
    pub async fn subscribe(&self, server: &str, uri: &str) -> Result<()> {
        // TODO: Implement resources/subscribe
        tracing::debug!(
            server = %server,
            uri = %uri,
            "Resource subscription (stub - not yet implemented)"
        );
        Ok(())
    }

    /// Unsubscribe from resource updates
    ///
    /// # Arguments
    ///
    /// * `server` - The server name
    /// * `uri` - The resource URI to unsubscribe from
    pub async fn unsubscribe(&self, server: &str, uri: &str) -> Result<()> {
        // TODO: Implement resources/unsubscribe
        tracing::debug!(
            server = %server,
            uri = %uri,
            "Resource unsubscription (stub - not yet implemented)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_resource_manager_creation() {
        let client = Arc::new(McpClient::new());
        let manager = McpResourceManager::new(client);

        let resources = manager.list_all().await;
        assert!(resources.is_ok());
        assert!(resources.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_resource_manager_list_empty() {
        let client = Arc::new(McpClient::new());
        let manager = McpResourceManager::new(client);

        let resources = manager.list("nonexistent-server").await;
        assert!(resources.is_ok());
        assert!(resources.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_resource_manager_read_not_found() {
        let client = Arc::new(McpClient::new());
        let manager = McpResourceManager::new(client);

        // With no servers, reading a resource should return NotFound
        let content = manager.read("server", "file:///test.txt").await;
        assert!(content.is_err());
    }

    #[test]
    fn test_resource_content_text() {
        let content = ResourceContent::text("Hello, world!");
        assert!(content.is_text());
        assert_eq!(content.as_text(), Some("Hello, world!"));
    }

    #[test]
    fn test_resource_content_binary() {
        let content = ResourceContent::binary(vec![1, 2, 3], "application/octet-stream");
        assert!(!content.is_text());
        assert!(content.as_text().is_none());
    }

    #[test]
    fn test_resource_content_image() {
        let content = ResourceContent::image(vec![0x89, 0x50, 0x4E, 0x47], "image/png");
        assert!(!content.is_text());
    }
}
