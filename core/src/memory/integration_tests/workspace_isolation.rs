//! Integration tests for workspace-based memory isolation.
//!
//! Verifies that facts, graph nodes, and CRUD operations
//! are properly scoped to their respective workspaces.

#[cfg(test)]
mod tests {
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::lance::LanceMemoryBackend;
    use crate::memory::store::types::SearchFilter;
    use crate::memory::store::{GraphNode, GraphStore, MemoryStore};
    use crate::gateway::workspace::{WorkspaceContext, WorkspaceFilter, DEFAULT_WORKSPACE};
    use crate::memory::workspace_store;

    /// Helper: create a MemoryFact with a synthetic embedding and assigned workspace.
    fn make_fact(content: &str, workspace: &str, embedding: Vec<f32>) -> MemoryFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.embedding = Some(embedding);
        fact.embedding_model = "test-model".to_string();
        fact.content_hash = format!("hash-{}", uuid::Uuid::new_v4());
        fact.workspace = workspace.to_string();
        fact
    }

    // -----------------------------------------------------------------------
    // Test 1: Facts inserted into different workspaces are isolated.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_workspace_isolation_facts() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();

        // Insert fact into workspace "ws-a"
        let emb_a = vec![0.9f32; 1024];
        let fact_a = make_fact("Bitcoin price is $100k", "ws-a", emb_a.clone());
        backend.insert_fact(&fact_a).await.unwrap();

        // Insert fact into workspace "ws-b"
        let emb_b = vec![0.1f32; 1024];
        let fact_b = make_fact("Chapter 3 outline complete", "ws-b", emb_b.clone());
        backend.insert_fact(&fact_b).await.unwrap();

        // Search in workspace A — should only return A's fact
        let filter_a = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Single("ws-a".into()));
        let results_a = backend
            .vector_search(&emb_a, 1024, &filter_a, 10)
            .await
            .unwrap();
        assert_eq!(results_a.len(), 1, "workspace ws-a should have exactly 1 fact");
        assert_eq!(results_a[0].fact.content, "Bitcoin price is $100k");
        assert_eq!(results_a[0].fact.workspace, "ws-a");

        // Search in workspace B — should only return B's fact
        let filter_b = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Single("ws-b".into()));
        let results_b = backend
            .vector_search(&emb_b, 1024, &filter_b, 10)
            .await
            .unwrap();
        assert_eq!(results_b.len(), 1, "workspace ws-b should have exactly 1 fact");
        assert_eq!(results_b[0].fact.content, "Chapter 3 outline complete");
        assert_eq!(results_b[0].fact.workspace, "ws-b");

        // Search with All — should return both
        let filter_all = SearchFilter::new()
            .with_workspace(WorkspaceFilter::All);
        let results_all = backend
            .vector_search(&emb_a, 1024, &filter_all, 10)
            .await
            .unwrap();
        assert_eq!(results_all.len(), 2, "All workspaces should return both facts");
    }

    // -----------------------------------------------------------------------
    // Test 2: Graph nodes in different workspaces are isolated.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_workspace_isolation_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();

        // Create "Bitcoin" node as kind="asset" in workspace ws-a
        let node_a = GraphNode {
            id: "bitcoin-ws-a".to_string(),
            name: "Bitcoin".to_string(),
            kind: "asset".to_string(),
            aliases: vec!["BTC".to_string()],
            metadata_json: "{}".to_string(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            workspace: "ws-a".to_string(),
        };
        backend.upsert_node(&node_a, "ws-a").await.unwrap();

        // Create "Bitcoin" node as kind="character" in workspace ws-b
        let node_b = GraphNode {
            id: "bitcoin-ws-b".to_string(),
            name: "Bitcoin".to_string(),
            kind: "character".to_string(),
            aliases: vec![],
            metadata_json: "{}".to_string(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            workspace: "ws-b".to_string(),
        };
        backend.upsert_node(&node_b, "ws-b").await.unwrap();

        // Resolve "Bitcoin" in workspace ws-a — should be "asset"
        let resolved_a = backend
            .resolve_entity("Bitcoin", None, "ws-a")
            .await
            .unwrap();
        assert_eq!(resolved_a.len(), 1, "should find exactly 1 entity in ws-a");
        assert_eq!(resolved_a[0].kind, "asset");

        // Resolve "Bitcoin" in workspace ws-b — should be "character"
        let resolved_b = backend
            .resolve_entity("Bitcoin", None, "ws-b")
            .await
            .unwrap();
        assert_eq!(resolved_b.len(), 1, "should find exactly 1 entity in ws-b");
        assert_eq!(resolved_b[0].kind, "character");

        // get_node should respect workspace
        let got_a = backend.get_node("bitcoin-ws-a", "ws-a").await.unwrap();
        assert!(got_a.is_some(), "node should exist in ws-a");
        assert_eq!(got_a.unwrap().kind, "asset");

        let got_b = backend.get_node("bitcoin-ws-b", "ws-b").await.unwrap();
        assert!(got_b.is_some(), "node should exist in ws-b");
        assert_eq!(got_b.unwrap().kind, "character");
    }

    // -----------------------------------------------------------------------
    // Test 3: Default workspace backward compatibility.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_default_workspace_backward_compat() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();

        // Insert a fact with the default workspace (simulating legacy behavior)
        let emb = vec![0.5f32; 1024];
        let fact = make_fact("User prefers dark mode", DEFAULT_WORKSPACE, emb.clone());
        backend.insert_fact(&fact).await.unwrap();

        // Search with default workspace filter — should find it
        let filter_default = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Single(DEFAULT_WORKSPACE.to_string()));
        let results = backend
            .vector_search(&emb, 1024, &filter_default, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "should find the fact in the default workspace");
        assert_eq!(results[0].fact.content, "User prefers dark mode");

        // Search in a nonexistent workspace — should find nothing
        let filter_none = SearchFilter::new()
            .with_workspace(WorkspaceFilter::Single("nonexistent".to_string()));
        let results_empty = backend
            .vector_search(&emb, 1024, &filter_none, 10)
            .await
            .unwrap();
        assert!(
            results_empty.is_empty(),
            "nonexistent workspace should return no results"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: WorkspaceContext filter propagation.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_workspace_context_propagation() {
        // Create a WorkspaceContext for workspace "crypto" with Owner scope
        let ctx = WorkspaceContext::new("crypto", NamespaceScope::Owner);

        // Build the search filter
        let filter = ctx.to_search_filter();
        let sql = filter
            .to_lance_filter()
            .expect("filter should produce a SQL clause");

        // Verify all expected constraints are present
        assert!(
            sql.contains("workspace = 'crypto'"),
            "SQL should contain workspace filter, got: {}",
            sql
        );
        assert!(
            sql.contains("namespace = 'owner'"),
            "SQL should contain namespace filter, got: {}",
            sql
        );
        assert!(
            sql.contains("is_valid = true"),
            "SQL should contain validity filter, got: {}",
            sql
        );

        // Verify default_owner() context produces default workspace
        let default_ctx = WorkspaceContext::default_owner();
        assert_eq!(default_ctx.workspace_id(), DEFAULT_WORKSPACE);
        let default_sql = default_ctx
            .to_search_filter()
            .to_lance_filter()
            .unwrap();
        assert!(default_sql.contains("workspace = 'default'"));
    }

    // -----------------------------------------------------------------------
    // Test 5: Workspace CRUD operations.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_workspace_crud_operations() {
        use crate::gateway::workspace::Workspace;
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            LanceMemoryBackend::open_or_create(tmp.path())
                .await
                .unwrap(),
        );

        // Create a workspace
        let ws = Workspace::new("crypto", "Crypto Research");
        workspace_store::create_workspace(&backend, &ws)
            .await
            .unwrap();

        // List workspaces — should contain our new one
        let list = workspace_store::list_workspaces(&backend).await.unwrap();
        assert!(
            list.iter().any(|w| w.id == "crypto"),
            "workspace list should contain 'crypto'"
        );

        // Get workspace by ID — verify details
        let fetched = workspace_store::get_workspace(&backend, "crypto")
            .await
            .unwrap();
        assert!(fetched.is_some(), "should find workspace 'crypto'");
        let fetched = fetched.unwrap();
        assert_eq!(fetched.name, "Crypto Research");
        assert!(!fetched.is_archived);
        assert!(!fetched.is_default);

        // Archive workspace — verify is_archived = true
        workspace_store::archive_workspace(&backend, "crypto")
            .await
            .unwrap();
        let archived = workspace_store::get_workspace(&backend, "crypto")
            .await
            .unwrap()
            .unwrap();
        assert!(archived.is_archived, "workspace should be archived");

        // Verify default workspace cannot be archived
        let default_ws = Workspace::default_workspace();
        workspace_store::create_workspace(&backend, &default_ws)
            .await
            .unwrap();
        let result = workspace_store::archive_workspace(&backend, DEFAULT_WORKSPACE).await;
        assert!(
            result.is_err(),
            "archiving default workspace should return an error"
        );
    }
}
