//! Step definitions for perception features

use crate::world::{AlephWorld, PerceptionContext};
use alephcore::daemon::{
    perception::{
        FSEventWatcher, FSWatcherConfig, PerceptionConfig, ProcessWatcher, ProcessWatcherConfig,
        SystemStateWatcher, SystemWatcherConfig, TimeWatcher, TimeWatcherConfig, Watcher,
        WatcherControl, WatcherRegistry,
    },
    DaemonEvent, DaemonEventBus, RawEvent,
};
use cucumber::{given, then, when};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{timeout, Duration};

// ═══════════════════════════════════════════════════════════════════════════
// Config Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a default PerceptionConfig")]
async fn given_default_perception_config(w: &mut AlephWorld) {
    let ctx = w.perception.get_or_insert_with(PerceptionContext::default);
    ctx.config = Some(PerceptionConfig::default());
}

#[then("perception should be enabled")]
async fn then_perception_enabled(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.enabled);
}

#[then("process watcher should be enabled")]
async fn then_process_enabled(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.process.enabled);
}

#[then(expr = "process poll interval should be {int} seconds")]
async fn then_process_poll_interval(w: &mut AlephWorld, expected: i32) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert_eq!(config.process.poll_interval_secs, expected as u64);
}

#[then(expr = "process watched apps should include {string}")]
async fn then_process_watched_apps(w: &mut AlephWorld, app: String) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(
        config.process.watched_apps.contains(&app),
        "Expected watched apps to contain '{}', but got {:?}",
        app,
        config.process.watched_apps
    );
}

#[then("filesystem watcher should be enabled")]
async fn then_filesystem_enabled(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.filesystem.enabled);
}

#[then(expr = "filesystem debounce should be {int} ms")]
async fn then_filesystem_debounce(w: &mut AlephWorld, expected: i32) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert_eq!(config.filesystem.debounce_ms, expected as u64);
}

#[then("time watcher should be enabled")]
async fn then_time_enabled(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.time.enabled);
}

#[then(expr = "time heartbeat interval should be {int} seconds")]
async fn then_time_heartbeat_interval(w: &mut AlephWorld, expected: i32) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert_eq!(config.time.heartbeat_interval_secs, expected as u64);
}

#[then("system watcher should be enabled")]
async fn then_system_enabled(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.system.enabled);
}

#[then(expr = "system poll interval should be {int} seconds")]
async fn then_system_poll_interval(w: &mut AlephWorld, expected: i32) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert_eq!(config.system.poll_interval_secs, expected as u64);
}

#[then("system should track battery")]
async fn then_system_track_battery(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.system.track_battery);
}

#[when("I serialize the config to TOML")]
async fn when_serialize_to_toml(w: &mut AlephWorld) {
    let ctx = w.perception.as_mut().expect("Perception context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    ctx.config_toml = Some(toml::to_string(config).expect("Failed to serialize"));
}

#[then(expr = "the TOML should contain {string}")]
async fn then_toml_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let toml = ctx.config_toml.as_ref().expect("TOML not generated");
    assert!(toml.contains(&expected), "TOML does not contain '{}'\nActual TOML:\n{}", expected, toml);
}

// ═══════════════════════════════════════════════════════════════════════════
// Registry Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given("a new WatcherRegistry")]
async fn given_new_registry(w: &mut AlephWorld) {
    let ctx = w.perception.get_or_insert_with(PerceptionContext::default);
    ctx.registry = Some(WatcherRegistry::new());
}

#[then(expr = "the registry watcher count should be {int}")]
async fn then_registry_count(w: &mut AlephWorld, expected: i32) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert_eq!(registry.watcher_count(), expected as usize);
}

