//! Startup adapter to the single library Policy factory.
use alephcore::capability::ownership::OwnershipTree;
use alephcore::capability::policy::{PolicyDenial, PolicyEngine};
use alephcore::Config;
use std::sync::Arc;

pub fn build_capability_policy(
    config: &Config,
    tree: Arc<OwnershipTree>,
) -> Result<Arc<PolicyEngine>, PolicyDenial> {
    alephcore::capability::production_init::build_capability_policy(config, tree)
}

/// One bounded observation source shared by production emitters, audit and facade.
pub fn build_capability_events() -> Arc<alephcore::capability::events::InMemoryEventStream> {
    Arc::new(alephcore::capability::events::InMemoryEventStream::default())
}

/// Subscribe before mounting any producer so startup emissions are not missed.
pub fn start_capability_event_bridge(
    events: &Arc<alephcore::capability::events::InMemoryEventStream>,
    bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) -> alephcore::gateway::capability_events::CapabilityEventBridge {
    use alephcore::capability::events::CapabilityEventStream;
    alephcore::gateway::capability_events::CapabilityEventBridge::start(
        events.subscribe(alephcore::capability::events::EventFilter::default()),
        bus,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::capability::descriptor::CapabilityId;
    use alephcore::capability::special::CapabilityHandlerRegistry;
    use alephcore::capability::tool_adapters::{
        register_canonical_adapters, TERMINAL_ADAPTER_ID, UI_ADAPTER_ID,
    };

    #[test]
    fn delegate_uses_loaded_config_and_preserves_invalid_rule_errors() {
        let tree = Arc::new(OwnershipTree::new());
        let id = CapabilityId {
            namespace: "aleph/tools".into(),
            name: "file_write".into(),
        };
        let config: Config = toml::from_str("[policies.capability_constraints.\"aleph/tools/file_write\"]\nresource = { unit = \"active_calls\", max_active = 2 }\n").unwrap();
        assert!(build_capability_policy(&config, tree.clone())
            .unwrap()
            .is_configured(&id));
        assert!(!build_capability_policy(&Config::default(), tree.clone())
            .unwrap()
            .is_configured(&id));
        let invalid: Config = toml::from_str("[policies.capability_constraints.\"aleph/tools/file_write\"]\nresource = { unit = \"active_calls\", max_active = 0 }\n").unwrap();
        assert!(matches!(
            build_capability_policy(&invalid, tree),
            Err(PolicyDenial::InvalidConstraints { .. })
        ));
    }

    // Narrow source-wiring check: compare the complete argument token sequence
    // of one named call, allowing whitespace and one legal final comma only.
    fn exact_startup_call(code: &str, callee: &str, arguments: &str) -> usize {
        let needle = format!("{callee}(");
        let calls: Vec<_> = code.match_indices(&needle).collect();
        assert_eq!(calls.len(), 1, "startup must call {callee} exactly once");
        let start = calls[0].0;
        let body_start = start + needle.len();
        let mut depth = 1;
        let end = code[body_start..]
            .char_indices()
            .find_map(|(offset, character)| {
                match character {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(body_start + offset)
            })
            .expect("startup call must have balanced parentheses");
        let actual = &code[body_start..end];
        let expected: String = arguments
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert_eq!(
            actual.strip_suffix(',').unwrap_or(actual),
            expected.strip_suffix(',').unwrap_or(&expected),
            "complete arguments for {callee}"
        );
        start
    }

    #[test]
    fn actual_startup_and_raw_registration_share_the_factory_arc() {
        // Source-wiring evidence, not runtime proof of these consumers. Real
        // production factory/source behavior is tested separately below.
        let production = alephcore::utils::source_scan::code_keeping_literals(
            &alephcore::utils::source_scan::production_prefix(include_str!("mod.rs")),
        );
        let compact: String = production
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let tree_owner = exact_startup_call(
            &compact,
            "startup_resources.install_ownership_tree",
            "ownership_tree.clone()",
        );
        let factory = exact_startup_call(
            &compact,
            "capability_init::build_capability_policy",
            "&loaded_app_config, Arc::clone(&ownership_tree)",
        );
        exact_startup_call(&compact, "capability_init::build_capability_events", "");
        assert!(
            compact.contains("letcapability_events=capability_init::build_capability_events();")
        );
        let bridge = exact_startup_call(
            &compact,
            "capability_init::start_capability_event_bridge",
            "&capability_events, server.event_bus().clone()",
        );
        let shutdown_owner = exact_startup_call(
            &compact,
            "startup_resources.install_capability_events",
            "capability_policy.clone(), capability_events.clone(), capability_event_bridge",
        );
        let host = exact_startup_call(&compact, "ProjectionHost::mount_with_runtime_and_events", "(*tool_registry_phase2).clone(), Arc::clone(&ownership_tree), memory_runtime.clone(), special_registry, Arc::clone(&capability_policy), capability_events, diagnostics_enabled");
        assert!(
            tree_owner < factory
                && factory < bridge
                && bridge < shutdown_owner
                && shutdown_owner < host
        );
        exact_startup_call(&compact, "alephcore::capability::tool_adapters::TerminalCapabilitySource::new", "projection_host.specials(), Arc::clone(&capability_policy), Arc::downgrade(&projection_host.facade()), Arc::clone(&app_config)");
        exact_startup_call(&compact, "register_agent_handlers", "&mut server, session_store.clone(), event_bus.clone(), router.clone(), &full_config, &*app_config.read().await, app_config.clone(), memory_runtime.clone(), raw_store.clone(), workspace_manager.clone(), agent_manager.clone(), acp_manager.clone(), a2a_tool_handle.clone(), cron_service.clone(), heartbeat_service.clone(), args.daemon, auth_bundle.auth_ctx.shared_token_mgr.clone(), auth_bundle.security_store.clone(), Some(sandbox.clone()), early_catalog_cache, early_marketplace_configs, mcp_handle.clone(), canvas_store.clone(), tool_registry_phase2.clone(), diagnostics_control, Arc::clone(&capability_policy)");
        exact_startup_call(&compact, "alephcore::gateway::mcp_face::McpFace::new_with_canonical_source", "&mcp_server_cfg, registry, tool_registry.unified_tools().cloned().collect(), Some(app_config.clone()), agent_result.tool_catalog.as_ref().map(|c| c.health()), server.operator_presence_probe(), Arc::clone(&tool_registry_phase2), projection_host.clone(), Arc::clone(&capability_policy)");
        assert_eq!(compact.matches("Arc::clone(&capability_policy)").count(), 4, "additional spelling guard for the four named consumers; shutdown is a fifth logical owner");
        let catalog = include_str!("builder/agent_init/tool_catalog_init.rs");
        let code = alephcore::utils::source_scan::code_keeping_literals(
            &alephcore::utils::source_scan::production_prefix(catalog),
        );
        let compact_catalog: String = code
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert!(compact_catalog.contains("letpolicy=Arc::clone(&capability_policy);asyncmove{alephcore::gateway::handlers::tools_invoke::handle_invoke("));
        exact_startup_call(
            &compact_catalog,
            "alephcore::gateway::handlers::tools_invoke::handle_invoke",
            "req, registry, agents, policy",
        );
    }

    #[tokio::test]
    async fn actual_event_factory_forwards_real_memory_source_with_same_sequence() {
        use alephcore::capability::events::{CapabilityEvent, CapabilityEventStream};
        use alephcore::capability::projection_host::ProjectionHost;
        use alephcore::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig};
        use alephcore::tools::registry::ToolHandlerRegistry;
        use std::time::Duration;
        tokio::time::timeout(Duration::from_secs(10), async {
            let root = tempfile::tempdir().unwrap();
            let tree = Arc::new(OwnershipTree::new());
            let policy = build_capability_policy(&Config::default(), tree.clone()).unwrap();
            let events = build_capability_events();
            let bus = Arc::new(alephcore::gateway::event_bus::GatewayEventBus::new());
            let mut forwarded = bus.subscribe();
            let bridge = start_capability_event_bridge(&events, bus);
            let memory = MemoryRuntime::new(MemoryRuntimeConfig {
                metadata_path: root.path().join("memory.db"),
                note_memory_dir: root.path().join("notes"),
            }, tree.clone());
            let host = ProjectionHost::mount_with_runtime_and_events(
                ToolHandlerRegistry::new(), tree, memory.clone(), CapabilityHandlerRegistry::new(), policy, events.clone(), false,
            );
            host.wait_until_ready().await;
            memory.start().await.unwrap();
            memory.stop().await.unwrap();
            let records = events.snapshot().unwrap();
            assert_eq!(records.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityActivated { id, .. } if id.namespace == "aleph/resources" && id.name == "memory.runtime")).count(), 1);
            assert_eq!(records.records().iter().filter(|record| matches!(record.event(), CapabilityEvent::CapabilityRevoked { id, .. } if id.namespace == "aleph/resources" && id.name == "memory.runtime")).count(), 1);
            events.close().unwrap();
            bridge.clone().join().await.unwrap();
            bridge.join().await.unwrap();
            let mut sequences = Vec::new();
            while let Ok(frame) = forwarded.try_recv() {
                let value: serde_json::Value = serde_json::from_str(&frame).unwrap();
                if value["topic"] == "capability.event" {
                    sequences.push(value["data"]["sequence"].as_u64().unwrap());
                }
            }
            assert_eq!(sequences, records.records().iter().map(|record| record.sequence()).collect::<Vec<_>>());
            let outcome = host.close_and_await().await;
            assert!(outcome.source_joined && outcome.applier_joined);
        }).await.unwrap();
    }

    #[test]
    fn register_canonical_builtin_definitions() {
        let registry = CapabilityHandlerRegistry::new();
        let ids = register_canonical_adapters(&registry).expect("canonical adapters register");
        assert_eq!(ids.len(), 2);
        assert!(ids
            .iter()
            .any(|id| format!("{}/{}", id.namespace, id.name) == TERMINAL_ADAPTER_ID));
        assert!(ids
            .iter()
            .any(|id| format!("{}/{}", id.namespace, id.name) == UI_ADAPTER_ID));
        assert!(registry
            .describe(&Default::default())
            .into_iter()
            .all(|capture| capture.schema().get("additionalProperties").is_some()));
    }
}
