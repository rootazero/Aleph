//! Plugin Runtime Systems
//!
//! Provides execution environments for different plugin types:
//! - WASM (Extism) - Sandboxed WebAssembly execution
//! - Node.js (IPC) - JavaScript/TypeScript via subprocess
//! - Static - Markdown-based skills/commands/agents
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐    ┌─────────────────┐    ┌────────────────┐
//! │  WasmRuntime    │    │  NpmInstaller   │    │  PluginRuntime │
//! │                 │    │                 │    │                │
//! │  - Extism       │    │  - fnm/node     │    │  - Load .ts/.js│
//! │  - Sandboxed    │    │  - npm install  │    │  - IPC (JSON)  │
//! └─────────────────┘    └─────────────────┘    └────────────────┘
//! ```
//!
//! # Usage
//!
//! ## Node.js Runtime
//!
//! ```rust,ignore
//! use aethecore::extension::runtime::{NpmInstaller, PluginRuntime};
//! use aethecore::runtimes::RuntimeRegistry;
//!
//! let registry = RuntimeRegistry::new(runtimes_dir)?;
//! let installer = NpmInstaller::new(registry);
//!
//! // Install an npm package
//! installer.install("@anthropic/aether-plugin@1.0.0").await?;
//!
//! // Run a JS plugin
//! let runtime = PluginRuntime::new(&registry)?;
//! runtime.load_plugin("file:///path/to/plugin.ts").await?;
//! ```
//!
//! ## Node.js Runtime (Synchronous)
//!
//! ```rust,ignore
//! use aethecore::extension::runtime::{NodeJsRuntime, JsonRpcRequest, JsonRpcResponse};
//!
//! let mut runtime = NodeJsRuntime::new("/usr/bin/node", "/path/to/host.js");
//! let registrations = runtime.load_plugin(&manifest)?;
//! let result = runtime.call_tool("plugin-id", "handler", serde_json::json!({"key": "value"}))?;
//! ```
//!
//! ## WASM Runtime
//!
//! ```rust,ignore
//! use aethecore::extension::runtime::{WasmRuntime, WasmToolInput};
//!
//! let mut runtime = WasmRuntime::new();
//! runtime.load_plugin(&manifest)?;
//!
//! let input = WasmToolInput {
//!     name: "my_tool".to_string(),
//!     arguments: serde_json::json!({"key": "value"}),
//! };
//! let output = runtime.call_tool("plugin-id", "handler", input)?;
//! ```

pub mod nodejs;
pub mod wasm;

// Re-export WASM runtime types
pub use wasm::{PermissionChecker, WasmRuntime, WasmToolInput, WasmToolOutput};

// Re-export Node.js runtime types
pub use nodejs::{HostScript, JsonRpcRequest, JsonRpcResponse, NodeJsRuntime, NodeProcess};

use crate::extension::ExtensionError;
use crate::runtimes::{get_runtimes_dir, FnmRuntime, RuntimeManager};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, info, trace, warn};

/// Default timeout for npm operations (5 minutes)
const NPM_TIMEOUT_SECS: u64 = 300;

/// Default timeout for IPC operations (300 seconds)
const IPC_TIMEOUT_SECS: u64 = 300;

/// npm package installer using Aether's fnm/Node.js runtime
pub struct NpmInstaller {
    /// fnm runtime
    fnm: FnmRuntime,
    /// Installation directory (e.g., ~/.aether/plugins/node_modules/)
    install_dir: PathBuf,
}

impl NpmInstaller {
    /// Create a new npm installer
    pub fn new(install_dir: PathBuf) -> Result<Self, ExtensionError> {
        let runtimes_dir = get_runtimes_dir().map_err(|e| {
            ExtensionError::Runtime(format!("Failed to get runtimes directory: {}", e))
        })?;
        let fnm = FnmRuntime::new(runtimes_dir);
        Ok(Self { fnm, install_dir })
    }

    /// Create with explicit runtimes directory
    pub fn with_runtimes_dir(runtimes_dir: PathBuf, install_dir: PathBuf) -> Self {
        let fnm = FnmRuntime::new(runtimes_dir);
        Self { fnm, install_dir }
    }

