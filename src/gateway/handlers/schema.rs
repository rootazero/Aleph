//! Typed RPC handler registry with compile-time schema verification.
//!
//! Rust's type system catches parameter mismatches at compile time.
//! JSON Schema auto-generated via schemars for API documentation.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Marker type for handlers that accept no parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct NoParams;

impl NoParams {
    /// Create an empty params object for handlers that don't need any.
    pub fn empty() -> Self {
        Self
    }
}

/// Error returned when parameter parsing fails.
#[derive(Debug, Clone)]
pub struct ParamParseError {
    /// Human-readable error message.
    pub message: String,
}

impl ParamParseError {
    pub fn missing() -> Self {
        Self {
            message: "Missing params".to_string(),
        }
    }

    pub fn invalid(e: serde_json::Error) -> Self {
        Self {
            message: format!("Invalid params: {}", e),
        }
    }
}

/// Trait for RPC handlers that declare their parameter schema.
///
/// Implement this trait to get compile-time type checking and auto-generated
/// JSON Schema documentation for your handler's parameters.
///
/// # Example
///
/// ```ignore
/// use async_trait::async_trait;
///
/// struct StartServiceHandler;
///
/// #[async_trait]
/// impl HandlerSchema for StartServiceHandler {
///     type Params = StartParams; // Must implement DeserializeOwned + JsonSchema
///     const METHOD: &'static str = "services.start";
///     const DESCRIPTION: &'static str = "Start a background service";
///
///     async fn handle(params: Self::Params) -> JsonRpcResponse {
///         // ... implementation
///     }
/// }
/// ```
pub trait HandlerSchema: Send + Sync {
    /// The parameter type for this handler.
    /// Must implement `DeserializeOwned` for JSON-RPC param parsing.
    /// Should also derive `JsonSchema` for auto-generated documentation.
    type Params: DeserializeOwned + JsonSchema + Send + 'static;

    /// The JSON-RPC method name (e.g., "services.start").
    const METHOD: &'static str;

    /// Human-readable description of what this handler does.
    const DESCRIPTION: &'static str;

    /// Whether this handler requires authentication.
    /// Defaults to false.
    const REQUIRES_AUTH: bool = false;

    /// Handle a request with typed parameters.
    ///
    /// The default implementation parses params from the request and delegates
    /// to `handle_with_params`. Override if you need direct request access.
    fn handle(request: &JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send + '_>>
    where
        Self: Sized,
    {
        let id = request.id.clone();
        let params_result: Result<Self::Params, _> = serde_json::from_value(
            request.params.clone().unwrap_or(Value::Null),
        );

        let params = match params_result {
            Ok(p) => p,
            Err(e) => {
                return Box::pin(async move {
                    JsonRpcResponse::error(id, INVALID_PARAMS, ParamParseError::invalid(e).message)
                });
            }
        };

        Box::pin(Self::handle_with_params(id, params))
    }

    /// Handle with already-parsed typed parameters.
    ///
    /// Override this when you need direct access to the request ID or need
    /// to do custom error handling before parsing.
    fn handle_with_params(
        id: Option<Value>,
        params: Self::Params,
    ) -> impl Future<Output = JsonRpcResponse> + Send + 'static
    where
        Self: Sized;
}

/// Information about a registered handler for introspection.
#[derive(Debug, Clone)]
pub struct HandlerInfo {
    /// The JSON-RPC method name.
    pub method: String,
    /// Human-readable description.
    pub description: String,
    /// Whether authentication is required.
    pub requires_auth: bool,
    /// The JSON Schema for the parameters (as a JSON Value).
    pub params_schema: Value,
}

impl HandlerInfo {
    /// Generate a JSON Schema object for this handler's parameters.
    pub fn params_schema_json(&self) -> &Value {
        &self.params_schema
    }
}

/// A typed handler with compile-time schema verification.
pub struct TypedHandler<H: HandlerSchema> {
    _marker: std::marker::PhantomData<H>,
}

impl<H: HandlerSchema> TypedHandler<H> {
    /// Create a new typed handler wrapper.
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Get handler metadata (method, description, schema).
    pub fn info() -> HandlerInfo {
        let schema = schemars::schema_for!(H::Params);
        let schema_json = serde_json::to_value(&schema).unwrap_or_else(|_| Value::Null);

        HandlerInfo {
            method: H::METHOD.to_string(),
            description: H::DESCRIPTION.to_string(),
            requires_auth: H::REQUIRES_AUTH,
            params_schema: schema_json,
        }
    }

