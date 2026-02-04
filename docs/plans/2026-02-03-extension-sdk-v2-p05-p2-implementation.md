# Aleph Extension SDK V2 - P0.5 to P2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement P0.5 (Direct Commands), P1 (Background Services), and P2 (Channels/Providers/HTTP Routes) for the Aleph Extension SDK V2.

**Architecture:** Build on existing PluginRegistry and PluginLoader infrastructure. Direct commands integrate with CommandRegistry. Services add lifecycle management to PluginLoader. Channels/Providers/HTTP use trait adapters to bridge plugins to core systems.

**Tech Stack:** Rust, Tokio (async), JSON-RPC 2.0 (plugin IPC), Axum (HTTP routes)

---

## Phase P0.5: Direct Commands

Direct commands allow plugins to register commands that execute immediately without LLM involvement (like `/status`, `/clear`).

### Task P0.5.1: Define DirectCommand Types

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add DirectCommand types**

Add after existing types:

```rust
/// Direct command execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectCommandResult {
    /// Command output to display to user
    pub content: String,
    /// Optional structured data
    pub data: Option<serde_json::Value>,
    /// Whether command was successful
    pub success: bool,
}

impl DirectCommandResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            success: true,
        }
    }

    pub fn with_data(content: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            content: content.into(),
            data: Some(data),
            success: true,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            data: None,
            success: false,
        }
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test types`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add DirectCommandResult type"
```

---

### Task P0.5.2: Add Command Execution to PluginLoader

**Files:**
- Modify: `core/src/extension/plugin_loader.rs`

**Step 1: Add execute_command method**

Add to `impl PluginLoader`:

```rust
/// Execute a direct command on a plugin
pub async fn execute_command(
    &mut self,
    plugin_id: &str,
    handler: &str,
    args: serde_json::Value,
) -> ExtensionResult<DirectCommandResult> {
    // Check if plugin is loaded
    let kind = self.loaded_plugins.get(plugin_id).ok_or_else(|| {
        ExtensionError::plugin_not_found(plugin_id)
    })?;

    match kind {
        PluginKind::NodeJs => {
            let runtime = self.nodejs_runtime.as_mut().ok_or_else(|| {
                ExtensionError::runtime_not_initialized("nodejs")
            })?;

            let result = runtime.call_handler(plugin_id, handler, args).await?;
            Ok(serde_json::from_value(result).unwrap_or_else(|_| {
                DirectCommandResult::success("Command executed")
            }))
        }
        #[cfg(feature = "plugin-wasm")]
        PluginKind::Wasm => {
            let runtime = self.wasm_runtime.as_mut().ok_or_else(|| {
                ExtensionError::runtime_not_initialized("wasm")
            })?;

            let result = runtime.call_handler(plugin_id, handler, args)?;
            Ok(serde_json::from_value(result).unwrap_or_else(|_| {
                DirectCommandResult::success("Command executed")
            }))
        }
        PluginKind::Static => {
            Err(ExtensionError::invalid_operation(
                "Static plugins cannot have direct commands"
            ))
        }
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test plugin_loader`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add command execution to PluginLoader"
```

---

### Task P0.5.3: Add Command Gateway Handler

**Files:**
- Modify: `core/src/gateway/handlers/plugins.rs`

**Step 1: Add execute_command handler**

Add the RPC handler:

```rust
/// Handle plugins.executeCommand RPC
pub async fn handle_execute_command(
    request: JsonRpcRequest,
    extension_manager: Arc<RwLock<ExtensionManager>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        plugin_id: String,
        command_name: String,
        #[serde(default)]
        args: serde_json::Value,
    }

    let params: Params = match serde_json::from_value(request.params.unwrap_or_default()) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, -32602, format!("Invalid params: {}", e)),
    };

    // Get the command registration
    let handler = {
        let manager = extension_manager.read().await;
        let registry = manager.plugin_registry();

        registry
            .get_command(&params.plugin_id, &params.command_name)
            .map(|cmd| cmd.handler.clone())
    };

    let handler = match handler {
        Some(h) => h,
        None => return JsonRpcResponse::error(
            request.id,
            -32001,
            format!("Command '{}' not found in plugin '{}'", params.command_name, params.plugin_id),
        ),
    };

    // Execute the command
    let result = {
        let mut manager = extension_manager.write().await;
        manager.execute_command(&params.plugin_id, &handler, params.args).await
    };

    match result {
        Ok(cmd_result) => JsonRpcResponse::success(request.id, serde_json::to_value(cmd_result).unwrap()),
        Err(e) => JsonRpcResponse::error(request.id, -32000, e.to_string()),
    }
}
```

**Step 2: Register the handler**

In handler registration (mod.rs or wherever handlers are registered):

```rust
registry.register("plugins.executeCommand", handle_execute_command);
```

**Step 3: Run tests**

Run: `cd core && cargo test gateway`

**Step 4: Commit**

```bash
git commit -am "feat(gateway): add plugins.executeCommand RPC handler"
```

---

### Task P0.5.4: Add ExtensionManager Command Execution

**Files:**
- Modify: `core/src/extension/mod.rs`

**Step 1: Add execute_command method to ExtensionManager**

```rust
/// Execute a direct command from a plugin
pub async fn execute_command(
    &mut self,
    plugin_id: &str,
    handler: &str,
    args: serde_json::Value,
) -> ExtensionResult<DirectCommandResult> {
    let loader = self.plugin_loader.as_mut().ok_or_else(|| {
        ExtensionError::runtime_not_initialized("plugin loader")
    })?;

    loader.execute_command(plugin_id, handler, args).await
}
```

**Step 2: Run tests**

Run: `cd core && cargo test extension`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add execute_command to ExtensionManager"
```

---

### Task P0.5.5: Add Command Tests

**Files:**
- Modify: `core/tests/extension_v2_test.rs`

**Step 1: Add command execution tests**

```rust
#[test]
fn test_v2_commands_with_handler() {
    let content = r#"
[plugin]
id = "test-commands"
kind = "nodejs"
entry = "dist/index.js"

[[commands]]
name = "status"
description = "Show status"
handler = "handleStatus"

[[commands]]
name = "clear"
description = "Clear screen"
handler = "handleClear"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let commands = manifest.commands_v2.unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].name, "status");
    assert_eq!(commands[0].handler, Some("handleStatus".to_string()));
    assert_eq!(commands[1].name, "clear");
}

