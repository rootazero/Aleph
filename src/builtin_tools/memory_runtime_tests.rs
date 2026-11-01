use crate::builtin_tools::note_manage::NoteManageTool;
use crate::builtin_tools::remember::RememberTool;
use crate::capability::descriptor::{Service, ServiceRelation};
use crate::capability::facade::Reference;
use crate::capability::ownership::{OwnershipTree, VisibilityScope};
use crate::memory::curated::CuratedMemoryStore;
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig};
use crate::memory::store::markdown::MarkdownSessionStore;
use crate::sync_primitives::Arc;
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::builtin::BuiltinHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::AlephTool;
use futures::FutureExt;
use serde_json::json;

struct Fixture {
    root: tempfile::TempDir,
    runtime: Arc<MemoryRuntime>,
    _raw: Arc<MarkdownSessionStore>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let raw = Arc::new(MarkdownSessionStore::new(root.path()).await.unwrap());
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: root.path().join("metadata.db"),
                note_memory_dir: root.path().join("note"),
            },
            Arc::new(OwnershipTree::new()),
        );
        Self {
            root,
            runtime,
            _raw: raw,
        }
    }

    fn admit(&self) -> MemoryOperation {
        self.runtime
            .capture(&Reference {
                id: crate::memory::runtime::resource_id(),
                visibility: VisibilityScope::default(),
            })
            .unwrap()
            .admit()
            .unwrap()
    }
}

fn create_note(filename: &str) -> serde_json::Value {
    json!({
        "action": "create", "agent_id": "main", "category": "reference",
        "filename": filename, "title": "Lifecycle evidence",
        "content": "This note belongs to the admitted Memory bundle."
    })
}

#[tokio::test]
async fn old_tool_object_cannot_bypass_new_resource_lease() {
    let fixture = Fixture::new().await;
    fixture.runtime.start().await.unwrap();
    let operation = fixture.admit();
    let tool = NoteManageTool::new(
        fixture.root.path().join("note"),
        operation.backend().clone(),
    );
    drop(operation);
    fixture.runtime.stop().await.unwrap();
    fixture.runtime.start().await.unwrap();
    let operation = fixture.admit();
    let result = operation
        .clone()
        .scope(tool.call_json(create_note("stale")))
        .await;
    assert!(
        result.is_err(),
        "an old concrete tool cannot use the new bundle"
    );
    assert!(
        !fixture
            .root
            .path()
            .join("note/main/reference/stale.md")
            .exists(),
        "admission must precede the Markdown write, not only its SQLite index update"
    );
    drop(operation);
    fixture.runtime.stop().await.unwrap();
}

#[tokio::test]
async fn admitted_note_manage_uses_one_bundle() {
    let fixture = Fixture::new().await;
    fixture.runtime.start().await.unwrap();
    let operation = fixture.admit();
    let backend = operation.backend().clone();
    let tool = NoteManageTool::new(fixture.root.path().join("note"), backend.clone());
    let mut stop = Box::pin(fixture.runtime.stop());
    assert!(stop.as_mut().now_or_never().is_none());
    operation
        .clone()
        .scope(tool.call_json(create_note("admitted")))
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&backend, operation.backend()));
    assert!(fixture
        .root
        .path()
        .join("note/main/reference/admitted.md")
        .exists());
    drop(operation);
    stop.await.unwrap();
    let connection = rusqlite::Connection::open(fixture.root.path().join("metadata.db")).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM notes_index WHERE agent_id = 'main'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "the admitted Markdown effect must reach the same SQLite bundle"
    );
}

#[tokio::test]
async fn remember_off_reports_unavailable_without_side_effects() {
    let fixture = Fixture::new().await;
    fixture.runtime.start().await.unwrap();
    let operation = fixture.admit();
    let path = fixture.root.path().join("MEMORY.md");
    let store = Arc::new(
        CuratedMemoryStore::load(path.clone(), 2000, "main")
            .await
            .unwrap(),
    );
    let tool = RememberTool::new(store).with_decision_log(Some(operation.backend().clone()));
    drop(operation);
    fixture.runtime.stop().await.unwrap();
    let before = std::fs::read(&path).ok();
    let result = tool
        .call_json(json!({"action":"add", "content":"Prefer one clear example."}))
        .await;
    assert!(
        result.is_err(),
        "an explicit Memory write must fail while Off: {result:?}"
    );
    assert_eq!(
        std::fs::read(&path).ok(),
        before,
        "a best-effort audit must not hide an unadmitted write"
    );
}

