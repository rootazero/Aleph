//! Canonical handlers bound to one real RuntimeOwner and mounted export.

use super::registry::ToolRegistration;
use super::runtime::wasm::WasmBinding;
use super::runtime_owner::RuntimeOwner;
use super::PluginLoader;
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
use serde_json::Value;
use std::sync::Weak;

/// Weak ownership avoids a receipt -> snapshot -> handler -> owner cycle.
struct PluginCanonicalHandler {
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    owner: Weak<RuntimeOwner>,
    generation: u64,
    binding: WasmBinding,
    registration: ToolRegistration,
}

#[async_trait::async_trait]
impl ToolHandler for PluginCanonicalHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let error = |cause: String| ToolError::Execution {
            name: self.registration.name.clone(),
            cause,
        };
        let owner = self
            .owner
            .upgrade()
            .ok_or_else(|| error("runtime owner is gone".into()))?;
        let authority = crate::tools::dispatch_verdict::PluginActivationAuthority::from_current(
            &self.registration.name,
            &self.registration.plugin_id,
            &self.registration.handler,
            &input,
        )
        .ok_or_else(|| {
            error("canonical plugin invocation has no sealed producer authority".into())
        })?;
        let validate = || {
            if !authority.validates(
                &self.registration.plugin_id,
                &self.registration.handler,
                &input,
            ) || !authority
                .binding_for(&owner.provider, owner.owner_generation)
                .is_some_and(|binding| binding.same_binding(&self.binding))
                || !owner
                    .identity
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|mount| self.binding.matches_mount(mount))
            {
                return Err(error(
                    "canonical plugin authority or owner binding is stale".into(),
                ));
            }
            owner
                .provider
                .acquire_call(self.generation)
                .map_err(|e| error(e.to_string()))
        };
        validate()?;
        let permit = owner.acquire_permit().await.map_err(&error)?;
        let loader = self.loader.clone().read_owned().await;
        if !loader.wasm_binding_is_current(&self.binding) {
            return Err(error("runtime owner binding is stale".into()));
        }
        validate()?;
        let binding = self.binding.clone();
        let operation = Arc::clone(&owner);
        let generation = self.generation;
        let guest_input = input.clone();
        let proof = authority.publication_proof();
        let result = tokio::task::spawn_blocking(move || {
            // A cancelled caller cannot release admission while the guest is
            // still running. Stop joins the permit, never cancels the guest.
            let _permit = permit;
            operation
                .provider
                .acquire_call(generation)
                .map_err(|e| super::error::ExtensionError::Runtime(e.to_string()))?;
            if !proof.is_current() {
                return Err(super::error::ExtensionError::Runtime(
                    "canonical plugin authority or owner binding is stale".into(),
                ));
            }
            let result = loader.call_bound(&binding, guest_input);
            operation
                .provider
                .acquire_call(generation)
                .map_err(|e| super::error::ExtensionError::Runtime(e.to_string()))?;
            result
        })
        .await
        .map_err(|e| error(format!("WASM task join failed: {e}")))?;
        if !self
            .loader
            .read()
            .await
            .wasm_binding_is_current(&self.binding)
        {
            return Err(error(
                "runtime owner binding changed during execution".into(),
            ));
        }
        validate()?;
        Ok(ToolOutput {
            value: result.map_err(|e| error(e.to_string()))?,
            metadata: Default::default(),
        })
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.registration.name.clone(),
            description: self.registration.description.clone(),
            input_schema: self.registration.parameters.clone(),
            source: ToolSource::Extension {
                plugin_id: self.registration.plugin_id.clone(),
            },
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}

pub(super) fn build_canonical_handler(
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    owner: &Arc<RuntimeOwner>,
    generation: u64,
    binding: WasmBinding,
    registration: ToolRegistration,
) -> Arc<dyn ToolHandler> {
    Arc::new(PluginCanonicalHandler {
        loader,
        owner: Arc::downgrade(owner),
        generation,
        binding,
        registration,
    })
}