#[test]
fn test_direct_command_result() {
    use alephcore::extension::types::DirectCommandResult;

    let success = DirectCommandResult::success("Done!");
    assert!(success.success);
    assert_eq!(success.content, "Done!");

    let with_data = DirectCommandResult::with_data("Result", json!({"count": 42}));
    assert!(with_data.data.is_some());

    let error = DirectCommandResult::error("Failed");
    assert!(!error.success);
}
```

**Step 2: Run tests**

Run: `cd core && cargo test extension_v2`

**Step 3: Commit**

```bash
git commit -am "test(extension): add direct command tests"
```

---

## Phase P1: Background Services

Background services allow plugins to run long-running tasks with managed lifecycle.

### Task P1.1: Define Service Types

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add service types**

```rust
/// Service state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

impl Default for ServiceState {
    fn default() -> Self {
        ServiceState::Stopped
    }
}

/// Running service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub state: ServiceState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

/// Service lifecycle result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResult {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

impl ServiceResult {
    pub fn ok() -> Self {
        Self { success: true, message: None, data: None }
    }

    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        Self { success: true, message: Some(msg.into()), data: None }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { success: false, message: Some(msg.into()), data: None }
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test types`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add service lifecycle types"
```

---

### Task P1.2: Create ServiceManager

**Files:**
- Create: `core/src/extension/service_manager.rs`

**Step 1: Create ServiceManager**

```rust
//! Service lifecycle manager for background plugin services

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::Utc;

use super::types::{ServiceInfo, ServiceResult, ServiceState};
use super::registry::types::ServiceRegistration;
use super::plugin_loader::PluginLoader;
use super::ExtensionError;

/// Manages background service lifecycle
pub struct ServiceManager {
    /// Running services by service_id
    services: HashMap<String, ServiceInfo>,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
        }
    }

    /// Start a service
    pub async fn start_service(
        &mut self,
        registration: &ServiceRegistration,
        loader: &mut PluginLoader,
    ) -> Result<ServiceInfo, ExtensionError> {
        let service_id = format!("{}:{}", registration.plugin_id, registration.id);

        // Check if already running
        if let Some(info) = self.services.get(&service_id) {
            if info.state == ServiceState::Running {
                return Ok(info.clone());
            }
        }

        // Update state to starting
        let mut info = ServiceInfo {
            id: registration.id.clone(),
            plugin_id: registration.plugin_id.clone(),
            name: registration.name.clone(),
            state: ServiceState::Starting,
            started_at: None,
            error: None,
        };
        self.services.insert(service_id.clone(), info.clone());

        // Call start handler
        let result: ServiceResult = match loader
            .call_tool(&registration.plugin_id, &registration.start_handler, serde_json::json!({}))
            .await
        {
            Ok(value) => serde_json::from_value(value).unwrap_or(ServiceResult::ok()),
            Err(e) => {
                info.state = ServiceState::Failed;
                info.error = Some(e.to_string());
                self.services.insert(service_id, info.clone());
                return Err(e);
            }
        };

        if result.success {
            info.state = ServiceState::Running;
            info.started_at = Some(Utc::now());
        } else {
            info.state = ServiceState::Failed;
            info.error = result.message;
        }

        self.services.insert(service_id, info.clone());
        Ok(info)
    }

    /// Stop a service
    pub async fn stop_service(
        &mut self,
        registration: &ServiceRegistration,
        loader: &mut PluginLoader,
    ) -> Result<ServiceInfo, ExtensionError> {
        let service_id = format!("{}:{}", registration.plugin_id, registration.id);

        let mut info = self.services.get(&service_id).cloned().unwrap_or(ServiceInfo {
            id: registration.id.clone(),
            plugin_id: registration.plugin_id.clone(),
            name: registration.name.clone(),
            state: ServiceState::Stopped,
            started_at: None,
            error: None,
        });

        if info.state != ServiceState::Running {
            return Ok(info);
        }

        info.state = ServiceState::Stopping;
        self.services.insert(service_id.clone(), info.clone());

        // Call stop handler
        let result: ServiceResult = match loader
            .call_tool(&registration.plugin_id, &registration.stop_handler, serde_json::json!({}))
            .await
        {
            Ok(value) => serde_json::from_value(value).unwrap_or(ServiceResult::ok()),
            Err(e) => ServiceResult::error(e.to_string()),
        };

        info.state = if result.success {
            ServiceState::Stopped
        } else {
            ServiceState::Failed
        };
        info.error = result.message;

        self.services.insert(service_id, info.clone());
        Ok(info)
    }

    /// Get service status
    pub fn get_service(&self, plugin_id: &str, service_id: &str) -> Option<&ServiceInfo> {
        let key = format!("{}:{}", plugin_id, service_id);
        self.services.get(&key)
    }

    /// List all services
    pub fn list_services(&self) -> Vec<&ServiceInfo> {
        self.services.values().collect()
    }

    /// Stop all services for a plugin (used when unloading plugin)
    pub async fn stop_plugin_services(
        &mut self,
        plugin_id: &str,
        registrations: &[ServiceRegistration],
        loader: &mut PluginLoader,
    ) -> Vec<ServiceInfo> {
        let mut results = Vec::new();

        for reg in registrations.iter().filter(|r| r.plugin_id == plugin_id) {
            if let Ok(info) = self.stop_service(reg, loader).await {
                results.push(info);
            }
        }

        results
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_manager_new() {
        let manager = ServiceManager::new();
        assert!(manager.list_services().is_empty());
    }
}
```

**Step 2: Register module**

In `core/src/extension/mod.rs`, add:

```rust
mod service_manager;
pub use service_manager::ServiceManager;
```

**Step 3: Run tests**

Run: `cd core && cargo test service_manager`

**Step 4: Commit**

```bash
git commit -am "feat(extension): add ServiceManager for background services"
```

---

### Task P1.3: Integrate ServiceManager with ExtensionManager

**Files:**
- Modify: `core/src/extension/mod.rs`

**Step 1: Add ServiceManager field**

Add to ExtensionManager struct:

```rust
/// Service lifecycle manager
service_manager: ServiceManager,
```

Initialize in constructor:

```rust
service_manager: ServiceManager::new(),
```

**Step 2: Add service methods**

```rust
/// Start a background service
pub async fn start_service(
    &mut self,
    plugin_id: &str,
    service_id: &str,
) -> ExtensionResult<ServiceInfo> {
    let registration = self.plugin_registry
        .get_service(plugin_id, service_id)
        .ok_or_else(|| ExtensionError::not_found(format!(
            "Service '{}' not found in plugin '{}'", service_id, plugin_id
        )))?
        .clone();

    let loader = self.plugin_loader.as_mut().ok_or_else(|| {
        ExtensionError::runtime_not_initialized("plugin loader")
    })?;

    self.service_manager.start_service(&registration, loader).await
}

/// Stop a background service
pub async fn stop_service(
    &mut self,
    plugin_id: &str,
    service_id: &str,
) -> ExtensionResult<ServiceInfo> {
    let registration = self.plugin_registry
        .get_service(plugin_id, service_id)
        .ok_or_else(|| ExtensionError::not_found(format!(
            "Service '{}' not found in plugin '{}'", service_id, plugin_id
        )))?
        .clone();

    let loader = self.plugin_loader.as_mut().ok_or_else(|| {
        ExtensionError::runtime_not_initialized("plugin loader")
    })?;

    self.service_manager.stop_service(&registration, loader).await
}

/// Get service status
pub fn get_service_status(&self, plugin_id: &str, service_id: &str) -> Option<ServiceInfo> {
    self.service_manager.get_service(plugin_id, service_id).cloned()
}

/// List all running services
pub fn list_services(&self) -> Vec<ServiceInfo> {
    self.service_manager.list_services().into_iter().cloned().collect()
}
```

**Step 3: Run tests**

Run: `cd core && cargo test extension`

**Step 4: Commit**

```bash
git commit -am "feat(extension): integrate ServiceManager with ExtensionManager"
```

---

### Task P1.4: Add Service Gateway Handlers

**Files:**
- Modify: `core/src/gateway/handlers/plugins.rs`

**Step 1: Add service RPC handlers**

```rust
/// Handle services.start RPC
pub async fn handle_service_start(
    request: JsonRpcRequest,
    extension_manager: Arc<RwLock<ExtensionManager>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        plugin_id: String,
        service_id: String,
    }

    let params: Params = match serde_json::from_value(request.params.unwrap_or_default()) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, -32602, format!("Invalid params: {}", e)),
    };

    let result = {
        let mut manager = extension_manager.write().await;
        manager.start_service(&params.plugin_id, &params.service_id).await
    };

    match result {
        Ok(info) => JsonRpcResponse::success(request.id, serde_json::to_value(info).unwrap()),
        Err(e) => JsonRpcResponse::error(request.id, -32000, e.to_string()),
    }
}

/// Handle services.stop RPC
pub async fn handle_service_stop(
    request: JsonRpcRequest,
    extension_manager: Arc<RwLock<ExtensionManager>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        plugin_id: String,
        service_id: String,
    }

    let params: Params = match serde_json::from_value(request.params.unwrap_or_default()) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, -32602, format!("Invalid params: {}", e)),
    };

    let result = {
        let mut manager = extension_manager.write().await;
        manager.stop_service(&params.plugin_id, &params.service_id).await
    };

    match result {
        Ok(info) => JsonRpcResponse::success(request.id, serde_json::to_value(info).unwrap()),
        Err(e) => JsonRpcResponse::error(request.id, -32000, e.to_string()),
    }
}

/// Handle services.list RPC
pub async fn handle_service_list(
    request: JsonRpcRequest,
    extension_manager: Arc<RwLock<ExtensionManager>>,
) -> JsonRpcResponse {
    let services = {
        let manager = extension_manager.read().await;
        manager.list_services()
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(services).unwrap())
}

/// Handle services.status RPC
pub async fn handle_service_status(
    request: JsonRpcRequest,
    extension_manager: Arc<RwLock<ExtensionManager>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        plugin_id: String,
        service_id: String,
    }

    let params: Params = match serde_json::from_value(request.params.unwrap_or_default()) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, -32602, format!("Invalid params: {}", e)),
    };

    let status = {
        let manager = extension_manager.read().await;
        manager.get_service_status(&params.plugin_id, &params.service_id)
    };

    match status {
        Some(info) => JsonRpcResponse::success(request.id, serde_json::to_value(info).unwrap()),
        None => JsonRpcResponse::error(request.id, -32001, "Service not found"),
    }
}
```

**Step 2: Register handlers**

```rust
registry.register("services.start", handle_service_start);
registry.register("services.stop", handle_service_stop);
registry.register("services.list", handle_service_list);
registry.register("services.status", handle_service_status);
```

**Step 3: Run tests**

Run: `cd core && cargo test gateway`

**Step 4: Commit**

```bash
git commit -am "feat(gateway): add service lifecycle RPC handlers"
```

---

### Task P1.5: Add Service Tests

**Files:**
- Modify: `core/tests/extension_v2_test.rs`

**Step 1: Add service tests**

```rust
#[test]
fn test_v2_services_full() {
    let content = r#"
[plugin]
id = "test-services"
kind = "nodejs"
entry = "dist/index.js"

[[services]]
name = "file-watcher"
description = "Watches files for changes"
start_handler = "startWatcher"
stop_handler = "stopWatcher"

[[services]]
name = "sync-daemon"
start_handler = "startSync"
stop_handler = "stopSync"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let services = manifest.services_v2.unwrap();
    assert_eq!(services.len(), 2);
    assert_eq!(services[0].name, "file-watcher");
    assert_eq!(services[0].start_handler, "startWatcher");
    assert_eq!(services[0].stop_handler, "stopWatcher");
}

#[test]
fn test_service_state_serialization() {
    use alephcore::extension::types::ServiceState;

    let running = ServiceState::Running;
    let json = serde_json::to_string(&running).unwrap();
    assert_eq!(json, "\"running\"");

    let stopped: ServiceState = serde_json::from_str("\"stopped\"").unwrap();
    assert_eq!(stopped, ServiceState::Stopped);
}
```

**Step 2: Run tests**

Run: `cd core && cargo test extension_v2`

**Step 3: Commit**

```bash
git commit -am "test(extension): add service lifecycle tests"
```

---

## Phase P2: Channels, Providers, HTTP Routes

### Task P2.1: Define Channel Plugin Types

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add channel types**

```rust
/// Channel message from external platform
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub channel_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub metadata: Option<serde_json::Value>,
}

/// Channel send request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSendRequest {
    pub conversation_id: String,
    pub content: String,
    pub reply_to: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Channel connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

impl Default for ChannelState {
    fn default() -> Self {
        ChannelState::Disconnected
    }
}

/// Channel info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub plugin_id: String,
    pub label: String,
    pub state: ChannelState,
    pub error: Option<String>,
}
```

**Step 2: Run tests**

Run: `cd core && cargo test types`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add channel plugin types"
```

---

### Task P2.2: Define Provider Plugin Types

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add provider types**

```rust
/// Provider chat request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatRequest {
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

/// Provider message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: String,
    pub content: String,
}

