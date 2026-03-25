//! WASM Registrar — host function for WASM plugin capability registration
//!
//! Provides a host function that WASM plugins call to register capabilities.
//! The function runs synchronously within the Extism host function context.

use anyhow::Result;
use std::sync::{PoisonError, RwLock, RwLockWriteGuard};
use crate::extension::capability::CapabilityDeclaration;
use crate::extension::manifest::PluginPermission;
use crate::extension::registrar::api::CapabilityApi;
use crate::extension::registry::PluginRegistry;

/// Host function for WASM plugins to register capabilities.
///
/// WASM plugins serialize a `Vec<CapabilityDeclaration>` as JSON,
/// pass it to this function via the Extism host function interface.
/// The function deserializes, validates permissions, and writes to registry.
pub fn host_fn_register(
    plugin_id: &str,
    registry: &RwLock<PluginRegistry>,
    permissions: &[PluginPermission],
    input: &[u8],
) -> Result<()> {
    let declarations: Vec<CapabilityDeclaration> = serde_json::from_slice(input)?;
    let mut reg = registry.write().unwrap_or_else(|e: PoisonError<RwLockWriteGuard<'_, PluginRegistry>>| e.into_inner());
    let mut api = CapabilityApi::new(&mut reg, plugin_id.to_string(), permissions.to_vec());
    for decl in declarations {
        api.register_capability(decl)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::*;
    use crate::extension::registry::{ServiceRegistration, ToolRegistration};
    use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord};

    fn create_test_record(id: &str) -> PluginRecord {
        PluginRecord::new(
            id.to_string(),
            id.to_string(),
            PluginKind::Wasm,
            PluginOrigin::Global,
        )
    }

    #[test]
    fn test_host_fn_register_deserialize_and_write() {
        let registry = RwLock::new(PluginRegistry::new());

        // Register plugin first
        {
            let mut reg = registry.write().unwrap();
            let record = create_test_record("wasm-plugin");
            reg.register_plugin(record);
        }

        let caps = vec![
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "wasm-tool".into(),
                description: "From WASM".into(),
                parameters: serde_json::json!({}),
                handler: "handle".into(),
                plugin_id: "wasm-plugin".into(),
            }),
        ];

        let input = serde_json::to_vec(&caps).unwrap();
        host_fn_register("wasm-plugin", &registry, &[], &input).unwrap();

        let reg = registry.read().unwrap();
        assert!(reg.get_tool("wasm-tool").is_some());
    }

    #[test]
    fn test_host_fn_register_invalid_json() {
        let registry = RwLock::new(PluginRegistry::new());
        let result = host_fn_register("test", &registry, &[], b"not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_host_fn_register_multiple_tools() {
        let registry = RwLock::new(PluginRegistry::new());
        {
            let mut reg = registry.write().unwrap();
            reg.register_plugin(create_test_record("wasm-multi"));
        }

        let caps = vec![
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "tool-x".into(),
                description: "X".into(),
                parameters: serde_json::json!({}),
                handler: "hx".into(),
                plugin_id: "wasm-multi".into(),
            }),
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "tool-y".into(),
                description: "Y".into(),
                parameters: serde_json::json!({}),
                handler: "hy".into(),
                plugin_id: "wasm-multi".into(),
            }),
        ];

        let input = serde_json::to_vec(&caps).unwrap();
        host_fn_register("wasm-multi", &registry, &[], &input).unwrap();

        let reg = registry.read().unwrap();
        assert!(reg.get_tool("tool-x").is_some());
        assert!(reg.get_tool("tool-y").is_some());
    }

    #[test]
    fn test_host_fn_register_permission_denied() {
        let registry = RwLock::new(PluginRegistry::new());
        {
            let mut reg = registry.write().unwrap();
            reg.register_plugin(create_test_record("wasm-noperm"));
        }

        // Service requires Background permission — should fail without it
        let caps = vec![
            CapabilityDeclaration::Service(ServiceRegistration {
                id: "bg-service".into(),
                name: "BG Service".into(),
                start_handler: "start".into(),
                stop_handler: "stop".into(),
                plugin_id: "wasm-noperm".into(),
            }),
        ];

        let input = serde_json::to_vec(&caps).unwrap();
        let result = host_fn_register("wasm-noperm", &registry, &[], &input);
        assert!(result.is_err());
    }

    #[test]
    fn test_host_fn_register_empty_input() {
        let registry = RwLock::new(PluginRegistry::new());
        {
            let mut reg = registry.write().unwrap();
            reg.register_plugin(create_test_record("wasm-empty"));
        }

        // Empty JSON array — should succeed
        let input = serde_json::to_vec(&Vec::<CapabilityDeclaration>::new()).unwrap();
        let result = host_fn_register("wasm-empty", &registry, &[], &input);
        assert!(result.is_ok());
    }
}
