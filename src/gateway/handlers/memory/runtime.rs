//! RPC adapters resolve a fresh admitted bundle instead of retaining services.

use crate::config::Config;
use crate::error::AlephError;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::handlers::{dreaming, graph, memory_config};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, METHOD_NOT_FOUND, SERVICE_UNAVAILABLE,
};
use crate::memory::runtime::MemoryRuntime;
use crate::sync_primitives::Arc;
use crate::thinker::memory_context_provider::RuntimeMemoryContext;
use serde_json::json;
use tokio::sync::RwLock;

fn unavailable(request: JsonRpcRequest, runtime: &MemoryRuntime) -> JsonRpcResponse {
    JsonRpcResponse::error_with_data(
        request.id,
        SERVICE_UNAVAILABLE,
        "memory_unavailable",
        json!({ "state": runtime.status().state }),
    )
}

pub async fn handle_memory(
    request: JsonRpcRequest,
    runtime: Arc<MemoryRuntime>,
    app_config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let operation = match runtime.operation() {
        Ok(operation) => operation,
        Err(_) => return unavailable(request, &runtime),
    };
    let id = request.id.clone();
    let admitted = operation.clone();
    operation
        .run(async move {
            let services = admitted.services()?;
            let db = admitted.backend().clone();
            let response = match request.method.as_str() {
                "memory.stats" => {
                    super::handle_stats(request, services.raw_store.clone(), db).await
                }
                "memory.listFacts" => super::handle_list_facts(request, db).await,
                "memory.list_corrections" => {
                    super::handle_list_corrections(request, services.raw_store.clone(), db).await
                }
                "memory.trace" => {
                    super::handle_trace(
                        request,
                        services.raw_store.clone(),
                        db,
                        Some(Arc::new(RuntimeMemoryContext::new(runtime))),
                    )
                    .await
                }
                "memory.compress" => match &services.compression {
                    Some(compression) => super::handle_compress(request, compression.clone()).await,
                    None => JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        "Compression not available: missing AI or embedding provider",
                    ),
                },
                "memory.reembed" => match services.embedding.get_active_provider().await {
                    Some(embedder) => {
                        super::handle_owned_reembed(
                            request,
                            admitted.clone(),
                            services.note_memory_dir.clone(),
                            embedder,
                            event_bus,
                            services.reembed.clone(),
                        )
                        .await
                    }
                    None => JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        "Reembed not available: missing embedding provider",
                    ),
                },
                "memory.reembed.cancel" => {
                    super::handle_reembed_cancel(request, services.reembed.clone()).await
                }
                "memory.retrieve_with_trace" => {
                    memory_config::handle_retrieve_with_trace(
                        request,
                        db,
                        services.embedding.get_active_provider().await,
                        app_config,
                    )
                    .await
                }
                "dreaming.list_insights" => dreaming::handle_list_insights(request, db).await,
                "dreaming.run_now" => match &services.dream {
                    Some(dream) => match dream.run_now().await {
                        Ok(report) => JsonRpcResponse::success(
                            request.id,
                            json!({ "ok": true, "report": report }),
                        ),
                        Err(error) => JsonRpcResponse::error(
                            request.id,
                            INTERNAL_ERROR,
                            format!("dreaming.run_now failed: {error}"),
                        ),
                    },
                    None => JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        "dreaming.run_now failed: Dream daemon is not configured",
                    ),
                },
                _ => JsonRpcResponse::error(request.id, METHOD_NOT_FOUND, "Unknown Memory method"),
            };
            Ok::<_, AlephError>(response)
        })
        .await
        .unwrap_or_else(|error| JsonRpcResponse::error(id, INTERNAL_ERROR, error.to_string()))
}

pub async fn handle_graph(request: JsonRpcRequest, runtime: Arc<MemoryRuntime>) -> JsonRpcResponse {
    let operation = match runtime.operation() {
        Ok(operation) => operation,
        Err(_) => return unavailable(request, &runtime),
    };
    let id = request.id.clone();
    let admitted = operation.clone();
    operation
        .run(async move {
            let services = admitted.services()?;
            let response = match request.method.as_str() {
                "graph.query" => {
                    graph::handle_query_impl(request, admitted.backend().clone()).await
                }
                "graph.search" => {
                    graph::handle_search_impl(request, admitted.backend().clone()).await
                }
                "graph.node_detail" => {
                    graph::handle_node_detail_impl(request, admitted.backend().clone()).await
                }
                "graph.update_note" => {
                    graph::handle_update_note_impl(request, services.indexer.clone()).await
                }
                "graph.rename_note" => {
                    graph::handle_rename_note_impl(request, services.indexer.clone()).await
                }
                "graph.delete_note" => {
                    graph::handle_delete_note_impl(request, services.indexer.clone()).await
                }
                _ => JsonRpcResponse::error(request.id, METHOD_NOT_FOUND, "Unknown Graph method"),
            };
            Ok::<_, AlephError>(response)
        })
        .await
        .unwrap_or_else(|error| JsonRpcResponse::error(id, INTERNAL_ERROR, error.to_string()))
}
