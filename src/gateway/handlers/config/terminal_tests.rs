//! Real write surfaces share one Terminal source and the existing Gateway bus.
use super::{handle_patch_config, handle_reload_with_subsystems};
use crate::builtin_tools::self_config::{SelfConfigArgs, SelfConfigTool};
use crate::capability::provider::ProviderState;
use crate::config::backup::ConfigBackup;
use crate::config::patcher::ConfigPatcher;
use crate::config::Config;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::hot_reload::{ConfigWatcher, ConfigWatcherConfig};
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::pty;
use crate::gateway::security::{SecurityStore, SharedTokenManager};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use serde_json::json;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::RwLock;

fn rpc(method: &str) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(1)),
        method: method.into(),
        params: None,
    }
}

fn child_only(test: &str, marker: &str) -> bool {
    if std::env::var_os(marker).is_some() {
        return true;
    }
    let home = tempfile::tempdir().unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(marker, "1")
        .env("ALEPH_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    false
}

fn watcher() -> Arc<ConfigWatcher> {
    let path =
        std::path::PathBuf::from(std::env::var_os("ALEPH_HOME").unwrap()).join("gateway.toml");
    std::fs::write(
        &path,
        "[gateway]\nport = 18790\n[agents.main]\nmodel = \"claude-sonnet-4-5\"\n",
    )
    .unwrap();
    Arc::new(
        ConfigWatcher::new(ConfigWatcherConfig {
            config_path: path,
            debounce_duration: Duration::from_millis(100),
            channel_capacity: 8,
        })
        .unwrap(),
    )
}

fn tool_update(enabled: bool) -> SelfConfigArgs {
    SelfConfigArgs::UpdateConfig {
        config_path: "policies.terminal.enabled".into(),
        config_value: json!(enabled),
        dry_run: false,
        verify: false,
    }
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
#[serial_test::serial(spend_global_policy, search_live_handle, subagent_concurrency_cap)]
async fn terminal_cold_patch_self_config_noop_and_explicit_reload_recovery() {
    if !child_only("gateway::handlers::config::terminal_tests::terminal_cold_patch_self_config_noop_and_explicit_reload_recovery", "ALEPH_TERMINAL_SURFACES_CHILD") { return; }
    let home = std::path::PathBuf::from(std::env::var_os("ALEPH_HOME").unwrap());
    let path = home.join("config.toml");
    let mut initial = Config::default();
    initial.policies.terminal.enabled = false;
    initial.save_to_file(&path).unwrap();
    let config = Arc::new(RwLock::new(initial));
    let mut server = crate::gateway::server::GatewayServer::new("127.0.0.1:0".parse().unwrap());
    server.set_application_config(config.clone());
    let _router = server.build_router();
    let bus = server.event_bus().clone();
    let mut events = bus.subscribe();
    let patcher = Arc::new(
        ConfigPatcher::new(
            config.clone(),
            path.clone(),
            ConfigBackup::new(home.join("backups"), 10),
        )
        .with_event_bus(bus.clone()),
    );
    let tool = SelfConfigTool::new("terminal-fixture")
        .unwrap()
        .with_config(config.clone())
        .with_patcher(patcher.clone());
    assert!(
        pty::try_manager().is_none(),
        "wiring contexts must remain cold"
    );
    let vault = Arc::new(SharedTokenManager::new(
        Arc::new(SecurityStore::in_memory().unwrap()),
        home.join("test.vault"),
    ));
    let mut patch = rpc("config.patch");
    patch.params = Some(json!({"path":"policies.terminal.enabled", "patch":true}));
    let response = handle_patch_config(patch, patcher.clone(), bus.clone(), vault).await;
    let result = response.result.unwrap();
    assert_eq!(result["success"], true);
    assert_eq!(result["reload_impact"]["kind"], "live");
    let first = pty::manager().ready_generation().unwrap();
    let _router = server.build_router();
    assert_eq!(pty::manager().ready_generation().unwrap(), first);
    let session = pty::manager()
        .spawn(&pty::SpawnOptions::default())
        .unwrap()
        .session_id;
    pty::manager()
        .write(
            &session,
            if cfg!(windows) {
                b"echo TERMINAL_LIVE_CONFIG_OK\r\n"
            } else {
                b"printf 'TERMINAL_LIVE_CONFIG_OK'\n"
            },
        )
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let raw = events.recv().await.unwrap();
            let event: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if event["topic"] == "pty.screen"
                && event["data"]["session_id"] == session
                && raw.contains("TERMINAL_LIVE_CONFIG_OK")
            {
                break event;
            }
        }
    })
    .await
    .expect("a real PTY screen frame on the original Gateway bus");
    assert_eq!(frame["data"]["provider_generation"], first);
    let shell = pty::manager::test_session(pty::manager(), &session).unwrap();
    let (driver, release) = pty::manager::hold_test_driver(pty::manager());
    tokio::time::pause();
    let disabled = AlephTool::call(&tool, tool_update(false)).await.unwrap();
    assert!(
        disabled.success,
        "persistence succeeds independently of live apply"
    );
    assert_eq!(disabled.data.unwrap()["reload_impact"]["kind"], "restart");
    assert!(
        !Config::load_from_file(&path)
            .unwrap()
            .policies
            .terminal
            .enabled
    );
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    assert_eq!(
        pty::manager().lifecycle_snapshot().state,
        ProviderState::Stopping
    );
    assert!(
        shell.is_closed(),
        "source cleanup must close the actual shell"
    );
    assert!(
        pty::manager::test_session(pty::manager(), &session).is_none(),
        "cleanup removed the source-owned session"
    );
    let saved = std::fs::read(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let backups = patcher.list_backups().unwrap().len();
    let noop = AlephTool::call(&tool, tool_update(false)).await.unwrap();
    assert!(noop.message.contains("no change applied"));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert_eq!(patcher.list_backups().unwrap().len(), backups);
    let no_retry = patcher
        .apply(crate::config::patcher::PatchRequest {
            path: "policies.terminal.enabled".into(),
            patch: json!(false),
            dry_run: false,
            health_check: false,
        })
        .await
        .unwrap();
    assert!(no_retry.diff.is_empty());
    assert!(no_retry.live_applied.is_empty());
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    release.send(()).unwrap();
    let watcher = watcher();
    let recovered = handle_reload_with_subsystems(
        rpc("config.reload"),
        watcher.clone(),
        config.clone(),
        bus.clone(),
    )
    .await
    .result
    .unwrap();
    assert!(recovered["reloaded"]
        .as_array()
        .unwrap()
        .contains(&json!("live:policies.terminal")));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
    assert_eq!(
        pty::manager().lifecycle_snapshot().state,
        ProviderState::Stopped
    );
    let enabled = AlephTool::call(&tool, tool_update(true)).await.unwrap();
    assert_eq!(enabled.data.unwrap()["reload_impact"]["kind"], "live");
    assert!(pty::manager().ready_generation().unwrap() > first);
    assert!(pty::manager().admit_generation(first).is_err());
    let (_, release) = pty::manager::hold_test_driver(pty::manager());
    // Cancellation after a real persisted disable must recover through the same RPC too.
    {
        let mut stop = std::pin::pin!(AlephTool::call(&tool, tool_update(false)));
        std::future::poll_fn(|cx| {
            assert!(stop.as_mut().poll(cx).is_pending());
            if pty::manager().lifecycle_snapshot().state == ProviderState::Stopping {
                return Poll::Ready(());
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;
    }
    assert!(pty::manager::test_driver_id(pty::manager()).is_some());
    release.send(()).unwrap();
    let recovered = handle_reload_with_subsystems(rpc("config.reload"), watcher, config, bus)
        .await
        .result
        .unwrap();
    assert!(recovered["reloaded"]
        .as_array()
        .unwrap()
        .contains(&json!("live:policies.terminal")));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
    // A cancelled persisted update must not poison the shared patcher's mtime fence.
    let enabled = AlephTool::call(&tool, tool_update(true)).await.unwrap();
    assert!(
        enabled.success,
        "a subsequent update must not conflict with its own cancelled write"
    );
    assert_eq!(enabled.data.unwrap()["reload_impact"]["kind"], "live");
    let disabled = AlephTool::call(&tool, tool_update(false)).await.unwrap();
    assert!(disabled.success);
    assert_eq!(disabled.data.unwrap()["reload_impact"]["kind"], "live");
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
#[serial_test::serial(spend_global_policy, search_live_handle, subagent_concurrency_cap)]
async fn terminal_reload_releases_write_guard_and_applies_the_newest_shared_intent() {
    if !child_only("gateway::handlers::config::terminal_tests::terminal_reload_releases_write_guard_and_applies_the_newest_shared_intent", "ALEPH_TERMINAL_RELOAD_ORDER_CHILD") { return; }
    let path =
        std::path::PathBuf::from(std::env::var_os("ALEPH_HOME").unwrap()).join("config.toml");
    let mut disk = Config::default();
    disk.policies.terminal.enabled = false;
    disk.save_to_file(&path).unwrap();
    let config = Arc::new(RwLock::new(Config::default()));
    let bus = Arc::new(GatewayEventBus::new());
    let first = pty::attach_event_bus(bus.clone()).unwrap();
    let held_read = config.read().await;
    let mut reload = std::pin::pin!(handle_reload_with_subsystems(
        rpc("config.reload"),
        watcher(),
        config.clone(),
        bus.clone()
    ));
    // Keep the reload's installation write queued, then put another writer behind it.
    std::future::poll_fn(|cx| {
        assert!(reload.as_mut().poll(cx).is_pending());
        if config.try_read().is_err() {
            return Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
    let mut newer = std::pin::pin!(config.write());
    assert!(newer
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(held_read);
    assert!(
        reload
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending(),
        "reload must release its installation guard and wait for a fresh read"
    );
    let Poll::Ready(mut latest) = newer.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("reload retained the write guard across live apply")
    };
    assert!(
        !latest.policies.terminal.enabled,
        "reload installed disk intent first"
    );
    latest.policies.terminal.enabled = true;
    latest.save_to_file(&path).unwrap();
    drop(latest);
    let result = reload.await.result.unwrap();
    assert!(result["reloaded"]
        .as_array()
        .unwrap()
        .contains(&json!("live:policies.terminal")));
    assert!(config.read().await.policies.terminal.enabled);
    assert_eq!(
        pty::manager().ready_generation().unwrap(),
        first,
        "apply used newest enabled intent, without stopping the ready source"
    );
    disk.policies.terminal.enabled = false;
    disk.save_to_file(&path).unwrap();
    let result =
        handle_reload_with_subsystems(rpc("config.reload"), watcher(), config.clone(), bus)
            .await
            .result
            .unwrap();
    assert!(result["reloaded"]
        .as_array()
        .unwrap()
        .contains(&json!("live:policies.terminal")));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
}
