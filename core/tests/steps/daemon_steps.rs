//! Step definitions for daemon features

use crate::world::{AlephWorld, DaemonContext};
use alephcore::daemon::{
    worldmodel::PendingAction, ActionType, ActivityType, DaemonCli, DaemonCommand, DaemonConfig,
    DaemonEvent, DaemonEventBus, DaemonStatus, DispatcherMode, GovernorDecision, IpcServer, ProcessEventType, RawEvent, ResourceGovernor, ResourceLimits, RiskLevel,
    ServiceManager, ServiceStatus,
};
use alephcore::{
    ProactiveDispatcher, ProactiveDispatcherConfig, WorldModel, WorldModelConfig,
};
#[cfg(target_os = "macos")]
use alephcore::daemon::platforms::launchd::LaunchdService;
use chrono::Utc;
use clap::Parser;
use cucumber::{given, then, when};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

// ═══ Event Bus Steps ═══

#[given(expr = "an event bus with capacity {int}")]
async fn given_event_bus(w: &mut AlephWorld, capacity: i32) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.event_bus = Some(DaemonEventBus::new(capacity as usize));
}

#[given("a subscriber to the event bus")]
async fn given_subscriber(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let bus = ctx.event_bus.as_ref().expect("Event bus not initialized");
    ctx.receivers.push(bus.subscribe());
}

#[given(expr = "{int} subscribers to the event bus")]
async fn given_multiple_subscribers(w: &mut AlephWorld, count: i32) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let bus = ctx.event_bus.as_ref().expect("Event bus not initialized");
    for _ in 0..count {
        ctx.receivers.push(bus.subscribe());
    }
}

#[when("I send a heartbeat event")]
async fn when_send_heartbeat(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let bus = ctx.event_bus.as_ref().expect("Event bus not initialized");
    let event = DaemonEvent::Raw(RawEvent::Heartbeat {
        timestamp: Utc::now(),
    });
    bus.send(event).unwrap();
}

#[then("the subscriber should receive a heartbeat event")]
async fn then_receive_heartbeat(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let receiver = ctx.receivers.first_mut().expect("No subscribers");
    let event = receiver.recv().await.unwrap();
    assert!(matches!(
        event,
        DaemonEvent::Raw(RawEvent::Heartbeat { .. })
    ));
}

#[then("all subscribers should receive a heartbeat event")]
async fn then_all_receive_heartbeat(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    for receiver in ctx.receivers.iter_mut() {
        let event = receiver.recv().await.unwrap();
        assert!(matches!(
            event,
            DaemonEvent::Raw(RawEvent::Heartbeat { .. })
        ));
    }
}

// ═══ CLI Steps ═══

#[when(expr = "I parse CLI arguments {string}")]
async fn when_parse_cli(w: &mut AlephWorld, args_str: String) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    let args: Vec<&str> = args_str.split_whitespace().collect();
    ctx.cli_parse_result = Some(
        DaemonCli::try_parse_from(args)
            .map(|cli| {
                // Verify it's an Install command
                assert!(matches!(cli.command, DaemonCommand::Install));
            })
            .map_err(|e| e.to_string()),
    );
}

#[then("the CLI parsing should succeed")]
async fn then_cli_success(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let result = ctx
        .cli_parse_result
        .as_ref()
        .expect("No CLI parse attempted");
    assert!(result.is_ok(), "CLI parsing failed: {:?}", result);
}

#[then(expr = "the command should be Install")]
async fn then_command_install(_w: &mut AlephWorld) {
    // The parsing already validated this matches Install command in when_parse_cli
    // This step is for documentation/readability
}

// ═══ Service Manager Steps ═══

#[given("a mock service manager")]
async fn given_mock_service(w: &mut AlephWorld) {
    // Mock service manager is created inline for the test
    let _ = w.daemon.get_or_insert_with(DaemonContext::default);
}

#[when("I query the service status")]
async fn when_query_service_status(w: &mut AlephWorld) {
    // Create and test mock inline
    struct MockService;

    #[async_trait::async_trait]
    impl ServiceManager for MockService {
        async fn install(&self, _config: &DaemonConfig) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn uninstall(&self) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn start(&self) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn stop(&self) -> alephcore::daemon::Result<()> {
            Ok(())
        }
        async fn status(&self) -> alephcore::daemon::Result<DaemonStatus> {
            Ok(DaemonStatus::Unknown)
        }
        async fn service_status(&self) -> alephcore::daemon::Result<ServiceStatus> {
            Ok(ServiceStatus::NotInstalled)
        }
    }

    let service: Box<dyn ServiceManager> = Box::new(MockService);
    let result = service.service_status().await;

    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    ctx.cli_parse_result = Some(result.map(|_| ()).map_err(|e| e.to_string()));
}