/// Provider chat response (non-streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatResponse {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: Option<ProviderUsage>,
}

/// Provider usage info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Provider streaming chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderStreamChunk {
    #[serde(rename = "delta")]
    Delta { content: String },
    #[serde(rename = "done")]
    Done { usage: Option<ProviderUsage> },
    #[serde(rename = "error")]
    Error { message: String },
}

/// Provider model info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelInfo {
    pub id: String,
    pub display_name: String,
    pub context_window: Option<u32>,
    pub supports_tools: bool,
    pub supports_vision: bool,
}
```

**Step 2: Run tests**

Run: `cd core && cargo test types`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add provider plugin types"
```

---

### Task P2.3: Define HTTP Route Types

**Files:**
- Modify: `core/src/extension/types.rs`

**Step 1: Add HTTP types**

```rust
/// HTTP request from plugin route
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub query: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
    pub path_params: HashMap<String, String>,
}

/// HTTP response from plugin handler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

impl HttpResponse {
    pub fn ok() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: None,
        }
    }

    pub fn json(data: serde_json::Value) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        Self {
            status: 200,
            headers,
            body: Some(data),
        }
    }

    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: Some(serde_json::json!({"error": message.into()})),
        }
    }

    pub fn not_found() -> Self {
        Self::error(404, "Not Found")
    }
}
```

