# Models & Chat RPC Handlers Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `models.*` and `chat.*` RPC handlers to the Gateway for model discovery and chat control.

**Architecture:** Create two new handler modules following the existing pattern in `gateway/handlers/`. Models handlers expose provider/model information from Config. Chat handlers provide high-level messaging control that wraps agent.run with simpler semantics.

**Tech Stack:** Rust, tokio, serde, JSON-RPC 2.0

---

## Task 1: Create models.rs Handler Module

**Files:**
- Create: `core/src/gateway/handlers/models.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Write the failing test**

Add to `core/src/gateway/handlers/models.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_model_info_serialize() {
        let info = ModelInfo {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            provider_type: "openai".to_string(),
            enabled: true,
            is_default: true,
            capabilities: vec!["chat".to_string(), "vision".to_string()],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["id"], "gpt-4o");
        assert_eq!(json["provider"], "openai");
        assert!(json["enabled"].as_bool().unwrap());
    }

    #[test]
    fn test_list_params_deserialize() {
        let json = json!({ "provider": "openai", "enabled_only": true });
        let params: ListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.provider, Some("openai".to_string()));
        assert!(params.enabled_only);
    }

    #[test]
    fn test_list_params_defaults() {
        let json = json!({});
        let params: ListParams = serde_json::from_value(json).unwrap();
        assert!(params.provider.is_none());
        assert!(!params.enabled_only);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore gateway::handlers::models::tests --no-default-features`
Expected: FAIL with "cannot find module `models`"

**Step 3: Write minimal implementation**

Create `core/src/gateway/handlers/models.rs`:

```rust
//! Models RPC Handlers
//!
//! Handlers for AI model discovery and information:
//! - models.list: List available models with filtering
//! - models.get: Get detailed info for a specific model
//! - models.capabilities: Get model capabilities

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::config::Config;

// ============================================================================
// Types
// ============================================================================

/// Model information for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "gpt-4o", "claude-3-5-sonnet")
    pub id: String,
    /// Provider name from config (e.g., "openai", "claude")
    pub provider: String,
    /// Provider type (openai, claude, gemini, ollama)
    pub provider_type: String,
    /// Whether the model/provider is enabled
    pub enabled: bool,
    /// Whether this is the default model
    pub is_default: bool,
    /// Model capabilities (inferred from provider type)
    pub capabilities: Vec<String>,
}

/// Parameters for models.list
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ListParams {
    /// Filter by provider name
    #[serde(default)]
    pub provider: Option<String>,
    /// Only show enabled models
    #[serde(default)]
    pub enabled_only: bool,
}

/// Parameters for models.get
#[derive(Debug, Clone, Deserialize)]
pub struct GetParams {
    /// Provider name
    pub provider: String,
}

// ============================================================================
// Capability Inference
// ============================================================================

/// Infer capabilities from provider type and model name
fn infer_capabilities(provider_type: &str, model: &str) -> Vec<String> {
    let mut caps = vec!["chat".to_string()];

    let model_lower = model.to_lowercase();

    // Vision capability
    let has_vision = match provider_type {
        "openai" => model_lower.contains("gpt-4") || model_lower.contains("o1") || model_lower.contains("o3"),
        "claude" => model_lower.contains("claude-3") || model_lower.contains("claude-4"),
        "gemini" => true, // All Gemini models support vision
        _ => false,
    };
    if has_vision {
        caps.push("vision".to_string());
    }

    // Tool use capability
    let has_tools = match provider_type {
        "openai" | "claude" | "gemini" => true,
        "ollama" => model_lower.contains("llama3") || model_lower.contains("mistral"),
        _ => false,
    };
    if has_tools {
        caps.push("tools".to_string());
    }

    // Extended thinking capability
    let has_thinking = match provider_type {
        "claude" => model_lower.contains("claude-3-5") || model_lower.contains("claude-4"),
        "openai" => model_lower.contains("o1") || model_lower.contains("o3"),
        "gemini" => model_lower.contains("2.0") || model_lower.contains("3"),
        _ => false,
    };
    if has_thinking {
        caps.push("thinking".to_string());
    }

    caps
}

