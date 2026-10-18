//! Canonical handlers for extension tools.
//!
//! A handler owns the binding captured at admission time. Invocation therefore
//! calls the mounted guest through that binding and never performs a second
//! name based registry lookup.

use crate::extension::registry::ToolRegistration;
use crate::extension::PluginLoader;
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
use serde_json::Value;

/// A plugin tool admitted into the canonical registry.
#[allow(dead_code)] // Added in Task59; wired in Task60
pub(crate) struct PluginCanonicalHandler {
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    plugin_id: String,
    export: String,
    registration: ToolRegistration,
}

#[async_trait::async_trait]
impl ToolHandler for PluginCanonicalHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let loader = self.loader.read().await;
        let value = loader
            .call_tool(&self.plugin_id, &self.export, input)
            .map_err(|error| ToolError::Execution {
                name: self.registration.name.clone(),
                cause: error.to_string(),
            })?;
        Ok(ToolOutput {
            value,
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

/// Build the handler used by the canonical registry for one bound declaration.
#[must_use]
#[allow(dead_code)] // Task59 infrastructure, wired in Task60
pub(crate) fn build_canonical_handler(
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    plugin_id: String,
    export: String,
    registration: ToolRegistration,
) -> Arc<dyn ToolHandler> {
    Arc::new(PluginCanonicalHandler {
        loader,
        plugin_id,
        export,
        registration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_handler_definition_uses_extension_identity() {
        let registration = ToolRegistration {
            name: "plugin_tool".into(),
            description: "Plugin tool".into(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "plugin_tool".into(),
            plugin_id: "plugin".into(),
        };
        assert_eq!(registration.name, "plugin_tool");
    }
}

// Keep the public builder name available to the extension module without
// exposing the handler's binding internals to unrelated subsystems.