    /// Get the node_modules directory
    pub fn node_modules_dir(&self) -> PathBuf {
        self.install_dir.join("node_modules")
    }

    /// Ensure Node.js is installed via fnm
    async fn ensure_node(&self) -> Result<&FnmRuntime, ExtensionError> {
        if !self.fnm.is_installed() {
            info!("Node.js not installed, installing via fnm...");
            self.fnm.install().await.map_err(|e| {
                ExtensionError::Runtime(format!("Failed to install Node.js: {}", e))
            })?;
        }

        Ok(&self.fnm)
    }

    /// Install an npm package
    ///
    /// Supports formats:
    /// - `package-name` (latest)
    /// - `package-name@version`
    /// - `@scope/package@version`
    pub async fn install(&self, package: &str) -> Result<PathBuf, ExtensionError> {
        let fnm = self.ensure_node().await?;
        let (name, version) = parse_package_spec(package);

        info!("Installing npm package: {}@{}", name, version);

        // Ensure install directory exists
        std::fs::create_dir_all(&self.install_dir).map_err(|e| {
            ExtensionError::Runtime(format!("Failed to create install directory: {}", e))
        })?;

        // Create package.json if it doesn't exist
        let package_json = self.install_dir.join("package.json");
        if !package_json.exists() {
            let content = r#"{"name": "aether-plugins", "version": "1.0.0", "private": true}"#;
            std::fs::write(&package_json, content).map_err(|e| {
                ExtensionError::Runtime(format!("Failed to create package.json: {}", e))
            })?;
        }

        // Run npm install
        let package_spec = if version == "latest" {
            name.clone()
        } else {
            format!("{}@{}", name, version)
        };

        let mut cmd = Command::new(fnm.npm_path());
        cmd.current_dir(&self.install_dir)
            .args(["install", "--save", &package_spec])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        debug!("Running: npm install --save {}", package_spec);

        let output = timeout(Duration::from_secs(NPM_TIMEOUT_SECS), cmd.output())
            .await
            .map_err(|_| ExtensionError::Runtime("npm install timed out".to_string()))?
            .map_err(|e| ExtensionError::Runtime(format!("Failed to run npm: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExtensionError::Runtime(format!(
                "npm install failed: {}",
                stderr
            )));
        }

        // Return path to installed package
        let package_path = self.node_modules_dir().join(&name);
        if !package_path.exists() {
            return Err(ExtensionError::Runtime(format!(
                "Package installed but not found at {:?}",
                package_path
            )));
        }

        info!("Package {} installed successfully", name);
        Ok(package_path)
    }

    /// Check if a package is installed
    pub fn is_installed(&self, package: &str) -> bool {
        let (name, _) = parse_package_spec(package);
        self.node_modules_dir().join(&name).exists()
    }

    /// Get the path to an installed package
    pub fn get_package_path(&self, package: &str) -> Option<PathBuf> {
        let (name, _) = parse_package_spec(package);
        let path = self.node_modules_dir().join(&name);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// Uninstall a package
    pub async fn uninstall(&self, package: &str) -> Result<(), ExtensionError> {
        let fnm = self.ensure_node().await?;
        let (name, _) = parse_package_spec(package);

        if !self.is_installed(&name) {
            return Ok(()); // Already uninstalled
        }

        info!("Uninstalling npm package: {}", name);

        let mut cmd = Command::new(fnm.npm_path());
        cmd.current_dir(&self.install_dir)
            .args(["uninstall", &name])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = timeout(Duration::from_secs(NPM_TIMEOUT_SECS), cmd.output())
            .await
            .map_err(|_| ExtensionError::Runtime("npm uninstall timed out".to_string()))?
            .map_err(|e| ExtensionError::Runtime(format!("Failed to run npm: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ExtensionError::Runtime(format!(
                "npm uninstall failed: {}",
                stderr
            )));
        }

        info!("Package {} uninstalled successfully", name);
        Ok(())
    }

    /// List installed packages
    pub async fn list_installed(&self) -> Result<Vec<InstalledPackage>, ExtensionError> {
        let fnm = self.ensure_node().await?;

        if !self.node_modules_dir().exists() {
            return Ok(Vec::new());
        }

        let mut cmd = Command::new(fnm.npm_path());
        cmd.current_dir(&self.install_dir)
            .args(["list", "--json", "--depth=0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let output = cmd
            .output()
            .await
            .map_err(|e| ExtensionError::Runtime(format!("Failed to run npm list: {}", e)))?;

        if output.stdout.is_empty() {
            return Ok(Vec::new());
        }

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to parse npm list output: {}", e)))?;

        let mut packages = Vec::new();
        if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
            for (name, info) in deps {
                let version = info
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                packages.push(InstalledPackage {
                    name: name.clone(),
                    version,
                    path: self.node_modules_dir().join(name),
                });
            }
        }

        Ok(packages)
    }
}