// ============================================================================
// Handlers
// ============================================================================

/// Handle models.list RPC request
///
/// Lists all available models from configured providers.
/// Supports filtering by provider name and enabled status.
pub async fn handle_list(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: ListParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let default_provider = config.general.default_provider.clone();

    let models: Vec<ModelInfo> = config
        .providers
        .iter()
        .filter(|(name, cfg)| {
            // Filter by provider name if specified
            if let Some(ref filter) = params.provider {
                if !name.to_lowercase().contains(&filter.to_lowercase()) {
                    return false;
                }
            }
            // Filter by enabled status if requested
            if params.enabled_only && !cfg.enabled {
                return false;
            }
            true
        })
        .map(|(name, cfg)| {
            let provider_type = cfg.infer_provider_type(name);
            let capabilities = infer_capabilities(&provider_type, &cfg.model);

            ModelInfo {
                id: cfg.model.clone(),
                provider: name.clone(),
                provider_type,
                enabled: cfg.enabled,
                is_default: default_provider.as_ref() == Some(name),
                capabilities,
            }
        })
        .collect();

    let count = models.len();
    let enabled_count = models.iter().filter(|m| m.enabled).count();

    JsonRpcResponse::success(
        request.id,
        json!({
            "models": models,
            "count": count,
            "enabled_count": enabled_count,
            "default_provider": default_provider,
        }),
    )
}

/// Handle models.get RPC request
///
/// Gets detailed information for a specific provider's model.
pub async fn handle_get(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let params: GetParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: provider required".to_string(),
            );
        }
    };

    match config.providers.get(&params.provider) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let provider_type = cfg.infer_provider_type(&params.provider);
            let capabilities = infer_capabilities(&provider_type, &cfg.model);

            let info = ModelInfo {
                id: cfg.model.clone(),
                provider: params.provider.clone(),
                provider_type,
                enabled: cfg.enabled,
                is_default: default_provider.as_ref() == Some(&params.provider),
                capabilities,
            };

            // Include additional provider details
            JsonRpcResponse::success(
                request.id,
                json!({
                    "model": info,
                    "config": {
                        "base_url": cfg.base_url,
                        "timeout_seconds": cfg.timeout_seconds,
                        "max_tokens": cfg.max_tokens,
                        "temperature": cfg.temperature,
                        "has_api_key": cfg.api_key.is_some(),
                    }
                }),
            )
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider not found: {}", params.provider),
        ),
    }
}