#[tokio::test]
async fn runtime_tool_stays_cold_and_uses_fresh_services_after_restart() {
    use crate::builtin_tools::note_schema::{NoteSchemaArgs, NoteSchemaTool};
    use crate::builtin_tools::runtime::RuntimeTool;
    use crate::config::types::memory::MemoryConfig;
    use crate::executor::{BuiltinToolConfig, BuiltinToolRegistry, ToolRegistry};
    use crate::memory::extensions::MemoryExtensionRegistry;
    use crate::memory::runtime::MemoryServicesConfig;

    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let root = tempfile::tempdir().unwrap();
    let raw = Arc::new(MarkdownSessionStore::new(root.path()).await.unwrap());
    let mut memory = MemoryConfig::default();
    memory.embedding.providers.clear();
    memory.project_scoped = false;
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("metadata.db"),
            note_memory_dir: root.path().join("note"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let inputs = MemoryServicesConfig {
        memory,
        raw_store: raw,
        provider: None,
        extensions: Arc::new(MemoryExtensionRegistry::new()),
        state_db: None,
        default_agent_id: "main".into(),
        curated_root: root.path().join("curated"),
        snapshot_dir: root.path().join("snapshots"),
    };
    let frozen = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("unconfigured.db"),
            note_memory_dir: root.path().join("unconfigured-note"),
        },
        Arc::new(OwnershipTree::new()),
    );
    frozen.start().await.unwrap();
    frozen.stop().await.unwrap();
    assert!(frozen.configure_services(inputs.clone()).is_err());
    runtime.configure_services(inputs.clone()).unwrap();
    assert!(runtime.configure_services(inputs.clone()).is_err());
    let tool = RuntimeTool::<NoteManageTool>::new(runtime.clone());
    let registry = BuiltinToolRegistry::with_config(BuiltinToolConfig {
        memory_runtime: Some(runtime.clone()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(registry
        .execute_tool("note_manage", create_note("registered-off"))
        .await
        .is_err());
    assert!(registry
        .execute_tool(
            "remember",
            serde_json::json!({"action": "add", "content": "Unadmitted registry memory"}),
        )
        .await
        .is_err());
    assert!(!root.path().join("curated").exists());
    assert!(!root.path().join("note").exists());
    let schema = RuntimeTool::<NoteSchemaTool>::new(runtime.clone());
    assert!(tool.call_json(create_note("off")).await.is_err());
    assert!(schema.call("main", NoteSchemaArgs::Read).await.is_err());
    assert!(!root.path().join("note").exists());
    runtime.start().await.unwrap();
    assert!(runtime.configure_services(inputs).is_err());
    let schema_text = "# Memory Schema\n\nThis schema belongs to the admitted runtime.";
    let registered_read = registry
        .execute_tool("note_schema", serde_json::json!({"action": "read"}))
        .await;
    assert!(
        registered_read.is_ok(),
        "production schema adapter: {registered_read:?}"
    );
    let written = schema
        .call(
            "main",
            NoteSchemaArgs::Write {
                content: schema_text.into(),
                expected_hash: None,
            },
        )
        .await
        .unwrap();
    assert!(!written.conflict);
    tool.call_json(create_note("first")).await.unwrap();
    runtime.stop().await.unwrap();
    assert!(schema
        .call(
            "main",
            NoteSchemaArgs::Write {
                content: "unadmitted schema overwrite".into(),
                expected_hash: written.content_hash,
            }
        )
        .await
        .is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("note/main/SCHEMA.md")).unwrap(),
        schema_text
    );
    assert!(tool.call_json(create_note("off-again")).await.is_err());
    assert!(!root
        .path()
        .join("note/main/reference/off-again.md")
        .exists());
    runtime.start().await.unwrap();
    assert_eq!(
        schema
            .call("main", NoteSchemaArgs::Read)
            .await
            .unwrap()
            .content
            .as_deref(),
        Some(schema_text)
    );
    let registered_read = registry
        .execute_tool("note_schema", serde_json::json!({"action": "read"}))
        .await
        .unwrap();
    assert_eq!(registered_read["content"].as_str(), Some(schema_text));
    tool.call_json(create_note("second")).await.unwrap();
    let remembered = registry
        .execute_tool(
            "remember",
            serde_json::json!({"action": "add", "content": "The admitted registry uses the current memory bundle."}),
        )
        .await
        .unwrap();
    assert_eq!(remembered["written"], true);
    let graph = registry
        .execute_tool("note_graph_query", serde_json::json!({"op": "schema"}))
        .await
        .unwrap();
    assert_eq!(graph["schema"]["total_notes"], 2);
    let trace = registry
        .execute_tool(
            "memory_trace",
            serde_json::json!({"kind": "write_decision", "target": "admitted registry"}),
        )
        .await
        .unwrap();
    assert!(!trace["write_decisions"].as_array().unwrap().is_empty());
    runtime.stop().await.unwrap();
    let connection = rusqlite::Connection::open(root.path().join("metadata.db")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM notes_index", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn memory_definition_reaches_registered_canonical_descriptor() {
    let fixture = Fixture::new().await;
    fixture.runtime.start().await.unwrap();
    let operation = fixture.admit();
    let tool = NoteManageTool::new(
        fixture.root.path().join("note"),
        operation.backend().clone(),
    );
    let handler = Arc::new(BuiltinHandler::new("note_manage".into(), Arc::new(tool)));
    let registry = ToolHandlerRegistry::new();
    let _registration = registry
        .register(
            ToolCapabilityDescriptor::from_definition(&handler.definition(), 0),
            handler,
        )
        .unwrap();
    assert_eq!(
        registry.snapshot_state().entries()["note_manage"]
            .descriptor
            .services,
        vec![Service {
            relation: ServiceRelation::Requires,
            target: crate::memory::runtime::resource_id(),
        }]
    );
    drop(operation);
    fixture.runtime.stop().await.unwrap();
}
