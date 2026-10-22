//! WASM Plugin Runtime using Extism
//!
//! Provides sandboxed execution of WASM plugins with permission-based
//! access to host functions.

mod allowlist;
mod capabilities;
mod capability_kernel;
mod credential_injector;
mod host_functions;
mod limits;
mod secret_resolver;

pub use allowlist::{AllowlistError, AllowlistValidator};
pub use capabilities::{
    host_matches_pattern, CredentialBinding, CredentialInject, EndpointPattern, HttpCapability,
    RateLimit, SecretsCapability, WasmCapabilities, WorkspaceCapability,
};
pub use capability_kernel::{CapabilityError, WasmCapabilityKernel};
pub use credential_injector::{inject_credential, CredentialError};
pub use limits::WasmResourceLimits;
pub use secret_resolver::{
    shared_resolver, DenyAllSecretResolver, InMemorySecretResolver, SecretResolver,
    VaultBackedSecretResolver,
};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::PluginManifest;

use crate::sync_primitives::{Arc, Mutex};

use extism::{Manifest as ExtismManifest, PluginBuilder, UserData, Wasm, PTR};

/// Input for WASM tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolInput {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Output from WASM tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// WASM plugin runtime manager
#[derive(Default)]
pub struct WasmRuntime {
    plugins: HashMap<String, Arc<LoadedWasmPlugin>>,
}

#[derive(Debug)]
struct LoadedWasmPlugin {
    plugin: Mutex<extism::Plugin>,
    // Captured from the effective configuration at build time, never from a
    // manifest supplied by a later binding caller. No per-call attenuation.
    context_effect_free: bool,
    // Immutable discovery facts of THIS incarnation, not active registrations.
    context_declarations: Vec<crate::extension::capability::ContextDeclaration>,
}

#[derive(Clone, Debug)]
pub(crate) struct WasmBinding {
    plugin_id: String,
    export: String,
    instance: Arc<LoadedWasmPlugin>,
}

impl WasmBinding {
    pub(crate) fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub(crate) fn export(&self) -> &str {
        &self.export
    }

    pub(crate) fn context_effect_free(&self) -> bool {
        self.instance.context_effect_free
    }

    pub(crate) fn context_declaration(
        &self,
        id: &str,
    ) -> Option<&crate::extension::capability::ContextDeclaration> {
        self.instance
            .context_declarations
            .iter()
            .find(|declaration| {
                declaration.descriptor.id == id && declaration.handler == self.export
            })
    }
}

/// Opaque identity of one actually mounted WASM plugin instance.
///
/// The only way to obtain one is [`WasmRuntime::mounted_instance`], which
/// clones the `Arc` the runtime's own map holds; the field is private, so no
/// caller can construct an identity that names an instance the runtime did
/// not mount, and no export needs to exist for a module to have one — a
/// service-only module has a lifetime too. Holding the identity keeps that
/// instance's VM allocation alive until the holder releases it (the same
/// trade-off [`WasmBinding`] already makes), but grants no execution after
/// unmount and promises no immediate deallocation.
#[derive(Clone)]
pub(crate) struct WasmMountIdentity {
    instance: Arc<LoadedWasmPlugin>,
}

impl WasmMountIdentity {
    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.instance, &other.instance)
    }
}

impl WasmRuntime {
    /// Create a new WASM runtime
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a WASM plugin.
    ///
    /// Installs a [`DenyAllSecretResolver`] as the secret resolver — the
    /// plugin can declare `http.credentials` but they will be no-ops until a
    /// caller installs a real resolver via
    /// [`WasmCapabilityKernel::with_secret_resolver`] (call
    /// [`Self::load_plugin_with_resolver`] with `Some(resolver)` to do so).
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> Result<(), ExtensionError> {
        self.load_plugin_with_resolver(manifest, None)
    }

    /// Load a WASM plugin with an optional host-side secret resolver.
    ///
    /// When `resolver` is `None`, the kernel falls back to
    /// [`DenyAllSecretResolver`] — outbound `http_fetch` calls then bypass
    /// credential injection and the plugin must supply its own
    /// `Authorization` header (the legacy behaviour). When the resolver is
    /// `Some`, `inject_credential` runs host-side and the plugin guest never
    /// sees the secret value.
    pub fn load_plugin_with_resolver(
        &mut self,
        manifest: &PluginManifest,
        resolver: Option<Arc<dyn SecretResolver>>,
    ) -> Result<(), ExtensionError> {
        self.load_plugin_with(manifest, resolver, &serde_json::Value::Null)
    }

