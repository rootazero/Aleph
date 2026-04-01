//! MCP Notification Router
//!
//! Routes server-initiated notifications to the Aleph event bus.
//!
//! MCP servers can send notifications to inform clients about state changes:
//! - `notifications/tools/listChanged` - Tool list has changed
//! - `notifications/resources/listChanged` - Resource list has changed
//! - `notifications/prompts/listChanged` - Prompt list has changed
//!
//! This module provides a notification router that converts these MCP
//! notifications into Aleph events for the rest of the system to handle.

use crate::sync_primitives::Arc;

use crate::mcp::jsonrpc::JsonRpcNotification;
use crate::mcp::transport::NotificationCallback;

/// MCP-specific events for the event bus
///
/// These events are emitted when MCP servers send notifications about
/// state changes. The UI and other components can subscribe to these
/// events to stay up-to-date.
#[derive(Debug, Clone, PartialEq)]
pub enum McpEvent {
    /// Tool list changed on a server
    ///
    /// Clients should call `tools/list` to get the updated list.
    ToolsChanged {
        /// Server that sent the notification
        server: String,
    },
    /// Resource list changed on a server
    ///
    /// Clients should call `resources/list` to get the updated list.
    ResourcesChanged {
        /// Server that sent the notification
        server: String,
    },
    /// Prompt list changed on a server
    ///
    /// Clients should call `prompts/list` to get the updated list.
    PromptsChanged {
        /// Server that sent the notification
        server: String,
    },
    /// Server connection status changed
    ConnectionChanged {
        /// Server whose connection status changed
        server: String,
        /// Whether the server is now connected
        connected: bool,
    },
    /// Server sent a progress notification
    Progress {
        /// Server that sent the notification
        server: String,
        /// Progress token
        token: String,
        /// Progress value (0-100)
        progress: f64,
        /// Optional progress total
        total: Option<f64>,
    },
    /// Server sent a log message
    LogMessage {
        /// Server that sent the notification
        server: String,
        /// Log level (debug, info, warn, error)
        level: String,
        /// Log message
        message: String,
    },
}

impl McpEvent {
    /// Get the server name associated with this event
    pub fn server(&self) -> &str {
        match self {
            Self::ToolsChanged { server } => server,
            Self::ResourcesChanged { server } => server,
            Self::PromptsChanged { server } => server,
            Self::ConnectionChanged { server, .. } => server,
            Self::Progress { server, .. } => server,
            Self::LogMessage { server, .. } => server,
        }
    }

    /// Check if this is a list-changed event
    pub fn is_list_changed(&self) -> bool {
        matches!(
            self,
            Self::ToolsChanged { .. } | Self::ResourcesChanged { .. } | Self::PromptsChanged { .. }
        )
    }
}

/// Handler type for MCP events
pub type McpEventHandler = Box<dyn Fn(McpEvent) + Send + Sync>;

/// Routes MCP notifications to event handlers
///
/// The notification router converts incoming JSON-RPC notifications
/// from MCP servers into typed events that can be handled by the
/// Aleph event system.
pub struct McpNotificationRouter {
    /// Event handlers
    handlers: Vec<McpEventHandler>,
}

impl McpNotificationRouter {
    /// Create a new notification router
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Add an event handler
    ///
    /// Handlers are called for every MCP event. Multiple handlers
    /// can be registered.
    pub fn add_handler(&mut self, handler: McpEventHandler) {
        self.handlers.push(handler);
    }

