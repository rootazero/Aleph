//! HTTP route handler for plugin-provided REST endpoints
//!
//! This module provides the `PluginHttpHandler` which routes incoming HTTP requests
//! to the appropriate plugin handlers based on registered routes.
//!
//! # Path Parameter Matching
//!
//! Routes can contain path parameters using curly brace syntax:
//! - `/api/users/{id}` matches `/api/users/123` with `{"id": "123"}`
//! - `/api/{org}/repos/{repo}` matches `/api/acme/repos/widgets` with `{"org": "acme", "repo": "widgets"}`
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::extension::{PluginHttpHandler, PluginLoader};
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//!
//! let loader = Arc::new(RwLock::new(PluginLoader::new()));
//! let mut handler = PluginHttpHandler::new(loader);
//!
//! // Routes are registered when plugins load
//! // handler.register_routes(routes);
//!
//! // Handle an incoming request
//! let request = HttpRequest {
//!     method: "GET".to_string(),
//!     path: "/api/users/123".to_string(),
//!     headers: HashMap::new(),
//!     query: HashMap::new(),
//!     body: None,
//!     path_params: HashMap::new(),
//! };
//!
//! let response = handler.handle_request(request).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::error::{ExtensionError, ExtensionResult};
use super::plugin_loader::PluginLoader;
use super::registry::HttpRouteRegistration;
use super::types::{HttpRequest, HttpResponse};

/// Handles HTTP requests to plugin-provided REST routes.
///
/// The handler maintains a list of registered routes and matches incoming
/// requests against them. When a match is found, it delegates the request
/// to the plugin that registered the route.
pub struct PluginHttpHandler {
    /// Registered HTTP routes from plugins
    routes: Vec<HttpRouteRegistration>,
    /// Plugin loader for calling route handlers
    loader: Arc<RwLock<PluginLoader>>,
}

impl PluginHttpHandler {
    /// Create a new HTTP handler.
    ///
    /// # Arguments
    ///
    /// * `loader` - The plugin loader used to call handler functions
    pub fn new(loader: Arc<RwLock<PluginLoader>>) -> Self {
        Self {
            routes: Vec::new(),
            loader,
        }
    }

    /// Register routes from a plugin.
    ///
    /// Routes are appended to the existing list. Order matters for matching -
    /// the first matching route wins.
    ///
    /// # Arguments
    ///
    /// * `routes` - List of route registrations from a plugin
    pub fn register_routes(&mut self, routes: Vec<HttpRouteRegistration>) {
        self.routes.extend(routes);
    }

    /// Unregister all routes for a specific plugin.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin whose routes should be removed
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        self.routes.retain(|r| r.plugin_id != plugin_id);
    }

    /// Find a matching route for the given HTTP method and path.
    ///
    /// Returns the matching route registration and any captured path parameters.
    ///
    /// # Arguments
    ///
    /// * `method` - The HTTP method (GET, POST, etc.)
    /// * `path` - The request path (e.g., "/api/users/123")
    ///
    /// # Returns
    ///
    /// * `Some((route, params))` - If a matching route was found
    /// * `None` - If no route matches
    pub fn find_route(
        &self,
        method: &str,
        path: &str,
    ) -> Option<(&HttpRouteRegistration, HashMap<String, String>)> {
        for route in &self.routes {
            // Check if method is allowed
            if !route
                .methods
                .iter()
                .any(|m: &String| m.eq_ignore_ascii_case(method))
            {
                continue;
            }

            // Try to match the path pattern
            if let Some(params) = match_path(&route.path, path) {
                return Some((route, params));
            }
        }
        None
    }

    /// Handle an incoming HTTP request.
    ///
    /// This method:
    /// 1. Finds a matching route for the request
    /// 2. Extracts path parameters
    /// 3. Calls the plugin handler
    /// 4. Returns the response
    ///
    /// # Arguments
    ///
    /// * `request` - The incoming HTTP request
    ///
    /// # Returns
    ///
    /// * `Ok(HttpResponse)` - The response from the plugin handler
    /// * `Err(ExtensionError)` - If no route matches or the handler fails
    pub async fn handle_request(&self, mut request: HttpRequest) -> ExtensionResult<HttpResponse> {
        // Find matching route
        let (route, params) = self
            .find_route(&request.method, &request.path)
            .ok_or_else(|| {
                ExtensionError::Runtime(format!(
                    "No route found for {} {}",
                    request.method, request.path
                ))
            })?;

        // Set path parameters from route matching
        request.path_params = params;

        // Prepare handler arguments
        let args = serde_json::to_value(&request).map_err(|e| {
            ExtensionError::Runtime(format!("Failed to serialize request: {}", e))
        })?;

        // Call the plugin handler
        let result = {
            let mut loader = self.loader.write().await;
            loader.call_tool(&route.plugin_id, &route.handler, args)?
        };

        // Parse the response
        let response: HttpResponse = serde_json::from_value(result).map_err(|e| {
            ExtensionError::Runtime(format!("Invalid response from handler: {}", e))
        })?;

        Ok(response)
    }

    /// List all registered routes.
    ///
    /// # Returns
    ///
    /// A slice of all registered route registrations.
    pub fn list_routes(&self) -> &[HttpRouteRegistration] {
        &self.routes
    }

    /// Get the number of registered routes.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Check if any routes are registered.
    pub fn has_routes(&self) -> bool {
        !self.routes.is_empty()
    }

    /// Get routes for a specific plugin.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin
    ///
    /// # Returns
    ///
    /// A vector of routes registered by the specified plugin.
    pub fn routes_for_plugin(&self, plugin_id: &str) -> Vec<&HttpRouteRegistration> {
        self.routes
            .iter()
            .filter(|r| r.plugin_id == plugin_id)
            .collect()
    }
}