**Step 2: Run tests**

Run: `cd core && cargo test types`

**Step 3: Commit**

```bash
git commit -am "feat(extension): add HTTP route types"
```

---

### Task P2.4: Create Channel Manager Skeleton

**Files:**
- Create: `core/src/extension/channel_manager.rs`

**Step 1: Create ChannelManager**

```rust
//! Channel manager for plugin-provided messaging channels

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::types::{ChannelInfo, ChannelMessage, ChannelSendRequest, ChannelState};
use super::registry::types::ChannelRegistration;
use super::plugin_loader::PluginLoader;
use super::{ExtensionError, ExtensionResult};

/// Channel handle for message passing
pub struct ChannelHandle {
    pub info: ChannelInfo,
    /// Sender for outgoing messages
    pub outgoing_tx: mpsc::Sender<ChannelSendRequest>,
    /// Receiver for incoming messages (owned by gateway)
    pub incoming_rx: Option<mpsc::Receiver<ChannelMessage>>,
}

/// Manages plugin-provided messaging channels
pub struct ChannelManager {
    channels: HashMap<String, ChannelHandle>,
}

impl ChannelManager {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    /// Connect a channel
    pub async fn connect_channel(
        &mut self,
        registration: &ChannelRegistration,
        config: serde_json::Value,
        loader: &mut PluginLoader,
    ) -> ExtensionResult<ChannelInfo> {
        let channel_id = format!("{}:{}", registration.plugin_id, registration.id);

        // Create message channels
        let (outgoing_tx, _outgoing_rx) = mpsc::channel(100);
        let (incoming_tx, incoming_rx) = mpsc::channel(100);

        let mut info = ChannelInfo {
            id: registration.id.clone(),
            plugin_id: registration.plugin_id.clone(),
            label: registration.label.clone(),
            state: ChannelState::Connecting,
            error: None,
        };

        // Call connect handler
        let connect_result = loader
            .call_tool(
                &registration.plugin_id,
                "connect",
                serde_json::json!({
                    "channel_id": registration.id,
                    "config": config,
                }),
            )
            .await;

        match connect_result {
            Ok(_) => {
                info.state = ChannelState::Connected;
            }
            Err(e) => {
                info.state = ChannelState::Failed;
                info.error = Some(e.to_string());
                return Err(e);
            }
        }

        let handle = ChannelHandle {
            info: info.clone(),
            outgoing_tx,
            incoming_rx: Some(incoming_rx),
        };

        self.channels.insert(channel_id, handle);
        Ok(info)
    }

    /// Disconnect a channel
    pub async fn disconnect_channel(
        &mut self,
        plugin_id: &str,
        channel_id: &str,
        loader: &mut PluginLoader,
    ) -> ExtensionResult<()> {
        let key = format!("{}:{}", plugin_id, channel_id);

        if let Some(mut handle) = self.channels.remove(&key) {
            // Call disconnect handler
            let _ = loader
                .call_tool(
                    plugin_id,
                    "disconnect",
                    serde_json::json!({"channel_id": channel_id}),
                )
                .await;

            handle.info.state = ChannelState::Disconnected;
        }

        Ok(())
    }

    /// List all channels
    pub fn list_channels(&self) -> Vec<ChannelInfo> {
        self.channels.values().map(|h| h.info.clone()).collect()
    }

    /// Get channel info
    pub fn get_channel(&self, plugin_id: &str, channel_id: &str) -> Option<ChannelInfo> {
        let key = format!("{}:{}", plugin_id, channel_id);
        self.channels.get(&key).map(|h| h.info.clone())
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 2: Register module**

In `core/src/extension/mod.rs`:

```rust
mod channel_manager;
pub use channel_manager::ChannelManager;
```

**Step 3: Run tests**

Run: `cd core && cargo test channel_manager`

**Step 4: Commit**

```bash
git commit -am "feat(extension): add ChannelManager skeleton for plugin channels"
```

---

### Task P2.5: Create Provider Adapter Skeleton

**Files:**
- Create: `core/src/extension/provider_adapter.rs`

**Step 1: Create PluginProvider adapter**

```rust
//! Provider adapter for plugin-provided AI models

