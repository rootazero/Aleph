//! Task4 behavior tests for the real PTY owner and all terminal faces.
//!
//! These tests deliberately use the existing process-global manager and the
//! canonical shared wire types. They must not approve a fixture-only executor.

use crate::approval::{with_call_identity, CallIdentity};
use crate::capability::ownership::OwnershipTree;
use crate::capability::policy::PolicyEngine;
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
};
use crate::capability::special::CapabilityHandlerRegistry;
use crate::capability::tool_adapters::{
    register_canonical_adapters_with_tools_and_source, TerminalCapabilitySource,
};
use crate::capability::zahir_facade::ZahirFacade;
use crate::config::types::policies::capability_constraints::CapabilityConstraintRule;
use crate::config::types::policies::capability_constraints::CapabilityConstraintsConfig;
use crate::gateway::handlers::pty::{handle_attach_with_source, install_handler_handoff_hook};
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::pty::{self, SpawnOptions};
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sync_primitives::Arc;
use crate::tools::adapters::McpRegistryTool;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::runtime::LoopToolRegistry;
use crate::tools::scoped::ScopedToolService;
use crate::tools::service::{ToolError, ToolService};
use aleph_protocol::capability::{PtyOperation, PtySpawnParams, PtyToolParams};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Weak;
use tokio::sync::RwLock;

#[test]
#[serial_test::parallel(pty_global_manager)]
fn manager_source_binding_is_idempotent_and_conflict_fenced() {
    let manager = crate::gateway::pty::manager::PtyManager::cold_for_test();
    let source = Arc::new(TerminalCapabilitySource::new(
        CapabilityHandlerRegistry::new(),
        Arc::new(PolicyEngine::new(OwnershipTree::new())),
        Weak::new(),
        Arc::new(RwLock::new(crate::Config::default())),
    ));
    let other = Arc::new(TerminalCapabilitySource::new(
        CapabilityHandlerRegistry::new(),
        Arc::new(PolicyEngine::new(OwnershipTree::new())),
        Weak::new(),
        Arc::new(RwLock::new(crate::Config::default())),
    ));
    let weak = Arc::downgrade(&source);
    assert!(manager.bind_capability_source(weak.clone()).is_ok());
    assert!(manager.bind_capability_source(weak).is_ok());
    assert!(manager
        .bind_capability_source(Arc::downgrade(&other))
        .is_err());
}

#[test]
fn source_binding_accepts_same_incarnation_and_rejects_conflict() {
    let source = TerminalCapabilitySource::new(
        CapabilityHandlerRegistry::new(),
        Arc::new(PolicyEngine::new(OwnershipTree::new())),
        Weak::new(),
        Arc::new(RwLock::new(crate::Config::default())),
    );
    let provider_id = ProviderId::new("terminal-fix1").expect("provider id");
    let first = ProviderHandle::new(ProviderLifecycle::new(
        provider_id.clone(),
        ProviderTransport::InProcess,
    ));
    let same = first.clone();
    let conflict = ProviderHandle::new(ProviderLifecycle::new(
        provider_id,
        ProviderTransport::InProcess,
    ));
    assert!(source.bind_manager_handle(first).is_ok());
    assert!(source.bind_manager_handle(same).is_ok());
    assert!(source.bind_manager_handle(conflict).is_err());
}

#[test]
fn terminal_schema_never_claims_three_stream_output() {
    let operations = [
        PtyToolParams::Input {
            session_id: "s".into(),
            data: "x".into(),
            base64: false,
        },
        PtyToolParams::Attach {
            session_id: "s".into(),
        },
        PtyToolParams::Resize {
            session_id: "s".into(),
            rows: 24,
            cols: 80,
        },
        PtyToolParams::Close {
            session_id: "s".into(),
        },
    ];
    let encoded = operations
        .iter()
        .map(|operation| serde_json::to_value(operation).expect("terminal operation schema"))
        .collect::<Vec<_>>();
    assert!(encoded.iter().all(|value| {
        let operation = value["operation"].as_str().expect("tagged operation");
        matches!(operation, "input" | "attach" | "resize" | "close")
    }));
    assert!(encoded.iter().all(|value| {
        !value.to_string().contains("read_stdin")
            && !value.to_string().contains("write_stdout")
            && !value.to_string().contains("write_stderr")
    }));
}