    /// Say out loud that a plugin's declared credentials cannot work.
    ///
    /// Host-side credential injection is fully implemented and has no
    /// production caller that installs a resolver, so every
    /// `[capabilities.http.credentials]` binding turns the plugin's
    /// `http_fetch` to that host into a guaranteed `secret not found` error.
    /// Without this line the author discovers it at runtime, from an error
    /// that names a secret rather than the missing wire.
    ///
    /// A warning rather than a refusal: the plugin's other capabilities work,
    /// and refusing to load it would be a larger change than the gap
    /// justifies.
    fn warn_if_credentials_are_unreachable(
        manifest: &PluginManifest,
        capabilities: &WasmCapabilities,
    ) {
        let declared = capabilities
            .http
            .as_ref()
            .map_or(0, |http| http.credentials.len());
        if declared == 0 {
            return;
        }
        warn!(
            plugin = %manifest.id,
            bindings = declared,
            "plugin declares http credentials, but no host secret resolver is installed — \
             every request matching those bindings will fail with `secret not found`. \
             See extension::runtime::wasm::secret_resolver for what connecting it requires."
        );
    }

    /// Load a WASM plugin with a secret resolver and the operator's stored
    /// configuration.
    ///
    /// `settings` reaches the guest through Extism's config map, which is the
    /// mechanism `extism_pdk::config::get` reads — so a plugin author writes
    /// `config::get("api_key")` and gets the value the operator set, with no
    /// Aleph-specific host function. Passing it at build time (rather than as
    /// a call argument) is what makes it available during the guest's own
    /// initialisation.
    ///
    /// A non-object `settings` (including the `Null` the two-argument form
    /// passes) means "no configuration", not an error: a plugin with no
    /// `config_schema` must load exactly as it did before this existed.
    pub fn load_plugin_with(
        &mut self,
        manifest: &PluginManifest,
        resolver: Option<Arc<dyn SecretResolver>>,
        settings: &serde_json::Value,
    ) -> Result<(), ExtensionError> {
        let wasm_path = manifest.entry_path()?;

        if !wasm_path.exists() {
            return Err(ExtensionError::Runtime(format!(
                "WASM file not found: {wasm_path:?}"
            )));
        }

        info!("Loading WASM plugin: {} from {:?}", manifest.id, wasm_path);

        // Parse capabilities from manifest (default = zero permissions)
        let capabilities = manifest.wasm_capabilities.clone().unwrap_or_default();
        let limits = manifest.wasm_resource_limits.clone().unwrap_or_default();
        let call_timeout = std::time::Duration::from_secs(limits.timeout_secs);
        let memory_pages = limits.memory_mb.checked_mul(16).filter(|pages| *pages > 0);
        let fuel = limits.fuel;

        // Build the per-plugin kernel. The resolver is installed via the
        // builder so the kernel can be cloned freely; the default
        // deny-all resolver preserves the legacy "plugin supplies its own
        // credentials" behaviour when no resolver is provided.
        let resolver: Arc<dyn SecretResolver> = match resolver {
            Some(r) => r,
            None => {
                Self::warn_if_credentials_are_unreachable(manifest, &capabilities);
                Arc::new(DenyAllSecretResolver)
            }
        };
        let kernel = Arc::new(
            WasmCapabilityKernel::new(manifest.id.clone(), capabilities, limits)
                .with_secret_resolver(resolver),
        );

        let has_contexts = manifest
            .aleph_extensions
            .as_ref()
            .is_some_and(|extensions| !extensions.contexts.is_empty());
        // Create host state for Extism UserData. Context guests receive only
        // the audited host imports; WASI stdout/stderr and ambient I/O are not
        // part of their authority, even when an ordinary plugin enables WASI.
        let host_state = UserData::new(host_functions::HostState {
            kernel: kernel.clone(),
            workspace_root: manifest.root_dir.clone(),
            suppress_guest_logs: has_contexts,
        });

        // Enforce a wall-clock deadline on guest execution: this is
        // marketplace-supplied untrusted code, so an infinite loop must not
        // run forever. Extism interrupts the call once the deadline is hit
        // (the plugin instance cannot continue afterwards and must be
        // reloaded).
        let mut extism_manifest =
            ExtismManifest::new([Wasm::file(&wasm_path)]).with_timeout(call_timeout);
        if has_contexts {
            // WASM pages are 64 KiB; memory_mb is measured in MiB. Apply the
            // existing mounted limits to the VM, not just the host kernel.
            let pages = memory_pages.ok_or_else(|| {
                ExtensionError::Runtime(
                    "context WASM memory limit must be positive and fit in pages".into(),
                )
            })?;
            if fuel == 0 {
                return Err(ExtensionError::Runtime(
                    "context WASM fuel limit must be positive".into(),
                ));
            }
            extism_manifest = extism_manifest.with_memory_max(pages);
        }

        // The operator's configuration, as Extism config keys. Scalars are
        // passed in their natural spelling and structured values as JSON, so
        // `config::get("api_key")` returns the string an operator typed rather
        // than a quoted one.
        if let serde_json::Value::Object(map) = settings {
            for (key, value) in map {
                let rendered = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => continue,
                    other => other.to_string(),
                };
                extism_manifest = extism_manifest.with_config_key(key, rendered);
            }
        }