    /// Handle a request by parsing params and dispatching to the handler.
    pub async fn handle(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        H::handle(request).await
    }
}

impl<H: HandlerSchema> Default for TypedHandler<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry for typed RPC handlers with compile-time schema verification.
///
/// This registry provides:
/// - Type-safe handler registration with compile-time param type checking
/// - Auto-generated JSON Schema for all registered handlers
/// - Introspection API for documentation and tooling
///
/// # Example
///
/// ```ignore
/// let mut registry = TypedHandlerRegistry::new();
///
/// registry.register::<StartServiceHandler>();
/// registry.register::<StopServiceHandler>();
/// registry.register::<ListServicesHandler>();
///
/// // Get all handler schemas for documentation
/// let schemas = registry.all_schemas();
/// ```
pub struct TypedHandlerRegistry {
    handlers: Vec<HandlerInfo>,
}

impl TypedHandlerRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a handler and verify its schema.
    ///
    /// The type parameter `H` must implement `HandlerSchema`, which provides
    /// compile-time guarantees about the handler's parameter type.
    pub fn register<H: HandlerSchema>(&mut self) -> &mut Self {
        self.handlers.push(TypedHandler::<H>::info());
        self
    }

    /// Register multiple handlers at once.
    pub fn register_many<H1: HandlerSchema, H2: HandlerSchema>(&mut self) -> &mut Self {
        self.register::<H1>();
        self.register::<H2>();
        self
    }

    /// Get information about a specific handler by method name.
    pub fn get(&self, method: &str) -> Option<&HandlerInfo> {
        self.handlers.iter().find(|h| h.method == method)
    }

    /// Get all registered handler information.
    pub fn all(&self) -> &[HandlerInfo] {
        &self.handlers
    }

    /// Get all method names registered in this registry.
    pub fn methods(&self) -> Vec<&str> {
        self.handlers.iter().map(|h| h.method.as_str()).collect()
    }

    /// Get the total number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Generate a JSON Schema document for the entire RPC protocol.
    ///
    /// This creates a schema that documents all available methods,
    /// their parameters, and their descriptions.
    pub fn protocol_schema(&self) -> Value {
        let mut methods_schema = serde_json::Map::new();

        for handler in &self.handlers {
            let mut method_schema = serde_json::Map::new();
            method_schema.insert(
                "description".to_string(),
                serde_json::Value::String(handler.description.clone()),
            );
            method_schema.insert(
                "params".to_string(),
                handler.params_schema.clone(),
            );
            method_schema.insert(
                "requiresAuth".to_string(),
                serde_json::Value::Bool(handler.requires_auth),
            );

            methods_schema.insert(handler.method.clone(), serde_json::Value::Object(method_schema));
        }

        serde_json::json!({
            "$schema": "https://json-schema.org/draft-07/schema#",
            "title": "Aleph Gateway RPC Protocol",
            "description": "JSON-RPC 2.0 protocol schema for Aleph Gateway",
            "methods": methods_schema
        })
    }