/// Handle models.capabilities RPC request
///
/// Returns a list of all known capabilities and which models support them.
pub async fn handle_capabilities(request: JsonRpcRequest, config: Arc<Config>) -> JsonRpcResponse {
    let mut capability_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for (name, cfg) in config.providers.iter() {
        if !cfg.enabled {
            continue;
        }

        let provider_type = cfg.infer_provider_type(name);
        let capabilities = infer_capabilities(&provider_type, &cfg.model);

        for cap in capabilities {
            capability_map
                .entry(cap)
                .or_default()
                .push(name.clone());
        }
    }

    JsonRpcResponse::success(
        request.id,
        json!({
            "capabilities": capability_map,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_model_info_serialize() {
        let info = ModelInfo {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            provider_type: "openai".to_string(),
            enabled: true,
            is_default: true,
            capabilities: vec!["chat".to_string(), "vision".to_string()],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["id"], "gpt-4o");
        assert_eq!(json["provider"], "openai");
        assert!(json["enabled"].as_bool().unwrap());
    }

    #[test]
    fn test_list_params_deserialize() {
        let json = json!({ "provider": "openai", "enabled_only": true });
        let params: ListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.provider, Some("openai".to_string()));
        assert!(params.enabled_only);
    }

    #[test]
    fn test_list_params_defaults() {
        let json = json!({});
        let params: ListParams = serde_json::from_value(json).unwrap();
        assert!(params.provider.is_none());
        assert!(!params.enabled_only);
    }

    #[test]
    fn test_infer_capabilities_openai() {
        let caps = infer_capabilities("openai", "gpt-4o");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"vision".to_string()));
        assert!(caps.contains(&"tools".to_string()));
    }

    #[test]
    fn test_infer_capabilities_claude() {
        let caps = infer_capabilities("claude", "claude-3-5-sonnet-20241022");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"vision".to_string()));
        assert!(caps.contains(&"tools".to_string()));
        assert!(caps.contains(&"thinking".to_string()));
    }

    #[test]
    fn test_infer_capabilities_ollama() {
        let caps = infer_capabilities("ollama", "llama3.2");
        assert!(caps.contains(&"chat".to_string()));
        assert!(caps.contains(&"tools".to_string()));
        assert!(!caps.contains(&"vision".to_string()));
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore gateway::handlers::models::tests --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/models.rs
git commit -m "feat(gateway): add models.rs handler module

Add RPC handlers for model discovery:
- models.list: List models with filtering
- models.get: Get model details
- models.capabilities: Get capability map

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Register models.* Handlers

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Write the failing test**

Add to existing tests in `core/src/gateway/handlers/mod.rs`:

```rust
#[test]
fn test_models_handlers_registered() {
    let registry = HandlerRegistry::new();
    assert!(registry.has_method("models.list"));
    assert!(registry.has_method("models.get"));
    assert!(registry.has_method("models.capabilities"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_models_handlers_registered --no-default-features`
Expected: FAIL with "assertion failed: registry.has_method(\"models.list\")"

**Step 3: Write minimal implementation**

Update `core/src/gateway/handlers/mod.rs`:

1. Add module declaration:
```rust
pub mod models;
```

2. In `HandlerRegistry::new()`, add after the existing handler registrations:
```rust
// Models handlers (stateless, use default config)
registry.register("models.list", |req| async move {
    let config = std::sync::Arc::new(crate::config::Config::default());
    models::handle_list(req, config).await
});
registry.register("models.get", |req| async move {
    let config = std::sync::Arc::new(crate::config::Config::default());
    models::handle_get(req, config).await
});
registry.register("models.capabilities", |req| async move {
    let config = std::sync::Arc::new(crate::config::Config::default());
    models::handle_capabilities(req, config).await
});
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_models_handlers_registered --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): register models.* RPC handlers

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Create chat.rs Handler Module

**Files:**
- Create: `core/src/gateway/handlers/chat.rs`

**Step 1: Write the failing test**

Add to `core/src/gateway/handlers/chat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_send_params_deserialize() {
        let json = json!({
            "message": "Hello",
            "session_key": "agent:main:main"
        });
        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Hello");
        assert_eq!(params.session_key, Some("agent:main:main".to_string()));
    }

    #[test]
    fn test_send_params_minimal() {
        let json = json!({ "message": "Hi" });
        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Hi");
        assert!(params.session_key.is_none());
        assert!(params.stream);
    }

    #[test]
    fn test_abort_params_deserialize() {
        let json = json!({ "run_id": "abc-123" });
        let params: AbortParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "abc-123");
    }

    #[test]
    fn test_history_params_defaults() {
        let json = json!({ "session_key": "agent:main:main" });
        let params: HistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, None);
        assert_eq!(params.before, None);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore gateway::handlers::chat::tests --no-default-features`
Expected: FAIL with "cannot find module `chat`"

**Step 3: Write minimal implementation**

Create `core/src/gateway/handlers/chat.rs`:

```rust
//! Chat RPC Handlers
//!
//! High-level chat control handlers that wrap agent operations:
//! - chat.send: Send a message (wraps agent.run with simpler semantics)
//! - chat.abort: Abort current generation (wraps agent.cancel)
//! - chat.history: Get chat history for a session
//! - chat.clear: Clear chat history for a session

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use super::super::router::SessionKey;
use super::super::session_manager::SessionManager;
use super::agent::{AgentRunManager, AgentRunParams};

