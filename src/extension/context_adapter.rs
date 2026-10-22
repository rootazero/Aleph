//! WASM-only context transport using the existing shared plugin loader.
//!
//! Binding is discovery, not a grant. Invoke only after the host has checked its
//! live registry grant, and revalidate that grant before committing the result.
//! These synchronous methods belong on a bounded blocking worker, never directly
//! on a Tokio core thread. No lock is held across an await here.
//!
//! Normal lifecycle mounts supply an immutable permission proof on the actual
//! WASM binding. Dynamic handlers read descriptors and grants from the shared
//! registry on every call; only backend bindings are cached.

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::fabric::{
    AuthenticatedCaller, ContextCapabilityRequest, ContextNamespace, ContextTransformResult,
};
use crate::extension::capability::{ContextDeclaration, ContextGrant};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::{PluginManifest, PluginPermission};
use crate::extension::runtime::wasm::{WasmBinding, WasmCapabilities};
use crate::extension::{PluginKind, PluginLoader};

/// One descriptor and one incarnation-bound export, with no registry or store.
pub struct WasmContextAdapter {
    declaration: ContextDeclaration,
    binding: WasmBinding,
}

impl WasmContextAdapter {
    /// Bind a normally mounted export using its actual permission proof.
    /// The synchronous fresh-load form remains available for transport fixtures;
    /// production installs bindings inside the existing lifecycle mount effect.
    pub fn bind(
        loader: &mut PluginLoader,
        manifest: &PluginManifest,
        declaration: &ContextDeclaration,
    ) -> ExtensionResult<Self> {
        declaration.descriptor.validate().map_err(rejected)?;
        if declaration.handler.trim().is_empty()
            || declaration.handler.trim() != declaration.handler
        {
            return Err(rejected("context handler must name a WASM export"));
        }
        if !manifest.aleph_extensions.as_ref().is_some_and(|ext| {
            ext.contexts.iter().any(|item| {
                item.handler == declaration.handler && item.descriptor == declaration.descriptor
            })
        }) {
            return Err(rejected("context declaration is absent from the manifest"));
        }
        let fresh = !loader.is_loaded(&manifest.id);
        if fresh {
            require_no_ambient_authority(manifest)?;
            loader.load_plugin(manifest)?;
        }
        let result = loader
            .bind_wasm(&manifest.id, &declaration.handler)
            .and_then(|binding| {
                if !binding.context_effect_free() {
                    return Err(rejected("actual mounted guest has ambient authority"));
                }
                if !binding
                    .context_declaration(&declaration.descriptor.id)
                    .is_some_and(|mounted| {
                        mounted.descriptor == declaration.descriptor
                            && mounted.handler == declaration.handler
                    })
                {
                    return Err(rejected(
                        "declaration differs from the actual mounted guest",
                    ));
                }
                Ok(Self {
                    declaration: declaration.clone(),
                    binding,
                })
            });
        if fresh && result.is_err() {
            loader.unload_plugin(&manifest.id)?;
        }
        result
    }

    pub fn declaration(&self) -> &ContextDeclaration {
        &self.declaration
    }

    pub(crate) fn binding(&self) -> &WasmBinding {
        &self.binding
    }

