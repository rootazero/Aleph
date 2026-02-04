//! E2E Test: WorldModel + Dispatcher Integration
//!
//! Basic smoke test to verify that WorldModel and Dispatcher can start
//! and process events without crashing.

use alephcore::{
    DaemonEventBus, ProactiveDispatcher as Dispatcher, ProactiveDispatcherConfig as DispatcherConfig,
    WorldModel, WorldModelConfig,
    DaemonEvent, RawEvent, ProcessEventType, ActivityType,
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

/// Create a test WorldModel with temporary state persistence
async fn create_test_worldmodel(
    event_bus: Arc<DaemonEventBus>,
) -> Arc<WorldModel> {
    let dir = tempdir().unwrap();
    let state_path = dir.path().join("worldmodel_state.json");

    let config = WorldModelConfig {
        state_path: Some(state_path),
        batch_interval: 5,
        periodic_interval: 30,
        cache_size: 100,
        confidence_threshold: 0.7,
    };

    Arc::new(WorldModel::new(config, event_bus).await.unwrap())
}

/// Basic smoke test: Start WorldModel and Dispatcher, send a test event
#[tokio::test(flavor = "multi_thread")]
async fn test_worldmodel_dispatcher_integration() {
    // Initialize logging for debugging
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .try_init();

    // 1. Create EventBus
    let event_bus = Arc::new(DaemonEventBus::new(1000));

    // 2. Create WorldModel
    let worldmodel = create_test_worldmodel(event_bus.clone()).await;

    // 3. Create Dispatcher
    let dispatcher_config = DispatcherConfig::default();
    let dispatcher = Dispatcher::new(
        dispatcher_config,
        worldmodel.clone(),
        event_bus.clone(),
    );

    // 4. Subscribe to events (required to avoid "No active receivers" error)
    let mut rx = event_bus.subscribe();

    // 5. Spawn WorldModel loop
    let worldmodel_clone = worldmodel.clone();
    let worldmodel_handle = tokio::spawn(async move {
        // Run with timeout to prevent hanging
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            worldmodel_clone.run()
        ).await;
    });

    // 6. Spawn Dispatcher loop
    let dispatcher_clone = dispatcher.clone();
    let dispatcher_handle = tokio::spawn(async move {
        // Run with timeout to prevent hanging
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            dispatcher_clone.run()
        ).await;
    });

    // 7. Wait for spawned tasks to start subscribing
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 8. Send a test RawEvent (IDE start)
    let test_event = DaemonEvent::Raw(RawEvent::ProcessEvent {
        timestamp: chrono::Utc::now(),
        pid: 12345,
        name: "Code".to_string(),
        event_type: ProcessEventType::Started,
    });

    event_bus.send(test_event).expect("Failed to send test event");

    // 9. Wait for processing - give time for async event loop
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 10. Verify WorldModel state changed to Programming
    let state = worldmodel.get_core_state().await;
    assert!(
        matches!(state.activity, ActivityType::Programming { .. }),
        "Expected Programming activity after IDE start, got: {:?}",
        state.activity
    );

    // 11. Try to receive a DerivedEvent (ActivityChanged should be published)
    let received_event = tokio::time::timeout(
        Duration::from_secs(1),
        rx.recv()
    ).await;

    assert!(received_event.is_ok(), "Should receive ActivityChanged event");

    // 12. Cleanup: abort spawned tasks
    worldmodel_handle.abort();
    dispatcher_handle.abort();

    // Wait briefly for cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Test: Dispatcher mode transitions
#[tokio::test]
async fn test_dispatcher_mode() {
    use alephcore::daemon::DispatcherMode;

    let event_bus = Arc::new(DaemonEventBus::new(100));
    let worldmodel = create_test_worldmodel(event_bus.clone()).await;
    let dispatcher_config = DispatcherConfig::default();
    let dispatcher = Dispatcher::new(
        dispatcher_config,
        worldmodel.clone(),
        event_bus.clone(),
    );

    // Initial mode should be Running
    let mode = dispatcher.get_mode().await;
    assert!(matches!(mode, DispatcherMode::Running));

    // Should be able to set mode to Reconciling
    dispatcher.set_mode(DispatcherMode::Reconciling {
        pending_high_risk: vec![],
        started_at: chrono::Utc::now(),
    }).await;

    let mode = dispatcher.get_mode().await;
    assert!(matches!(mode, DispatcherMode::Reconciling { .. }));

    // Should be able to set back to Running
    dispatcher.set_mode(DispatcherMode::Running).await;

    let mode = dispatcher.get_mode().await;
    assert!(matches!(mode, DispatcherMode::Running));
}

/// Test: WorldModel persistence across restarts
#[tokio::test]
async fn test_worldmodel_persistence() {
    use alephcore::daemon::worldmodel::PendingAction;
    use alephcore::daemon::{ActionType, RiskLevel};

    let dir = tempdir().unwrap();
    let state_path = dir.path().join("worldmodel_state.json");

    let event_bus = Arc::new(DaemonEventBus::new(100));

    // First instance: Add a pending action
    {
        let config = WorldModelConfig {
            state_path: Some(state_path.clone()),
            batch_interval: 5,
            periodic_interval: 30,
            cache_size: 100,
            confidence_threshold: 0.7,
        };

        let worldmodel = WorldModel::new(config, event_bus.clone()).await.unwrap();

        let action = PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test persistence".to_string(),
            created_at: chrono::Utc::now(),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            risk_level: RiskLevel::Medium,
        };

        worldmodel.add_pending_action(action).await.unwrap();

        // Verify action was added
        let state = worldmodel.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);
    }

    // Second instance: Verify state was persisted and restored
    {
        let config = WorldModelConfig {
            state_path: Some(state_path),
            batch_interval: 5,
            periodic_interval: 30,
            cache_size: 100,
            confidence_threshold: 0.7,
        };

        let worldmodel = WorldModel::new(config, event_bus).await.unwrap();
        let state = worldmodel.get_core_state().await;

        // Should have restored the pending action
        assert_eq!(state.pending_actions.len(), 1);
        assert_eq!(state.pending_actions[0].reason, "Test persistence");
    }
}