    pub fn openapi_schema(&self) -> Value {
        let mut paths = serde_json::Map::new();

        for handler in &self.handlers {
            let path_name = handler.method.replace('.', "/");
            let rpc_path = format!("/rpc/{}", path_name);

            let params_schema = &handler.params_schema;
            let request_body_schema = params_schema
                .get("definitions")
                .and_then(|d| d.get("Root"))
                .or_else(|| params_schema.get("properties"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({
                    "type": "object",
                    "description": handler.description.clone()
                }));

            let operation = serde_json::json!({
                "post": {
                    "operationId": handler.method.clone(),
                    "summary": handler.description.clone(),
                    "description": handler.description.clone(),
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": request_body_schema
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Successful JSON-RPC response",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "jsonrpc": {
                                                "type": "string",
                                                "enum": ["2.0"]
                                            },
                                            "id": {
                                                "oneOf": [
                                                    {"type": "string"},
                                                    {"type": "number"},
                                                    {"type": "null"}
                                                ]
                                            },
                                            "result": {
                                                "type": "object"
                                            },
                                            "error": {
                                                "type": "object",
                                                "properties": {
                                                    "code": {"type": "integer"},
                                                    "message": {"type": "string"},
                                                    "data": {}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "security": if handler.requires_auth {
                        serde_json::json!([{"bearerAuth": []}])
                    } else {
                        serde_json::json!([])
                    },
                    "tags": handler.method.split('.').next()
                }
            });

            paths.insert(rpc_path, serde_json::json!({ "post": operation }));
        }

        serde_json::json!({
            "openapi": "3.0.3",
            "info": {
                "title": "Aleph Gateway RPC API",
                "description": "JSON-RPC 2.0 API for Aleph messaging gateway. All methods are exposed as POST /rpc/{method}.",
                "version": "1.0.0",
                "contact": {
                    "name": "Aleph Support"
                }
            },
            "servers": [
                {
                    "url": "ws://localhost:8080",
                    "description": "Local gateway WebSocket server"
                }
            ],
            "paths": paths,
            "components": {
                "securitySchemes": {
                    "bearerAuth": {
                        "type": "http",
                        "scheme": "bearer",
                        "bearerFormat": "JWT",
                        "description": "JWT token obtained from identity.get"
                    }
                }
            }
        })
    }
}

impl Default for TypedHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Utility for validating incoming params against a handler's schema.
///
/// This is useful for:
/// - Pre-validation before calling a handler
/// - Generating validation errors with schema context
/// - Testing handler parameter parsing
pub struct SchemaValidator;

impl SchemaValidator {
    /// Validate params against a handler's schema.
    ///
    /// Returns `Ok(params)` if valid, or an error with details otherwise.
    pub fn validate<T: DeserializeOwned + JsonSchema>(params: &Value) -> Result<T, String> {
        serde_json::from_value(params.clone())
            .map_err(|e| format!("Invalid params: {}", e))
    }

    /// Validate that params are present (not null/omitted) for a handler.
    ///
    /// Returns `Ok(())` if params exist, or an error if missing.
    pub fn validate_params_present(params: &Option<Value>) -> Result<(), ParamParseError> {
        match params {
            Some(Value::Null) => Err(ParamParseError::missing()),
            None => Err(ParamParseError::missing()),
            _ => Ok(()),
        }
    }
}

// Global registry for schema introspection
static SCHEMA_REGISTRY: std::sync::OnceLock<TypedHandlerRegistry> = std::sync::OnceLock::new();

/// Get the global schema registry for introspection
pub fn schema_registry() -> &'static TypedHandlerRegistry {
    SCHEMA_REGISTRY.get_or_init(TypedHandlerRegistry::new)
}

/// Register a handler with the global schema registry
pub fn register_handler<H: HandlerSchema + 'static>() {
    let _ = SCHEMA_REGISTRY.get_or_init(TypedHandlerRegistry::new);
}

/// Params for schema.list
#[derive(Debug, Deserialize)]
pub struct SchemaListParams {
    pub category: Option<String>,
}

/// Handle schema.list RPC request
///
/// Returns a list of all registered handler schemas, optionally filtered by category.
pub async fn handle_schema_list(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let registry = SCHEMA_REGISTRY.get_or_init(TypedHandlerRegistry::new);

    let params: SchemaListParams = request.params
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or(SchemaListParams { category: None });

    let handlers = registry.all();
    let total = handlers.len();

    let methods: Vec<serde_json::Value> = handlers
        .iter()
        .filter(|h| {
            if let Some(ref cat) = params.category {
                h.method.starts_with(cat)
            } else {
                true
            }
        })
        .map(|h| {
            serde_json::json!({
                "method": h.method,
                "description": h.description,
                "requiresAuth": h.requires_auth,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "handlers": total,
            "methods": methods,
        }),
    )
}

/// Params for schema.get
#[derive(Debug, Deserialize)]
pub struct SchemaGetParams {
    pub method: String,
}

/// Handle schema.get RPC request
///
/// Returns detailed schema information for a specific handler method.
pub async fn handle_schema_get(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let params: SchemaGetParams = match request.params.and_then(|p| serde_json::from_value(p).ok()) {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            );
        }
    };

    let registry = SCHEMA_REGISTRY.get_or_init(TypedHandlerRegistry::new);

    match registry.get(&params.method) {
        Some(info) => {
            JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "method": info.method,
                    "description": info.description,
                    "requiresAuth": info.requires_auth,
                    "paramsSchema": info.params_schema,
                }),
            )
        }
        None => {
            JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Method not found: {}", params.method),
            )
        }
    }
}

/// Handle schema.protocol RPC request
///
/// Returns the full protocol schema as a JSON Schema document.
pub async fn handle_schema_protocol(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let registry = SCHEMA_REGISTRY.get_or_init(TypedHandlerRegistry::new);
    let schema = registry.protocol_schema();

    JsonRpcResponse::success(request.id, schema)
}

