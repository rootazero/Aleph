//! Two-crate production evidence: this leg uses the shared startup factory and
//! real request builder; the binary leg checks its delegate/startup wiring.
use super::{DeferredTools, ScopedToolService};
use crate::approval::{with_call_identity, CallIdentity};
use crate::capability::backend::TOOL_NAMESPACE;
use crate::capability::constraints::{ConstraintContext, ConstraintViolation};
use crate::capability::descriptor::CapabilityId;
use crate::capability::facade::{Reference, Zahir};
use crate::capability::ownership::{OwnerRef, OwnershipTree, VisibilityScope};
use crate::capability::policy_runtime::{
    CapabilityCallContext, ConstraintFactsSource, PolicyFactSourceKey,
};
use crate::capability::production_init::build_capability_policy;
use crate::capability::projection_host::ProjectionHost;
use crate::capability::provider::{ProviderHandle, ProviderState};
use crate::config::types::policies::capability_constraints::ResourceUnit;
use crate::config::types::policies::ExecTier;
use crate::gateway::execution_engine::build_request_tool_service;
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig};
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;
use crate::sync_primitives::Arc;
use crate::tools::adapters::McpRegistryTool;
use crate::tools::descriptor::{ToolCapabilityDescriptor, ToolSource};
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService};
use crate::tools::turn_context::TurnContext;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, Semaphore};