    /// Invoke the existing JSON ABI: arguments is the canonical context request;
    /// the successful WasmToolOutput.result is the canonical transform result.
    /// The host must supply its current, non-revoked grant on every call. This
    /// transport does not independently track revocation or commit model state.
    pub fn invoke_json(
        &self,
        loader: &PluginLoader,
        request: &ContextCapabilityRequest,
        grant: &ContextGrant,
        caller: &AuthenticatedCaller,
        now: u64,
    ) -> ExtensionResult<ContextTransformResult> {
        if !self.binding.context_effect_free() || !loader.wasm_binding_is_current(&self.binding) {
            return Err(rejected("effect-free WASM binding is no longer mounted"));
        }
        let descriptor = &self.declaration.descriptor;
        grant.validate_against(descriptor, now).map_err(rejected)?;
        // The initial transport supports only a session-local, effect-free
        // grant. Broader scopes need the registry's live admission integration.
        if grant.plugin_id != self.binding.plugin_id()
            || grant.session_id != caller.session_id()
            || grant.scope.namespace != ContextNamespace::Session
            || grant.cross_session
            || grant.egress
            || grant.project_write
            || grant
                .scope
                .handles
                .iter()
                .any(|handle| handle != caller.session_id())
        {
            return Err(rejected(
                "context WASM transport requires a session-local, effect-free grant",
            ));
        }
        let admitted = request.admit(descriptor, caller).map_err(rejected)?;
        if let Some(snapshot) = &request.snapshot {
            if crate::context::fabric::privacy::contains_recognised_credential(&snapshot.payload) {
                return Err(rejected(
                    "context snapshot contains a recognised credential",
                ));
            }
            if snapshot.scope.namespace != ContextNamespace::Session
                || snapshot.session_id != caller.session_id()
            {
                return Err(rejected(
                    "context WASM transport cannot widen the granted session scope",
                ));
            }
            let encoded = serde_json::to_vec(&snapshot.payload).map_err(rejected)?;
            if encoded.len() as u64 != snapshot.byte_count
                || estimate_tokens_smart(std::str::from_utf8(&encoded).map_err(rejected)?) as u64
                    != snapshot.token_count
            {
                return Err(rejected(
                    "context snapshot measurements do not match its JSON payload",
                ));
            }
        }
        let args = serde_json::to_value(request).map_err(rejected)?;
        check_wire_size(&args, descriptor.input_budget.max_bytes)?;
        let output = loader.call_bound(&self.binding, args)?;
        check_wire_size(&output, descriptor.output_budget.max_bytes)?;
        let result: ContextTransformResult = serde_json::from_value(output).map_err(rejected)?;
        result.validate_for(&admitted).map_err(rejected)?;
        let encoded = serde_json::to_vec(&result.output).map_err(rejected)?;
        if encoded.len() as u64 != result.output_bytes
            || estimate_tokens_smart(std::str::from_utf8(&encoded).map_err(rejected)?) as u64
                != result.output_tokens
        {
            return Err(rejected(
                "context result measurements do not match its JSON output",
            ));
        }
        Ok(result)
    }
}