/// Match a path pattern against an actual path, returning captured parameters.
///
/// Supports path parameters using curly brace syntax:
/// - `/api/users/{id}` matches `/api/users/123` -> `{"id": "123"}`
/// - `/api/{org}/repos/{repo}` matches `/api/acme/repos/widgets` -> `{"org": "acme", "repo": "widgets"}`
///
/// # Arguments
///
/// * `pattern` - The route pattern (e.g., "/api/users/{id}")
/// * `path` - The actual request path (e.g., "/api/users/123")
///
/// # Returns
///
/// * `Some(HashMap)` - If the pattern matches, with captured parameters
/// * `None` - If the pattern does not match
fn match_path(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pattern_segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // Must have same number of segments
    if pattern_segments.len() != path_segments.len() {
        return None;
    }

    let mut params = HashMap::new();

    for (pattern_seg, path_seg) in pattern_segments.iter().zip(path_segments.iter()) {
        if pattern_seg.starts_with('{') && pattern_seg.ends_with('}') {
            // This is a parameter segment - extract the parameter name
            let param_name = &pattern_seg[1..pattern_seg.len() - 1];
            params.insert(param_name.to_string(), (*path_seg).to_string());
        } else if pattern_seg != path_seg {
            // Literal segment that doesn't match
            return None;
        }
    }

    Some(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_path_exact() {
        let result = match_path("/api/health", "/api/health");
        assert!(result.is_some());
        let params: HashMap<String, String> = result.unwrap();
        assert!(params.is_empty());
    }

    #[test]
    fn test_match_path_single_param() {
        let result = match_path("/api/users/{id}", "/api/users/123");
        assert!(result.is_some());
        let params: HashMap<String, String> = result.unwrap();
        assert_eq!(params.get("id"), Some(&"123".to_string()));
    }

    #[test]
    fn test_match_path_multiple_params() {
        let params = match_path("/api/{org}/repos/{repo}", "/api/acme/repos/widgets");
        assert!(params.is_some());
        let params = params.unwrap();
        assert_eq!(params.get("org"), Some(&"acme".to_string()));
        assert_eq!(params.get("repo"), Some(&"widgets".to_string()));
    }

    #[test]
    fn test_match_path_no_match_different_segments() {
        let params = match_path("/api/users/{id}", "/api/posts/123");
        assert!(params.is_none());
    }

    #[test]
    fn test_match_path_no_match_different_length() {
        let params = match_path("/api/users/{id}", "/api/users/123/posts");
        assert!(params.is_none());
    }

    #[test]
    fn test_match_path_trailing_slash() {
        // Both with trailing slash
        let params = match_path("/api/users/", "/api/users/");
        assert!(params.is_some());

        // Pattern without, path with
        let params = match_path("/api/users", "/api/users/");
        assert!(params.is_some());

        // Pattern with, path without
        let params = match_path("/api/users/", "/api/users");
        assert!(params.is_some());
    }

    #[test]
    fn test_match_path_root() {
        let params = match_path("/", "/");
        assert!(params.is_some());
        let params: HashMap<String, String> = params.unwrap();
        assert!(params.is_empty());
    }

    #[test]
    fn test_match_path_param_only() {
        let params = match_path("/{id}", "/123");
        assert!(params.is_some());
        let params = params.unwrap();
        assert_eq!(params.get("id"), Some(&"123".to_string()));
    }

    #[test]
    fn test_match_path_complex_params() {
        let params = match_path(
            "/v1/{version}/users/{user_id}/posts/{post_id}",
            "/v1/2024/users/alice/posts/42",
        );
        assert!(params.is_some());
        let params = params.unwrap();
        assert_eq!(params.get("version"), Some(&"2024".to_string()));
        assert_eq!(params.get("user_id"), Some(&"alice".to_string()));
        assert_eq!(params.get("post_id"), Some(&"42".to_string()));
    }

    #[test]
    fn test_match_path_empty_segment() {
        // Empty pattern and path should match
        let params = match_path("", "");
        assert!(params.is_some());
    }

    #[test]
    fn test_plugin_http_handler_new() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let handler = PluginHttpHandler::new(loader);
        assert!(!handler.has_routes());
        assert_eq!(handler.route_count(), 0);
    }

    #[test]
    fn test_plugin_http_handler_register_routes() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let mut handler = PluginHttpHandler::new(loader);

        let routes = vec![
            HttpRouteRegistration {
                path: "/api/users".to_string(),
                methods: vec!["GET".to_string(), "POST".to_string()],
                handler: "handleUsers".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
            HttpRouteRegistration {
                path: "/api/users/{id}".to_string(),
                methods: vec!["GET".to_string(), "PUT".to_string(), "DELETE".to_string()],
                handler: "handleUser".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
        ];

        handler.register_routes(routes);

        assert!(handler.has_routes());
        assert_eq!(handler.route_count(), 2);
        assert_eq!(handler.list_routes().len(), 2);
    }

    #[test]
    fn test_plugin_http_handler_unregister_plugin() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let mut handler = PluginHttpHandler::new(loader);

        handler.register_routes(vec![
            HttpRouteRegistration {
                path: "/api/users".to_string(),
                methods: vec!["GET".to_string()],
                handler: "handleUsers".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
            HttpRouteRegistration {
                path: "/api/posts".to_string(),
                methods: vec!["GET".to_string()],
                handler: "handlePosts".to_string(),
                plugin_id: "post-plugin".to_string(),
            },
        ]);

        assert_eq!(handler.route_count(), 2);

        handler.unregister_plugin("user-plugin");

        assert_eq!(handler.route_count(), 1);
        assert_eq!(handler.list_routes()[0].plugin_id, "post-plugin");
    }

    #[test]
    fn test_plugin_http_handler_find_route() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let mut handler = PluginHttpHandler::new(loader);

        handler.register_routes(vec![
            HttpRouteRegistration {
                path: "/api/users".to_string(),
                methods: vec!["GET".to_string(), "POST".to_string()],
                handler: "handleUsers".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
            HttpRouteRegistration {
                path: "/api/users/{id}".to_string(),
                methods: vec!["GET".to_string()],
                handler: "handleUser".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
        ]);

        // Find exact match
        let result = handler.find_route("GET", "/api/users");
        assert!(result.is_some());
        let (route, params) = result.unwrap();
        assert_eq!(route.handler, "handleUsers");
        assert!(params.is_empty());

        // Find parameterized match
        let result = handler.find_route("GET", "/api/users/123");
        assert!(result.is_some());
        let (route, params) = result.unwrap();
        assert_eq!(route.handler, "handleUser");
        assert_eq!(params.get("id"), Some(&"123".to_string()));

        // No match for wrong method
        let result = handler.find_route("DELETE", "/api/users");
        assert!(result.is_none());

        // No match for unknown path
        let result = handler.find_route("GET", "/api/posts");
        assert!(result.is_none());
    }

    #[test]
    fn test_plugin_http_handler_find_route_case_insensitive_method() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let mut handler = PluginHttpHandler::new(loader);

        handler.register_routes(vec![HttpRouteRegistration {
            path: "/api/test".to_string(),
            methods: vec!["GET".to_string()],
            handler: "handleTest".to_string(),
            plugin_id: "test-plugin".to_string(),
        }]);

        // Should match regardless of case
        assert!(handler.find_route("get", "/api/test").is_some());
        assert!(handler.find_route("GET", "/api/test").is_some());
        assert!(handler.find_route("Get", "/api/test").is_some());
    }

    #[test]
    fn test_plugin_http_handler_routes_for_plugin() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let mut handler = PluginHttpHandler::new(loader);

        handler.register_routes(vec![
            HttpRouteRegistration {
                path: "/api/users".to_string(),
                methods: vec!["GET".to_string()],
                handler: "handleUsers".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
            HttpRouteRegistration {
                path: "/api/users/{id}".to_string(),
                methods: vec!["GET".to_string()],
                handler: "handleUser".to_string(),
                plugin_id: "user-plugin".to_string(),
            },
            HttpRouteRegistration {
                path: "/api/posts".to_string(),
                methods: vec!["GET".to_string()],
                handler: "handlePosts".to_string(),
                plugin_id: "post-plugin".to_string(),
            },
        ]);

        let user_routes = handler.routes_for_plugin("user-plugin");
        assert_eq!(user_routes.len(), 2);

        let post_routes = handler.routes_for_plugin("post-plugin");
        assert_eq!(post_routes.len(), 1);

        let unknown_routes = handler.routes_for_plugin("unknown-plugin");
        assert!(unknown_routes.is_empty());
    }

    #[tokio::test]
    async fn test_plugin_http_handler_handle_request_no_route() {
        let loader = Arc::new(RwLock::new(PluginLoader::new()));
        let handler = PluginHttpHandler::new(loader);

        let request = HttpRequest {
            method: "GET".to_string(),
            path: "/api/nonexistent".to_string(),
            headers: HashMap::new(),
            query: HashMap::new(),
            body: None,
            path_params: HashMap::new(),
        };

        let result = handler.handle_request(request).await;
        assert!(result.is_err());

        match result {
            Err(ExtensionError::Runtime(msg)) => {
                assert!(msg.contains("No route found"));
            }
            _ => panic!("Expected Runtime error"),
        }
    }
}