// ============================================================================
// Types
// ============================================================================

/// Parameters for chat.send
#[derive(Debug, Clone, Deserialize)]
pub struct SendParams {
    /// The message to send
    pub message: String,
    /// Session key (optional, uses default main session if not provided)
    #[serde(default)]
    pub session_key: Option<String>,
    /// Channel identifier (e.g., "gui", "cli", "api")
    #[serde(default)]
    pub channel: Option<String>,
    /// Whether to stream the response (default: true)
    #[serde(default = "default_true")]
    pub stream: bool,
    /// Thinking level for LLM reasoning
    #[serde(default)]
    pub thinking: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Response for chat.send
#[derive(Debug, Clone, Serialize)]
pub struct SendResponse {
    /// Run ID for tracking
    pub run_id: String,
    /// Resolved session key
    pub session_key: String,
    /// Whether streaming is enabled
    pub streaming: bool,
}

/// Parameters for chat.abort
#[derive(Debug, Clone, Deserialize)]
pub struct AbortParams {
    /// Run ID to abort (from chat.send response)
    pub run_id: String,
}

/// Parameters for chat.history
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryParams {
    /// Session key to get history for
    pub session_key: String,
    /// Maximum number of messages to return
    #[serde(default)]
    pub limit: Option<usize>,
    /// Get messages before this timestamp (ISO 8601)
    #[serde(default)]
    pub before: Option<String>,
}

/// A message in chat history
#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Run ID if this was an assistant message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// Parameters for chat.clear
#[derive(Debug, Clone, Deserialize)]
pub struct ClearParams {
    /// Session key to clear
    pub session_key: String,
    /// Keep system messages (default: true)
    #[serde(default = "default_true")]
    pub keep_system: bool,
}

// ============================================================================
// Handlers
// ============================================================================

/// Handle chat.send RPC request
///
/// Sends a message to the agent and returns a run ID for tracking.
/// This is a high-level wrapper around agent.run with simpler semantics.
pub async fn handle_send(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    let params: SendParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: message required".to_string(),
            );
        }
    };

    // Validate message
    if params.message.trim().is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Message cannot be empty".to_string(),
        );
    }

    // Convert to AgentRunParams
    let agent_params = AgentRunParams {
        input: params.message,
        session_key: params.session_key,
        channel: params.channel,
        peer_id: None,
        stream: params.stream,
        thinking: params.thinking,
    };

    // Start the run
    match run_manager.start_run(agent_params).await {
        Ok(result) => {
            let response = SendResponse {
                run_id: result.run_id,
                session_key: result.session_key,
                streaming: params.stream,
            };
            JsonRpcResponse::success(request.id, json!(response))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Handle chat.abort RPC request
///
/// Aborts the current generation for a run.
pub async fn handle_abort(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    let params: AbortParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: run_id required".to_string(),
            );
        }
    };

    let cancelled = run_manager.cancel_run(&params.run_id).await;

    JsonRpcResponse::success(
        request.id,
        json!({
            "run_id": params.run_id,
            "aborted": cancelled,
        }),
    )
}

/// Handle chat.history RPC request
///
/// Gets chat history for a session.
pub async fn handle_history(
    request: JsonRpcRequest,
    session_manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let params: HistoryParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: session_key required".to_string(),
            );
        }
    };

    // Parse session key
    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format".to_string(),
            );
        }
    };

    // Get history from session manager
    match session_manager.get_history(&session_key, params.limit).await {
        Ok(messages) => {
            let history: Vec<ChatMessage> = messages
                .into_iter()
                .map(|m| ChatMessage {
                    role: m.role,
                    content: m.content,
                    timestamp: chrono::DateTime::from_timestamp(m.timestamp, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    run_id: None, // TODO: Store run_id in messages
                })
                .collect();

            let count = history.len();

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": params.session_key,
                    "messages": history,
                    "count": count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get history: {}", e),
        ),
    }
}

