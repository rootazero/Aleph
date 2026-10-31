use super::{SelfConfigArgs, SelfConfigTool};
use crate::capability::provider::ProviderState;
use crate::config::backup::ConfigBackup;
use crate::config::patcher::ConfigPatcher;
use crate::config::Config;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::pty;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use std::future::Future;
use std::task::Poll;
use tokio::sync::RwLock;

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
#[serial_test::serial(spend_global_policy, search_live_handle, subagent_concurrency_cap)]
async fn terminal_self_config_rollback_waits_for_the_same_source_driver() {
    if let Some(manager) = pty::try_manager() {
        manager.deactivate().await.unwrap();
    }
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let mut initial = Config::default();
    initial.policies.terminal.enabled = false;
    initial.save_to_file(&path).unwrap();
    let config = Arc::new(RwLock::new(initial));
    let bus = Arc::new(GatewayEventBus::new());
    let patcher = Arc::new(
        ConfigPatcher::new(
            config.clone(),
            path.clone(),
            ConfigBackup::new(tmp.path().join("backups"), 10),
        )
        .with_event_bus(bus),
    );
    let tool = SelfConfigTool::new("terminal-rollback")
        .unwrap()
        .with_config(config.clone())
        .with_patcher(patcher);
    let enabled = AlephTool::call(
        &tool,
        SelfConfigArgs::UpdateConfig {
            config_path: "policies.terminal.enabled".into(),
            config_value: serde_json::json!(true),
            dry_run: false,
            verify: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(enabled.data.unwrap()["reload_impact"]["kind"], "live");
    let (driver, release) = pty::manager::hold_test_driver(pty::manager());
    let mut rollback = std::pin::pin!(AlephTool::call(
        &tool,
        SelfConfigArgs::RollbackConfig {
            timestamp: None,
            dry_run: false
        }
    ));
    std::future::poll_fn(|cx| {
        assert!(
            rollback.as_mut().poll(cx).is_pending(),
            "tool must not report an unjoined rollback as live"
        );
        if pty::manager().lifecycle_snapshot().state == ProviderState::Stopping {
            return Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    assert!(!config.read().await.policies.terminal.enabled);
    release.send(()).unwrap();
    let result = rollback.await.unwrap();
    assert!(result.success);
    assert!(result.data.unwrap()["live_applied"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("policies.terminal")));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
    assert_eq!(
        pty::manager().lifecycle_snapshot().state,
        ProviderState::Stopped
    );
    assert!(
        !Config::load_from_file(&path)
            .unwrap()
            .policies
            .terminal
            .enabled
    );
}
