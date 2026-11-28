//! Exercise the real Panel handler and queue before publishing a Shell input schema.
#![cfg(target_os = "linux")]

#[path = "gateway_chat_common/mod.rs"]
mod common;
#[path = "../src/bin/aleph-server/server_init.rs"]
mod server_init;

use std::sync::Arc;
use std::time::Duration;

use alephcore::approval::{with_call_identity, CallIdentity, OperatorApprovalRequester};
use alephcore::builtin_tools::process_registry::{
    process_registry, KillOutcome, LiveProcessState, PollOutcome, WaitOutcome,
};
use alephcore::capability::ownership::OwnershipTree;
use alephcore::capability::projection_host::{set_projection_host, ProjectionHost};
use alephcore::exec::{socket::ApprovalDecisionType, ExecApprovalManager};
use alephcore::extension::PermissionAction;
use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry};
use alephcore::gateway::busy_queue::PanelShellOwners;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::events::GatewayEventFrame;
use alephcore::gateway::execution_engine::{
    set_config_approval_requester, set_mcp_tool_registry, ExecutionEngine, ExecutionEngineConfig,
};
use alephcore::gateway::handlers::agent::AgentRunManager;
use alephcore::gateway::handlers::auth::TransportPolicy;
use alephcore::gateway::handlers::connect::{handle_connect, ConnectContext};
use alephcore::gateway::hot_reload::{ConfigWatcher, ConfigWatcherConfig};
use alephcore::gateway::router::{AgentRouter, SessionKey};
use alephcore::gateway::security::store::{UserRole, UserStatus};
use alephcore::gateway::security::{DeviceTokenManager, SecurityStore};
use alephcore::gateway::server::GatewayConfig;
use alephcore::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use alephcore::gateway::session_store::types::SessionPatch;
use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::state_version::StateVersionTracker;
use alephcore::gateway::GatewayServer;
use alephcore::orchestrator::{FlowOutcome, FlowStreamEvent};
use alephcore::sandbox::exec_approval::gate::ApprovalGate;
use alephcore::sandbox::platforms::linux::bwrap::{BubblewrapDriver, LinuxSandboxOptions};
use alephcore::thinker::SingleProviderRegistry;
use alephcore::tools::descriptor::ToolCapabilityDescriptor;
use alephcore::tools::handlers::{builtin::BuiltinHandler, ToolHandler};
use alephcore::tools::registry::ToolHandlerRegistry;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Barrier, Mutex, RwLock};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

async fn rpc(
    socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    socket
        .send(Message::Text(
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    while let Some(message) = socket.next().await {
        if let Message::Text(text) = message.unwrap() {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["id"] == id {
                return value;
            }
        }
    }
    panic!("connection ended before {method} replied");
}

#[derive(Clone, Copy, Debug)]
enum Revocation {
    Demote,
    Device,
    Disconnect,
    Permission,
    PolicyReload,
    Agent,
    AgentReplace,
    SessionTier,
    Archive,
    Workspace,
    Capability,
    Stop,
    ChildExit,
    Busy,
    Cancelled,
}

async fn resolve_next_approval(
    events: &mut tokio::sync::broadcast::Receiver<GatewayEventFrame>,
    approvals: &ExecApprovalManager,
    decision: ApprovalDecisionType,
) {
    loop {
        if let GatewayEventFrame::ApprovalRequested { approval_id, .. } =
            events.recv().await.unwrap()
        {
            assert!(approvals.resolve(&approval_id, decision, None));
            return;
        }
    }
}

fn terminate_fixture_job_group() {
    let membership = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    let supervisor = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::/"))
        .expect("unified cgroup membership");
    let supervisor = std::path::Path::new(supervisor);
    assert!(supervisor.ends_with("supervisor"));
    let root = std::path::Path::new("/sys/fs/cgroup").join(supervisor.parent().unwrap());
    let mut jobs = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("aleph-jsonl-")
        });
    let job = jobs.next().expect("the fixture owns one fixed JSONL group");
    assert!(jobs.next().is_none());
    std::fs::write(job.join("cgroup.kill"), "1").unwrap();
}