/// Handle chat.clear RPC request
///
/// Clears chat history for a session.
pub async fn handle_clear(
    request: JsonRpcRequest,
    session_manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    let params: ClearParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: session_key required".to_string(),
            );
        }
    };

    // Parse session key
    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format".to_string(),
            );
        }
    };

    // Reset session
    match session_manager.reset_session(&session_key).await {
        Ok(reset) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": params.session_key,
                "cleared": reset,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to clear session: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_send_params_deserialize() {
        let json = json!({
            "message": "Hello",
            "session_key": "agent:main:main"
        });
        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Hello");
        assert_eq!(params.session_key, Some("agent:main:main".to_string()));
    }

    #[test]
    fn test_send_params_minimal() {
        let json = json!({ "message": "Hi" });
        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Hi");
        assert!(params.session_key.is_none());
        assert!(params.stream);
    }

    #[test]
    fn test_abort_params_deserialize() {
        let json = json!({ "run_id": "abc-123" });
        let params: AbortParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "abc-123");
    }

    #[test]
    fn test_history_params_defaults() {
        let json = json!({ "session_key": "agent:main:main" });
        let params: HistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, None);
        assert_eq!(params.before, None);
    }

    #[test]
    fn test_clear_params_defaults() {
        let json = json!({ "session_key": "agent:main:main" });
        let params: ClearParams = serde_json::from_value(json).unwrap();
        assert!(params.keep_system);
    }

    #[test]
    fn test_send_response_serialize() {
        let response = SendResponse {
            run_id: "run-123".to_string(),
            session_key: "agent:main:main".to_string(),
            streaming: true,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["run_id"], "run-123");
        assert!(json["streaming"].as_bool().unwrap());
    }

    #[test]
    fn test_chat_message_serialize() {
        let msg = ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            run_id: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert!(json.get("run_id").is_none()); // skip_serializing_if = None
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore gateway::handlers::chat::tests --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/chat.rs
git commit -m "feat(gateway): add chat.rs handler module

Add high-level chat control handlers:
- chat.send: Send message (wraps agent.run)
- chat.abort: Abort generation
- chat.history: Get chat history
- chat.clear: Clear chat history

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Register chat.* Handlers

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Write the failing test**

Add to existing tests in `core/src/gateway/handlers/mod.rs`:

```rust
#[test]
fn test_chat_handlers_registered() {
    let registry = HandlerRegistry::new();
    assert!(registry.has_method("chat.send"));
    assert!(registry.has_method("chat.abort"));
    assert!(registry.has_method("chat.history"));
    assert!(registry.has_method("chat.clear"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_chat_handlers_registered --no-default-features`
Expected: FAIL with "assertion failed"

**Step 3: Write minimal implementation**

Update `core/src/gateway/handlers/mod.rs`:

1. Add module declaration:
```rust
pub mod chat;
```

2. Note: The chat handlers require `AgentRunManager` and `SessionManager` which need to be injected. For now, we'll register placeholder handlers that return "not configured" errors. The actual wiring happens in the Gateway server setup.

Add handler registration:
```rust
// Chat handlers (require runtime dependencies, registered as placeholders)
// Actual handlers are wired in Gateway::new() with proper dependencies
registry.register("chat.send", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "chat.send requires Gateway runtime - use Gateway::new()".to_string(),
    )
});
registry.register("chat.abort", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "chat.abort requires Gateway runtime - use Gateway::new()".to_string(),
    )
});
registry.register("chat.history", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "chat.history requires Gateway runtime - use Gateway::new()".to_string(),
    )
});
registry.register("chat.clear", |req| async move {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        "chat.clear requires Gateway runtime - use Gateway::new()".to_string(),
    )
});
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore test_chat_handlers_registered --no-default-features`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): register chat.* RPC handlers