#[then("the query should succeed")]
async fn then_query_success(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let result = ctx
        .cli_parse_result
        .as_ref()
        .expect("No query attempted");
    assert!(result.is_ok(), "Service status query failed: {:?}", result);
}

// ═══ Resource Governor Steps ═══

#[given(expr = "a resource governor with CPU threshold {float}")]
async fn given_governor_custom(w: &mut AlephWorld, cpu_threshold: f32) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.governor = Some(ResourceGovernor::new(ResourceLimits {
        cpu_threshold,
        mem_threshold: 512 * 1024 * 1024,
        battery_threshold: 20.0,
    }));
}

#[given("a resource governor with default limits")]
async fn given_governor_default(w: &mut AlephWorld) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.governor = Some(ResourceGovernor::new(ResourceLimits::default()));
}

#[when("I check the governor decision")]
async fn when_check_governor(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let governor = ctx.governor.as_ref().expect("Governor not initialized");
    ctx.governor_decision = Some(governor.check().await.map_err(|e| e.to_string()));
}

#[then(expr = "the governor CPU threshold should be {float}")]
async fn then_governor_cpu_threshold(w: &mut AlephWorld, expected: f32) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let governor = ctx.governor.as_ref().expect("Governor not initialized");
    assert!(
        (governor.limits().cpu_threshold - expected).abs() < 0.001,
        "Expected CPU threshold {}, got {}",
        expected,
        governor.limits().cpu_threshold
    );
}

#[then("the decision should be Proceed or Throttle")]
async fn then_decision_proceed_or_throttle(w: &mut AlephWorld) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let decision = ctx.governor_decision.as_ref().expect("No decision made");
    match decision {
        Ok(GovernorDecision::Proceed) | Ok(GovernorDecision::Throttle) => {}
        other => panic!("Expected Proceed or Throttle, got {:?}", other),
    }
}

// ═══ IPC Steps ═══

#[given(expr = "a socket path {string}")]
async fn given_socket_path(w: &mut AlephWorld, path: String) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    // Clean up if exists
    let _ = std::fs::remove_file(&path);
    ctx.socket_path = Some(path);
}

#[when("I create an IPC server")]
async fn when_create_ipc_server(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let path = ctx.socket_path.as_ref().expect("Socket path not set");
    ctx.ipc_server = Some(IpcServer::new(path.clone()));
}

#[then(expr = "the server socket path should be {string}")]
async fn then_server_socket_path(w: &mut AlephWorld, expected: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let server = ctx.ipc_server.as_ref().expect("IPC server not created");
    assert_eq!(server.socket_path(), expected);
}

#[given(expr = "a JSON-RPC request {string}")]
async fn given_json_rpc_request(w: &mut AlephWorld, json: String) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    // Remove surrounding quotes if present
    let json = json.trim_matches('\'').to_string();
    ctx.json_rpc_json = Some(json);
}

#[when("I parse the JSON-RPC request")]
async fn when_parse_json_rpc(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let json = ctx.json_rpc_json.as_ref().expect("JSON not set");
    ctx.json_rpc_request = Some(serde_json::from_str(json).expect("Failed to parse JSON-RPC"));
}

#[then(expr = "the method should be {string}")]
async fn then_method_should_be(w: &mut AlephWorld, expected: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let request = ctx.json_rpc_request.as_ref().expect("Request not parsed");
    assert_eq!(request.method, expected);
}

#[then(expr = "the request id should be {int}")]
async fn then_request_id(w: &mut AlephWorld, expected: i32) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let request = ctx.json_rpc_request.as_ref().expect("Request not parsed");
    assert_eq!(request.id, serde_json::json!(expected));
}

// ═══ Launchd Steps (macOS only) ═══

#[cfg(target_os = "macos")]
#[when("I create a LaunchdService")]
async fn when_create_launchd_service(w: &mut AlephWorld) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.launchd_service = Some(LaunchdService::new().expect("Failed to create LaunchdService"));
}

