//! Request Handlers
//!
//! Handlers for processing JSON-RPC 2.0 method calls.

pub mod health;
pub mod echo;
pub mod version;
pub mod agent;
pub mod session;
pub mod auth;
pub mod events;
pub mod channel;
pub mod config;
pub mod logs;
pub mod commands;
pub mod ocr;
#[cfg(feature = "browser")]
pub mod browser;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::protocol::{JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND};

/// Type alias for async handler functions
pub type HandlerFn = Arc<
    dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync,
>;

/// Registry for JSON-RPC method handlers
///
/// Maps method names to their handler functions. Handlers are invoked
/// asynchronously when a request with a matching method is received.
pub struct HandlerRegistry {
    handlers: HashMap<String, HandlerFn>,
}

impl HandlerRegistry {
    /// Create a new handler registry with built-in handlers
    pub fn new() -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };

        // Register built-in handlers
        registry.register("health", health::handle);
        registry.register("echo", echo::handle);
        registry.register("version", version::handle);

        // Logs handlers
        registry.register("logs.getLevel", logs::handle_get_level);
        registry.register("logs.setLevel", logs::handle_set_level);
        registry.register("logs.getDirectory", logs::handle_get_directory);

        // Commands handlers
        registry.register("commands.list", commands::handle_list);

        registry
    }

    /// Create an empty handler registry
    pub fn empty() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a method
    ///
    /// # Arguments
    ///
    /// * `method` - The method name to handle
    /// * `handler` - An async function that takes a request and returns a response
    pub fn register<F, Fut>(&mut self, method: &str, handler: F)
    where
        F: Fn(JsonRpcRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = JsonRpcResponse> + Send + 'static,
    {
        self.handlers.insert(
            method.to_string(),
            Arc::new(move |req| Box::pin(handler(req))),
        );
    }

    /// Unregister a handler
    ///
    /// # Arguments
    ///
    /// * `method` - The method name to unregister
    ///
    /// # Returns
    ///
    /// `true` if a handler was removed
    pub fn unregister(&mut self, method: &str) -> bool {
        self.handlers.remove(method).is_some()
    }

    /// Handle a request by dispatching to the appropriate handler
    ///
    /// # Arguments
    ///
    /// * `request` - The JSON-RPC request to handle
    ///
    /// # Returns
    ///
    /// The response from the handler, or a method not found error
    pub async fn handle(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        if let Some(handler) = self.handlers.get(&request.method) {
            handler(request.clone()).await
        } else {
            JsonRpcResponse::error(
                request.id.clone(),
                METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            )
        }
    }

    /// Check if a method is registered
    pub fn has_method(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    /// Get a list of all registered method names
    pub fn methods(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Get the number of registered handlers
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_builtin_handlers() {
        let registry = HandlerRegistry::new();

        assert!(registry.has_method("health"));
        assert!(registry.has_method("echo"));
        assert!(registry.has_method("version"));
    }

    #[tokio::test]
    async fn test_custom_handler() {
        let mut registry = HandlerRegistry::empty();

        registry.register("custom", |req| async move {
            JsonRpcResponse::success(req.id, json!({"custom": true}))
        });

        let request = JsonRpcRequest::new("custom", None, Some(json!(1)));
        let response = registry.handle(&request).await;

        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let registry = HandlerRegistry::empty();

        let request = JsonRpcRequest::new("nonexistent", None, Some(json!(1)));
        let response = registry.handle(&request).await;

        assert!(response.is_error());
        assert_eq!(response.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_unregister() {
        let mut registry = HandlerRegistry::new();

        assert!(registry.has_method("health"));
        assert!(registry.unregister("health"));
        assert!(!registry.has_method("health"));
    }

    #[test]
    fn test_methods_list() {
        let registry = HandlerRegistry::new();
        let methods = registry.methods();

        assert!(methods.contains(&"health".to_string()));
        assert!(methods.contains(&"echo".to_string()));
        assert!(methods.contains(&"version".to_string()));
    }
}