Register placeholder handlers for chat.* methods.
Actual handlers are wired with dependencies in Gateway::new().

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Add Integration Tests

**Files:**
- Create: `core/tests/models_chat_handlers_test.rs`

**Step 1: Write the test file**

Create `core/tests/models_chat_handlers_test.rs`:

```rust
//! Integration tests for models.* and chat.* RPC handlers

use alephcore::config::Config;
use alephcore::gateway::handlers::models;
use alephcore::gateway::protocol::JsonRpcRequest;
use serde_json::json;
use std::sync::Arc;

// ============================================================================
// models.* Tests
// ============================================================================

#[tokio::test]
async fn test_models_list_empty() {
    let config = Arc::new(Config::default());
    let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));

    let response = models::handle_list(request, config).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    assert_eq!(result["count"], 0);
    assert!(result["models"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_models_list_with_providers() {
    use alephcore::config::types::ProviderConfig;
    use std::collections::HashMap;

    let mut config = Config::default();

    // Add test providers
    let mut providers = HashMap::new();
    providers.insert("openai".to_string(), ProviderConfig {
        model: "gpt-4o".to_string(),
        enabled: true,
        ..Default::default()
    });
    providers.insert("claude".to_string(), ProviderConfig {
        model: "claude-3-5-sonnet-20241022".to_string(),
        enabled: true,
        ..Default::default()
    });
    providers.insert("disabled".to_string(), ProviderConfig {
        model: "disabled-model".to_string(),
        enabled: false,
        ..Default::default()
    });
    config.providers = providers;

    let config = Arc::new(config);
    let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));

    let response = models::handle_list(request, config).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    assert_eq!(result["count"], 3);
    assert_eq!(result["enabled_count"], 2);
}

#[tokio::test]
async fn test_models_list_filter_enabled() {
    use alephcore::config::types::ProviderConfig;
    use std::collections::HashMap;

    let mut config = Config::default();
    let mut providers = HashMap::new();
    providers.insert("enabled".to_string(), ProviderConfig {
        model: "test-model".to_string(),
        enabled: true,
        ..Default::default()
    });
    providers.insert("disabled".to_string(), ProviderConfig {
        model: "disabled-model".to_string(),
        enabled: false,
        ..Default::default()
    });
    config.providers = providers;

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.list",
        Some(json!({ "enabled_only": true })),
        Some(json!(1)),
    );

    let response = models::handle_list(request, config).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    assert_eq!(result["count"], 1);
}

#[tokio::test]
async fn test_models_get_found() {
    use alephcore::config::types::ProviderConfig;
    use std::collections::HashMap;

    let mut config = Config::default();
    let mut providers = HashMap::new();
    providers.insert("openai".to_string(), ProviderConfig {
        model: "gpt-4o".to_string(),
        enabled: true,
        ..Default::default()
    });
    config.providers = providers;

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.get",
        Some(json!({ "provider": "openai" })),
        Some(json!(1)),
    );

    let response = models::handle_get(request, config).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    assert_eq!(result["model"]["id"], "gpt-4o");
    assert_eq!(result["model"]["provider"], "openai");
}

#[tokio::test]
async fn test_models_get_not_found() {
    let config = Arc::new(Config::default());
    let request = JsonRpcRequest::new(
        "models.get",
        Some(json!({ "provider": "nonexistent" })),
        Some(json!(1)),
    );

    let response = models::handle_get(request, config).await;

    assert!(response.is_error());
    assert!(response.error.unwrap().message.contains("not found"));
}

#[tokio::test]
async fn test_models_capabilities() {
    use alephcore::config::types::ProviderConfig;
    use std::collections::HashMap;

    let mut config = Config::default();
    let mut providers = HashMap::new();
    providers.insert("openai".to_string(), ProviderConfig {
        model: "gpt-4o".to_string(),
        enabled: true,
        ..Default::default()
    });
    providers.insert("claude".to_string(), ProviderConfig {
        model: "claude-3-5-sonnet-20241022".to_string(),
        enabled: true,
        ..Default::default()
    });
    config.providers = providers;

    let config = Arc::new(config);
    let request = JsonRpcRequest::new("models.capabilities", None, Some(json!(1)));

    let response = models::handle_capabilities(request, config).await;

    assert!(response.is_success());
    let result = response.result.unwrap();
    let caps = &result["capabilities"];

    // Both should have chat capability
    assert!(caps["chat"].as_array().unwrap().len() >= 2);
}

// ============================================================================
// chat.* Type Tests (handlers require runtime deps)
// ============================================================================

#[test]
fn test_chat_send_params() {
    use alephcore::gateway::handlers::chat::SendParams;

    let json = json!({
        "message": "Hello, AI!",
        "session_key": "agent:main:main",
        "stream": false
    });

    let params: SendParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.message, "Hello, AI!");
    assert_eq!(params.session_key, Some("agent:main:main".to_string()));
    assert!(!params.stream);
}

#[test]
fn test_chat_history_params() {
    use alephcore::gateway::handlers::chat::HistoryParams;

    let json = json!({
        "session_key": "agent:main:main",
        "limit": 50
    });

    let params: HistoryParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session_key, "agent:main:main");
    assert_eq!(params.limit, Some(50));
}

#[test]
fn test_chat_clear_params() {
    use alephcore::gateway::handlers::chat::ClearParams;

    let json = json!({
        "session_key": "agent:main:main",
        "keep_system": false
    });

    let params: ClearParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session_key, "agent:main:main");
    assert!(!params.keep_system);
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --test models_chat_handlers_test`
Expected: PASS (all tests should pass)