/// Handle schema.openapi RPC request
///
/// Returns the full API specification as an OpenAPI 3.0 document.
/// The spec can be used with Swagger UI, Redoc, or openapi-generator.
pub async fn handle_schema_openapi(
    request: JsonRpcRequest,
) -> JsonRpcResponse {
    let registry = SCHEMA_REGISTRY.get_or_init(TypedHandlerRegistry::new);
    let spec = registry.openapi_schema();

    JsonRpcResponse::success(request.id, spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct TestParams {
        pub name: String,
        pub value: Option<i32>,
    }

    struct TestHandler;

    impl HandlerSchema for TestHandler {
        type Params = TestParams;
        const METHOD: &'static str = "test.method";
        const DESCRIPTION: &'static str = "A test handler";

        async fn handle_with_params(id: Option<Value>, params: Self::Params) -> JsonRpcResponse {
            JsonRpcResponse::success(
                id,
                json!({
                    "received": {
                        "name": params.name,
                        "value": params.value
                    }
                }),
            )
        }
    }

    #[tokio::test]
    async fn test_handler_schema_info() {
        let info = TypedHandler::<TestHandler>::info();

        assert_eq!(info.method, "test.method");
        assert_eq!(info.description, "A test handler");
        assert!(info.params_schema.is_object());
    }

    #[tokio::test]
    async fn test_typed_handler_registry() {
        let mut registry = TypedHandlerRegistry::new();
        registry.register::<TestHandler>();

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
        assert_eq!(registry.methods(), vec!["test.method"]);
    }

    #[tokio::test]
    async fn test_typed_handler_dispatch() {
        let handler = TypedHandler::<TestHandler>::new();

        let request = JsonRpcRequest::with_id(
            "test.method",
            Some(json!({"name": "hello", "value": 42})),
            json!(1),
        );

        let response = handler.handle(&request).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_typed_handler_invalid_params() {
        let handler = TypedHandler::<TestHandler>::new();

        let request = JsonRpcRequest::with_id(
            "test.method",
            Some(json!({"invalid": "params"})),
            json!(1),
        );

        let response = handler.handle(&request).await;
        assert!(response.is_error());
        assert!(response
            .error
            .unwrap()
            .message
            .contains("Invalid params"));
    }

    #[tokio::test]
    async fn test_typed_handler_missing_params() {
        let handler = TypedHandler::<TestHandler>::new();

        let request = JsonRpcRequest::with_id("test.method", None, json!(1));

        let response = handler.handle(&request).await;
        assert!(response.is_error());
    }

    #[test]
    fn test_protocol_schema_generation() {
        let mut registry = TypedHandlerRegistry::new();
        registry.register::<TestHandler>();

        let schema = registry.protocol_schema();

        assert!(schema.get("methods").is_some());
        let methods = schema.get("methods").unwrap();
        assert!(methods.get("test.method").is_some());
    }

    #[test]
    fn test_no_params_schema() {
        let schema = schemars::schema_for!(NoParams);
        let json = serde_json::to_value(&schema).unwrap();
        // NoParams should generate a valid but empty schema
        assert!(json.is_object());
    }

    #[tokio::test]
    async fn test_schema_list_handler() {
        let request = JsonRpcRequest::with_id("schema.list", None, json!(1));
        let response = handle_schema_list(request).await;
        assert!(response.is_success());
        if let Some(result) = response.result {
            let obj = result.as_object().unwrap();
            assert!(obj.contains_key("handlers"));
            assert!(obj.contains_key("methods"));
        }
    }

    #[tokio::test]
    async fn test_schema_get_handler_missing_method() {
        let request = JsonRpcRequest::with_id(
            "schema.get",
            Some(json!({"method": "nonexistent.method"})),
            json!(1),
        );
        let response = handle_schema_get(request).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_schema_protocol_handler() {
        let request = JsonRpcRequest::with_id("schema.protocol", None, json!(1));
        let response = handle_schema_protocol(request).await;
        assert!(response.is_success());
        if let Some(result) = response.result {
            let obj = result.as_object().unwrap();
            assert!(obj.contains_key("$schema"));
            assert!(obj.contains_key("title"));
            assert!(obj.contains_key("methods"));
        }
    }

    #[tokio::test]
    async fn test_openapi_schema_generation() {
        let mut registry = TypedHandlerRegistry::new();
        registry.register::<TestHandler>();

        let spec = registry.openapi_schema();
        let obj = spec.as_object().unwrap();

        assert_eq!(obj.get("openapi").unwrap(), "3.0.3");
        assert!(obj.contains_key("info"));
        assert!(obj.contains_key("paths"));
        assert!(obj.contains_key("components"));
    }

    #[tokio::test]
    async fn test_openapi_handler() {
        let request = JsonRpcRequest::with_id("schema.openapi", None, json!(1));
        let response = handle_schema_openapi(request).await;
        assert!(response.is_success());
        if let Some(result) = response.result {
            let obj = result.as_object().unwrap();
            assert_eq!(obj.get("openapi").unwrap(), "3.0.3");
        }
    }
}