#[test]
#[ignore = "requires a delegated Linux supervisor subgroup"]
fn paired_panel_queue_reaches_fixed_jq_after_once_approval() {
    // The daemon also sizes debug worker stacks for ordinary scoped dispatch.
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(8 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_paired_shell_fixture());
}

async fn run_paired_shell_fixture() {
    // The server's Capability and approval slots install once per process.
    // Run each irreversible mutation in its own invocation of this test.
    let mutation =
        std::env::var("ALEPH_TEST_SHELL_MUTATION")
            .ok()
            .map(|name| match name.as_str() {
                "demote" => Revocation::Demote,
                "device" => Revocation::Device,
                "disconnect" => Revocation::Disconnect,
                "permission" => Revocation::Permission,
                "policy-reload" => Revocation::PolicyReload,
                "agent" => Revocation::Agent,
                "agent-replace" => Revocation::AgentReplace,
                "session-tier" => Revocation::SessionTier,
                "archive" => Revocation::Archive,
                "workspace" => Revocation::Workspace,
                "capability" => Revocation::Capability,
                "stop" => Revocation::Stop,
                "child-exit" => Revocation::ChildExit,
                "busy" => Revocation::Busy,
                "cancelled" => Revocation::Cancelled,
                _ => panic!("unknown Shell mutation: {name}"),
            });
    let revoke_eof = std::env::var_os("ALEPH_TEST_SHELL_REVOKE_EOF").is_some();
    tokio::time::timeout(
        Duration::from_secs(30),
        exercise_paired_shell(mutation, revoke_eof),
    )
    .await
    .unwrap_or_else(|_| panic!("real Panel queue and consumer did not finish: {mutation:?}"));
}