#[when("I start all watchers")]
async fn when_start_all_watchers(w: &mut AlephWorld) {
    let ctx = w.perception.as_mut().expect("Perception context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    let bus = ctx
        .event_bus
        .get_or_insert_with(|| Arc::new(DaemonEventBus::new(10)));
    match registry.start_all(bus.clone()).await {
        Ok(_) => w.last_result = Some(Ok(())),
        Err(e) => w.last_result = Some(Err(e.to_string())),
    }
}

#[when("I shutdown all watchers")]
async fn when_shutdown_all_watchers(w: &mut AlephWorld) {
    let ctx = w.perception.as_mut().expect("Perception context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    match registry.shutdown_all().await {
        Ok(_) => w.last_result = Some(Ok(())),
        Err(e) => w.last_result = Some(Err(e.to_string())),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Watcher Steps
// ═══════════════════════════════════════════════════════════════════════════

#[given(expr = "a TimeWatcher with heartbeat interval {int} second(s)")]
async fn given_time_watcher(w: &mut AlephWorld, interval: i32) {
    let config = TimeWatcherConfig {
        enabled: true,
        heartbeat_interval_secs: interval as u64,
    };
    let watcher = TimeWatcher::new(config);
    let ctx = w.perception.get_or_insert_with(PerceptionContext::default);
    ctx.watcher_info = Some((watcher.id().to_string(), watcher.is_pausable()));
    ctx.event_bus = Some(Arc::new(DaemonEventBus::new(10)));
}

#[when("I start the time watcher")]
async fn when_start_time_watcher(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let bus = ctx
        .event_bus
        .as_ref()
        .expect("Event bus not initialized")
        .clone();

    let config = TimeWatcherConfig {
        enabled: true,
        heartbeat_interval_secs: 1,
    };
    let watcher = TimeWatcher::new(config);
    let mut receiver = bus.subscribe();
    let (tx, rx) = watch::channel(WatcherControl::Run);

    let watcher_task = tokio::spawn({
        let bus = bus.clone();
        async move { watcher.run(bus, rx).await }
    });

    let result = timeout(Duration::from_secs(2), receiver.recv()).await;
    let _ = tx.send(WatcherControl::Shutdown);
    let _ = watcher_task.await;

    if result.is_ok() {
        if let Ok(event) = result.unwrap() {
            if matches!(event, DaemonEvent::Raw(RawEvent::Heartbeat { .. })) {
                w.last_result = Some(Ok(()));
                return;
            }
        }
    }
    w.last_result = Some(Err("Did not receive heartbeat".to_string()));
}

#[then(expr = "I should receive a heartbeat event within {int} seconds")]
async fn then_receive_heartbeat_within(w: &mut AlephWorld, _seconds: i32) {
    let result = w.last_result.as_ref().expect("No result recorded");
    assert!(result.is_ok(), "Did not receive heartbeat event");
}

#[then(expr = "the watcher id should be {string}")]
async fn then_watcher_id(w: &mut AlephWorld, expected: String) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let (id, _) = ctx.watcher_info.as_ref().expect("Watcher info not set");
    assert_eq!(id, &expected, "Expected watcher id '{}', got '{}'", expected, id);
}

#[then("the watcher should not be pausable")]
async fn then_watcher_not_pausable(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let (_, pausable) = ctx.watcher_info.as_ref().expect("Watcher info not set");
    assert!(!pausable, "Expected watcher to not be pausable");
}

#[then("the watcher should be pausable")]
async fn then_watcher_pausable(w: &mut AlephWorld) {
    let ctx = w.perception.as_ref().expect("Perception context not initialized");
    let (_, pausable) = ctx.watcher_info.as_ref().expect("Watcher info not set");
    assert!(*pausable, "Expected watcher to be pausable");
}

#[given(expr = "a ProcessWatcher watching {string}")]
async fn given_process_watcher(w: &mut AlephWorld, app: String) {
    let config = ProcessWatcherConfig {
        enabled: true,
        poll_interval_secs: 5,
        watched_apps: vec![app],
    };
    let watcher = ProcessWatcher::new(config);
    let ctx = w.perception.get_or_insert_with(PerceptionContext::default);
    ctx.watcher_info = Some((watcher.id().to_string(), watcher.is_pausable()));
}

#[given(expr = "a SystemStateWatcher with poll interval {int} seconds")]
async fn given_system_watcher(w: &mut AlephWorld, interval: i32) {
    let config = SystemWatcherConfig {
        enabled: true,
        poll_interval_secs: interval as u64,
        track_battery: true,
        track_network: true,
        idle_threshold_secs: 300,
    };
    let watcher = SystemStateWatcher::new(config);
    let ctx = w.perception.get_or_insert_with(PerceptionContext::default);
    ctx.watcher_info = Some((watcher.id().to_string(), watcher.is_pausable()));
}

#[given(expr = "a FSEventWatcher watching {string}")]
async fn given_fs_watcher(w: &mut AlephWorld, path: String) {
    let config = FSWatcherConfig {
        enabled: true,
        watched_paths: vec![path],
        ignore_patterns: vec!["**/.git/**".to_string()],
        debounce_ms: 500,
    };
    let watcher = FSEventWatcher::new(config);
    let ctx = w.perception.get_or_insert_with(PerceptionContext::default);
    ctx.watcher_info = Some((watcher.id().to_string(), watcher.is_pausable()));
}