// Bound the JSON envelope as well as the canonical payload. The allowance is
// for request identifiers, descriptor budgets and result metadata, not payload.
fn check_wire_size(value: &serde_json::Value, payload_limit: u64) -> ExtensionResult<()> {
    let limit = payload_limit.saturating_add(16 * 1024);
    struct Counter {
        bytes: u64,
        limit: u64,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self.bytes.saturating_add(bytes.len() as u64);
            if self.bytes > self.limit {
                return Err(std::io::Error::other(
                    "context JSON envelope exceeds transport limit",
                ));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter { bytes: 0, limit }, value).map_err(rejected)
}

fn rejected(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError::Runtime(format!("context WASM transport rejected: {error}"))
}

fn has_authority(capabilities: &WasmCapabilities) -> bool {
    capabilities.workspace.is_some()
        || capabilities.http.is_some()
        || capabilities.secrets.is_some()
}

pub(crate) fn require_no_ambient_authority(manifest: &PluginManifest) -> ExtensionResult<()> {
    if manifest.kind != PluginKind::Wasm {
        return Err(rejected("only WASM transport is supported; native Pi JavaScript and MCP processes are unsupported"));
    }
    // EXTISM_ENABLE_WASI_OUTPUT causes Extism to inherit stdout/stderr. Refuse
    // that configuration rather than silently opening a second output channel.
    if std::env::var_os("EXTISM_ENABLE_WASI_OUTPUT").is_some() {
        return Err(rejected("EXTISM_ENABLE_WASI_OUTPUT must be unset"));
    }
    if manifest.permissions.iter().any(|permission| {
        !matches!(permission, PluginPermission::Custom(name) if name.starts_with("context:"))
    }) || manifest.wasm_capabilities.as_ref().is_some_and(has_authority)
        || manifest.capabilities_v2.as_ref().and_then(
            crate::extension::manifest::toml_types::convert_wasm_capabilities,
        ).as_ref().is_some_and(has_authority)
        || manifest.aleph_extensions.as_ref().and_then(|ext| ext.capabilities.as_ref())
            .and_then(crate::extension::manifest::toml_types::convert_wasm_capabilities)
            .as_ref().is_some_and(has_authority)
    {
        return Err(rejected("guest has ambient permissions; the existing runtime cannot narrow them per call"));
    }
    Ok(())
}

/// Dynamic backend resolver. It carries no descriptor, owner, or grant authority.
pub type ContextHandlerResolver = std::sync::Arc<
    dyn Fn(&str) -> Option<std::sync::Arc<dyn crate::context::fabric::ContextHandler>>
        + Send
        + Sync,
>;

type SharedLoader = std::sync::Arc<tokio::sync::RwLock<PluginLoader>>;
type SharedRegistry =
    std::sync::Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>;

/// A handler resolved for one concrete incarnation. Retaining it cannot rebind it
/// to a same-id replacement. Contention resolves to Core fallback, not blocking.
pub(crate) fn handler_resolver(
    loader: SharedLoader,
    registry: SharedRegistry,
) -> ContextHandlerResolver {
    std::sync::Arc::new(move |key| {
        let guard = loader.try_read().ok()?;
        let binding = guard.context_binding(key)?;
        let slots = guard.context_call_slots();
        drop(guard);
        Some(std::sync::Arc::new(MountedContextHandler {
            key: key.to_owned(),
            binding,
            slots,
            loader: loader.clone(),
            registry: registry.clone(),
        }))
    })
}

struct MountedContextHandler {
    key: String,
    binding: WasmBinding,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    loader: SharedLoader,
    registry: SharedRegistry,
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Read and validate authority only from the shared registry. The returned
/// grant is a short-lived comparison snapshot, never cached on a handler.
fn authorized_adapter(
    registry: &mut crate::extension::registry::PluginRegistry,
    key: &str,
    binding: &WasmBinding,
    request: &ContextCapabilityRequest,
    caller: &AuthenticatedCaller,
) -> Result<(WasmContextAdapter, ContextGrant), String> {
    registry
        .validate_context_grant(key, request, caller, now_seconds())
        .map_err(|e| e.to_string())?;
    let registration = registry
        .resolve_context(key)
        .ok_or("context registration disappeared")?;
    let declaration = binding
        .context_declaration(&registration.descriptor.id)
        .filter(|d| d.descriptor == registration.descriptor)
        .ok_or("registry descriptor differs from mounted context declaration")?;
    if registration.plugin_id != binding.plugin_id() || !binding.context_effect_free() {
        return Err("context binding does not match registry owner".into());
    }
    let grant = registration
        .grant_for_session(caller.session_id())
        .ok_or("context grant is unavailable")?
        .clone();
    Ok((
        WasmContextAdapter {
            declaration: declaration.clone(),
            binding: binding.clone(),
        },
        grant,
    ))
}

#[async_trait::async_trait]
impl crate::context::fabric::ContextHandler for MountedContextHandler {
    async fn invoke(
        &self,
        _request: ContextCapabilityRequest,
    ) -> Result<ContextTransformResult, String> {
        // A serialized snapshot session_id is NOT an authenticated caller.
        Err("WASM context invocation requires the authenticated host boundary".into())
    }

    async fn invoke_authenticated(
        &self,
        request: ContextCapabilityRequest,
        caller: &AuthenticatedCaller,
    ) -> Result<ContextTransformResult, String> {
        let initial_grant = {
            let mut registry = self.registry.write().await;
            authorized_adapter(&mut registry, &self.key, &self.binding, &request, caller)?.1
        };
        // No queue of detached workers: acquire before spawning and retain the
        // owned permit IN the closure until the actual native call returns.
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "context WASM blocking capacity exhausted".to_owned())?;
        let loader = self.loader.clone();
        let registry = self.registry.clone();
        let binding = self.binding.clone();
        let key = self.key.clone();
        let call_request = request.clone();
        let call_caller = caller.clone();
        let expected_grant = initial_grant.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let loader = loader.blocking_read();
            if !loader.wasm_binding_is_current(&binding) {
                return Err("context WASM incarnation was unmounted".into());
            }
            let (adapter, grant) = {
                let mut registry = registry.blocking_write();
                authorized_adapter(&mut registry, &key, &binding, &call_request, &call_caller)?
            };
            if grant != expected_grant {
                return Err("context grant changed before native execution".into());
            }
            adapter
                .invoke_json(&loader, &call_request, &grant, &call_caller, now_seconds())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("context WASM worker failed: {e}"))??;
        // No guard crosses an async await. A busy registry fails closed here;
        // it cannot let a stale result slip through unload/regrant races.
        let loader = self.loader.read().await;
        if !loader.wasm_binding_is_current(&self.binding) {
            return Err("context WASM incarnation changed while awaiting output".into());
        }
        let mut registry = self
            .registry
            .try_write()
            .map_err(|_| "context registry busy during result authorization".to_owned())?;
        let (_, grant) =
            authorized_adapter(&mut registry, &self.key, &self.binding, &request, caller)?;
        if grant != initial_grant {
            return Err("context grant changed while awaiting output".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::fabric::{
        canonical_fingerprint, ContextBudget, ContextOperation, ContextPhase, ContextScope,
        ContextSnapshot, CONTEXT_CAPABILITY_API_VERSION,
    };
    use crate::extension::capability::{CapabilityDeclaration, ContextGrantApproval};
    use crate::extension::manifest::adapter::AdapterRegistry;
    use crate::extension::manifest::{parse_manifest_from_dir_sync, FilesystemAccess};
    use crate::extension::runtime::wasm::tests::guest_fixture::{
        manifest_with_checked_input_guest, HANDLER,
    };
    use serde_json::{json, Value};

    fn declaration() -> ContextDeclaration {
        serde_json::from_value(json!({
            "handler": HANDLER,
            "descriptor": {
                "id": "fixture/context", "api_version": CONTEXT_CAPABILITY_API_VERSION,
                "implementation_version": "1", "phases": ["compaction"],
                "mode": "replace", "scope": {"namespace": "session", "handles": []},
                "input_budget": {"max_tokens": 4096, "max_bytes": 16384},
                "output_budget": {"max_tokens": 4096, "max_bytes": 16384}
            }
        }))
        .unwrap()
    }

    fn caller() -> AuthenticatedCaller {
        AuthenticatedCaller::from_host_boundary("session-a").unwrap()
    }

    fn grant(declaration: &ContextDeclaration) -> ContextGrant {
        ContextGrant::host_issue(
            &declaration.descriptor,
            "context-guest",
            &caller(),
            ContextGrantApproval {
                expires_at: 2000,
                audit_reason: "explicit test host approval".into(),
                ..Default::default()
            },
            1000,
        )
        .unwrap()
    }

    fn request(declaration: &ContextDeclaration) -> ContextCapabilityRequest {
        let payload = json!({"messages": [{"role": "user", "content": "fixture input"}]});
        let encoded = serde_json::to_string(&payload).unwrap();
        let fingerprint = canonical_fingerprint(&payload);
        ContextCapabilityRequest {
            request_id: "request-1".into(),
            capability_id: declaration.descriptor.id.clone(),
            api_version: CONTEXT_CAPABILITY_API_VERSION.into(),
            phase: ContextPhase::Compaction,
            operation: ContextOperation::Compact,
            budget: Some(ContextBudget::new(4096, 16384)),
            input_budget: None,
            output_budget: None,
            input_fingerprint: Some(fingerprint.clone()),
            snapshot: Some(ContextSnapshot {
                snapshot_id: "snapshot-1".into(),
                session_id: "session-a".into(),
                input_fingerprint: fingerprint,
                scope: ContextScope {
                    namespace: ContextNamespace::Session,
                    handles: vec![],
                },
                token_count: estimate_tokens_smart(&encoded) as u64,
                byte_count: encoded.len() as u64,
                payload,
            }),
        }
    }

    fn result(request: &ContextCapabilityRequest) -> Value {
        let output = json!({"messages": []});
        let encoded = serde_json::to_string(&output).unwrap();
        json!({
            "input_fingerprint": request.input_fingerprint,
            "output": output, "output_bytes": encoded.len(),
            "output_tokens": estimate_tokens_smart(&encoded),
            "strategy": "fixture.empty", "quality": "condensed"
        })
    }

    fn fixture(directory: &std::path::Path, format: &str) -> PluginManifest {
        let declaration = declaration();
        let reply =
            json!({"success": true, "result": result(&request(&declaration)), "error": null});
        let expected = serde_json::to_string(&crate::extension::runtime::WasmToolInput {
            name: HANDLER.into(),
            arguments: serde_json::to_value(request(&declaration)).unwrap(),
        })
        .unwrap();
        let manifest = manifest_with_checked_input_guest(
            directory,
            "context-guest",
            &reply.to_string(),
            &expected,
        );
        if format == "native" {
            let document = json!({
                "plugin": {"id": manifest.id, "kind": "wasm", "entry": manifest.entry},
                "contexts": [declaration]
            });
            std::fs::write(
                directory.join("aleph.plugin.toml"),
                toml::to_string(&document).unwrap(),
            )
            .unwrap();
        } else {
            std::fs::create_dir_all(directory.join(".claude-plugin")).unwrap();
            let document = json!({
                "name": manifest.id,
                "aleph": {"runtime": "wasm", "entry": manifest.entry, "contexts": [declaration]}
            });
            let path = directory
                .join(".claude-plugin")
                .join(format!("plugin.{format}"));
            let text = if format == "json" {
                document.to_string()
            } else {
                toml::to_string(&document).unwrap()
            };
            std::fs::write(path, text).unwrap();
        }
        parse_manifest_from_dir_sync(directory).unwrap()
    }

    #[test]
    fn context_guest_cannot_import_wasi_even_when_ordinary_plugins_can() {
        use crate::extension::runtime::wasm::tests::guest_fixture::write_wasi_guest;
        use crate::extension::runtime::wasm::WasmRuntime;
        let directory = tempfile::tempdir().unwrap();
        let manifest = fixture(directory.path(), "native");
        write_wasi_guest(&manifest.entry_path().unwrap());
        let mut ordinary = manifest.clone();
        ordinary.aleph_extensions = None;
        let mut runtime = WasmRuntime::new();
        runtime
            .load_plugin(&ordinary)
            .expect("the same WASI module is valid for an ordinary plugin");
        runtime.unload_plugin(&ordinary.id);
        let error = runtime
            .load_plugin(&manifest)
            .expect_err("context guests must have no WASI linker authority");
        assert!(
            error.to_string().contains("wasi_snapshot_preview1"),
            "unexpected load failure: {error}"
        );
    }

    #[test]
    fn context_guest_cannot_persist_payload_through_extism_builtin_logging() {
        use crate::extension::runtime::wasm::tests::guest_fixture::write_builtin_log_guest;
        use crate::extension::runtime::wasm::{WasmRuntime, WasmToolInput};
        use std::io::Write;
        #[derive(Clone)]
        struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let manifest = fixture(directory.path(), "native");
        const SENTINEL: &str = "PRIVATE_CONTEXT_BUILTIN_LOG_SENTINEL";
        write_builtin_log_guest(&manifest.entry_path().unwrap(), SENTINEL);
        let mut ordinary = manifest.clone();
        ordinary.aleph_extensions = None;
        let run = |manifest: &PluginManifest| {
            let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let capture = bytes.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::ERROR)
                .with_writer(move || Capture(capture.clone()))
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                let mut runtime = WasmRuntime::new();
                runtime.load_plugin(manifest).unwrap();
                // The export deliberately has no JSON output. Execution and
                // the PDK log happen before the ordinary tool JSON decoder.
                let _ = runtime.call_tool(
                    &manifest.id,
                    HANDLER,
                    WasmToolInput {
                        name: HANDLER.into(),
                        arguments: json!({}),
                    },
                );
            });
            let contents = bytes.lock().unwrap().clone();
            String::from_utf8(contents).unwrap()
        };
        assert!(
            run(&ordinary).contains(SENTINEL),
            "ordinary guest must exercise the actual PDK logger"
        );
        assert!(
            !run(&manifest).contains(SENTINEL),
            "context payload reached a persistent logging sink"
        );
    }

    #[test]
    fn context_guest_vm_enforces_mounted_memory_limit() {
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            call_guest_raw, write_memory_probe_guest, PAYLOAD_A,
        };
        use crate::extension::runtime::wasm::{WasmResourceLimits, WasmRuntime};
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = fixture(directory.path(), "native");
        manifest.wasm_resource_limits = Some(WasmResourceLimits {
            memory_mb: 10,
            fuel: 1_000_000_000,
            timeout_secs: 2,
            ..Default::default()
        });
        write_memory_probe_guest(&manifest.entry_path().unwrap(), 32 * 1024 * 1024);
        let mut ordinary = manifest.clone();
        ordinary.aleph_extensions = None;
        let mut runtime = WasmRuntime::new();
        runtime.load_plugin(&ordinary).unwrap();
        assert_eq!(
            call_guest_raw(&runtime, &ordinary.id).unwrap(),
            PAYLOAD_A.as_bytes()
        );
        runtime.unload_plugin(&ordinary.id);
        runtime.load_plugin(&manifest).unwrap();
        assert!(
            call_guest_raw(&runtime, &manifest.id).is_err(),
            "guest allocated 32 MiB despite its mounted 10 MiB VM limit"
        );
    }

    #[test]
    fn context_guest_vm_exhausts_fuel_before_its_deadline() {
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            call_guest_raw, write_fuel_probe_guest,
        };
        use crate::extension::runtime::wasm::{WasmResourceLimits, WasmRuntime};
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = fixture(directory.path(), "native");
        manifest.wasm_resource_limits = Some(WasmResourceLimits {
            fuel: 1_000_000,
            timeout_secs: 2,
            ..Default::default()
        });
        write_fuel_probe_guest(&manifest.entry_path().unwrap());
        let mut runtime = WasmRuntime::new();
        runtime.load_plugin(&manifest).unwrap();
        let error = call_guest_raw(&runtime, &manifest.id)
            .expect_err("infinite guest loop exhausted no fuel");
        assert!(
            error.to_lowercase().contains("fuel"),
            "guest was stopped by a different guard: {error}"
        );
    }

    #[test]
    fn manifests_discover_and_real_wasm_returns_canonical_json() {
        for format in ["native", "json", "toml"] {
            let directory = tempfile::tempdir().unwrap();
            let manifest = fixture(directory.path(), format);
            let output = AdapterRegistry::with_defaults()
                .parse_dir(directory.path())
                .unwrap();
            let contexts: Vec<_> = output
                .capabilities
                .iter()
                .filter_map(|cap| match cap {
                    CapabilityDeclaration::Context(context) => Some(context),
                    _ => None,
                })
                .collect();
            assert_eq!(contexts.len(), 1);
            assert_eq!(contexts[0].descriptor, declaration().descriptor);
            let mut loader = PluginLoader::new();
            let adapter = WasmContextAdapter::bind(&mut loader, &manifest, contexts[0]).unwrap();
            let request = request(contexts[0]);
            // Discovery and binding issue no grants: only this separate host
            // consent can authorize a data-bearing invocation.
            let grant = grant(contexts[0]);
            let response = adapter
                .invoke_json(&loader, &request, &grant, &caller(), 1000)
                .unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), result(&request));
            // The real guest checks all request bytes, not only a canned response.
            let mut changed = request.clone();
            changed.request_id = "different-wire-request".into();
            assert!(adapter
                .invoke_json(&loader, &changed, &grant, &caller(), 1000)
                .is_err());
        }
    }

    #[test]
    fn unload_and_same_id_replacement_reject_stale_binding() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = fixture(directory.path(), "native");
        let declaration = declaration();
        let mut loader = PluginLoader::new();
        let adapter = WasmContextAdapter::bind(&mut loader, &manifest, &declaration).unwrap();
        loader.unload_plugin(&manifest.id).unwrap();
        let invoke = |loader: &PluginLoader| {
            adapter.invoke_json(
                loader,
                &request(&declaration),
                &grant(&declaration),
                &caller(),
                1000,
            )
        };
        assert!(invoke(&loader).is_err());
        let replacement = WasmContextAdapter::bind(&mut loader, &manifest, &declaration).unwrap();
        assert!(invoke(&loader).is_err());
        assert!(replacement
            .invoke_json(
                &loader,
                &request(&declaration),
                &grant(&declaration),
                &caller(),
                1000
            )
            .is_ok());
    }

    #[test]
    fn ambient_authority_and_unsupported_transports_are_rejected_before_load() {
        let directory = tempfile::tempdir().unwrap();
        let original = fixture(directory.path(), "json");
        let declaration = declaration();
        for permission in [
            PluginPermission::Network,
            PluginPermission::Filesystem(FilesystemAccess::Read),
            PluginPermission::Shell,
            PluginPermission::Env,
        ] {
            let mut manifest = original.clone();
            manifest.permissions.push(permission);
            let mut loader = PluginLoader::new();
            assert!(WasmContextAdapter::bind(&mut loader, &manifest, &declaration).is_err());
            assert_eq!(loader.loaded_count(), 0);
        }
        let mut manifest = original.clone();
        manifest.wasm_capabilities = Some(WasmCapabilities {
            workspace: Some(crate::extension::runtime::wasm::WorkspaceCapability {
                allowed_prefixes: vec![],
            }),
            ..Default::default()
        });
        assert!(
            WasmContextAdapter::bind(&mut PluginLoader::new(), &manifest, &declaration).is_err()
        );
        for kind in [PluginKind::Mcp, PluginKind::Static] {
            let mut manifest = original.clone();
            manifest.kind = kind;
            assert!(
                WasmContextAdapter::bind(&mut PluginLoader::new(), &manifest, &declaration)
                    .is_err()
            );
        }
    }

    #[test]
    fn actual_mount_proof_allows_incumbent_and_failed_bind_rolls_back_only_fresh_mount() {
        let directory = tempfile::tempdir().unwrap();
        let mut manifest = fixture(directory.path(), "native");
        let mut declaration = declaration();
        let mut loader = PluginLoader::new();
        loader.load_plugin(&manifest).unwrap();
        assert!(WasmContextAdapter::bind(&mut loader, &manifest, &declaration).is_ok());
        assert!(loader.is_loaded(&manifest.id));
        loader.unload_plugin(&manifest.id).unwrap();
        let mut ambient = manifest.clone();
        ambient.permissions.push(PluginPermission::Network);
        loader.load_plugin(&ambient).unwrap();
        // A later weaker manifest cannot erase the mounted instance's proof.
        assert!(WasmContextAdapter::bind(&mut loader, &manifest, &declaration).is_err());
        assert!(loader.is_loaded(&manifest.id));
        loader.unload_plugin(&manifest.id).unwrap();
        declaration.handler = "absent_export".into();
        manifest.aleph_extensions.as_mut().unwrap().contexts = vec![declaration.clone()];
        assert!(WasmContextAdapter::bind(&mut loader, &manifest, &declaration).is_err());
        assert!(!loader.is_loaded(&manifest.id));
    }

    /// Real disk manifest -> normal lifecycle -> explicit host grant -> facade,
    /// using a guest that derives its result from each actual request.
    #[tokio::test]
    async fn lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement() {
        use crate::context::fabric::{ContextFabric, ContextOwner};
        use crate::discovery::{DiscoveryConfig, ProjectPluginParent};
        use crate::extension::plugin_state::{PluginEntryConfig, PluginsConfig};
        use crate::extension::runtime::wasm::tests::guest_fixture::manifest_with_context_identity_guest;
        use crate::extension::{ExtensionConfig, ExtensionManager};
        use crate::providers::message::UnifiedMessage;
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("plugins/context-guest");
        std::fs::create_dir_all(&directory).unwrap();
        let mounted = manifest_with_context_identity_guest(&directory, "context-guest");
        let mut first = declaration();
        first.descriptor.phases = vec![ContextPhase::Preflight];
        let mut second = declaration();
        second.descriptor.id = "fixture/compact".into();
        let document = json!({
            "plugin": {"id": mounted.id, "kind": "wasm", "entry": mounted.entry},
            "contexts": [first.clone(), second.clone()]
        });
        let manifest_path = directory.join("aleph.plugin.toml");
        let original_manifest = toml::to_string(&document).unwrap();
        std::fs::write(&manifest_path, &original_manifest).unwrap();
        let config_path = root.path().join("plugins.toml");
        PluginsConfig {
            entries: [(
                "context-guest".into(),
                PluginEntryConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        }
        .save(&config_path)
        .await
        .unwrap();
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: root.path().into(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
                claude_home_override: None,
            },
            plugins_config_path: Some(config_path),
            extra_plugin_parents: vec![ProjectPluginParent {
                project_root: root.path().into(),
                dir: root.path().join("plugins"),
            }],
        })
        .await
        .unwrap();
        manager.load_all().await.unwrap();
        // Assemble BEFORE mount: no static list can make this test pass.
        let resolver = manager.context_handler_resolver();
        let fabric = ContextFabric::builder(manager.plugin_registry_handle())
            .handler_resolver(resolver.clone())
            .build();
        assert!(resolver("context-guest:fixture/context").is_none());
        assert!(manager
            .set_plugin_enabled("context-guest", true)
            .await
            .unwrap()
            .changed());
        assert_eq!(
            manager
                .context_declarations("context-guest")
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(manager
            .plugin_registry_handle()
            .read()
            .await
            .eligible_context_keys("session-a", now_seconds())
            .is_empty());
        let pressure = crate::context::budget::ContextPressure {
            used_tokens: 1000,
            budget_tokens: 1000,
            ratio: 1.0,
            overhead_tokens: 0,
            available_for_messages: 1000,
        };
        let mut messages = vec![UnifiedMessage::user(
            "real lifecycle input with escaped \\\" braces } [",
        )];
        let original = messages.clone();
        let before = fabric
            .preflight(&mut messages, &pressure, 0, &caller(), None)
            .await
            .unwrap();
        assert_eq!(before.owner, ContextOwner::Builtin);
        let approval = ContextGrantApproval {
            expires_at: now_seconds() + 300,
            audit_reason: "confirmed host fixture approval".into(),
            ..Default::default()
        };
        // Incomplete advertised bundle and unknown ids do not mutate authority.
        assert!(manager
            .grant_context_capabilities(
                "context-guest",
                &["absent".into()],
                &caller(),
                approval.clone(),
                now_seconds()
            )
            .await
            .is_err());
        assert_eq!(
            manager
                .grant_context_capabilities(
                    "context-guest",
                    &[first.descriptor.id.clone()],
                    &caller(),
                    approval.clone(),
                    now_seconds()
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            manager
                .grant_context_capabilities(
                    "context-guest",
                    &[second.descriptor.id.clone()],
                    &caller(),
                    approval.clone(),
                    now_seconds()
                )
                .await
                .unwrap(),
            1
        );
        let other = AuthenticatedCaller::from_host_boundary("session-b").unwrap();
        manager
            .grant_context_capabilities(
                "context-guest",
                &[first.descriptor.id.clone()],
                &other,
                approval.clone(),
                now_seconds(),
            )
            .await
            .unwrap();
        for text in [
            "fresh request one",
            "fresh request two with different content",
        ] {
            messages = vec![UnifiedMessage::user(text)];
            let expected = messages.clone();
            let outcome = fabric
                .preflight(&mut messages, &pressure, 0, &caller(), None)
                .await
                .unwrap();
            assert_eq!(
                outcome.owner,
                ContextOwner::Plugin("context-guest:fixture/context".into())
            );
            assert_eq!(
                serde_json::to_value(&messages).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
        }
        // Mutating disk does not change what status/grant see on this mount.
        std::fs::write(
            &manifest_path,
            original_manifest.replace("fixture/context", "forged/new"),
        )
        .unwrap();
        assert!(manager
            .context_declarations("context-guest")
            .await
            .unwrap()
            .iter()
            .any(|d| d.descriptor.id == first.descriptor.id));
        assert!(manager
            .grant_context_capabilities(
                "context-guest",
                &["forged/new".into()],
                &caller(),
                approval.clone(),
                now_seconds()
            )
            .await
            .is_err());
        std::fs::write(&manifest_path, &original_manifest).unwrap();
        let retained = resolver("context-guest:fixture/context").unwrap();
        let mut direct = request(&first);
        direct.phase = ContextPhase::Preflight;
        direct.operation = ContextOperation::Preflight;
        assert!(retained.invoke(direct.clone()).await.is_err());
        assert!(retained
            .invoke_authenticated(direct.clone(), &caller())
            .await
            .is_ok());
        let registry = manager.plugin_registry_handle();
        assert_eq!(
            registry
                .write()
                .await
                .revoke_context_grants("context-guest", "session-a"),
            2
        );
        assert!(retained
            .invoke_authenticated(direct.clone(), &caller())
            .await
            .is_err());
        assert!(!registry
            .read()
            .await
            .eligible_context_keys("session-b", now_seconds())
            .is_empty());
        messages = original;
        let revoked = fabric
            .preflight(&mut messages, &pressure, 0, &caller(), None)
            .await
            .unwrap();
        assert_eq!(revoked.owner, ContextOwner::Builtin);
        manager.unmount("context-guest").await.unwrap();
        assert!(resolver("context-guest:fixture/context").is_none());
        assert!(registry
            .read()
            .await
            .eligible_context_keys("session-b", now_seconds())
            .is_empty());
        manager.mount("context-guest").await.unwrap();
        manager
            .grant_context_capabilities(
                "context-guest",
                &[first.descriptor.id.clone()],
                &caller(),
                approval,
                now_seconds(),
            )
            .await
            .unwrap();
        assert!(
            retained
                .invoke_authenticated(direct.clone(), &caller())
                .await
                .is_err(),
            "old handler must not rebind to the replacement"
        );
        assert!(resolver("context-guest:fixture/context")
            .unwrap()
            .invoke_authenticated(direct, &caller())
            .await
            .is_ok());
    }

    #[test]
    fn expired_foreign_and_changed_grants_cannot_invoke() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = fixture(directory.path(), "native");
        let declaration = declaration();
        let mut loader = PluginLoader::new();
        let adapter = WasmContextAdapter::bind(&mut loader, &manifest, &declaration).unwrap();
        let request = request(&declaration);
        let grant = grant(&declaration);
        assert!(adapter
            .invoke_json(&loader, &request, &grant, &caller(), 2000)
            .is_err());
        let foreign = AuthenticatedCaller::from_host_boundary("session-b").unwrap();
        assert!(adapter
            .invoke_json(&loader, &request, &grant, &foreign, 1000)
            .is_err());
        let mut changed = declaration.clone();
        changed.descriptor.implementation_version = "2".into();
        assert!(adapter
            .invoke_json(
                &loader,
                &request,
                &super::tests::grant(&changed),
                &caller(),
                1000
            )
            .is_err());
    }
}