use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::RwLock;

use super::types::{ProviderChatRequest, ProviderChatResponse, ProviderModelInfo};
use super::registry::types::ProviderRegistration;
use super::plugin_loader::PluginLoader;
use super::ExtensionResult;

/// Adapter that wraps a plugin provider to implement the core AiProvider trait
pub struct PluginProviderAdapter {
    registration: ProviderRegistration,
    loader: Arc<RwLock<PluginLoader>>,
}

impl PluginProviderAdapter {
    pub fn new(registration: ProviderRegistration, loader: Arc<RwLock<PluginLoader>>) -> Self {
        Self { registration, loader }
    }

    /// Get provider ID
    pub fn id(&self) -> &str {
        &self.registration.id
    }

    /// Get plugin ID
    pub fn plugin_id(&self) -> &str {
        &self.registration.plugin_id
    }

    /// List available models
    pub async fn list_models(&self) -> ExtensionResult<Vec<ProviderModelInfo>> {
        let mut loader = self.loader.write().await;

        let result = loader
            .call_tool(
                &self.registration.plugin_id,
                "listModels",
                serde_json::json!({}),
            )
            .await?;

        Ok(serde_json::from_value(result).unwrap_or_else(|_| {
            // Fallback to static models from registration
            self.registration
                .models
                .iter()
                .map(|m| ProviderModelInfo {
                    id: m.clone(),
                    display_name: m.clone(),
                    context_window: None,
                    supports_tools: false,
                    supports_vision: false,
                })
                .collect()
        }))
    }