        let mut builder = PluginBuilder::new(extism_manifest)
            .with_wasi(!has_contexts)
            .with_function(
                "log",
                [PTR, PTR],
                [],
                host_state.clone(),
                host_functions::host_log,
            )
            .with_function(
                "now_millis",
                [],
                [PTR],
                host_state.clone(),
                host_functions::host_now_millis,
            )
            .with_function(
                "workspace_read",
                [PTR],
                [PTR],
                host_state.clone(),
                host_functions::host_workspace_read,
            )
            .with_function(
                "secret_exists",
                [PTR],
                [PTR],
                host_state.clone(),
                host_functions::host_secret_exists,
            )
            .with_function(
                "http_fetch",
                [PTR],
                [PTR],
                host_state,
                host_functions::host_http_fetch,
            );
        if has_contexts {
            let debug = extism::DebugOptions {
                coredump: None,
                memdump: None,
                ..Default::default()
            };
            builder = builder.with_fuel_limit(fuel).with_debug_options(debug);
            // Extism installs these independently of our `log` import. Its
            // linker explicitly permits namespaced host functions to shadow
            // builtins, so raw context cannot escape through PDK logging.
            for name in [
                "log_warn",
                "log_info",
                "log_debug",
                "log_error",
                "log_trace",
            ] {
                builder = builder.with_function_in_namespace(
                    "extism:host/env",
                    name,
                    [PTR],
                    [],
                    UserData::new(()),
                    |plugin, input, _, _| {
                        let Some(extism::Val::I64(offset)) = input.first() else {
                            return Ok(());
                        };
                        if let Some(handle) = plugin.memory_handle(*offset as u64) {
                            plugin.memory_free(handle)?;
                        }
                        Ok(())
                    },
                );
            }
        }
        let plugin = builder
            .build()
            .map_err(|e| ExtensionError::Runtime(format!("Failed to load WASM: {e}")))?;

        let context_declarations = manifest
            .aleph_extensions
            .as_ref()
            .map(|ext| ext.contexts.clone())
            .unwrap_or_default();
        let loaded = Arc::new(LoadedWasmPlugin {
            plugin: Mutex::new(plugin),
            context_declarations,
            context_effect_free: super::super::context_adapter::require_no_ambient_authority(
                manifest,
            )
            .is_ok()
                && (settings.is_null() || settings.as_object().is_some_and(|map| map.is_empty())),
        });

        self.plugins.insert(manifest.id.clone(), loaded);

        Ok(())
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> bool {
        self.plugins.remove(plugin_id).is_some()
    }

    /// Capture the identity of the instance currently mounted under
    /// `plugin_id`, reading only the actual map — never names, exports, or
    /// metadata. `None` when nothing is mounted under that id.
    pub(crate) fn mounted_instance(&self, plugin_id: &str) -> Option<WasmMountIdentity> {
        self.plugins
            .get(plugin_id)
            .map(|instance| WasmMountIdentity {
                instance: Arc::clone(instance),
            })
    }