async fn exercise_paired_shell(mutation: Option<Revocation>, revoke_eof: bool) {
    let root = tempfile::tempdir().unwrap();
    alephcore::Config::set_effective_path(root.path().join("config.toml")).unwrap();
    let workspace = root.path().join("workspaces/main");
    std::fs::create_dir_all(&workspace).unwrap();
    let config_path = root.path().join("aleph.toml");
    std::fs::write(
        &config_path,
        "[[agents.list]]\nid = 'main'\n[[agents.list]]\nid = 'backup'\ndefault = true\n",
    )
    .unwrap();
    std::fs::File::create(config_path.with_extension("toml.lock")).unwrap();
    let manager = Arc::new(alephcore::AgentManager::new(
        config_path,
        root.path().join("workspaces"),
        root.path().join("agents"),
        root.path().join("trash"),
    ));
    let store: Arc<dyn SessionStore> = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: root.path().join("sessions"),
            ..Default::default()
        })
        .unwrap(),
    );
    let agent_config = AgentInstanceConfig {
        workspace: workspace.clone(),
        agent_dir: root.path().join("agents/main"),
        ..Default::default()
    };
    let agents = Arc::new(AgentRegistry::new());
    agents
        .register(AgentInstance::new(agent_config.clone(), store.clone()).unwrap())
        .await;

    let mut config = alephcore::Config::default();
    config.security = serde_json::from_value(json!({
        "enable_custom_patterns": true,
        "custom_blocked": [{"pattern": "(?s)PANEL_TEST_PREFIX.*PANEL_TEST_SUFFIX"}],
    }))
    .unwrap();
    config.sandbox.enabled = true;
    config.sandbox.workspace_root = root.path().join("sandbox");
    config
        .policies
        .tool_permissions
        .overrides
        .insert("bash".into(), PermissionAction::Allow);
    let driver = BubblewrapDriver::with_options(LinuxSandboxOptions {
        cpu_quota_percent: Some(50),
        max_pids: Some(16),
        ..Default::default()
    })
    .with_test_sandbox_init(env!("CARGO_BIN_EXE_aleph-server").into());
    let sandbox = alephcore::sandbox::build_sandbox(
        &config.sandbox,
        Arc::new(driver),
        Arc::new(ApprovalGate::new(None)),
        Default::default(),
        &config.security,
    );
    let config = Arc::new(RwLock::new(config));
    let owners = PanelShellOwners {
        config: config.clone(),
        agents: agents.clone(),
        agent_manager: manager.clone(),
        sessions: store.clone(),
        sandbox,
    };
    let registry = Arc::new(ToolHandlerRegistry::new());
    let handler = Arc::new(BuiltinHandler::new(
        "bash".into(),
        Arc::new(alephcore::builtin_tools::BashExecTool::new()),
    ));
    registry
        .register(
            ToolCapabilityDescriptor::from_definition(&handler.definition(), 0),
            handler,
        )
        .unwrap();
    let host = ProjectionHost::mount(registry.as_ref().clone(), Arc::new(OwnershipTree::new()));
    assert!(host.wait_until_ready().await);
    set_mcp_tool_registry(registry.clone());
    set_projection_host(host.clone());

    let bus = Arc::new(GatewayEventBus::new());
    let mut events = bus.subscribe_typed();
    let approvals = Arc::new(ExecApprovalManager::new());
    set_config_approval_requester(Arc::new(OperatorApprovalRequester::for_config_tier(
        approvals.clone(),
        bus.clone(),
    )));
    let security = Arc::new(SecurityStore::in_memory().unwrap());
    let starting_role = match mutation {
        Some(Revocation::Demote) => UserRole::Admin,
        _ => UserRole::Member,
    };
    security
        .create_user("alice", "Alice", starting_role)
        .unwrap();
    let tokens = Arc::new(DeviceTokenManager::new(security.clone()));
    let ticket = tokens.create_bootstrap_ticket(None, Some("alice")).unwrap();
    let token = tokens
        .exchange_bootstrap_ticket(&ticket, Some("shell-panel".into()), None, None)
        .unwrap()
        .device_token;

    let (completed, mut results) = mpsc::channel(1);
    let (session_key, session_key_rx) = oneshot::channel::<String>();
    let session_key_rx = Arc::new(Mutex::new(Some(session_key_rx)));
    let approval_and_revocation = Arc::new(Barrier::new(2));
    let denied = Arc::new(Barrier::new(2));
    let reactivated = Arc::new(Barrier::new(2));
    let runner = common::StubHarnessRunner::new(Arc::new({
        let approval_and_revocation = approval_and_revocation.clone();
        let denied = denied.clone();
        let reactivated = reactivated.clone();
        move |ctx| {
            let completed = completed.clone();
            let session_key_rx = session_key_rx.clone();
            let approval_and_revocation = approval_and_revocation.clone();
            let denied = denied.clone();
            let reactivated = reactivated.clone();
            Box::pin(async move {
                let service = ctx.tool_service_override.as_ref().unwrap();
                let scoped = service.paired_shell_test_service().unwrap();
                let id = if mutation.is_none() {
                    let schema = service.metadata_schema();
                    let bash = schema
                        .iter()
                        .find(|definition| definition.name == "bash")
                        .unwrap();
                    assert_eq!(
                        bash.parameters["properties"]["stdin_profile"]["enum"][0],
                        "jsonl_inspect"
                    );
                    let malformed = service.execute_with_cancel(
                        "bash",
                        json!({"stdin_profile":"jsonl_inspect", "background":true, "cmd":"PRIVATE_INVALID_CANARY"}),
                        ctx.cancel.clone(),
                    ).await.unwrap_err();
                    assert!(!malformed.to_string().contains("PRIVATE_INVALID_CANARY"));
                    let launched = service
                        .execute_with_cancel(
                            "bash",
                            json!({"stdin_profile":"jsonl_inspect", "yield_ms":10}),
                            ctx.cancel.clone(),
                        )
                        .await
                        .unwrap();
                    launched.value["process_id"].as_u64().unwrap()
                } else {
                    scoped.test_launch_fixed_shell(&ctx.cancel).await.unwrap()
                };
                let processes = process_registry();
                let receipt = session_key_rx.lock().await.take().expect("one Panel run");
                let key = receipt.await.expect("chat.send receipt");
                let session = SessionKey::from_key_string(&key).expect("Panel session key");
                let caller = serde_json::to_string(&session).unwrap();
                loop {
                    match processes.poll(id, Some(&caller)) {
                        PollOutcome::Running {
                            state: LiveProcessState::Running,
                            ..
                        } => break,
                        PollOutcome::Running { .. } => {
                            tokio::time::sleep(Duration::from_millis(5)).await
                        }
                        PollOutcome::Done(output) => panic!("fixed jq failed to start: {output:?}"),
                        _ => panic!("fixed jq did not start"),
                    }
                }
                if mutation.is_none() {
                    let mut write = Box::pin(scoped.test_fixed_shell_input(
                        id,
                        &ctx.cancel,
                        Some(json!({"must_not_deliver": true})),
                    ));
                    assert!(futures::poll!(&mut write).is_pending());
                    approval_and_revocation.wait().await;
                    assert_eq!(write.await, Err("Stale".into()));
                    denied.wait().await;
                    reactivated.wait().await;
                }
                if mutation.is_none() {
                    let refused = service.execute_shell_jsonl(
                        "bash",
                        json!({"process_action":"write_stdin", "process_id":id, "record":"OPERATOR_DENIED_CANARY"}),
                        ctx.cancel.clone(),
                    ).await.unwrap_err();
                    assert!(refused.to_string().contains("ApprovalRequired"));
                    assert!(!refused.to_string().contains("OPERATOR_DENIED_CANARY"));
                    let (write, effective) = service.execute_with_cancel_effective(
                        "bash",
                        json!({"process_action":"write_stdin", "process_id":id, "record":["PANEL_TEST_PREFIX", 2]}),
                        ctx.cancel.clone(),
                    ).await;
                    assert!(effective.is_none());
                    assert_eq!(write.unwrap().value["status"], "written");
                } else {
                    scoped
                        .test_fixed_shell_input(
                            id,
                            &ctx.cancel,
                            Some(json!(["PANEL_TEST_PREFIX", 2])),
                        )
                        .await
                        .unwrap();
                }
                assert_eq!(
                    scoped
                        .test_fixed_shell_input(id, &ctx.cancel, Some(json!("PANEL_TEST_SUFFIX")))
                        .await,
                    Err("PolicyBlocked".into()),
                );
                let mut expected = b"{\"kind\":\"array\",\"count\":2}\n".to_vec();
                loop {
                    match processes.poll(id, Some(&caller)) {
                        PollOutcome::Running {
                            partial: Some(tail),
                            ..
                        } if tail.stdout.ends_with(b"\n") => {
                            assert_eq!(tail.stdout, expected);
                            break;
                        }
                        PollOutcome::Running { .. } => {
                            tokio::time::sleep(Duration::from_millis(5)).await
                        }
                        _ => panic!("fixed jq stopped before EOF"),
                    }
                }
                if let Some(mutation) = mutation {
                    let attempt = ctx.cancel.child_token();
                    let record = (!revoke_eof).then(|| json!({"must_not_deliver": true}));
                    let mut pending = Box::pin(scoped.test_fixed_shell_input(id, &attempt, record));
                    let initial = futures::poll!(&mut pending);
                    assert!(
                        initial.is_pending(),
                        "{mutation:?} preapproval: {initial:?}"
                    );
                    approval_and_revocation.wait().await;
                    match mutation {
                        Revocation::Cancelled => attempt.cancel(),
                        Revocation::Stop => {
                            assert!(matches!(
                                processes.kill(id, Some(&caller)),
                                KillOutcome::Stopping
                            ));
                        }
                        Revocation::ChildExit => {
                            terminate_fixture_job_group();
                            let WaitOutcome::Done(output) = processes
                                .wait(id, Some(&caller), Duration::from_secs(5))
                                .await
                            else {
                                panic!("fixed jq did not settle after its native exit");
                            };
                            assert_ne!(output.exit_code, 0);
                            assert_eq!(output.stdout.as_bytes(), expected);
                        }
                        _ => {}
                    }
                    let expected_denial = match mutation {
                        Revocation::PolicyReload => "UnverifiedPolicy",
                        Revocation::Busy => "Busy",
                        Revocation::Cancelled => "Expired",
                        _ => "Stale",
                    };
                    assert_eq!(
                        pending.await,
                        Err(expected_denial.into()),
                        "{mutation:?}, EOF={revoke_eof}"
                    );
                    if matches!(mutation, Revocation::Busy) {
                        denied.wait().await;
                    }
                    if !matches!(mutation, Revocation::ChildExit) {
                        if !matches!(mutation, Revocation::Stop) {
                            match processes.poll(id, Some(&caller)) {
                                PollOutcome::Running {
                                    state: LiveProcessState::Running,
                                    partial: Some(tail),
                                    ..
                                } => assert_eq!(tail.stdout, expected),
                                _ => panic!("{mutation:?} changed the live jq input"),
                            }
                            assert!(matches!(
                                processes.kill(id, Some(&caller)),
                                KillOutcome::Stopping
                            ));
                        }
                        assert!(matches!(
                            processes
                                .wait(id, Some(&caller), Duration::from_secs(5))
                                .await,
                            WaitOutcome::Killed
                        ));
                    }
                } else {
                    let read = |cursor: Option<&str>| {
                        let mut args = json!({"process_action":"read", "process_id":id});
                        if let Some(cursor) = cursor {
                            args["cursor"] = json!(cursor);
                        }
                        let cancel = ctx.cancel.clone();
                        let service = service.clone();
                        async move {
                            let result = with_call_identity(
                                Some(CallIdentity {
                                    turn_id: uuid::Uuid::new_v4(),
                                    call_id: "fixture-cursor-read".into(),
                                }),
                                service.execute_with_cancel("bash", args, cancel),
                            )
                            .await
                            .unwrap();
                            let value = result.value;
                            let envelope: serde_json::Value =
                                serde_json::from_str(value.as_str().unwrap()).unwrap();
                            assert_eq!(envelope["success"], true, "{envelope}");
                            serde_json::from_str::<serde_json::Value>(
                                envelope["stdout"].as_str().unwrap(),
                            )
                            .unwrap()
                        }
                    };
                    let first = read(None).await;
                    assert_eq!(
                        first["stdout"]["text"].as_str().unwrap().as_bytes(),
                        expected
                    );
                    let cursor = first["next_cursor"].as_str().unwrap();
                    let next = service
                        .execute_with_cancel(
                            "bash",
                            json!({"process_action":"write_stdin", "process_id":id, "record":null}),
                            ctx.cancel.clone(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(next.value["status"], "written");
                    let next_line = b"{\"kind\":\"null\",\"count\":null}\n";
                    loop {
                        let increment = read(Some(cursor)).await;
                        let text = increment["stdout"]["text"].as_str().unwrap();
                        if !text.is_empty() {
                            assert_eq!(text.as_bytes(), next_line);
                            let current = increment["next_cursor"].as_str().unwrap();
                            let unchanged = read(Some(current)).await;
                            assert_eq!(unchanged["stdout"]["text"], "");
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    expected.extend_from_slice(next_line);
                    let mut close = Box::pin(scoped.test_fixed_shell_input(id, &ctx.cancel, None));
                    assert!(futures::poll!(&mut close).is_pending());
                    approval_and_revocation.wait().await;
                    assert_eq!(close.await, Err("Stale".into()));
                    match processes.poll(id, Some(&caller)) {
                        PollOutcome::Running {
                            state: LiveProcessState::Running,
                            partial: Some(tail),
                            ..
                        } => assert_eq!(tail.stdout, expected),
                        _ => panic!("revoked EOF closed the live consumer"),
                    }
                    denied.wait().await;
                    reactivated.wait().await;
                    let close = || {
                        service.execute_with_cancel(
                            "bash",
                            json!({"process_action":"close_stdin", "process_id":id}),
                            ctx.cancel.clone(),
                        )
                    };
                    assert_eq!(close().await.unwrap().value["status"], "closed");
                    let WaitOutcome::Done(output) = processes
                        .wait(id, Some(&caller), Duration::from_secs(5))
                        .await
                    else {
                        panic!("fixed jq did not settle after EOF");
                    };
                    assert_eq!(close().await.unwrap().value["status"], "already_closed");
                    assert_eq!(output.exit_code, 0);
                    assert_eq!(output.stdout.as_bytes(), expected);
                    assert!(output.stderr.is_empty());
                }
                let outcome = FlowOutcome {
                    final_text: "finished".into(),
                    ..Default::default()
                };
                let _ = ctx.events.send(FlowStreamEvent::Complete(outcome.clone()));
                completed.send(ctx.run_id).await.unwrap();
                Ok(outcome)
            })
        }
    }));
    let providers = Arc::new(SingleProviderRegistry::new(Arc::new(common::NeverProvider)));
    let engine = Arc::new(
        ExecutionEngine::new(
            ExecutionEngineConfig {
                default_timeout_secs: 20,
                ..Default::default()
            },
            providers.clone(),
            Arc::new(
                alephcore::executor::BuiltinToolRegistry::new()
                    .await
                    .unwrap(),
            ),
            Vec::new(),
        )
        .with_app_config(config.clone())
        .with_session_topic_support(store.clone(), bus.clone()),
    );
    engine
        .orchestrator_cell()
        .set(common::orchestrator_with_stub(runner))
        .unwrap_or_else(|_| panic!("install fixture orchestrator"));
    let mut router = AgentRouter::new();
    router.set_session_store(store.clone());
    let router = Arc::new(router);
    let runs = Arc::new(AgentRunManager::new(
        router.clone(),
        bus.clone(),
        agents.clone(),
        engine.clone(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut gateway = GatewayServer::with_config(
        addr,
        GatewayConfig {
            require_idempotency_key: true,
            ..Default::default()
        },
    );
    gateway.set_device_token_manager(tokens.clone());
    gateway.set_security_store(security.clone());
    gateway.set_session_store(store.clone());
    let connect = Arc::new(ConnectContext {
        state_versions: Arc::new(StateVersionTracker::new()),
        transport_policy: TransportPolicy::defaults(),
    });
    gateway.handlers_mut().register("connect", move |request| {
        handle_connect(request, connect.clone())
    });
    let mutation_config = config.clone();
    let mutation_agents = agents.clone();
    let mutation_bus = bus.clone();
    let mutation_store = store.clone();
    gateway
        .handlers_mut()
        .register("chat.send", move |request| {
            server_init::handle_chat_send_with_engine(
                request,
                Some(owners.clone()),
                engine.clone(),
                bus.clone(),
                router.clone(),
                agents.clone(),
                config.clone(),
                None,
                providers.clone(),
                store.clone(),
                None,
                runs.clone(),
            )
        });
    let connections = gateway.connections.clone();
    let gateway = gateway.build_router();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            gateway.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let connected = rpc(&mut socket, 1, "connect", json!({"device_token": token})).await;
    assert_eq!(connected["result"]["authorized"], true, "{connected}");
    let sent = rpc(&mut socket, 2, "chat.send", json!({"input":"inspect a record", "agent_id":"main", "idempotency_key":"paired-native-shell"})).await;
    let run = sent["result"]["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("chat.send failed: {sent}"));
    let key = sent["result"]["session_key"].as_str().unwrap();
    let session = SessionKey::from_key_string(key).unwrap();
    session_key.send(key.to_owned()).unwrap();

    if let Some(mutation) = mutation {
        // Deliver one positive record, then approve the pending operation
        // while its future is parked at the runner's barrier.
        resolve_next_approval(&mut events, &approvals, ApprovalDecisionType::AllowOnce).await;
        resolve_next_approval(&mut events, &approvals, ApprovalDecisionType::AllowOnce).await;
        let busy_guard = if matches!(mutation, Revocation::Busy) {
            Some(mutation_config.write().await)
        } else {
            None
        };
        match mutation {
            Revocation::Demote => {
                security
                    .update_user("alice", None, Some(UserRole::Member), None)
                    .unwrap();
            }
            Revocation::Device => {
                assert!(tokens.revoke_panel_device("shell-panel").unwrap());
            }
            Revocation::Disconnect => {
                socket.close(None).await.unwrap();
                loop {
                    if connections.read().await.is_empty() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }
            Revocation::Permission => {
                let response =
                    alephcore::gateway::handlers::config::handle_update_tool_permissions(
                        alephcore::gateway::JsonRpcRequest::with_id(
                            "config.update_tool_permissions",
                            Some(json!({"overrides": {"bash": "deny"}})),
                            json!(3),
                        ),
                        mutation_config.clone(),
                        mutation_bus,
                    )
                    .await;
                assert!(response.error.is_none(), "{response:?}");
            }
            Revocation::PolicyReload => {
                let mut updated = mutation_config.read().await.clone();
                updated.security.enable_custom_patterns = false;
                updated.save().unwrap();
                let gateway_path = root.path().join("gateway.toml");
                std::fs::write(&gateway_path, "[agents.main]\n").unwrap();
                let watcher = Arc::new(
                    ConfigWatcher::new(ConfigWatcherConfig {
                        config_path: gateway_path,
                        ..Default::default()
                    })
                    .unwrap(),
                );
                let response = alephcore::gateway::handlers::config::handle_reload_with_subsystems(
                    alephcore::gateway::JsonRpcRequest::with_id("config.reload", None, json!(3)),
                    watcher,
                    mutation_config.clone(),
                    mutation_bus,
                )
                .await;
                assert!(response.error.is_none(), "{response:?}");
                assert!(!mutation_config.read().await.security.enable_custom_patterns);
            }
            Revocation::Agent => {
                assert!(
                    mutation_agents
                        .set_allowed_users("main", Some(vec!["another-user".into()]))
                        .await
                );
            }
            Revocation::AgentReplace => {
                mutation_agents
                    .register(AgentInstance::new(agent_config, mutation_store.clone()).unwrap())
                    .await;
            }
            Revocation::SessionTier => {
                assert!(mutation_store
                    .patch_session(
                        &session,
                        &SessionPatch {
                            metadata: Some(json!({"exec_tier": "plan"})),
                            ..Default::default()
                        }
                    )
                    .await
                    .unwrap());
            }
            Revocation::Archive => {
                assert!(
                    mutation_store
                        .delete_session(&session)
                        .await
                        .unwrap()
                        .deleted
                );
            }
            Revocation::Workspace => {
                manager.delete("main").unwrap();
                assert!(!workspace.exists());
                std::fs::create_dir(&workspace).unwrap();
            }
            Revocation::Capability => {
                let replacement = Arc::new(BuiltinHandler::new(
                    "bash".into(),
                    Arc::new(alephcore::builtin_tools::BashExecTool::new()),
                ));
                registry
                    .replace(
                        ToolCapabilityDescriptor::from_definition(&replacement.definition(), 0),
                        replacement,
                    )
                    .unwrap();
            }
            Revocation::Stop | Revocation::ChildExit | Revocation::Busy | Revocation::Cancelled => {
            }
        }
        approval_and_revocation.wait().await;
        if busy_guard.is_some() {
            denied.wait().await;
            drop(busy_guard);
        }
    } else {
        for operation in 0..6 {
            let revoke = matches!(operation, 0 | 4);
            let decision = if operation == 1 {
                ApprovalDecisionType::Deny
            } else {
                ApprovalDecisionType::AllowOnce
            };
            resolve_next_approval(&mut events, &approvals, decision).await;
            if revoke {
                security
                    .update_user("alice", None, None, Some(UserStatus::Deactivated))
                    .unwrap();
                approval_and_revocation.wait().await;
                denied.wait().await;
                security
                    .update_user("alice", None, None, Some(UserStatus::Active))
                    .unwrap();
                reactivated.wait().await;
            }
        }
    }
    assert_eq!(results.recv().await.as_deref(), Some(run));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    host.close_and_await().await;
}