    /// Generate chat completion (non-streaming)
    pub async fn chat(&self, request: ProviderChatRequest) -> ExtensionResult<ProviderChatResponse> {
        let mut loader = self.loader.write().await;

        let result = loader
            .call_tool(
                &self.registration.plugin_id,
                "chat",
                serde_json::to_value(&request).unwrap(),
            )
            .await?;

        Ok(serde_json::from_value(result).map_err(|e| {
            super::ExtensionError::invalid_response(format!("Invalid chat response: {}", e))
        })?)
    }

    // TODO: Add streaming support with async generators
}
```

**Step 2: Register module**

In `core/src/extension/mod.rs`:

```rust
mod provider_adapter;
pub use provider_adapter::PluginProviderAdapter;
```

**Step 3: Run tests**

Run: `cd core && cargo test provider_adapter`

**Step 4: Commit**

```bash
git commit -am "feat(extension): add PluginProviderAdapter for plugin AI providers"
```

---

### Task P2.6: Create HTTP Route Handler Skeleton

**Files:**
- Create: `core/src/extension/http_handler.rs`

**Step 1: Create HTTP handler**

```rust
//! HTTP route handler for plugin-provided REST endpoints

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::types::{HttpRequest, HttpResponse};
use super::registry::types::HttpRouteRegistration;
use super::plugin_loader::PluginLoader;
use super::{ExtensionError, ExtensionResult};