    /// Remove the entry for `plugin_id` only while the map still holds the
    /// exact instance `identity` was captured from. Returns `true` when this
    /// call performed the removal; a stale or absent identity removes
    /// nothing, so a late disposer can never evict a same-id replacement.
    pub(crate) fn remove_if_current(
        &mut self,
        plugin_id: &str,
        identity: &WasmMountIdentity,
    ) -> bool {
        let is_current = self
            .plugins
            .get(plugin_id)
            .is_some_and(|current| Arc::ptr_eq(current, &identity.instance));
        if is_current {
            self.plugins.remove(plugin_id);
        }
        is_current
    }

    /// Check if a plugin is loaded
    #[must_use]
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        self.plugins.contains_key(plugin_id)
    }

    pub(crate) fn bind(
        &self,
        plugin_id: &str,
        export: &str,
    ) -> Result<WasmBinding, ExtensionError> {
        let instance = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let plugin = instance
            .plugin
            .lock()
            .map_err(|e| ExtensionError::Runtime(format!("Failed to lock plugin: {e}")))?;
        if !plugin.function_exists(export) {
            return Err(ExtensionError::Runtime(format!(
                "WASM export not found: {export}"
            )));
        }
        drop(plugin);

        Ok(WasmBinding {
            plugin_id: plugin_id.to_string(),
            export: export.to_string(),
            instance: Arc::clone(instance),
        })
    }

    pub(crate) fn binding_is_current(&self, binding: &WasmBinding) -> bool {
        self.plugins
            .get(&binding.plugin_id)
            .is_some_and(|current| Arc::ptr_eq(current, &binding.instance))
    }

    pub(crate) fn call_bound(
        &self,
        binding: &WasmBinding,
        input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        let mounted = self
            .plugins
            .get(&binding.plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(binding.plugin_id.clone()))?;
        if !Arc::ptr_eq(mounted, &binding.instance) {
            return Err(ExtensionError::Runtime(format!(
                "WASM binding no longer mounted: {}",
                binding.plugin_id
            )));
        }

        Self::call_loaded(mounted, &binding.plugin_id, &binding.export, input)
    }

    /// Call a tool handler in a WASM plugin
    pub fn call_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        let loaded = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        Self::call_loaded(loaded, plugin_id, handler, input)
    }

    fn call_loaded(
        loaded: &LoadedWasmPlugin,
        plugin_id: &str,
        handler: &str,
        input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        let input_json = serde_json::to_string(&input)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to serialize input: {e}")))?;

        debug!(plugin = plugin_id, handler, "Calling WASM handler");

        let mut plugin = loaded
            .plugin
            .lock()
            .map_err(|e| ExtensionError::Runtime(format!("Failed to lock plugin: {e}")))?;

        let result = plugin
            .call::<&str, &str>(handler, &input_json)
            .map_err(|e| ExtensionError::Runtime(format!("WASM call failed: {e}")))?;

        let output: WasmToolOutput = serde_json::from_str(result)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to parse output: {e}")))?;

        Ok(output)
    }

    /// Get list of loaded plugin IDs
    #[must_use]
    pub fn loaded_plugins(&self) -> Vec<String> {
        self.plugins.keys().cloned().collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod guest_fixture;

    use super::*;
    use crate::extension::types::PluginKind;
    use std::path::PathBuf;

    #[test]
    fn test_wasm_runtime_not_found() {
        let mut runtime = WasmRuntime::new();
        let manifest = PluginManifest::new(
            "test".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PathBuf::from("nonexistent.wasm"),
        );

        let result = runtime.load_plugin(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_wasm_runtime_call_not_loaded() {
        let runtime = WasmRuntime::new();
        let input = WasmToolInput {
            name: "test".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = runtime.call_tool("nonexistent", "handler", input);
        assert!(result.is_err());
    }

    #[test]
    fn test_wasm_runtime_loaded_plugins_empty() {
        let runtime = WasmRuntime::new();
        assert!(runtime.loaded_plugins().is_empty());
    }

    #[test]
    fn test_wasm_runtime_is_loaded_returns_false() {
        let runtime = WasmRuntime::new();
        assert!(!runtime.is_loaded("any-plugin"));
    }

    #[test]
    fn test_wasm_runtime_unload_returns_false() {
        let mut runtime = WasmRuntime::new();
        assert!(!runtime.unload_plugin("any-plugin"));
    }
}