#[cfg(not(target_os = "macos"))]
#[when("I create a LaunchdService")]
async fn when_create_launchd_service(_w: &mut AlephWorld) {
    // Skip on non-macOS platforms
}

#[cfg(target_os = "macos")]
#[then(expr = "the plist path should contain {string}")]
async fn then_plist_path_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let service = ctx.launchd_service.as_ref().expect("LaunchdService not created");
    assert!(service.plist_path().to_string_lossy().contains(&expected));
}

#[cfg(not(target_os = "macos"))]
#[then(expr = "the plist path should contain {string}")]
async fn then_plist_path_contains(_w: &mut AlephWorld, _expected: String) {
    // Skip on non-macOS platforms
}

#[given("a default DaemonConfig")]
async fn given_default_daemon_config(w: &mut AlephWorld) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    ctx.daemon_config = Some(DaemonConfig::default());
}

#[cfg(target_os = "macos")]
#[when("I generate the plist")]
async fn when_generate_plist(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let service = ctx.launchd_service.as_ref().expect("LaunchdService not created");
    let config = ctx.daemon_config.as_ref().expect("DaemonConfig not set");
    ctx.plist_content = Some(service.generate_plist(config).expect("Failed to generate plist"));
}

#[cfg(not(target_os = "macos"))]
#[when("I generate the plist")]
async fn when_generate_plist(_w: &mut AlephWorld) {
    // Skip on non-macOS platforms
}

#[cfg(target_os = "macos")]
#[then(expr = "the plist should contain {string}")]
async fn then_plist_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let plist = ctx.plist_content.as_ref().expect("Plist not generated");
    assert!(plist.contains(&expected), "Plist does not contain '{}'", expected);
}

#[cfg(not(target_os = "macos"))]
#[then(expr = "the plist should contain {string}")]
async fn then_plist_contains(_w: &mut AlephWorld, _expected: String) {
    // Skip on non-macOS platforms
}

// ═══ WorldModel + Dispatcher Integration Steps ═══

#[given(expr = "a daemon event bus with capacity {int}")]
async fn given_daemon_event_bus(w: &mut AlephWorld, capacity: i32) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    let event_bus = Arc::new(DaemonEventBus::new(capacity as usize));
    ctx.arc_event_bus = Some(event_bus);
}

#[given("a WorldModel with test configuration")]
async fn given_worldmodel_test_config(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let event_bus = ctx.arc_event_bus.clone().expect("Event bus not initialized");

    let dir = tempdir().unwrap();
    let state_path = dir.path().join("worldmodel_state.json");

    let config = WorldModelConfig {
        state_path: Some(state_path),
        batch_interval: 5,
        periodic_interval: 30,
        cache_size: 100,
        confidence_threshold: 0.7,
    };

    let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());
    ctx.worldmodel = Some(worldmodel);
    ctx.persistence_temp_dir = Some(dir);
}

#[given("a Dispatcher with default configuration")]
async fn given_dispatcher_default_config(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let worldmodel = ctx.worldmodel.clone().expect("WorldModel not initialized");
    let event_bus = ctx.arc_event_bus.clone().expect("Event bus not initialized");

    let config = ProactiveDispatcherConfig::default();
    let dispatcher = ProactiveDispatcher::new(config, worldmodel, event_bus);
    ctx.dispatcher = Some(dispatcher);  // Already Arc<Dispatcher>
}

#[given("I subscribe to the event bus")]
async fn given_subscribe_to_event_bus(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let event_bus = ctx.arc_event_bus.as_ref().expect("Event bus not initialized");
    ctx.receivers.push(event_bus.subscribe());
}

#[when(expr = "I spawn WorldModel loop with {int} second timeout")]
async fn when_spawn_worldmodel_loop(w: &mut AlephWorld, timeout_secs: i32) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let worldmodel = ctx.worldmodel.clone().expect("WorldModel not initialized");

    let handle = tokio::spawn(async move {
        let _ = tokio::time::timeout(
            Duration::from_secs(timeout_secs as u64),
            worldmodel.run(),
        )
        .await;
    });
    ctx.worldmodel_handle = Some(handle);
}