    /// Handle an incoming notification
    ///
    /// Parses the notification and dispatches appropriate events
    /// to all registered handlers.
    pub fn handle(&self, server: &str, notification: JsonRpcNotification) {
        let event = match notification.method.as_str() {
            "notifications/tools/listChanged" => {
                tracing::info!(server = %server, "Tools list changed");
                Some(McpEvent::ToolsChanged {
                    server: server.to_string(),
                })
            }
            "notifications/resources/listChanged" => {
                tracing::info!(server = %server, "Resources list changed");
                Some(McpEvent::ResourcesChanged {
                    server: server.to_string(),
                })
            }
            "notifications/prompts/listChanged" => {
                tracing::info!(server = %server, "Prompts list changed");
                Some(McpEvent::PromptsChanged {
                    server: server.to_string(),
                })
            }
            "notifications/progress" => {
                if let Some(params) = notification.params {
                    let token = params
                        .get("progressToken")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let progress = params
                        .get("progress")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let total = params.get("total").and_then(|v| v.as_f64());

                    tracing::debug!(
                        server = %server,
                        token = %token,
                        progress = progress,
                        "Progress notification"
                    );

                    Some(McpEvent::Progress {
                        server: server.to_string(),
                        token,
                        progress,
                        total,
                    })
                } else {
                    None
                }
            }
            "notifications/message" => {
                if let Some(params) = notification.params {
                    let level = params
                        .get("level")
                        .and_then(|v| v.as_str())
                        .unwrap_or("info")
                        .to_string();
                    let message = params
                        .get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    tracing::debug!(
                        server = %server,
                        level = %level,
                        message = %message,
                        "Log message notification"
                    );

                    Some(McpEvent::LogMessage {
                        server: server.to_string(),
                        level,
                        message,
                    })
                } else {
                    None
                }
            }
            method => {
                tracing::debug!(
                    server = %server,
                    method = %method,
                    "Unknown MCP notification"
                );
                None
            }
        };

        // Dispatch to handlers
        if let Some(event) = event {
            for handler in &self.handlers {
                handler(event.clone());
            }
        }
    }

    /// Create a callback for use with transports
    ///
    /// Returns a callback that can be passed to a transport's
    /// `set_notification_handler` method.
    pub fn create_callback(self: Arc<Self>, server: String) -> NotificationCallback {
        Box::new(move |notification| {
            self.handle(&server, notification);
        })
    }
}

impl Default for McpNotificationRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::{AtomicUsize, Ordering};

    #[test]
    fn test_mcp_event_tools_changed() {
        let event = McpEvent::ToolsChanged {
            server: "test".to_string(),
        };

        assert_eq!(event.server(), "test");
        assert!(event.is_list_changed());
    }

    #[test]
    fn test_mcp_event_resources_changed() {
        let event = McpEvent::ResourcesChanged {
            server: "test".to_string(),
        };

        assert_eq!(event.server(), "test");
        assert!(event.is_list_changed());
    }

    #[test]
    fn test_mcp_event_prompts_changed() {
        let event = McpEvent::PromptsChanged {
            server: "test".to_string(),
        };

        assert_eq!(event.server(), "test");
        assert!(event.is_list_changed());
    }

    #[test]
    fn test_mcp_event_connection_changed() {
        let event = McpEvent::ConnectionChanged {
            server: "test".to_string(),
            connected: true,
        };

        assert_eq!(event.server(), "test");
        assert!(!event.is_list_changed());
    }

    #[test]
    fn test_notification_router_creation() {
        let router = McpNotificationRouter::new();
        assert!(router.handlers.is_empty());
    }

    #[test]
    fn test_notification_router_handler() {
        let mut router = McpNotificationRouter::new();
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        router.add_handler(Box::new(move |_event| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let notification = JsonRpcNotification::new("notifications/tools/listChanged");
        router.handle("test-server", notification);

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_notification_router_unknown_method() {
        let mut router = McpNotificationRouter::new();
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        router.add_handler(Box::new(move |_event| {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let notification = JsonRpcNotification::new("unknown/method");
        router.handle("test-server", notification);

        // Unknown methods should not trigger handlers
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_notification_router_multiple_handlers() {
        let mut router = McpNotificationRouter::new();

        let count1 = Arc::new(AtomicUsize::new(0));
        let count1_clone = Arc::clone(&count1);
        router.add_handler(Box::new(move |_| {
            count1_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let count2 = Arc::new(AtomicUsize::new(0));
        let count2_clone = Arc::clone(&count2);
        router.add_handler(Box::new(move |_| {
            count2_clone.fetch_add(1, Ordering::SeqCst);
        }));

        let notification = JsonRpcNotification::new("notifications/resources/listChanged");
        router.handle("test-server", notification);

        assert_eq!(count1.load(Ordering::SeqCst), 1);
        assert_eq!(count2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_notification_router_create_callback() {
        let router = Arc::new(McpNotificationRouter::new());
        let callback = router.create_callback("test-server".to_string());

        // The callback should be callable without panic
        callback(JsonRpcNotification::new("notifications/tools/listChanged"));
    }
}