/// Handles HTTP requests to plugin routes
pub struct PluginHttpHandler {
    routes: Vec<HttpRouteRegistration>,
    loader: Arc<RwLock<PluginLoader>>,
}

impl PluginHttpHandler {
    pub fn new(loader: Arc<RwLock<PluginLoader>>) -> Self {
        Self {
            routes: Vec::new(),
            loader,
        }
    }

    /// Register routes from a plugin
    pub fn register_routes(&mut self, routes: Vec<HttpRouteRegistration>) {
        self.routes.extend(routes);
    }

    /// Unregister routes for a plugin
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        self.routes.retain(|r| r.plugin_id != plugin_id);
    }

    /// Find matching route
    pub fn find_route(&self, method: &str, path: &str) -> Option<(&HttpRouteRegistration, HashMap<String, String>)> {
        for route in &self.routes {
            if !route.methods.iter().any(|m| m.eq_ignore_ascii_case(method)) {
                continue;
            }

            if let Some(params) = match_path(&route.path, path) {
                return Some((route, params));
            }
        }
        None
    }

    /// Handle an HTTP request
    pub async fn handle_request(&self, request: HttpRequest) -> ExtensionResult<HttpResponse> {
        let (route, path_params) = self
            .find_route(&request.method, &request.path)
            .ok_or_else(|| ExtensionError::not_found("Route not found"))?;

        let mut request = request;
        request.path_params = path_params;

        let mut loader = self.loader.write().await;

        let result = loader
            .call_tool(
                &route.plugin_id,
                &route.handler,
                serde_json::to_value(&request).unwrap(),
            )
            .await?;

        Ok(serde_json::from_value(result).unwrap_or_else(|_| HttpResponse::ok()))
    }

    /// List all registered routes
    pub fn list_routes(&self) -> &[HttpRouteRegistration] {
        &self.routes
    }
}