#[test]
fn shared_spawn_params_preserve_existing_rpc_defaults() {
    let params: PtySpawnParams = serde_json::from_value(json!({})).expect("default spawn params");
    assert_eq!(params.command, None);
    assert!(params.args.is_empty());
    assert!(params.env.is_empty());
    assert_eq!(params.rows, 0);
    assert_eq!(params.cols, 0);
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn full_pty_can_close_and_old_capture_cannot_restart_it() {
    let manager = pty::manager();
    let _provider_handle = manager.provider_handle();
    let bus = crate::sync_primitives::Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
    let generation = manager.activate(bus).expect("real manager activation");
    manager.set_max_sessions(1);
    let first = manager
        .spawn_at(
            generation,
            &SpawnOptions {
                command: Some(if cfg!(windows) { "cmd.exe" } else { "cat" }.into()),
                ..Default::default()
            },
        )
        .expect("real PTY spawn");

    // Close is a release operation and must still work at the allocation ceiling.
    assert!(manager.close_at(generation, &first.session_id).is_ok());
    assert!(manager
        .write_at(generation, &first.session_id, b"stale")
        .is_err());

    manager.deactivate().await.expect("real manager stop");
    let new_generation = manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("real manager re-enable");
    assert_ne!(generation, new_generation);
    assert!(manager
        .write_at(generation, &first.session_id, b"old")
        .is_err());
    manager.deactivate().await.expect("test manager cleanup");
}

#[test]
fn canonical_tool_wire_includes_spawn_and_list_without_streams() {
    let values = [
        serde_json::to_value(PtyToolParams::Spawn {
            params: PtySpawnParams::default(),
        })
        .expect("spawn tool wire"),
        serde_json::to_value(PtyToolParams::List {}).expect("list tool wire"),
    ];
    assert_eq!(values[0]["operation"], "spawn");
    assert_eq!(values[1]["operation"], "list");
    assert!(values.iter().all(|value| {
        !value.to_string().contains("read_stdin")
            && !value.to_string().contains("write_stdout")
            && !value.to_string().contains("write_stderr")
    }));
}

#[test]
fn canonical_terminal_operation_is_not_a_three_stream_adapter() {
    let operation = PtyOperation::Attach;
    let encoded = serde_json::to_value(operation).expect("attach wire");
    assert_eq!(encoded, json!({"operation": "attach"}));
}

struct AlwaysApprove;

#[async_trait::async_trait]
impl ApprovalRequester for AlwaysApprove {
    async fn request_approval(
        &self,
        _action: &crate::sandbox::exec_approval::action::ApprovalAction,
    ) -> ApprovalResponse {
        ApprovalOutcome::Approved.into()
    }
}

fn terminal_tool_domain(
    constraints: CapabilityConstraintsConfig,
) -> (
    Arc<ZahirFacade>,
    Arc<TerminalCapabilitySource>,
    ToolHandlerRegistry,
) {
    let (facade, source, tools, _) = terminal_tool_domain_with_specials(constraints);
    (facade, source, tools)
}

fn terminal_tool_domain_with_specials(
    constraints: CapabilityConstraintsConfig,
) -> (
    Arc<ZahirFacade>,
    Arc<TerminalCapabilitySource>,
    ToolHandlerRegistry,
    CapabilityHandlerRegistry,
) {
    let specials = CapabilityHandlerRegistry::new();
    let tools = ToolHandlerRegistry::new();
    let tree = Arc::new(OwnershipTree::new());
    let policy = Arc::new(
        PolicyEngine::with_shared_tree(tree.clone(), constraints).expect("valid test policy"),
    );
    let facade = ZahirFacade::new_with_specials_and_policy(
        tools.clone(),
        tree,
        specials.clone(),
        policy.clone(),
    );
    let config = Arc::new(RwLock::new(crate::Config::default()));
    config
        .try_write()
        .expect("test config is uncontended")
        .policies
        .terminal
        .enabled = true;
    let source = Arc::new(TerminalCapabilitySource::new(
        specials.clone(),
        policy,
        Arc::downgrade(&facade),
        config,
    ));
    facade
        .bind_terminal_source(Arc::downgrade(&source))
        .expect("bind test terminal source");
    source.bind_handler().expect("bind test handler");
    register_canonical_adapters_with_tools_and_source(&specials, Some(&tools), Some(&source))
        .expect("canonical terminal registrations");
    facade.reconcile_registry_bindings();
    (facade, source, tools, specials)
}

fn canonical_tool_service(
    facade: Arc<ZahirFacade>,
    registry: ToolHandlerRegistry,
) -> ScopedToolService {
    let registry = Arc::new(registry);
    let mut inner = LoopToolRegistry::new();
    inner.bind_canonical_registry(Arc::clone(&registry), Arc::new(|_| true));
    assert!(inner.bind_canonical_facade(facade));
    for entry in registry.entries_snapshot().values() {
        inner.register(Box::new(McpRegistryTool::from_registry_entry(
            Arc::clone(&entry.handler),
            &entry.descriptor,
        )));
    }
    ScopedToolService::new(Arc::new(inner), std::collections::BTreeSet::new())
        .with_confirmation(Arc::new(AlwaysApprove))
}

fn parse_tool_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => {
            serde_json::from_str(value).unwrap_or_else(|_| value.clone().into())
        }
        other => other.clone(),
    }
}

