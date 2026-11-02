use std::sync::Arc;

use alephcore::capability::ownership::OwnershipTree;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::protocol::{JsonRpcRequest, SERVICE_UNAVAILABLE};
use alephcore::gateway::GatewayServer;
use alephcore::memory::extensions::MemoryExtensionRegistry;
use alephcore::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig};
use alephcore::memory::store::markdown::MarkdownSessionStore;
use alephcore::memory::store::raw_memory::RawMemoryStore;
use alephcore::routing::DEFAULT_AGENT_ID;
use serde_json::json;

#[tokio::test]
async fn off_graph_and_memory_rpc_fail_explicitly() {
    let root = tempfile::tempdir().unwrap();
    let metadata = root.path().join("derived.db");
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: metadata.clone(),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let raw: Arc<dyn RawMemoryStore> = Arc::new(
        MarkdownSessionStore::new(&root.path().join("raw"))
            .await
            .unwrap(),
    );
    let mut config = alephcore::Config::default();
    config.memory.embedding.providers.clear();
    config.memory.dreaming.enabled = false;
    config.memory.profile.enabled = false;
    let config = Arc::new(tokio::sync::RwLock::new(config));
    runtime
        .configure_services(MemoryServicesConfig {
            app_config: config.clone(),
            raw_store: raw.clone(),
            session_store: None,
            provider: None,
            extensions: Arc::new(MemoryExtensionRegistry::new()),
            state_db: None,
            default_agent_id: DEFAULT_AGENT_ID.to_string(),
            curated_root: root.path().join("agents"),
            snapshot_dir: root.path().join("sessions"),
        })
        .unwrap();
    let mut server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    super::register_memory_handlers(
        &mut server,
        &raw,
        &Arc::new(GatewayEventBus::new()),
        &config,
        None,
        &runtime,
        false,
    );
    super::register_graph_handlers(&mut server, &runtime);

    assert!(
        !metadata.exists(),
        "registration must leave the source cold"
    );
    for method in [
        "memory.stats",
        "memory.listFacts",
        "memory.list_corrections",
        "memory.compress",
        "memory.reembed",
        "graph.query",
        "graph.search",
        "graph.node_detail",
        "graph.update_note",
        "graph.rename_note",
        "graph.delete_note",
    ] {
        let response = server
            .handlers_mut()
            .handle(&JsonRpcRequest::new(
                method,
                Some(json!({})),
                Some(json!(1)),
            ))
            .await;
        let error = response
            .error
            .unwrap_or_else(|| panic!("{method} served a stopped source"));
        assert_eq!(error.code, SERVICE_UNAVAILABLE, "{method}");
        assert_eq!(error.message, "memory_unavailable", "{method}");
        assert_eq!(error.data.unwrap()["state"], "off", "{method}");
    }
    assert!(
        !metadata.exists(),
        "unavailable RPCs must not open the source"
    );

    for _ in 0..2 {
        runtime.start().await.unwrap();
        for method in ["memory.stats", "graph.query"] {
            let response = server
                .handlers_mut()
                .handle(&JsonRpcRequest::new(
                    method,
                    Some(json!({})),
                    Some(json!(2)),
                ))
                .await;
            assert!(response.error.is_none(), "{method}: {:?}", response.error);
        }
        runtime.stop().await.unwrap();
        let response = server
            .handlers_mut()
            .handle(&JsonRpcRequest::new(
                "graph.query",
                Some(json!({})),
                Some(json!(3)),
            ))
            .await;
        assert_eq!(response.error.unwrap().message, "memory_unavailable");
    }
    drop(server);
}