/// Information about an installed npm package
#[derive(Debug, Clone)]
pub struct InstalledPackage {
    /// Package name
    pub name: String,
    /// Installed version
    pub version: String,
    /// Path to package directory
    pub path: PathBuf,
}

/// Parse npm package specifier
///
/// Returns (name, version) tuple.
pub fn parse_package_spec(spec: &str) -> (String, String) {
    // Handle scoped packages (@scope/package@version)
    if spec.starts_with('@') {
        // Find the @ that separates name from version (not the scope @)
        if let Some(at_pos) = spec[1..].find('@') {
            // at_pos is relative to spec[1..], so add 1 to get position in spec
            let name = spec[..at_pos + 1].to_string();
            let version = spec[at_pos + 2..].to_string();
            return (name, version);
        }
        return (spec.to_string(), "latest".to_string());
    }

    // Handle regular packages (package@version)
    if let Some(at_pos) = spec.find('@') {
        let name = spec[..at_pos].to_string();
        let version = spec[at_pos + 1..].to_string();
        return (name, version);
    }

    (spec.to_string(), "latest".to_string())
}

/// Plugin runtime for JavaScript/TypeScript plugins
///
/// Manages a Node.js child process that hosts JS plugins and communicates
/// via JSON-RPC over stdin/stdout.
pub struct PluginRuntime {
    /// fnm runtime
    fnm: FnmRuntime,
    /// Plugin host process (if running)
    host: Mutex<Option<PluginHost>>,
    /// Working directory for the host
    working_dir: PathBuf,
}

impl PluginRuntime {
    /// Create a new plugin runtime
    pub fn new(working_dir: PathBuf) -> Result<Self, ExtensionError> {
        let runtimes_dir = get_runtimes_dir().map_err(|e| {
            ExtensionError::Runtime(format!("Failed to get runtimes directory: {}", e))
        })?;
        let fnm = FnmRuntime::new(runtimes_dir);
        Ok(Self {
            fnm,
            host: Mutex::new(None),
            working_dir,
        })
    }

    /// Create with explicit runtimes directory
    pub fn with_runtimes_dir(runtimes_dir: PathBuf, working_dir: PathBuf) -> Self {
        let fnm = FnmRuntime::new(runtimes_dir);
        Self {
            fnm,
            host: Mutex::new(None),
            working_dir,
        }
    }

    /// Check if the host process is running
    pub async fn is_running(&self) -> bool {
        let host = self.host.lock().await;
        host.is_some()
    }

    /// Start the plugin host process
    pub async fn start(&self) -> Result<(), ExtensionError> {
        let mut host_guard = self.host.lock().await;

        if host_guard.is_some() {
            return Ok(()); // Already running
        }

        if !self.fnm.is_installed() {
            return Err(ExtensionError::Runtime(
                "Node.js not installed. Run initialization first.".to_string(),
            ));
        }

        info!("Starting plugin host process");

        // Create the host script
        let host_script = create_host_script();
        let script_path = self.working_dir.join(".plugin-host.mjs");
        std::fs::write(&script_path, host_script).map_err(|e| {
            ExtensionError::Runtime(format!("Failed to write host script: {}", e))
        })?;

        // Start Node.js with the host script
        let mut child = Command::new(self.fnm.node_path())
            .arg(&script_path)
            .current_dir(&self.working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ExtensionError::Runtime(format!("Failed to start plugin host: {}", e)))?;

        // Set up I/O
        let stdin = child.stdin.take().ok_or_else(|| {
            ExtensionError::Runtime("Failed to get plugin host stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ExtensionError::Runtime("Failed to get plugin host stdout".to_string())
        })?;

        let stderr = child.stderr.take().ok_or_else(|| {
            ExtensionError::Runtime("Failed to get plugin host stderr".to_string())
        })?;

        // Start stderr logging task
        let stderr_reader = BufReader::new(stderr);
        tokio::spawn(async move {
            let mut lines = stderr_reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!("[plugin-host stderr] {}", line);
            }
        });

        let host = PluginHost {
            child,
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            loaded_plugins: Mutex::new(HashMap::new()),
        };

        *host_guard = Some(host);

        info!("Plugin host started successfully");
        Ok(())
    }