#[when(expr = "I spawn Dispatcher loop with {int} second timeout")]
async fn when_spawn_dispatcher_loop(w: &mut AlephWorld, timeout_secs: i32) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let dispatcher = ctx.dispatcher.as_ref().expect("Dispatcher not initialized").clone();

    let handle = tokio::spawn(async move {
        let _ = tokio::time::timeout(
            Duration::from_secs(timeout_secs as u64),
            dispatcher.run(),
        )
        .await;
    });
    ctx.dispatcher_handle = Some(handle);
}

#[when(expr = "I wait {int} milliseconds for startup")]
async fn when_wait_milliseconds_startup(w: &mut AlephWorld, ms: i32) {
    let _ = w; // unused but needed for cucumber signature
    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
}

#[when(expr = "I wait {int} milliseconds for processing")]
async fn when_wait_milliseconds_processing(w: &mut AlephWorld, ms: i32) {
    let _ = w; // unused but needed for cucumber signature
    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
}

#[when(expr = "I wait {int} milliseconds for cleanup")]
async fn when_wait_milliseconds_cleanup(w: &mut AlephWorld, ms: i32) {
    let _ = w; // unused but needed for cucumber signature
    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
}

#[when(expr = "I send a process started event for {string} with pid {int}")]
async fn when_send_process_started_event(w: &mut AlephWorld, name: String, pid: i32) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let event_bus = ctx.arc_event_bus.as_ref().expect("Event bus not initialized");

    let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
        timestamp: Utc::now(),
        pid: pid as u32,
        name,
        event_type: ProcessEventType::Started,
    });

    event_bus.send(event).expect("Failed to send event");
}

#[then(expr = "the WorldModel state should be {string} activity")]
async fn then_worldmodel_state_activity(w: &mut AlephWorld, expected_activity: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let worldmodel = ctx.worldmodel.as_ref().expect("WorldModel not initialized");
    let state = worldmodel.get_core_state().await;

    let matches = match expected_activity.as_str() {
        "Programming" => matches!(state.activity, ActivityType::Programming { .. }),
        "Idle" => matches!(state.activity, ActivityType::Idle),
        "Meeting" => matches!(state.activity, ActivityType::Meeting { .. }),
        "Reading" => matches!(state.activity, ActivityType::Reading),
        "Unknown" => matches!(state.activity, ActivityType::Unknown),
        _ => panic!("Unknown activity type: {}", expected_activity),
    };

    assert!(
        matches,
        "Expected {} activity, got: {:?}",
        expected_activity, state.activity
    );
}

#[then(expr = "I should receive an event from the bus within {int} second")]
async fn then_should_receive_event(w: &mut AlephWorld, timeout_secs: i32) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    let receiver = ctx.receivers.first_mut().expect("No subscribers");

    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs as u64),
        receiver.recv(),
    )
    .await;

    assert!(result.is_ok(), "Should receive event within {} second(s)", timeout_secs);
    ctx.received_event = result.ok().and_then(|r| r.ok());
}

#[when("I abort spawned tasks")]
async fn when_abort_spawned_tasks(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");

    if let Some(handle) = ctx.worldmodel_handle.take() {
        handle.abort();
    }
    if let Some(handle) = ctx.dispatcher_handle.take() {
        handle.abort();
    }
}

// ═══ Dispatcher Mode Steps ═══

#[then(expr = "the dispatcher mode should be {string}")]
async fn then_dispatcher_mode(w: &mut AlephWorld, expected_mode: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let dispatcher = ctx.dispatcher.as_ref().expect("Dispatcher not initialized");
    let mode = dispatcher.get_mode().await;

    let matches = match expected_mode.as_str() {
        "Running" => matches!(mode, DispatcherMode::Running),
        "Reconciling" => matches!(mode, DispatcherMode::Reconciling { .. }),
        _ => panic!("Unknown dispatcher mode: {}", expected_mode),
    };

    assert!(matches, "Expected {} mode, got: {:?}", expected_mode, mode);
}

#[when(expr = "I set dispatcher mode to {string} with empty pending actions")]
async fn when_set_dispatcher_mode_reconciling(w: &mut AlephWorld, _mode: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let dispatcher = ctx.dispatcher.as_ref().expect("Dispatcher not initialized");

    dispatcher
        .set_mode(DispatcherMode::Reconciling {
            pending_high_risk: vec![],
            started_at: Utc::now(),
        })
        .await;
}