const NAME: &str = "production_policy_fixture";
fn id() -> CapabilityId {
    CapabilityId {
        namespace: TOOL_NAMESPACE.into(),
        name: NAME.into(),
    }
}
fn context(tag: &str) -> TurnContext {
    TurnContext {
        session_key: crate::routing::session_key::SessionKey::main(tag),
        run_id: tag.into(),
        channel_id: "test".into(),
        conversation_id: "conv".into(),
        caller_role: None,
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
        telecom_authority: None,
    }
}
fn config(conditions: &str) -> crate::Config {
    toml::from_str(&format!("[policies.capability_constraints.\"aleph/tools/{NAME}\"]\n{conditions}\nresource = {{ unit = \"active_calls\", max_active = 2 }}\n")).unwrap()
}
struct Effect {
    definition: ToolDefinition,
    writes: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    memory: bool,
}
#[async_trait::async_trait]
impl ToolHandler for Effect {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if input.get("write").and_then(Value::as_bool).unwrap_or(true) {
            if self.memory {
                let operation =
                    MemoryOperation::current().expect("real Memory-owned handler scope");
                operation
                    .backend()
                    .with_conn(|conn| {
                        conn.execute_batch("INSERT INTO production_policy_effect VALUES (1)")
                            .map_err(|error| crate::error::AlephError::other(error.to_string()))?;
                        Ok(())
                    })
                    .await
                    .map_err(|error| ToolError::Execution {
                        name: NAME.into(),
                        cause: error.to_string(),
                    })?;
            }
            self.writes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(ToolOutput {
            value: json!({"written": true}),
            metadata: Default::default(),
        })
    }
}
struct Facts {
    provider: ProviderHandle,
    key: PolicyFactSourceKey,
    mode: AtomicUsize,
    observed: AtomicUsize,
    reads: AtomicUsize,
}
impl ConstraintFactsSource for Facts {
    fn source_key(&self) -> PolicyFactSourceKey {
        self.key.clone()
    }
    fn supported_conditions(&self) -> &'static [&'static str] {
        &["provider_ready"]
    }
    fn supported_units(&self) -> &'static [ResourceUnit] {
        &[ResourceUnit::ActiveCalls]
    }
    fn snapshot(
        &self,
        call: &CapabilityCallContext,
        _: Option<ResourceUnit>,
        other: usize,
    ) -> Result<ConstraintContext, ConstraintViolation> {
        assert_ne!(
            call.actor(),
            &OwnerRef::Runtime,
            "trusted Run actor is not rewritten to Runtime Tool owner"
        );
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.observed.store(other, Ordering::SeqCst);
        let mode = self.mode.load(Ordering::SeqCst);
        if mode == 3 {
            return Err(ConstraintViolation::ClockUnavailable);
        }
        let facts = ConstraintContext::now(other)?;
        if mode == 2 {
            return Ok(facts);
        }
        Ok(facts.with_condition(
            "provider_ready",
            mode == 1 && self.provider.snapshot().0 == ProviderState::Ready,
        ))
    }
}
struct Fixture {
    root: tempfile::TempDir,
    memory: Arc<MemoryRuntime>,
    host: Arc<ProjectionHost>,
    registry: Arc<ToolHandlerRegistry>,
    inner: Arc<LoopToolRegistry>,
    writes: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
}
impl Fixture {
    async fn new(memory: bool, approval: bool, conditions: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let tree = Arc::new(OwnershipTree::new());
        let policy = build_capability_policy(&config(conditions), tree.clone()).unwrap();
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: root.path().join("metadata.db"),
                note_memory_dir: root.path().join("notes"),
            },
            tree.clone(),
        );
        runtime.start().await.unwrap();
        rusqlite::Connection::open(root.path().join("metadata.db"))
            .unwrap()
            .execute_batch("CREATE TABLE production_policy_effect(value INTEGER)")
            .unwrap();
        let registry = Arc::new(ToolHandlerRegistry::new());
        let writes = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let services = if memory {
            json!([{"relation":"requires","target":{"namespace":"aleph/resources","name":"memory.runtime"}}])
        } else {
            json!([])
        };
        let definition: ToolDefinition = serde_json::from_value(json!({
            "name":NAME, "description":"Production Policy effect fixture", "input_schema":{"type":"object"}, "source":ToolSource::Builtin,
            "metadata":{"requires_approval":approval, "max_duration_ms":10000, "services":services}
        })).unwrap();
        registry
            .register(
                ToolCapabilityDescriptor::from_definition(&definition, 0),
                Arc::new(Effect {
                    definition,
                    writes: writes.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                    memory,
                }),
            )
            .unwrap();
        let host = ProjectionHost::mount_with_policy(
            registry.as_ref().clone(),
            tree,
            runtime.clone(),
            policy.clone(),
            false,
        );
        assert!(host.wait_until_ready().await);
        assert!(Arc::ptr_eq(&host.facade().policy(), &policy));
        let mut inner = LoopToolRegistry::new();
        inner.bind_canonical_registry(registry.clone(), Arc::new(|_| true));
        assert!(inner.bind_canonical_facade(host.facade()));
        for entry in registry.entries_snapshot().values() {
            inner.register(Box::new(McpRegistryTool::from_registry_entry(
                entry.handler.clone(),
                &entry.descriptor,
            )));
        }
        Self {
            root,
            memory: runtime,
            host,
            registry,
            inner: Arc::new(inner),
            writes,
            entered,
            release,
        }
    }
    fn service(&self, tag: &str) -> ScopedToolService {
        ScopedToolService::new(self.inner.clone(), BTreeSet::new()).with_turn_context(context(tag))
    }
    fn facts(&self) -> Arc<Facts> {
        let facade = self.host.facade();
        let lease = facade
            .resolve(Reference {
                id: id(),
                visibility: VisibilityScope::default(),
            })
            .unwrap();
        let facts = Arc::new(Facts {
            provider: self.registry.provider(),
            key: PolicyFactSourceKey::for_source(
                &self.registry.provider(),
                &id(),
                lease.descriptor.revision.0,
            ),
            mode: AtomicUsize::new(1),
            observed: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
        });
        facade
            .policy()
            .prepare_attachment(&lease, facts.clone())
            .unwrap();
        facts
    }
    fn persisted(&self) -> i64 {
        rusqlite::Connection::open(self.root.path().join("metadata.db"))
            .unwrap()
            .query_row("SELECT COUNT(*) FROM production_policy_effect", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    async fn close(self) {
        self.memory.stop().await.unwrap();
        self.host.close_and_await().await;
    }
}
async fn call(service: &dyn ToolService, tag: &str, input: Value) -> Result<ToolOutput, ToolError> {
    with_call_identity(
        Some(CallIdentity {
            turn_id: crate::session::events::TurnId::nil(),
            call_id: tag.into(),
        }),
        service.execute(NAME, input),
    )
    .await
}
struct PausedApproval {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
#[async_trait::async_trait]
impl ApprovalRequester for PausedApproval {
    async fn request_approval(&self, _: &ApprovalAction) -> ApprovalResponse {
        self.entered.notify_one();
        self.release.notified().await;
        ApprovalOutcome::Approved.into()
    }
}
#[tokio::test]
async fn production_request_builder_installs_the_startup_policy() {
    let fixture = Fixture::new(
        false,
        false,
        "required_conditions = { provider_ready = true }",
    )
    .await;
    let facts = fixture.facts();
    facts.mode.store(0, Ordering::SeqCst);
    let service = build_request_tool_service(
        fixture.inner.clone(),
        BTreeSet::new(),
        None,
        Some(context("production-builder")),
        None,
        "production-builder",
        None,
        ExecTier::Full,
        false,
        &[],
        false,
        Arc::new(DeferredTools::default()),
        None,
    );
    fixture.release.add_permits(2);
    let denied = call(service.as_ref(), "builder-denied", json!({})).await;
    assert!(
        denied.is_err(),
        "configured current facts must deny: {denied:?}"
    );
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
    facts.mode.store(1, Ordering::SeqCst);
    call(service.as_ref(), "builder-allowed", json!({}))
        .await
        .unwrap();
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    fixture.close().await;
}
#[tokio::test]
async fn post_approval_facts_and_memory_ownership_reach_effect() {
    for memory in [false, true] {
        for mode in [0, 1, 2, 3, 4] {
            let fixture = Fixture::new(
                memory,
                true,
                "required_conditions = { provider_ready = true }",
            )
            .await;
            let facts = fixture.facts();
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let service = Arc::new(
                fixture
                    .service(&format!("post-approval-{memory}-{mode}"))
                    .with_confirmation(Arc::new(PausedApproval {
                        entered: entered.clone(),
                        release: release.clone(),
                    })),
            );
            fixture.release.add_permits(1);
            let task =
                tokio::spawn(
                    async move { call(service.as_ref(), "post-approval", json!({})).await },
                );
            tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
                .await
                .expect("authentic approval reached");
            assert_eq!(
                facts.reads.load(Ordering::SeqCst),
                0,
                "facts must not be cached before approval"
            );
            if mode == 4 {
                fixture
                    .host
                    .facade()
                    .policy()
                    .withdraw_fact_source(&facts.key);
            } else {
                facts.mode.store(mode, Ordering::SeqCst);
            }
            release.notify_one();
            let result = task.await.unwrap();
            if mode == 1 {
                result.expect("same genuine approval path must allow current healthy facts");
                assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
                assert_eq!(fixture.persisted(), if memory { 1 } else { 0 });
            } else {
                assert!(
                    result.is_err(),
                    "latest source facts {memory}/{mode}: {result:?}"
                );
                assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
                assert_eq!(fixture.persisted(), 0);
            }
            // Policy reads facts outside its locks, then rechecks whether the
            // attachment is still current. A read is not an authorization:
            // withdrawal must still produce Err and zero effects above.
            assert!(
                facts.reads.load(Ordering::SeqCst) > 0,
                "final guard must read current source facts before its final binding check"
            );
            fixture.close().await;
        }
    }
}
#[tokio::test]
async fn memory_owned_counter_survives_caller_drop_until_real_completion() {
    let fixture = Fixture::new(
        true,
        false,
        "required_conditions = { provider_ready = true }",
    )
    .await;
    let facts = fixture.facts();
    let service = Arc::new(fixture.service("memory-owned-count"));
    let first_service = service.clone();
    let first = tokio::spawn(async move {
        call(first_service.as_ref(), "first-owned", json!({"write":true})).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        fixture.entered.notified(),
    )
    .await
    .unwrap();
    let second_service = service.clone();
    let second = tokio::spawn(async move {
        call(
            second_service.as_ref(),
            "second-owned",
            json!({"write":false}),
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        fixture.entered.notified(),
    )
    .await
    .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let denied = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        call(service.as_ref(), "candidate-over-limit", json!({})),
    )
    .await
    .expect("full occupancy must reject before entering the blocked handler");
    assert!(
        denied.is_err(),
        "two actual owned operations occupy max_active=2: {denied:?}"
    );
    assert_eq!(facts.observed.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.persisted(), 0);
    fixture.release.add_permits(2);
    second.await.unwrap().unwrap();
    fixture.memory.stop().await.unwrap();
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.persisted(), 1);
    fixture.host.close_and_await().await;
}
#[tokio::test]
async fn post_approval_occupancy_is_read_from_real_running_operations() {
    for memory in [false, true] {
        let fixture = Fixture::new(
            memory,
            false,
            "required_conditions = { provider_ready = true }",
        )
        .await;
        let facts = fixture.facts();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let asking: crate::config::types::policies::ToolPermissionsConfig =
            toml::from_str("default = \"ask\"").unwrap();
        let candidate = Arc::new(
            fixture
                .service(&format!("occupancy-candidate-{memory}"))
                .with_tool_permissions(asking)
                .with_confirmation(Arc::new(PausedApproval {
                    entered: entered.clone(),
                    release: release.clone(),
                })),
        );
        let pending =
            tokio::spawn(
                async move { call(candidate.as_ref(), "occupancy-approved", json!({})).await },
            );
        tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
            .await
            .expect("real permission approval reached");
        let occupancy = Arc::new(fixture.service(&format!("occupancy-other-{memory}")));
        let mut tasks = Vec::new();
        for tag in ["occupying-first", "occupying-second"] {
            let service = occupancy.clone();
            tasks.push(tokio::spawn(async move {
                call(service.as_ref(), tag, json!({"write":false})).await
            }));
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                fixture.entered.notified(),
            )
            .await
            .unwrap();
        }
        release.notify_one();
        let result = pending.await.unwrap();
        assert!(result.is_err(), "approval is not a reservation: {result:?}");
        assert_eq!(facts.observed.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.persisted(), 0);
        fixture.release.add_permits(2);
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        fixture.release.add_permits(1);
        call(occupancy.as_ref(), "allowed-after-real-finish", json!({}))
            .await
            .unwrap();
        assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.persisted(), if memory { 1 } else { 0 });
        fixture.close().await;
    }
}

#[tokio::test]
async fn allowed_policy_does_not_replace_existing_tool_authorization() {
    let fixture = Fixture::new(
        false,
        false,
        "required_conditions = { provider_ready = true }",
    )
    .await;
    let facts = fixture.facts();
    let denied = ScopedToolService::new(
        fixture.inner.clone(),
        BTreeSet::from(["some_other_tool".into()]),
    )
    .with_turn_context(context("existing-allowlist"));
    fixture.release.add_permits(1);
    assert!(matches!(
        call(&denied, "existing-allowlist", json!({})).await,
        Err(ToolError::NotFound { .. })
    ));
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
    assert_eq!(facts.observed.load(Ordering::SeqCst), 0);
    call(
        &fixture.service("existing-allowlist-positive"),
        "allowed",
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    fixture.close().await;
}

#[tokio::test]
async fn memory_resource_rule_and_missing_condition_fail_closed() {
    for memory in [false, true] {
        let fixture = Fixture::new(
            memory,
            false,
            "required_conditions = { unknown_owner_fact = true }",
        )
        .await;
        fixture.release.add_permits(1);
        let result = call(
            &fixture.service("missing-condition"),
            "missing-condition",
            json!({}),
        )
        .await;
        assert!(
            result.is_err(),
            "unsupported source fact must deny: {result:?}"
        );
        assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.persisted(), 0);
        fixture.close().await;
    }
}
