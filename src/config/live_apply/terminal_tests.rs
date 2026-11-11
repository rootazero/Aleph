//! End-to-end explicit Terminal configuration tests against the canonical source.

use super::{apply_live_sections, classify_verified};
use crate::capability::provider::ProviderState;
use crate::config::backup::ConfigBackup;
use crate::config::patcher::{ConfigPatcher, PatchRequest};
use crate::config::{Config, ReloadImpact};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::pty;
use crate::sync_primitives::Arc;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::RwLock;

fn request(enabled: bool) -> PatchRequest {
    PatchRequest {
        path: "policies.terminal.enabled".into(),
        patch: serde_json::json!(enabled),
        dry_run: false,
        health_check: false,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    config: Arc<RwLock<Config>>,
    patcher: ConfigPatcher,
    bus: Arc<GatewayEventBus>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.policies.terminal.enabled = false;
        config
            .save_to_file(&dir.path().join("config.toml"))
            .unwrap();
        let config = Arc::new(RwLock::new(config));
        let bus = Arc::new(GatewayEventBus::new());
        let patcher = ConfigPatcher::new(
            config.clone(),
            dir.path().join("config.toml"),
            ConfigBackup::new(dir.path().join("backups"), 10),
        )
        .with_event_bus(bus.clone());
        Self {
            dir,
            config,
            patcher,
            bus,
        }
    }
}