#[when(expr = "I set dispatcher mode to {string}")]
async fn when_set_dispatcher_mode(w: &mut AlephWorld, mode: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let dispatcher = ctx.dispatcher.as_ref().expect("Dispatcher not initialized");

    let new_mode = match mode.as_str() {
        "Running" => DispatcherMode::Running,
        "Reconciling" => DispatcherMode::Reconciling {
            pending_high_risk: vec![],
            started_at: Utc::now(),
        },
        _ => panic!("Unknown dispatcher mode: {}", mode),
    };

    dispatcher.set_mode(new_mode).await;
}

// ═══ WorldModel Persistence Steps ═══

#[given("a temporary state file for persistence testing")]
async fn given_temp_state_file(w: &mut AlephWorld) {
    let ctx = w.daemon.get_or_insert_with(DaemonContext::default);
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("worldmodel_state.json");
    ctx.persistence_state_path = Some(state_path);
    ctx.persistence_temp_dir = Some(dir);
}

#[when("I create a WorldModel with the state file")]
async fn when_create_worldmodel_with_state_file(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");

    // Create event bus if not exists
    if ctx.arc_event_bus.is_none() {
        ctx.arc_event_bus = Some(Arc::new(DaemonEventBus::new(100)));
    }

    let event_bus = ctx.arc_event_bus.clone().unwrap();
    let state_path = ctx.persistence_state_path.clone().expect("State path not set");

    let config = WorldModelConfig {
        state_path: Some(state_path),
        batch_interval: 5,
        periodic_interval: 30,
        cache_size: 100,
        confidence_threshold: 0.7,
    };

    let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());
    ctx.worldmodel = Some(worldmodel);
}

#[when(expr = "I add a pending action {string} with reason {string}")]
async fn when_add_pending_action(w: &mut AlephWorld, action_type: String, reason: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let worldmodel = ctx.worldmodel.as_ref().expect("WorldModel not initialized");

    let action_type_enum = match action_type.as_str() {
        "MuteSystemAudio" => ActionType::MuteSystemAudio,
        "EnableDoNotDisturb" => ActionType::EnableDoNotDisturb,
        _ => panic!("Unknown action type: {}", action_type),
    };

    let action = PendingAction {
        action_type: action_type_enum,
        reason,
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        risk_level: RiskLevel::Medium,
    };

    worldmodel.add_pending_action(action).await.unwrap();
}

#[then(expr = "the WorldModel should have {int} pending action")]
async fn then_worldmodel_pending_actions(w: &mut AlephWorld, expected: i32) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let worldmodel = ctx.worldmodel.as_ref().expect("WorldModel not initialized");
    let state = worldmodel.get_core_state().await;

    assert_eq!(
        state.pending_actions.len(),
        expected as usize,
        "Expected {} pending action(s), got {}",
        expected,
        state.pending_actions.len()
    );
}

#[when("I drop the WorldModel instance")]
async fn when_drop_worldmodel(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");
    ctx.worldmodel = None;
}

#[when("I create a new WorldModel with the same state file")]
async fn when_create_new_worldmodel_same_state(w: &mut AlephWorld) {
    let ctx = w.daemon.as_mut().expect("Daemon context not initialized");

    // Create new event bus
    let event_bus = Arc::new(DaemonEventBus::new(100));
    ctx.arc_event_bus = Some(event_bus.clone());

    let state_path = ctx.persistence_state_path.clone().expect("State path not set");

    let config = WorldModelConfig {
        state_path: Some(state_path),
        batch_interval: 5,
        periodic_interval: 30,
        cache_size: 100,
        confidence_threshold: 0.7,
    };

    let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());
    ctx.worldmodel = Some(worldmodel);
}

#[then(expr = "the pending action reason should be {string}")]
async fn then_pending_action_reason(w: &mut AlephWorld, expected_reason: String) {
    let ctx = w.daemon.as_ref().expect("Daemon context not initialized");
    let worldmodel = ctx.worldmodel.as_ref().expect("WorldModel not initialized");
    let state = worldmodel.get_core_state().await;

    assert!(!state.pending_actions.is_empty(), "No pending actions");
    assert_eq!(
        state.pending_actions[0].reason, expected_reason,
        "Expected reason '{}', got '{}'",
        expected_reason, state.pending_actions[0].reason
    );
}