    /// Stop the plugin host process
    pub async fn stop(&self) -> Result<(), ExtensionError> {
        let mut host_guard = self.host.lock().await;

        if let Some(mut host) = host_guard.take() {
            info!("Stopping plugin host process");

            // Send shutdown command
            let _ = host.send_command("shutdown", serde_json::json!({})).await;

            // Kill the process
            let _ = host.child.kill().await;

            info!("Plugin host stopped");
        }

        Ok(())
    }

    /// Load a plugin
    pub async fn load_plugin(&self, path: &Path) -> Result<String, ExtensionError> {
        let host_guard = self.host.lock().await;
        let host = host_guard
            .as_ref()
            .ok_or_else(|| ExtensionError::Runtime("Plugin host not running".to_string()))?;

        let plugin_id = path.to_string_lossy().to_string();

        debug!("Loading plugin: {}", plugin_id);

        let response = host
            .send_command(
                "load",
                serde_json::json!({
                    "id": plugin_id,
                    "path": path.to_string_lossy()
                }),
            )
            .await?;

        if let Some(error) = response.get("error") {
            return Err(ExtensionError::Runtime(format!(
                "Failed to load plugin: {}",
                error
            )));
        }

        // Track loaded plugin
        host.loaded_plugins.lock().await.insert(
            plugin_id.clone(),
            LoadedPlugin {},
        );

        info!("Plugin loaded: {}", plugin_id);
        Ok(plugin_id)
    }

    /// Unload a plugin
    pub async fn unload_plugin(&self, plugin_id: &str) -> Result<(), ExtensionError> {
        let host_guard = self.host.lock().await;
        let host = host_guard
            .as_ref()
            .ok_or_else(|| ExtensionError::Runtime("Plugin host not running".to_string()))?;

        debug!("Unloading plugin: {}", plugin_id);

        let response = host
            .send_command("unload", serde_json::json!({ "id": plugin_id }))
            .await?;

        if let Some(error) = response.get("error") {
            return Err(ExtensionError::Runtime(format!(
                "Failed to unload plugin: {}",
                error
            )));
        }

        host.loaded_plugins.lock().await.remove(plugin_id);

        info!("Plugin unloaded: {}", plugin_id);
        Ok(())
    }

    /// Execute a hook in a plugin
    pub async fn execute_hook(
        &self,
        plugin_id: &str,
        hook_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let host_guard = self.host.lock().await;
        let host = host_guard
            .as_ref()
            .ok_or_else(|| ExtensionError::Runtime("Plugin host not running".to_string()))?;

        trace!("Executing hook {} in plugin {}", hook_name, plugin_id);

        let response = host
            .send_command(
                "executeHook",
                serde_json::json!({
                    "pluginId": plugin_id,
                    "hook": hook_name,
                    "args": args
                }),
            )
            .await?;

        if let Some(error) = response.get("error") {
            return Err(ExtensionError::Runtime(format!(
                "Hook execution failed: {}",
                error
            )));
        }

        Ok(response.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }
}

/// Plugin host process wrapper
struct PluginHost {
    child: Child,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    loaded_plugins: Mutex<HashMap<String, LoadedPlugin>>,
}

impl PluginHost {
    /// Send a JSON-RPC command and wait for response
    async fn send_command(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": uuid::Uuid::new_v4().to_string(),
            "method": method,
            "params": params
        });