async fn stop_previous_driver() {
    if let Some(manager) = pty::try_manager() {
        manager.deactivate().await.unwrap();
    }
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn cleanup_failure_never_reports_live_or_discards_the_failed_session() {
    #[derive(Debug)]
    struct RefuseKill;
    impl portable_pty::ChildKiller for RefuseKill {
        fn kill(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "live config cleanup refused",
            ))
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }
    stop_previous_driver().await;
    let bus = Arc::new(GatewayEventBus::new());
    let generation = pty::attach_event_bus(bus.clone()).unwrap();
    let sid = pty::manager()
        .spawn(&pty::SpawnOptions {
            created_by: Some("u-live-cleanup".into()),
            ..Default::default()
        })
        .unwrap()
        .session_id;
    let session = pty::manager::test_session(pty::manager(), &sid).unwrap();
    let original = session.replace_killer_for_test(Box::new(RefuseKill));
    let driver = pty::manager::test_driver_id(pty::manager());
    let mut disabled = Config::default();
    disabled.policies.terminal.enabled = false;
    let applied = apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None).await;
    assert!(applied.is_empty());
    let failure = pty::manager().lifecycle_snapshot();
    assert_eq!(failure.state, ProviderState::Failed);
    assert!(failure
        .failure
        .as_ref()
        .unwrap()
        .detail
        .contains("live config cleanup refused"));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), driver);
    assert!(pty::manager::test_session(pty::manager(), &sid).is_some());
    assert_eq!(
        pty::manager().owner_of(&sid),
        pty::SessionOwner::Known(Some("u-live-cleanup".into()))
    );
    assert!(pty::manager().admit_generation(generation).is_err());
    session.replace_killer_for_test(original);
    // Retried cleanup joins the driver, but the recorded source failure remains non-live.
    assert!(
        apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None)
            .await
            .is_empty()
    );
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
    assert!(pty::manager::test_session(pty::manager(), &sid).is_none());
    assert!(
        apply_live_sections(&Config::default(), &["policies.terminal"], Some(&bus), None)
            .await
            .contains(&"policies.terminal")
    );
    assert!(
        apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None)
            .await
            .contains(&"policies.terminal")
    );
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn enabling_while_stopping_observes_the_same_wait_budget() {
    stop_previous_driver().await;
    let bus = Arc::new(GatewayEventBus::new());
    let generation = pty::attach_event_bus(bus.clone()).unwrap();
    let (driver, release) = pty::manager::hold_test_driver(pty::manager());
    let mut disabled = Config::default();
    disabled.policies.terminal.enabled = false;
    {
        let mut stop = std::pin::pin!(apply_live_sections(
            &disabled,
            &["policies.terminal"],
            Some(&bus),
            None,
        ));
        assert!(stop
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    assert!(
        apply_live_sections(&Config::default(), &["policies.terminal"], Some(&bus), None)
            .await
            .is_empty()
    );
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    assert_eq!(pty::manager().lifecycle_snapshot().generation, generation);
    release.send(()).unwrap();
    assert!(
        apply_live_sections(&Config::default(), &["policies.terminal"], Some(&bus), None)
            .await
            .contains(&"policies.terminal")
    );
    assert!(pty::manager().ready_generation().unwrap() > generation);
    assert!(
        apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None)
            .await
            .contains(&"policies.terminal")
    );
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn stop_timeout_retains_the_same_driver_until_explicit_retry() {
    stop_previous_driver().await;
    let bus = Arc::new(GatewayEventBus::new());
    pty::attach_event_bus(bus.clone()).unwrap();
    let (driver, release) = pty::manager::hold_test_driver(pty::manager());
    let mut cfg = Config::default();
    cfg.policies.terminal.enabled = false;
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let applied = apply_live_sections(&cfg, &["policies.terminal"], Some(&bus), None).await;
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(applied.is_empty());
    assert_eq!(
        classify_verified("policies.terminal.enabled", &applied),
        ReloadImpact::Restart
    );
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    assert_eq!(
        pty::manager().lifecycle_snapshot().state,
        ProviderState::Stopping
    );
    assert!(pty::manager().spawn(&pty::SpawnOptions::default()).is_err());
    release.send(()).unwrap();
    assert_eq!(
        apply_live_sections(&cfg, &["policies.terminal"], Some(&bus), None).await,
        vec!["policies.terminal"]
    );
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn stopping_enable_waits_for_retained_join_before_new_generation() {
    stop_previous_driver().await;
    let bus = Arc::new(GatewayEventBus::new());
    let first = pty::attach_event_bus(bus.clone()).unwrap();
    let (driver, release) = pty::manager::hold_test_driver(pty::manager());
    let mut disabled = Config::default();
    disabled.policies.terminal.enabled = false;
    {
        let mut stop = std::pin::pin!(apply_live_sections(
            &disabled,
            &["policies.terminal"],
            Some(&bus),
            None,
        ));
        assert!(stop
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    }
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    assert!(pty::manager().admit_generation(first).is_err());
    let enabled = Config::default();
    let mut enable = std::pin::pin!(apply_live_sections(
        &enabled,
        &["policies.terminal"],
        Some(&bus),
        None,
    ));
    assert!(enable
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert_eq!(pty::manager().lifecycle_snapshot().generation, first);
    assert_eq!(pty::manager::test_driver_id(pty::manager()), Some(driver));
    release.send(()).unwrap();
    assert_eq!(enable.await, vec!["policies.terminal"]);
    let second = pty::manager().ready_generation().unwrap();
    assert!(second > first);
    assert!(pty::manager().admit_generation(first).is_err());
    assert!(!pty::manager::frame_admits(
        "pty.screen",
        Some(&serde_json::json!({"provider_generation": first}))
    ));
    assert_eq!(
        apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None).await,
        vec!["policies.terminal"]
    );
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn a_failed_join_and_missing_or_wrong_bus_never_earn_terminal_live() {
    stop_previous_driver().await;
    let bus = Arc::new(GatewayEventBus::new());
    let first = pty::attach_event_bus(bus.clone()).unwrap();
    let cfg = Config::default();
    assert!(apply_live_sections(&cfg, &["policies.terminal"], None, None)
        .await
        .is_empty());
    let wrong = Arc::new(GatewayEventBus::new());
    assert!(
        apply_live_sections(&cfg, &["policies.terminal"], Some(&wrong), None)
            .await
            .is_empty()
    );
    assert_eq!(pty::manager().ready_generation().unwrap(), first);
    let driver = pty::manager::test_driver_id(pty::manager());
    assert_eq!(
        apply_live_sections(&cfg, &["policies.terminal"], Some(&bus), None).await,
        vec!["policies.terminal"]
    );
    assert_eq!(pty::manager::test_driver_id(pty::manager()), driver);
    pty::manager::abort_test_driver(pty::manager());
    let mut disabled = cfg;
    disabled.policies.terminal.enabled = false;
    let applied = apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None).await;
    assert!(applied.is_empty());
    let snapshot = pty::manager().lifecycle_snapshot();
    assert_eq!(snapshot.state, ProviderState::Failed);
    assert!(snapshot.failure.is_some());
    assert_eq!(
        classify_verified("policies.terminal.enabled", &["policies.spend"]),
        ReloadImpact::Restart
    );
    assert_eq!(
        classify_verified("policies.spend.per_user_usd", &["policies.terminal"]),
        ReloadImpact::Restart
    );
    assert!(pty::attach_event_bus(bus.clone()).unwrap() > first);
    assert_eq!(
        apply_live_sections(&disabled, &["policies.terminal"], Some(&bus), None).await,
        vec!["policies.terminal"]
    );
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
#[serial_test::serial(spend_global_policy, search_live_handle, subagent_concurrency_cap)]
async fn patch_rollback_and_failed_persistence_observe_source_effects() {
    stop_previous_driver().await;
    let fixture = Fixture::new();
    let mut dry = request(true);
    dry.dry_run = true;
    assert!(fixture
        .patcher
        .apply(dry)
        .await
        .unwrap()
        .live_applied
        .is_empty());
    assert!(pty::try_manager().is_none_or(|manager| manager.ready_generation().is_err()));
    let enabled = fixture.patcher.apply(request(true)).await.unwrap();
    assert!(enabled.live_applied.contains(&"policies.terminal"));
    let generation = pty::manager().ready_generation().unwrap();
    assert!(Arc::ptr_eq(
        &pty::manager().current_bus().unwrap(),
        &fixture.bus
    ));
    let disk = Config::load_from_file(fixture.patcher.config_path()).unwrap();
    assert!(disk.policies.terminal.enabled);
    let saved = std::fs::read(fixture.patcher.config_path()).unwrap();
    let mtime = std::fs::metadata(fixture.patcher.config_path())
        .unwrap()
        .modified()
        .unwrap();
    let backups = fixture.patcher.list_backups().unwrap().len();
    let noop = fixture.patcher.apply(request(true)).await.unwrap();
    assert!(noop.diff.is_empty());
    assert!(noop.live_applied.is_empty());
    assert_eq!(std::fs::read(fixture.patcher.config_path()).unwrap(), saved);
    assert_eq!(
        std::fs::metadata(fixture.patcher.config_path())
            .unwrap()
            .modified()
            .unwrap(),
        mtime
    );
    assert_eq!(fixture.patcher.list_backups().unwrap().len(), backups);
    assert_eq!(pty::manager().ready_generation().unwrap(), generation);
    let preview = fixture.patcher.rollback(None, true).await.unwrap();
    assert!(preview.live_applied.is_empty());
    assert_eq!(pty::manager().ready_generation().unwrap(), generation);
    let rolled = fixture.patcher.rollback(None, false).await.unwrap();
    assert!(rolled.live_applied.contains(&"policies.terminal"));
    assert!(!fixture.config.read().await.policies.terminal.enabled);
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
    assert_eq!(
        pty::manager().lifecycle_snapshot().state,
        ProviderState::Stopped
    );
    // A new offline patcher can persist intent, but cannot recover the bus from the manager.
    let offline = ConfigPatcher::new(
        fixture.config.clone(),
        fixture.patcher.config_path().into(),
        ConfigBackup::new(fixture.dir.path().join("offline"), 10),
    );
    assert!(!offline
        .apply(request(true))
        .await
        .unwrap()
        .live_applied
        .contains(&"policies.terminal"));
    assert!(pty::manager().ready_generation().is_err());
    // A path beneath a regular file produces a real persistence error, even as root.
    let invalid = ConfigPatcher::new(
        fixture.config.clone(),
        fixture.patcher.config_path().join("blocked.toml"),
        ConfigBackup::new(fixture.dir.path().join("failed"), 10),
    )
    .with_event_bus(fixture.bus.clone());
    assert!(invalid.apply(request(false)).await.is_err());
    assert!(fixture.config.read().await.policies.terminal.enabled);
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
    // Rollback must reach persistence too, rather than merely failing backup lookup.
    ConfigBackup::new(fixture.dir.path().join("failed"), 10)
        .create_snapshot(fixture.patcher.config_path())
        .unwrap();
    assert!(invalid.rollback(None, false).await.is_err());
    assert!(fixture.config.read().await.policies.terminal.enabled);
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
}

#[tokio::test]
#[serial_test::serial(pty_global_manager)]
async fn competing_config_writer_waits_for_the_apply_read_guard() {
    stop_previous_driver().await;
    let fixture = Fixture::new();
    fixture.patcher.apply(request(true)).await.unwrap();
    let first = pty::manager().ready_generation().unwrap();
    let (_, release) = pty::manager::hold_test_driver(pty::manager());
    let mut stop = std::pin::pin!(fixture.patcher.apply(request(false)));
    // File/mtime work can yield before the executor is entered.
    let result = std::future::poll_fn(|cx| {
        if let Poll::Ready(result) = stop.as_mut().poll(cx) {
            return Poll::Ready(Some(result));
        }
        if pty::manager().lifecycle_snapshot().state == ProviderState::Stopping {
            return Poll::Ready(None);
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
    assert!(result.is_none(), "patch returned before the held join");
    assert!(
        fixture.config.try_read().is_ok(),
        "ordinary readers remain admitted"
    );
    let mut writer = std::pin::pin!(fixture.config.write());
    assert!(writer
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    release.send(()).unwrap();
    assert!(stop
        .await
        .unwrap()
        .live_applied
        .contains(&"policies.terminal"));
    writer.await.policies.terminal.enabled = true;
    let cfg = fixture.config.read().await;
    assert_eq!(
        apply_live_sections(&cfg, &["policies.terminal"], Some(&fixture.bus), None).await,
        vec!["policies.terminal"]
    );
    assert!(pty::manager().ready_generation().unwrap() > first);
    drop(cfg);
    assert!(fixture
        .patcher
        .apply(request(false))
        .await
        .unwrap()
        .live_applied
        .contains(&"policies.terminal"));
    assert_eq!(pty::manager::test_driver_id(pty::manager()), None);
}