/// Match path pattern against actual path, returning captured parameters
fn match_path(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if pattern_parts.len() != path_parts.len() {
        return None;
    }

    let mut params = HashMap::new();

    for (pattern_part, path_part) in pattern_parts.iter().zip(path_parts.iter()) {
        if pattern_part.starts_with('{') && pattern_part.ends_with('}') {
            let param_name = &pattern_part[1..pattern_part.len() - 1];
            params.insert(param_name.to_string(), path_part.to_string());
        } else if pattern_part != path_part {
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
        let params = match_path("/api/users", "/api/users");
        assert!(params.is_some());
        assert!(params.unwrap().is_empty());
    }

    #[test]
    fn test_match_path_with_param() {
        let params = match_path("/api/users/{id}", "/api/users/123");
        assert!(params.is_some());
        let params = params.unwrap();
        assert_eq!(params.get("id"), Some(&"123".to_string()));
    }

    #[test]
    fn test_match_path_multiple_params() {
        let params = match_path("/api/{resource}/{id}", "/api/posts/456");
        assert!(params.is_some());
        let params = params.unwrap();
        assert_eq!(params.get("resource"), Some(&"posts".to_string()));
        assert_eq!(params.get("id"), Some(&"456".to_string()));
    }

    #[test]
    fn test_match_path_no_match() {
        let params = match_path("/api/users", "/api/posts");
        assert!(params.is_none());
    }
}
```

**Step 2: Register module**

In `core/src/extension/mod.rs`:

```rust
mod http_handler;
pub use http_handler::PluginHttpHandler;
```

**Step 3: Run tests**

Run: `cd core && cargo test http_handler`

**Step 4: Commit**

```bash
git commit -am "feat(extension): add PluginHttpHandler for plugin REST routes"
```

---

### Task P2.7: Add P2 Integration Tests

**Files:**
- Modify: `core/tests/extension_v2_test.rs`

**Step 1: Add P2 feature tests**

```rust
#[test]
fn test_v2_channels_parsing() {
    let content = r#"
[plugin]
id = "test-channels"
kind = "nodejs"

[[channels]]
id = "slack"
label = "Slack"
handler = "handleSlackChannel"

[channels.config_schema]
token = { type = "string", sensitive = true }
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();
    // Verify parsing (channels_v2 field if added)
}

#[test]
fn test_v2_providers_parsing() {
    let content = r#"
[plugin]
id = "test-providers"
kind = "nodejs"

[[providers]]
id = "custom-llm"
name = "Custom LLM"
models = ["model-fast", "model-quality"]
handler = "handleChat"

[providers.config_schema]
api_key = { type = "string", sensitive = true }
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();
    // Verify parsing (providers_v2 field if added)
}

#[test]
fn test_v2_http_routes_parsing() {
    let content = r#"
[plugin]
id = "test-http"
kind = "nodejs"

[[http_routes]]
path = "/api/data"
methods = ["GET", "POST"]
handler = "handleData"

[[http_routes]]
path = "/api/items/{id}"
methods = ["GET", "PUT", "DELETE"]
handler = "handleItem"
"#;

    let manifest = parse_aether_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();
    // Verify parsing (http_routes_v2 field if added)
}

#[test]
fn test_http_path_matching() {
    use alephcore::extension::http_handler::match_path;

    // Exact match
    assert!(match_path("/api/users", "/api/users").is_some());

    // Parameter match
    let params = match_path("/api/users/{id}", "/api/users/123").unwrap();
    assert_eq!(params.get("id"), Some(&"123".to_string()));

    // No match
    assert!(match_path("/api/users", "/api/posts").is_none());
}
```

**Step 2: Run tests**

Run: `cd core && cargo test extension_v2`

**Step 3: Commit**

```bash
git commit -am "test(extension): add P2 channel/provider/http tests"
```

---

### Task P2.8: Update Documentation

**Files:**
- Modify: `docs/EXTENSION_SYSTEM.md`

**Step 1: Add P0.5 - P2 documentation**

Add sections for:
- Direct Commands (P0.5)
- Background Services (P1)
- Channel Plugins (P2)
- Provider Plugins (P2)
- HTTP Routes (P2)

Include manifest examples, handler signatures, and lifecycle descriptions.

**Step 2: Commit**

```bash
git commit -am "docs(extension): add P0.5-P2 feature documentation"
```

---

## Summary

| Phase | Tasks | Features |
|-------|-------|----------|
| **P0.5** | P0.5.1-P0.5.5 | Direct Commands - bypass LLM execution |
| **P1** | P1.1-P1.5 | Background Services - lifecycle management |
| **P2** | P2.1-P2.8 | Channels, Providers, HTTP Routes |

**Total tasks:** 18
**Estimated commits:** 18