        let request_str = serde_json::to_string(&request).map_err(|e| {
            ExtensionError::Runtime(format!("Failed to serialize request: {}", e))
        })?;

        trace!("Sending to plugin host: {}", request_str);

        // Send request
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(request_str.as_bytes())
                .await
                .map_err(|e| ExtensionError::Runtime(format!("Failed to write to host: {}", e)))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| ExtensionError::Runtime(format!("Failed to write newline: {}", e)))?;
            stdin.flush().await.map_err(|e| {
                ExtensionError::Runtime(format!("Failed to flush stdin: {}", e))
            })?;
        }

        // Read response with timeout
        let response = {
            let mut stdout = self.stdout.lock().await;
            timeout(Duration::from_secs(IPC_TIMEOUT_SECS), async {
                let mut line = String::new();
                stdout.read_line(&mut line).await.map_err(|e| {
                    ExtensionError::Runtime(format!("Failed to read from host: {}", e))
                })?;
                Ok::<_, ExtensionError>(line)
            })
            .await
            .map_err(|_| ExtensionError::Runtime("Host response timeout".to_string()))??
        };

        trace!("Received from plugin host: {}", response.trim());

        let response: serde_json::Value = serde_json::from_str(&response).map_err(|e| {
            ExtensionError::Runtime(format!("Failed to parse host response: {}", e))
        })?;

        // Check for JSON-RPC error
        if let Some(error) = response.get("error") {
            return Err(ExtensionError::Runtime(format!("Host error: {}", error)));
        }

        Ok(response.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }
}

/// Information about a loaded plugin
#[derive(Debug, Clone)]
struct LoadedPlugin {
    // All fields are redundant with HashMap keys
    // Kept as placeholder for future metadata
}

/// Create the plugin host script (ESM)
fn create_host_script() -> &'static str {
    r#"
import * as readline from 'readline';

const plugins = new Map();

const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false
});

function sendResponse(id, result, error = null) {
    const response = {
        jsonrpc: '2.0',
        id,
        ...(error ? { error: { message: error } } : { result })
    };
    console.log(JSON.stringify(response));
}

async function handleRequest(request) {
    const { id, method, params } = request;

    try {
        switch (method) {
            case 'load': {
                const { id: pluginId, path } = params;
                const module = await import(path);
                plugins.set(pluginId, module);
                sendResponse(id, { success: true, pluginId });
                break;
            }

            case 'unload': {
                const { id: pluginId } = params;
                plugins.delete(pluginId);
                sendResponse(id, { success: true });
                break;
            }

            case 'executeHook': {
                const { pluginId, hook, args } = params;
                const plugin = plugins.get(pluginId);
                if (!plugin) {
                    sendResponse(id, null, `Plugin not loaded: ${pluginId}`);
                    return;
                }
                const hookFn = plugin[hook] || plugin.default?.[hook];
                if (typeof hookFn !== 'function') {
                    sendResponse(id, null, `Hook not found: ${hook}`);
                    return;
                }
                const result = await hookFn(args);
                sendResponse(id, { result });
                break;
            }

            case 'shutdown': {
                sendResponse(id, { success: true });
                process.exit(0);
                break;
            }

            default:
                sendResponse(id, null, `Unknown method: ${method}`);
        }
    } catch (err) {
        sendResponse(id, null, err.message);
    }
}

rl.on('line', async (line) => {
    try {
        const request = JSON.parse(line);
        await handleRequest(request);
    } catch (err) {
        console.error('Parse error:', err.message);
    }
});

// Keep alive
process.on('uncaughtException', (err) => {
    console.error('Uncaught exception:', err);
});
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package_spec() {
        assert_eq!(
            parse_package_spec("my-package"),
            ("my-package".to_string(), "latest".to_string())
        );

        assert_eq!(
            parse_package_spec("my-package@1.0.0"),
            ("my-package".to_string(), "1.0.0".to_string())
        );

        assert_eq!(
            parse_package_spec("@scope/package"),
            ("@scope/package".to_string(), "latest".to_string())
        );

        assert_eq!(
            parse_package_spec("@scope/package@2.0.0"),
            ("@scope/package".to_string(), "2.0.0".to_string())
        );
    }
}