async fn execute_terminal_tool(
    service: &ScopedToolService,
    call_id: &str,
    input: serde_json::Value,
) -> Result<crate::tools::service::ToolOutput, ToolError> {
    with_call_identity(
        Some(CallIdentity {
            turn_id: crate::session::events::TurnId::nil(),
            call_id: call_id.to_owned(),
        }),
        service.execute_with_cancel(
            "terminal_capability",
            input,
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn registered_terminal_tool_executes_real_spawn_and_existing_list() {
    let (facade, source, tools) = terminal_tool_domain(BTreeMap::new());
    let manager = pty::manager();
    let generation = manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("activate real manager");
    source
        .bind_manager_handle(manager.provider_handle())
        .expect("bind real manager provider");

    let handler = tools
        .snapshot()
        .get("terminal_capability")
        .cloned()
        .expect("registered terminal Tool");
    let schema = handler.definition().input_schema.to_string();
    assert!(schema.contains("spawn"), "registered Tool schema: {schema}");
    assert!(schema.contains("list"), "registered Tool schema: {schema}");
    assert!(!schema.contains("read_stdin"));
    let service = canonical_tool_service(facade, tools);

    let spawned = execute_terminal_tool(
        &service,
        "fix3-real-tool-spawn",
        json!({"operation": "spawn", "params": {}}),
    )
    .await
    .expect("real registered Tool spawn");
    let spawned_value = parse_tool_value(&spawned.value);
    let session_id = spawned_value["session_id"]
        .as_str()
        .expect("spawn response session id")
        .to_owned();
    let listed = execute_terminal_tool(
        &service,
        "fix3-real-tool-list",
        json!({"operation": "list"}),
    )
    .await
    .expect("real registered Tool list");
    let listed_value = parse_tool_value(&listed.value);
    assert!(listed_value["sessions"]
        .as_array()
        .is_some_and(|sessions| sessions
            .iter()
            .any(|session| session["session_id"] == session_id)));
    manager
        .close_at(generation, &session_id)
        .expect("close real test session");
    manager.deactivate().await.expect("deactivate real manager");
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn denied_spawn_has_zero_effect_under_allocate_policy() {
    let mut constraints = CapabilityConstraintsConfig::new();
    constraints.insert(
        "aleph/tools/terminal_capability".into(),
        CapabilityConstraintRule {
            required_conditions: BTreeMap::from([(String::from("provider_ready"), false)]),
            ..Default::default()
        },
    );
    let (facade, source, tools) = terminal_tool_domain(constraints);
    let manager = pty::manager();
    manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("activate real manager");
    source
        .bind_manager_handle(manager.provider_handle())
        .expect("bind real manager provider");
    let service = canonical_tool_service(facade, tools);
    let denied = execute_terminal_tool(
        &service,
        "fix3-denied-spawn",
        json!({"operation": "spawn", "params": {}}),
    )
    .await;
    let count = manager.list().len();
    manager.deactivate().await.expect("deactivate real manager");
    assert!(
        matches!(denied, Err(ToolError::PermissionDenied { .. })),
        "configured real domain denial: {denied:?}"
    );
    assert_eq!(count, 0, "denied Spawn has zero manager effect");
}

struct StageOrdinaryReplacement {
    definition: crate::tools::service::ToolDefinition,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl crate::tools::handlers::ToolHandler for StageOrdinaryReplacement {
    async fn invoke(
        &self,
        _: serde_json::Value,
    ) -> Result<crate::tools::service::ToolOutput, ToolError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(crate::tools::service::ToolOutput {
            value: json!({"must_not_execute": true}),
            metadata: Default::default(),
        })
    }

    fn definition(&self) -> crate::tools::service::ToolDefinition {
        self.definition.clone()
    }
}

#[tokio::test]
async fn stage_a_plain_terminal_replacement_cannot_defer_runtime_policy() {
    let constraints = toml::from_str(
        "[\"aleph/tools/terminal_capability\".resource]\nunit = \"pty_sessions\"\nmax_active = 1\n",
    )
    .unwrap();
    let (facade, _source, tools) = terminal_tool_domain(constraints);
    let entry = tools.entries_snapshot()["terminal_capability"].clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tools
        .replace(
            entry.descriptor.as_ref().clone(),
            Arc::new(StageOrdinaryReplacement {
                definition: entry.handler.definition(),
                calls: calls.clone(),
            }),
        )
        .unwrap();
    let service = canonical_tool_service(facade, tools);
    let result = execute_terminal_tool(
        &service,
        "stage-ordinary-replacement",
        json!({"operation": "list"}),
    )
    .await;
    assert!(
        matches!(result, Err(ToolError::PermissionDenied { reason, .. }) if reason.contains("unsupported_resource_unit")),
        "ordinary replacement must use the generic runtime policy boundary"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "ordinary same-name replacement must not inherit domain deferral"
    );
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn stage_a_stable_terminal_deny_reaches_existing_session_ingress() {
    let constraints = BTreeMap::from([(
        "aleph/tools/terminal_capability".into(),
        CapabilityConstraintRule {
            required_conditions: BTreeMap::from([("provider_ready".into(), false)]),
            ..Default::default()
        },
    )]);
    let (facade, source, tools) = terminal_tool_domain(constraints);
    let manager = pty::manager();
    let generation = manager
        .activate(Arc::new(crate::gateway::event_bus::GatewayEventBus::new()))
        .unwrap();
    source
        .bind_manager_handle(manager.provider_handle())
        .unwrap();
    let id = manager
        .spawn_at(generation, &SpawnOptions::default())
        .unwrap()
        .session_id;
    source.register_session(&id, None).unwrap();
    let response = handle_attach_with_source(
        JsonRpcRequest::with_id(
            "pty.attach",
            Some(json!({"session_id":id})),
            json!("stage-a-denied-attach"),
        ),
        source.clone(),
    )
    .await;
    let legacy_resize = crate::gateway::handlers::pty::handle_resize_with_source(
        JsonRpcRequest::with_id(
            "pty.resize",
            Some(json!({"session_id":id,"rows":38,"cols":123})),
            json!("stage-a-denied-legacy-resize"),
        ),
        source,
    )
    .await;
    let typed_resize = facade
        .invoke_terminal_gateway(
            aleph_protocol::capability::PtyInvokeParams {
                session_id: id.clone(),
                operation: PtyOperation::Resize {
                    rows: 42,
                    cols: 132,
                },
            },
            "stage-a-denied-typed-resize".into(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await;
    let service = canonical_tool_service(facade, tools);
    let canonical_resize = execute_terminal_tool(
        &service,
        "stage-a-denied-canonical-resize",
        json!({"operation":"resize","session_id":id,"rows":44,"cols":144}),
    )
    .await;
    let screen = manager.attach_snapshot_at(generation, &id).unwrap();
    manager.close_at(generation, &id).unwrap();
    manager.deactivate().await.unwrap();
    assert!(
        response.result.is_none(),
        "stable ingress deny must prevent returning the actual screen: {response:?}"
    );
    assert!(response.error.is_some());
    assert!(legacy_resize.error.is_some());
    assert!(typed_resize.is_err());
    assert!(canonical_resize.is_err());
    assert_eq!(
        (screen.rows, screen.cols),
        (24, 80),
        "stable deny has zero real resize effect on every ingress"
    );
}

const PTY_RETIREMENT_COMPLETED: &str = "aleph-test:pty-fact-retirement:completed";

fn retired_capture_reclaimed(
    capture: &crate::capability::special::CapturedSpecialEntry,
) -> Result<(), usize> {
    let count = capture.terminal_policy_source_strong_count_for_test();
    if count == 1 {
        Ok(())
    } else {
        Err(count)
    }
}

fn retirement_child_completed(success: bool, stdout: &[u8]) -> Result<(), &'static str> {
    if !success {
        return Err("owning-source child failed");
    }
    if String::from_utf8_lossy(stdout)
        .lines()
        .any(|line| line == PTY_RETIREMENT_COMPLETED)
    {
        Ok(())
    } else {
        Err("NO_COVERAGE: intended retirement child did not complete")
    }
}

fn run_retirement_child(selector: &str) -> std::process::Output {
    use std::io::Write;
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", selector, "--nocapture", "--test-threads=1"])
        .env("ALEPH_PTY_FACT_RETIREMENT_CHILD", "1")
        .output()
        .expect("run the single process-isolated owning-source regression");
    std::io::stdout().write_all(&output.stdout).unwrap();
    std::io::stderr().write_all(&output.stderr).unwrap();
    output
}

#[test]
fn stage_a3_retirement_child_rejects_no_match() {
    let output = run_retirement_child(
        "gateway::capability_terminal_tests::stage_a3_intentionally_unmatched_selector_do_not_add",
    );
    assert!(
        output.status.success(),
        "the no-match control must reach libtest's successful empty result"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("running 0 tests"));
    assert!(
        retirement_child_completed(output.status.success(), &output.stdout).is_err(),
        "successful zero-match child must be rejected by the actual parent validator"
    );
    assert!(
        retirement_child_completed(true, b"running 1 test\ntest another_selected_test ... ok\n")
            .is_err(),
        "another successful test without the intended completion is not coverage"
    );
    assert!(
        retirement_child_completed(false, format!("{PTY_RETIREMENT_COMPLETED}\n").as_bytes())
            .is_err(),
        "a failed child cannot pass even if completion text was emitted"
    );
}

#[test]
#[serial_test::serial(pty_global_manager)]
fn a_stage_a2_closed_and_stopped_pty_fact_sources_are_reclaimed() {
    // The production manager source is a process-lifetime OnceLock. Exercise its
    // actual hooks without resetting it or occupying another test's source slot.
    // The child inherits the bounded process group; its full logs are replayed.
    if std::env::var_os("ALEPH_PTY_FACT_RETIREMENT_CHILD").is_none() {
        let output = run_retirement_child("gateway::capability_terminal_tests::a_stage_a2_closed_and_stopped_pty_fact_sources_are_reclaimed");
        retirement_child_completed(output.status.success(), &output.stdout)
            .expect("real owner lifecycle regression must complete in its isolated process");
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("own the retirement fixture runtime");
    let (retired_captures, policy, specials, stable, exact, baseline, retired_sessions) = runtime
        .block_on(async {
            use crate::capability::descriptor::CapabilityId;
            use crate::config::types::policies::capability_constraints::{
                ResourceLimitConfig, ResourceUnit,
            };

            let manager = pty::manager();
            let generation = manager
                .activate(Arc::new(crate::gateway::event_bus::GatewayEventBus::new()))
                .unwrap();
            let first = manager
                .spawn_at(generation, &SpawnOptions::default())
                .unwrap()
                .session_id;
            let stable = CapabilityId {
                namespace: "aleph/tools".into(),
                name: "terminal_capability".into(),
            };
            let exact = CapabilityId {
                namespace: "aleph/terminal".into(),
                name: format!("session-{first}"),
            };
            let mut retired_sessions = vec![exact.clone()];
            let rule = |unit, max_active| CapabilityConstraintRule {
                resource: Some(ResourceLimitConfig { unit, max_active }),
                ..Default::default()
            };
            let (facade, source, tools, specials) =
                terminal_tool_domain_with_specials(BTreeMap::from([
                    (
                        "aleph/tools/terminal_capability".into(),
                        rule(ResourceUnit::PtySessions, 2),
                    ),
                    (
                        format!("aleph/terminal/session-{first}"),
                        rule(ResourceUnit::ActiveCalls, 1),
                    ),
                ]));
            let policy = facade.policy();
            source
                .bind_manager_handle(manager.provider_handle())
                .unwrap();
            manager
                .bind_capability_source(Arc::downgrade(&source))
                .unwrap();
            // This first physical session predates the binding so its exact rule can be
            // configured. Subsequent Spawn/Close cycles use the real publication hook.
            source.register_session(&first, None).unwrap();
            let service = canonical_tool_service(facade.clone(), tools);
            let listed =
                execute_terminal_tool(&service, "r1-baseline", json!({"operation":"list"})).await;
            let baseline = policy.restriction_source_keys_for_test(&stable);
            let attached = execute_terminal_tool(
                &service,
                "r1-exact",
                json!({"operation":"attach","session_id":first}),
            )
            .await;
            let exact_installed = policy.restriction_source_keys_for_test(&exact).len();
            let mut retired_captures = vec![(
                "configured-exact".to_owned(),
                specials.capture(&exact).unwrap(),
            )];
            let closed = execute_terminal_tool(
                &service,
                "r1-exact-close",
                json!({"operation":"close","session_id":first}),
            )
            .await;
            let after_exact = (
                policy.restriction_source_keys_for_test(&stable),
                policy.restriction_source_keys_for_test(&exact),
            );

            let peer = execute_terminal_tool(
                &service,
                "r1-peer-spawn",
                json!({"operation":"spawn","params":{}}),
            )
            .await
            .unwrap();
            let peer_id = parse_tool_value(&peer.value)["session_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let peer_attached = execute_terminal_tool(
                &service,
                "r1-peer-attach",
                json!({"operation":"attach","session_id":peer_id}),
            )
            .await;
            let live_baseline = policy.restriction_source_keys_for_test(&stable);
            let mut cycles = Vec::new();
            for index in 0..3 {
                let spawned = execute_terminal_tool(
                    &service,
                    &format!("r1-spawn-{index}"),
                    json!({"operation":"spawn","params":{}}),
                )
                .await
                .unwrap();
                let id = parse_tool_value(&spawned.value)["session_id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let target = CapabilityId {
                    namespace: "aleph/terminal".into(),
                    name: format!("session-{id}"),
                };
                retired_captures
                    .push((format!("cycle-{index}"), specials.capture(&target).unwrap()));
                let closed = execute_terminal_tool(
                    &service,
                    &format!("r1-close-{index}"),
                    json!({"operation":"close","session_id":id}),
                )
                .await;
                // A repeated old-session callback must not clear the still-live peer.
                source.withdraw_session(&id);
                retired_sessions.push(CapabilityId {
                    namespace: "aleph/terminal".into(),
                    name: format!("session-{id}"),
                });
                cycles.push((
                    closed,
                    manager.list().len(),
                    policy.restriction_source_keys_for_test(&stable),
                ));
            }
            let later = execute_terminal_tool(
                &service,
                "r1-extra-spawn",
                json!({"operation":"spawn","params":{}}),
            )
            .await;
            let denied = execute_terminal_tool(
                &service,
                "r1-at-capacity",
                json!({"operation":"spawn","params":{}}),
            )
            .await;
            let full_occupancy = manager.list().len();
            if let Ok(output) = &later {
                let id = parse_tool_value(&output.value)["session_id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                let target = CapabilityId {
                    namespace: "aleph/terminal".into(),
                    name: format!("session-{id}"),
                };
                retired_captures.push((
                    "later-allocation".to_owned(),
                    specials.capture(&target).unwrap(),
                ));
                manager.close_at(generation, &id).unwrap();
                retired_sessions.push(target);
            }
            let peer_live = execute_terminal_tool(
                &service,
                "r1-peer-still-live",
                json!({"operation":"attach","session_id":peer_id}),
            )
            .await;
            let peer_key = CapabilityId {
                namespace: "aleph/terminal".into(),
                name: format!("session-{peer_id}"),
            };
            let retired_capture = specials.capture(&peer_key).unwrap();
            let retired_key = policy
                .restriction_source_keys_for_test(&stable)
                .into_iter()
                .find(|key| !baseline.contains(key))
                .unwrap();
            specials.unregister(&peer_key).unwrap();
            let replacement_config = Arc::new(RwLock::new(crate::Config::default()));
            replacement_config
                .try_write()
                .unwrap()
                .policies
                .terminal
                .enabled = true;
            let replacement = Arc::new(TerminalCapabilitySource::new(
                specials.clone(),
                policy.clone(),
                Arc::downgrade(&facade),
                replacement_config,
            ));
            replacement
                .bind_manager_handle(manager.provider_handle())
                .unwrap();
            replacement.register_session(&peer_id, None).unwrap();
            let replacement_live = execute_terminal_tool(
                &service,
                "r1-replacement-live",
                json!({"operation":"attach","session_id":peer_id}),
            )
            .await;
            let replacement_keys = policy.restriction_source_keys_for_test(&stable);
            let cannot_reinstall = retired_capture
                .with_terminal_policy_source(&policy, retired_key, || Ok(()))
                .is_err();
            source.withdraw_session(&peer_id);
            let after_old_callback = policy.restriction_source_keys_for_test(&stable);
            let replacement_still_live = execute_terminal_tool(
                &service,
                "r1-replacement-after-old-callback",
                json!({"operation":"attach","session_id":peer_id}),
            )
            .await;
            retired_captures.push((
                "peer-replacement".to_owned(),
                specials.capture(&peer_key).unwrap(),
            ));
            // The replacement, not the delayed original source, owns this registration.
            replacement.withdraw_session(&peer_id);
            retired_sessions.push(peer_key);
            retired_captures.push(("peer-original".to_owned(), retired_capture));
            drop(replacement);
            let stop_peer = execute_terminal_tool(
                &service,
                "r1-stop-spawn",
                json!({"operation":"spawn","params":{}}),
            )
            .await
            .unwrap();
            let stop_id = parse_tool_value(&stop_peer.value)["session_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let stop_attached = execute_terminal_tool(
                &service,
                "r1-stop-attach",
                json!({"operation":"attach","session_id":stop_id}),
            )
            .await;
            let before_stop = policy.restriction_source_keys_for_test(&stable).len();
            let stop_key = CapabilityId {
                namespace: "aleph/terminal".into(),
                name: format!("session-{stop_id}"),
            };
            retired_captures.push((
                "provider-stop".to_owned(),
                specials.capture(&stop_key).unwrap(),
            ));
            manager.deactivate().await.unwrap();
            let after_stop = policy.restriction_source_keys_for_test(&stable);
            let stopped_occupancy = manager.list().len();
            retired_sessions.push(stop_key);
            drop(service);
            drop(facade);
            drop(source);

            assert!(listed.is_ok() && attached.is_ok() && closed.is_ok());
            assert_eq!(
                baseline.len(),
                1,
                "one legitimate stable-provider attachment"
            );
            assert_eq!(
                exact_installed, 1,
                "the exact configured target really installed state"
            );
            assert_eq!(
                after_exact.0, baseline,
                "closed source must not remain on stable ingress"
            );
            assert!(
                after_exact.1.is_empty(),
                "closed exact attachment must be reclaimed"
            );
            assert!(peer_attached.is_ok());
            assert_eq!(live_baseline.len(), 2, "provider plus one real live peer");
            for (closed, occupancy, keys) in cycles {
                assert!(closed.is_ok());
                assert_eq!(occupancy, 1);
                assert_eq!(
                    keys, live_baseline,
                    "successful Spawn/Close must not accumulate retired state"
                );
            }
            assert!(
                later.is_ok(),
                "retirement must preserve the configured allocation rule for a later source"
            );
            assert!(
                denied.is_err(),
                "retirement must not erase the live source's configured quota"
            );
            assert_eq!(
                full_occupancy, 2,
                "denied allocation has zero physical session effect"
            );
            assert!(
                peer_live.is_ok(),
                "retiring other sessions must not revoke live peer facts"
            );
            assert!(replacement_live.is_ok() && replacement_still_live.is_ok());
            assert!(
                cannot_reinstall,
                "an already retired capture cannot recreate restriction state"
            );
            assert_eq!(
                after_old_callback, replacement_keys,
                "a late old source callback must not withdraw replacement facts"
            );
            assert!(stop_attached.is_ok());
            assert_eq!(
                before_stop, 2,
                "actual provider stop has a live session source to withdraw"
            );
            assert_eq!(stopped_occupancy, 0);
            assert_eq!(
                after_stop, baseline,
                "real provider stop withdraws the live session projection"
            );
            (
                retired_captures,
                policy,
                specials,
                stable,
                exact,
                baseline,
                retired_sessions,
            )
        });
    // Keep both independent owners alive: runtime teardown must not hide a
    // permanent Policy/registration cycle by dropping its owning container.
    drop(runtime);
    assert_eq!(
        policy.restriction_source_keys_for_test(&stable),
        baseline,
        "retired source facts stay absent after the runtime ends"
    );
    assert!(
        policy.restriction_source_keys_for_test(&exact).is_empty(),
        "retired exact facts stay absent with the Policy observer alive"
    );
    assert!(
        retired_sessions
            .iter()
            .all(|id| specials.capture(id).is_none()),
        "all retired session records stay absent with the registry observer alive"
    );
    // Whole-Source lifetime is not a waiter-thread fence. Each real captured
    // registration instead exposes the token shared by actual policy bindings.
    for (incarnation, capture) in &retired_captures {
        let sole_count = capture.terminal_policy_source_strong_count_for_test();
        assert_eq!(
            retired_capture_reclaimed(capture),
            Ok(()),
            "retired {incarnation} must have only its observer: {capture:?}"
        );
        let retained_copy = capture.clone();
        let retained_count = capture.terminal_policy_source_strong_count_for_test();
        assert!(
            matches!(retired_capture_reclaimed(capture), Err(count) if count > 1),
            "a retained real copy must be rejected for {incarnation}: {capture:?}"
        );
        drop(retained_copy);
        let released_count = capture.terminal_policy_source_strong_count_for_test();
        assert_eq!(
            retired_capture_reclaimed(capture),
            Ok(()),
            "dropping only the control copy must restore sole ownership for {incarnation}: {capture:?}"
        );
        println!(
            "aleph-test:retired-capture incarnation={incarnation} sole={sole_count} retained={retained_count} released={released_count} capture={capture:?}"
        );
    }
    // Emit only after every actual effect, cleanup and retirement assertion.
    println!("\n{PTY_RETIREMENT_COMPLETED}");
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn stage_a_canonical_pty_sessions_uses_real_occupancy_and_allows_close() {
    use crate::config::types::policies::capability_constraints::ResourceUnit;
    let constraints = BTreeMap::from([(
        "aleph/tools/terminal_capability".into(),
        CapabilityConstraintRule {
            resource: Some(
                crate::config::types::policies::capability_constraints::ResourceLimitConfig {
                    unit: ResourceUnit::PtySessions,
                    max_active: 1,
                },
            ),
            ..Default::default()
        },
    )]);
    let (facade, source, tools) = terminal_tool_domain(constraints);
    let manager = pty::manager();
    let generation = manager
        .activate(Arc::new(crate::gateway::event_bus::GatewayEventBus::new()))
        .unwrap();
    source
        .bind_manager_handle(manager.provider_handle())
        .unwrap();
    let service = canonical_tool_service(facade, tools);
    let first = execute_terminal_tool(
        &service,
        "stage-a-occupancy-first",
        json!({"operation":"spawn","params":{}}),
    )
    .await;
    let count = manager.list().len();
    let second = execute_terminal_tool(
        &service,
        "stage-a-occupancy-second",
        json!({"operation":"spawn","params":{}}),
    )
    .await;
    let final_count = manager.list().len();
    let closed = if let Ok(first) = &first {
        let id = parse_tool_value(&first.value)["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        // The process-global manager's production source slot is immutable.
        // Assemble this fixture's exact owner through the real source API,
        // without rebinding a source installed by an earlier module test.
        source.register_session(&id, None).unwrap();
        Some(
            execute_terminal_tool(
                &service,
                "stage-a-full-close",
                json!({"operation":"close","session_id":id}),
            )
            .await,
        )
    } else {
        None
    };
    manager.deactivate().await.unwrap();
    assert!(
        first.is_ok(),
        "healthy empty real manager must admit configured PtySessions: {first:?}"
    );
    assert_eq!(count, 1);
    assert!(second.is_err());
    assert_eq!(final_count, 1, "denied spawn must not create a session");
    assert!(closed.unwrap().is_ok(), "full capacity must still release");
    let _ = generation;
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn stage_a_resize_uses_real_connection_minimum_and_disconnect_growth() {
    let (facade, source, tools) = terminal_tool_domain(BTreeMap::new());
    let manager = pty::manager();
    let generation = manager
        .activate(Arc::new(crate::gateway::event_bus::GatewayEventBus::new()))
        .unwrap();
    source
        .bind_manager_handle(manager.provider_handle())
        .unwrap();
    let id = manager
        .spawn_at(generation, &SpawnOptions::default())
        .unwrap()
        .session_id;
    source.register_session(&id, None).unwrap();
    let service = canonical_tool_service(facade, tools);
    let mut responses = Vec::new();
    for (connection, rows, cols) in [("stage-small", 24, 80), ("stage-large", 40, 120)] {
        let request = JsonRpcRequest::with_id(
            "pty.resize",
            Some(json!({"session_id":id,"rows":rows,"cols":cols})),
            json!(connection),
        );
        let response = crate::gateway::caller_identity::CALLER_CONN_ID
            .scope(
                Some(connection.into()),
                crate::gateway::handlers::pty::handle_resize_with_source(request, source.clone()),
            )
            .await;
        responses.push(response);
    }
    let minimum = manager.attach_snapshot(&id).unwrap();
    let model = execute_terminal_tool(
        &service,
        "stage-no-connection-resize",
        json!({"operation":"resize","session_id":id,"rows":50,"cols":140}),
    )
    .await;
    let after_model = manager.attach_snapshot(&id).unwrap();
    manager.release_conn("stage-small");
    let expanded = manager.attach_snapshot(&id).unwrap();
    manager.close_at(generation, &id).unwrap();
    manager.deactivate().await.unwrap();
    for response in responses {
        assert!(
            response.error.is_none(),
            "actual resize failed: {response:?}"
        );
    }
    assert_eq!(
        (minimum.rows, minimum.cols),
        (24, 80),
        "real PTY must use both connection viewports"
    );
    assert!(
        model.is_ok(),
        "no-connection model call remains usable: {model:?}"
    );
    assert_eq!(
        (after_model.rows, after_model.cols),
        (24, 80),
        "model resize must not override attached clients"
    );
    assert_eq!(
        (expanded.rows, expanded.cols),
        (40, 120),
        "disconnect must resize actual PTY to remaining viewport"
    );
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn gateway_facade_rejects_captured_session_after_provider_restart() {
    let (_facade, source, _tools) = terminal_tool_domain(BTreeMap::new());
    let manager = pty::manager();
    let first_generation = manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("activate real manager");
    source
        .bind_manager_handle(manager.provider_handle())
        .expect("bind real manager provider");
    let session_id = manager
        .spawn_at(first_generation, &SpawnOptions::default())
        .expect("spawn real session")
        .session_id;
    let captured =
        crate::gateway::handlers::pty::require_owned_session_with_source(&session_id, &source)
            .expect("capture owned session");
    manager.deactivate().await.expect("stop first provider");
    manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("start replacement provider");

    let result = source
        .invoke_legacy_gateway(
            PtyToolParams::Attach { session_id },
            "fix4-stale-capture".into(),
            tokio_util::sync::CancellationToken::new(),
            Some(captured),
        )
        .await;
    assert!(matches!(
        result,
        Err(
            crate::capability::special_dispatch::CapabilityInvocationError::Resolve(
                crate::capability::facade::ResolveError::StaleProvider
            )
        )
    ));
    manager
        .deactivate()
        .await
        .expect("cleanup replacement provider");
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn addressed_handler_rejects_stale_generation_before_effect() {
    let (_facade, source, _tools) = terminal_tool_domain(BTreeMap::new());
    let manager = pty::manager();
    let first_generation = manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("activate first provider");
    manager
        .bind_capability_source(Arc::downgrade(&source))
        .expect("bind terminal source");
    source
        .bind_manager_handle(manager.provider_handle())
        .expect("bind first provider");
    let old_session = manager
        .spawn_at(first_generation, &SpawnOptions::default())
        .expect("spawn old session")
        .session_id;

    let current = handle_attach_with_source(
        JsonRpcRequest::with_id(
            "pty.attach",
            Some(json!({"session_id": old_session})),
            json!("fix5-current-handler"),
        ),
        Arc::clone(&source),
    )
    .await;
    assert!(
        current.error.is_none(),
        "current handler response: {current:?}"
    );
    assert!(current.result.is_some(), "current handler should attach");

    let (_hook_guard, captured, release, stale_observed) = install_handler_handoff_hook();
    let request = JsonRpcRequest::with_id(
        "pty.attach",
        Some(json!({"session_id": old_session})),
        json!("fix5-stale-handler"),
    );
    let handler = tokio::spawn(handle_attach_with_source(request, Arc::clone(&source)));
    tokio::time::timeout(std::time::Duration::from_secs(5), captured.notified())
        .await
        .expect("handler captured ingress before effect");

    manager.deactivate().await.expect("stop first provider");
    let _next_generation = manager
        .activate(crate::sync_primitives::Arc::new(
            crate::gateway::event_bus::GatewayEventBus::new(),
        ))
        .expect("activate replacement provider");
    let sessions_before_stale = manager.list().len();
    release.notify_one();
    let stale = handler.await.expect("stale handler task");
    tokio::time::timeout(std::time::Duration::from_secs(5), stale_observed.notified())
        .await
        .expect("real handler/facade observed typed stale-provider rejection");
    assert!(
        stale.result.is_none(),
        "stale handler must not return a screen"
    );
    assert!(
        stale
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("unavailable")),
        "stale handler response: {stale:?}"
    );
    assert_eq!(
        manager.list().len(),
        sessions_before_stale,
        "stale handler changed manager session state"
    );
    assert!(manager.list().is_empty(), "stale handler has no PTY effect");
    drop(_hook_guard);

    manager
        .deactivate()
        .await
        .expect("cleanup replacement provider");
}