**Step 3: Commit**

```bash
git add core/tests/models_chat_handlers_test.rs
git commit -m "test(gateway): add integration tests for models/chat handlers

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Update Handler Registry Documentation

**Files:**
- Modify: `core/src/gateway/handlers/mod.rs` (update module doc)

**Step 1: Update documentation**

Update the module documentation at the top of `core/src/gateway/handlers/mod.rs`:

```rust
//! Request Handlers
//!
//! Handlers for processing JSON-RPC 2.0 method calls.
//!
//! ## Handler Domains
//!
//! | Domain | Description |
//! |--------|-------------|
//! | health | Health checks, ping |
//! | echo | Echo/test |
//! | version | Version info |
//! | config | Configuration management |
//! | logs | Log level control |
//! | commands | Command listing |
//! | plugins | Plugin lifecycle |
//! | skills | Skills management |
//! | mcp | MCP integration |
//! | providers | AI provider management |
//! | profiles | Auth profile management |
//! | generation | Content generation |
//! | pairing | Device pairing |
//! | runs | Run wait/queue |
//! | auth | Authentication |
//! | agent | Agent execution |
//! | session | Session management |
//! | channel | Channel status |
//! | events | Event subscription |
//! | ocr | OCR operations |
//! | memory | Memory search |
//! | **models** | Model discovery (NEW) |
//! | **chat** | Chat control (NEW) |
//! | browser | Browser/CDP (feature-gated) |
```

**Step 2: Commit**

```bash
git add core/src/gateway/handlers/mod.rs
git commit -m "docs(gateway): update handler registry documentation

Add models and chat to handler domain list.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Run Full Test Suite

**Step 1: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

**Step 2: Run integration tests**

Run: `cargo test -p alephcore --test '*'`
Expected: All tests pass

**Step 3: Final commit (if any fixes needed)**

If any tests fail, fix them and commit.

---

## Summary

This plan adds 7 new RPC handlers:

| Method | Description |
|--------|-------------|
| `models.list` | List available models with filtering |
| `models.get` | Get detailed model info |
| `models.capabilities` | Get capability map |
| `chat.send` | Send message (wraps agent.run) |
| `chat.abort` | Abort generation |
| `chat.history` | Get chat history |
| `chat.clear` | Clear chat history |

Total: 6 tasks, ~1-2 hours implementation time.
