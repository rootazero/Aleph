use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_protocol::browser_live::*;

use super::*;

fn fixture() -> (
    Arc<TargetControl>,
    ViewerLease,
    Arc<Mutex<Vec<InterventionSummary>>>,
) {
    let summaries = Arc::new(Mutex::new(Vec::new()));
    let saved = summaries.clone();
    let control = TargetControl::new(
        BrowserLiveState {
            resource_id: "resource".into(),
            binding: ViewerBinding {
                principal: IssuerIdentity::User {
                    user_id: "user".into(),
                },
                task: ConversationBinding {
                    session_key: "conversation".into(),
                    run_id: "run".into(),
                },
                target: BrowserTargetBinding {
                    profile: "profile".into(),
                    browser_session_id: "browser-session".into(),
                    target_id: "T1".into(),
                },
            },
            connection_epoch: 1,
            stream_epoch: 2,
            viewport_css_px: ViewportCssPx {
                width: 800,
                height: 600,
            },
            controller: Controller::Agent,
            control_epoch: 0,
        },
        Arc::new(move |summary| {
            saved.lock().unwrap().push(summary);
            Ok(())
        }),
    );
    let lease = control
        .register_viewer(
            "viewer",
            3,
            i64::MAX,
            vec![ViewerPermission::Frames, ViewerPermission::HumanInput],
        )
        .unwrap();
    (control, lease, summaries)
}

fn request(control: &TargetControl, seq: u64) -> ControlRequest {
    ControlRequest {
        expected_control_epoch: control.snapshot().control_epoch,
        action_seq: seq,
        expires_at_ms: i64::MAX,
    }
}
fn input(control: &TargetControl, seq: u64) -> HumanInputRequest {
    let state = control.snapshot();
    HumanInputRequest {
        connection_epoch: state.connection_epoch,
        stream_epoch: state.stream_epoch,
        control_epoch: state.control_epoch,
        seq,
        expires_at_ms: i64::MAX,
        event: HumanInputEvent::Text {
            text: "text".into(),
        },
    }
}
async fn take(control: &TargetControl, lease: &ViewerLease, seq: u64) {
    assert!(control.take(lease, request(control, seq)).await.accepted);
}

#[tokio::test]
async fn all_agent_mutations_hold_bound_permit() {
    let (control, lease, _) = fixture();
    let permit = control.agent_permit().await.unwrap();
    let take_request = request(&control, 1);
    let taking = tokio::spawn({
        let c = control.clone();
        let l = lease.clone();
        async move { c.take(&l, take_request).await }
    });
    tokio::task::yield_now().await;
    assert!(
        !taking.is_finished(),
        "Take cannot pass a held mutation permit"
    );
    drop(permit);
    assert!(taking.await.unwrap().accepted);
    assert!(matches!(
        control.agent_permit().await,
        Err(BrowserError::HumanHasControl { .. })
    ));
}

#[tokio::test]
async fn stale_input_is_never_replayed() {
    let (control, lease, summaries) = fixture();
    take(&control, &lease, 1).await;
    let old = input(&control, 1);
    let queued = control.queue_input(&lease, old.clone()).unwrap();
    assert!(control.cede(&lease, request(&control, 2)).await.accepted);
    assert!(!control.cede(&lease, request(&control, 2)).await.accepted);
    assert_eq!(summaries.lock().unwrap().len(), 1);
    assert_eq!(
        summaries.lock().unwrap()[0].destination.session_key,
        "conversation"
    );
    take(&control, &lease, 3).await;
    assert_eq!(
        control.queue_input(&lease, old).err(),
        Some(InputRejectReason::StaleControl)
    );
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = starts.clone();
    let result = control
        .dispatch(queued, move |_| async move {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
    assert!(!result.accepted);
    assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_is_64_and_cancel_releases_slot_without_replaying_seq() {
    let (control, lease, _) = fixture();
    take(&control, &lease, 1).await;
    let mut queued = Vec::new();
    for seq in 1..=64 {
        queued.push(control.queue_input(&lease, input(&control, seq)).unwrap());
    }
    assert_eq!(
        control.queue_input(&lease, input(&control, 65)).err(),
        Some(InputRejectReason::DispatchBusy)
    );
    queued.pop();
    assert_eq!(
        control.queue_input(&lease, input(&control, 64)).err(),
        Some(InputRejectReason::ReplayedSeq)
    );
    assert!(control.queue_input(&lease, input(&control, 65)).is_ok());
}

#[tokio::test]
async fn dispatch_rechecks_every_axis_and_does_not_start_after_revoke() {
    for axis in [
        "connection",
        "stream",
        "control",
        "issuer",
        "run",
        "resource",
        "target",
        "ws",
        "expiry",
        "permission",
        "controller",
        "browser-session",
        "conversation",
        "revoke",
        "destroy",
    ] {
        let (control, lease, _) = fixture();
        take(&control, &lease, 1).await;
        let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
        {
            let mut state = control.lock();
            match axis {
                "connection" => state.live.connection_epoch += 1,
                "stream" => state.live.stream_epoch += 1,
                "control" => state.live.control_epoch += 1,
                "issuer" => {
                    state.live.binding.principal = IssuerIdentity::Connection {
                        connection_id: "user".into(),
                    }
                }
                "run" => state.live.binding.task.run_id = "other".into(),
                "resource" => state.live.resource_id = "other".into(),
                "target" => state.live.binding.target.target_id = "T2".into(),
                "ws" => state.viewers.get_mut("viewer").unwrap().ws_generation += 1,
                "expiry" => state.viewers.get_mut("viewer").unwrap().expires_at_ms = 0,
                "permission" => state.viewers.get_mut("viewer").unwrap().permissions.clear(),
                "controller" => state.live.controller = Controller::Agent,
                "browser-session" => state.live.binding.target.browser_session_id = "other".into(),
                "conversation" => state.live.binding.task.session_key = "other".into(),
                "revoke" => state.viewers.get_mut("viewer").unwrap().revoked = true,
                "destroy" => state.alive = false,
                _ => unreachable!(),
            }
        }
        let result = control
            .dispatch(queued, |_| async {
                panic!("post-bump CDP dispatch started");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap();
        assert!(!result.accepted, "axis {axis}");
    }
}

#[tokio::test]
async fn cede_keeps_frames_but_whole_revoke_closes_lease_and_purges() {
    let (control, lease, summaries) = fixture();
    take(&control, &lease, 1).await;
    let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
    assert!(control.cede(&lease, request(&control, 2)).await.accepted);
    assert!(!lease.closed());
    assert_eq!(control.lock().viewers["viewer"].ws_generation, 3);
    assert!(control.lock().viewers["viewer"]
        .permissions
        .contains(&ViewerPermission::Frames));
    assert_eq!(control.pending_count(), 0);
    drop(queued);
    take(&control, &lease, 3).await;
    control.revoke(&lease, InterventionEnd::Revoked).unwrap();
    assert!(lease.closed());
    assert_eq!(control.lock().viewers["viewer"].ws_generation, 4);
    assert_eq!(control.pending_count(), 0);
    assert_eq!(summaries.lock().unwrap().len(), 2);
    assert!(!control.take(&lease, request(&control, 4)).await.accepted);
}

#[tokio::test]
async fn dispatch_busy_at_250ms_and_cancelled_dispatch_has_no_effect() {
    let (control, lease, _) = fixture();
    take(&control, &lease, 1).await;
    let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
    tokio::time::sleep(Duration::from_millis(251)).await;
    let result = control
        .dispatch(queued, |_| async {
            panic!("expired queue started");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(result.reason, Some(InputRejectReason::DispatchBusy));
    let held = control.serial.clone().lock_owned().await;
    let queued = control.queue_input(&lease, input(&control, 2)).unwrap();
    let c = control.clone();
    let waiting = tokio::spawn(async move {
        c.dispatch(queued, |_| async {
            panic!("cancelled dispatch started");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await
    });
    tokio::task::yield_now().await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    drop(held);
    assert_eq!(control.pending_count(), 0);
    assert_eq!(
        control.queue_input(&lease, input(&control, 2)).err(),
        Some(InputRejectReason::ReplayedSeq)
    );
}

#[tokio::test]
async fn backend_mutation_entry_table_refuses_human_before_any_cdp_write() {
    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::{backend_with, open_guard, wire_session};
    use crate::browser::engine::Engine;
    use crate::browser::manager::ProfileManager;
    use crate::browser::profile::BrowserSystemConfig;
    use crate::browser::types::{ActionTarget, HistoryNav};
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
    wire_session(&server, "S1");
    server.on("Page.startScreencast", Responder::Reply(json!({})));
    let (registry, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
    let handle = backend.handle().await.unwrap();
    handle.tabs.lock().await.entries.insert(
        "T1".into(),
        crate::browser::engine::TabEntry::new(aleph_cdp::SessionId("S1".into())),
    );
    let owner = handle.live_owner();
    let binding = ViewerBinding {
        principal: IssuerIdentity::User {
            user_id: "user".into(),
        },
        task: ConversationBinding {
            session_key: "conversation".into(),
            run_id: "run".into(),
        },
        target: BrowserTargetBinding {
            profile: "default".into(),
            browser_session_id: owner.browser_session_id().into(),
            target_id: "T1".into(),
        },
    };
    owner.attach_target(binding.target.clone()).await.unwrap();
    let control = owner
        .bind_control(
            "resource",
            binding.clone(),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
    let lease = control
        .register_viewer("viewer", 1, i64::MAX, vec![ViewerPermission::HumanInput])
        .unwrap();
    take(&control, &lease, 1).await;
    let baseline = server.received().len();
    let state_dir = tempfile::tempdir().unwrap();
    let state_path = state_dir.path().join("state.json");
    for name in [
        "open",
        "navigate",
        "click",
        "type",
        "close",
        "history",
        "evaluate",
        "engine-switch",
        "batch-sub-operations",
        "save-state",
        "load-state",
    ] {
        let target = ActionTarget::Coordinates { x: 10.0, y: 10.0 };
        let result = match name {
            "open" => backend.open_tab("https://example.com").await.map(|_| ()),
            "navigate" => backend.navigate("T1", "https://example.com").await,
            "click" => backend.click("T1", target).await,
            "type" => backend.type_text("T1", target, "test").await,
            "close" => backend.close_tab("T1").await,
            "history" => backend.history("T1", HistoryNav::Back).await,
            "evaluate" => backend.evaluate("T1", "document.title").await.map(|_| ()),
            "engine-switch" => ProfileManager::with_engine_registry(
                BrowserSystemConfig::default(),
                registry.clone(),
            )
            .switch_engine("default", Engine::Obscura, false)
            .await
            .map(|_| ()),
            "batch-sub-operations" => {
                let first = backend.click("T1", target).await;
                assert!(matches!(first, Err(BrowserError::HumanHasControl { .. })));
                backend.press_key("T1", "Enter").await
            }
            // Runs page-main-world script (href + localStorage dump): gated
            // profile-wide like the cookies read form. Refusal writes no file.
            "save-state" => backend.save_state(&state_path).await,
            "load-state" => backend.load_state(&state_path).await,
            _ => unreachable!(),
        };
        assert!(
            matches!(result, Err(BrowserError::HumanHasControl { binding: b }) if *b == binding),
            "entry {name}"
        );
    }
    assert_eq!(
        server.received().len(),
        baseline,
        "HumanHasControl must be returned before CDP mutation, not after"
    );
    assert!(!state_path.exists(), "a refused save_state wrote no file");
    assert_eq!(server.connection_count(), 1);
}

#[tokio::test]
async fn dispatch_permit_spans_entire_backend_future_and_summary_is_factual() {
    let (control, lease, summaries) = fixture();
    take(&control, &lease, 1).await;
    let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let c = control.clone();
    let dispatching = tokio::spawn(async move {
        c.dispatch(queued, |_| async move {
            started_tx.send(()).unwrap();
            finish_rx.await.unwrap();
            Ok(())
        })
        .await
    });
    started_rx.await.unwrap();
    let c = control.clone();
    let l = lease.clone();
    let req = request(&control, 2);
    let ceding = tokio::spawn(async move { c.cede(&l, req).await });
    tokio::task::yield_now().await;
    assert!(!ceding.is_finished());
    finish_tx.send(()).unwrap();
    assert!(dispatching.await.unwrap().unwrap().accepted);
    assert!(ceding.await.unwrap().accepted);
    let saved = summaries.lock().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].facts.input_event_count, 1);
    assert_eq!(saved[0].facts.typed_text_chars, 4);
    assert!(saved[0].facts.final_origin.is_none());
    assert_eq!(saved[0].destination.run_id, "run");
}

#[tokio::test]
async fn permission_loss_retains_frames_and_failed_summary_closes_control() {
    let (control, lease, summaries) = fixture();
    take(&control, &lease, 1).await;
    control.revoke_human_permission(&lease).unwrap();
    assert!(!lease.closed());
    assert_eq!(
        control.lock().viewers["viewer"].permissions,
        vec![ViewerPermission::Frames]
    );
    assert_eq!(summaries.lock().unwrap().len(), 1);
    assert_eq!(
        control.take(&lease, request(&control, 2)).await.reason,
        Some(InputRejectReason::PermissionDenied)
    );
    let mut live = control.snapshot();
    live.controller = Controller::Agent;
    let control = TargetControl::new(
        live,
        Arc::new(|_| {
            Err(BrowserError::ActionFailed(
                "test destination unavailable".into(),
            ))
        }),
    );
    let lease = control
        .register_viewer("viewer", 1, i64::MAX, vec![ViewerPermission::HumanInput])
        .unwrap();
    take(&control, &lease, 1).await;
    assert!(!control.cede(&lease, request(&control, 2)).await.accepted);
    assert!(lease.closed());
    assert!(control.agent_permit().await.is_err());
}

#[tokio::test]
async fn out_of_order_dispatch_cannot_apply_an_older_sequence_after_a_newer_one() {
    let (control, lease, _) = fixture();
    take(&control, &lease, 1).await;
    let older = control.queue_input(&lease, input(&control, 1)).unwrap();
    let newer = control.queue_input(&lease, input(&control, 2)).unwrap();
    assert!(
        control
            .dispatch(newer, |_| async { Ok(()) })
            .await
            .unwrap()
            .accepted
    );
    assert_eq!(
        control
            .dispatch(older, |_| async {
                panic!("older sequence dispatched after newer input");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap()
            .reason,
        Some(InputRejectReason::ReplayedSeq)
    );
}

#[tokio::test]
async fn expiry_closes_without_waiting_for_another_input_and_errors_never_report_success() {
    let (control, _, summaries) = fixture();
    let lease = control
        .register_viewer(
            "short",
            1,
            now_ms() + 80,
            vec![ViewerPermission::HumanInput],
        )
        .unwrap();
    take(&control, &lease, 1).await;
    let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
    let mut closed = lease.close_signal();
    tokio::time::timeout(Duration::from_secs(1), closed.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(lease.closed());
    assert_eq!(control.pending_count(), 0);
    assert_eq!(summaries.lock().unwrap().len(), 1);
    assert_eq!(
        summaries.lock().unwrap()[0].ended_by,
        InterventionEnd::Expired
    );
    assert!(
        !control
            .dispatch(queued, |_| async {
                panic!("expired lease dispatched");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap()
            .accepted
    );
    let (control, lease, _) = fixture();
    take(&control, &lease, 1).await;
    let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
    assert!(
        matches!(control.dispatch(queued, |_| async { Err(BrowserError::ActionFailed("CDP send failed".into())) }).await, Err(BrowserError::ActionFailed(message)) if message == "CDP send failed")
    );
}

#[tokio::test]
async fn stale_agent_waiter_cannot_write_after_handoff_and_destroy_ends_control_once() {
    let (control, lease, summaries) = fixture();
    let held = control.serial.clone().lock_owned().await;
    let c = control.clone();
    let waiter = tokio::spawn(async move { c.agent_permit().await });
    tokio::task::yield_now().await;
    // This models lifecycle invalidation while the operation is queued.
    control.invalidate(InterventionEnd::TargetClosed).unwrap();
    drop(held);
    assert!(waiter.await.unwrap().is_err());
    assert!(lease.closed());
    assert!(summaries.lock().unwrap().is_empty());
    let (control, lease, summaries) = fixture();
    take(&control, &lease, 1).await;
    control.invalidate(InterventionEnd::TargetClosed).unwrap();
    control.invalidate(InterventionEnd::TargetClosed).unwrap();
    assert_eq!(summaries.lock().unwrap().len(), 1);
    assert_eq!(
        summaries.lock().unwrap()[0].ended_by,
        InterventionEnd::TargetClosed
    );
}

type LiveOwner = Arc<crate::browser::live::LiveProfileOwner>;

/// One CDP backend with a live owner, T1 attached and bound to `binding("T1")`.
/// The fake answers `Runtime.evaluate`, so an admitted `evaluate` is a
/// visible CDP write and a refused one is visibly absent.
async fn live_backend() -> (
    aleph_cdp::testkit::FakeCdpServer,
    crate::browser::cdp_backend::CdpBackend,
    LiveOwner,
    Arc<TargetControl>,
    ViewerBinding,
) {
    use crate::browser::cdp_backend::test_support::{backend_with, open_guard, wire_session};
    use crate::browser::engine::Engine;
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
    wire_session(&server, "S1");
    server.on("Page.startScreencast", Responder::Reply(json!({})));
    server.on(
        "Runtime.evaluate",
        Responder::Reply(json!({ "result": { "type": "number", "value": 1 } })),
    );
    let (_registry, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
    let handle = backend.handle().await.unwrap();
    for (target, session) in [("T1", "S1"), ("T2", "S2")] {
        handle.tabs.lock().await.entries.insert(
            target.into(),
            crate::browser::engine::TabEntry::new(aleph_cdp::SessionId(session.into())),
        );
    }
    let owner = handle.live_owner();
    let binding = owner_binding(&owner, "user", "run", "T1");
    owner.attach_target(binding.target.clone()).await.unwrap();
    let control = owner
        .bind_control(
            "resource",
            binding.clone(),
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
    (server, backend, owner, control, binding)
}

fn owner_binding(owner: &LiveOwner, user: &str, run: &str, target: &str) -> ViewerBinding {
    ViewerBinding {
        principal: IssuerIdentity::User {
            user_id: user.into(),
        },
        task: ConversationBinding {
            session_key: format!("conversation-{run}"),
            run_id: run.into(),
        },
        target: BrowserTargetBinding {
            profile: "default".into(),
            browser_session_id: owner.browser_session_id().into(),
            target_id: target.into(),
        },
    }
}

/// Negative binding: a human holding T1 (issuer `user`, run `run`, resource
/// `resource`) never blocks a different target bound to a different issuer,
/// run and resource; and that other binding's lease cannot take, cede or
/// revoke T1's control. Effects are asserted on the wire and on the state.
#[tokio::test]
async fn other_issuer_run_resource_target_neither_blocks_nor_satisfies_the_guard() {
    use crate::browser::backend::BrowserBackend;

    let (server, backend, owner, control, binding) = live_backend().await;
    let lease = control
        .register_viewer("viewer", 1, i64::MAX, vec![ViewerPermission::HumanInput])
        .unwrap();
    take(&control, &lease, 1).await;

    let other_binding = owner_binding(&owner, "other-user", "other-run", "T2");
    owner
        .attach_target(other_binding.target.clone())
        .await
        .unwrap();
    let other = owner
        .bind_control(
            "other-resource",
            other_binding,
            ViewportCssPx {
                width: 800,
                height: 600,
            },
            Arc::new(|_| Ok(())),
        )
        .await
        .unwrap();
    let other_lease = other
        .register_viewer(
            "other-viewer",
            1,
            i64::MAX,
            vec![ViewerPermission::HumanInput],
        )
        .unwrap();

    // The other target's agent mutation is admitted and reaches the wire.
    let before = server.received_for("Runtime.evaluate").len();
    assert!(backend.evaluate("T2", "1").await.is_ok());
    let admitted = server.received_for("Runtime.evaluate").len();
    assert!(
        admitted > before,
        "the unblocked target's write reached CDP"
    );
    // T1 stays refused with ITS binding, and nothing more reaches the wire.
    assert!(matches!(
        backend.evaluate("T1", "1").await,
        Err(BrowserError::HumanHasControl { binding: b }) if *b == binding
    ));
    assert_eq!(server.received_for("Runtime.evaluate").len(), admitted);

    // The foreign lease cannot act on T1's control: no state change at all.
    let epoch = control.snapshot().control_epoch;
    let foreign = ControlRequest {
        expected_control_epoch: epoch,
        action_seq: 9,
        expires_at_ms: i64::MAX,
    };
    assert_eq!(
        control.cede(&other_lease, foreign.clone()).await.reason,
        Some(InputRejectReason::Invalid)
    );
    assert_eq!(
        control.take(&other_lease, foreign).await.reason,
        Some(InputRejectReason::Invalid)
    );
    assert!(control
        .revoke(&other_lease, InterventionEnd::Revoked)
        .is_err());
    let after = control.snapshot();
    assert_eq!(after.control_epoch, epoch);
    assert!(matches!(after.controller, Controller::Human { .. }));
    assert!(!lease.closed());
    assert!(!other_lease.closed());
}

/// Take wins the serial queue after the agent read its epoch: the agent's
/// permit is refused and its backend future never writes to CDP.
#[tokio::test]
async fn take_queued_ahead_of_agent_permit_leaves_no_cdp_write() {
    use crate::browser::backend::BrowserBackend;

    let (server, backend, _owner, control, binding) = live_backend().await;
    let lease = control
        .register_viewer("viewer", 1, i64::MAX, vec![ViewerPermission::HumanInput])
        .unwrap();
    let before = server.received_for("Runtime.evaluate").len();
    let epoch = control.snapshot().control_epoch;
    // Hold the serial so both contenders queue (tokio Mutex is FIFO).
    let held = control.serial.clone().lock_owned().await;
    let taking = tokio::spawn({
        let c = control.clone();
        let l = lease.clone();
        let req = request(&control, 1);
        async move { c.take(&l, req).await }
    });
    tokio::task::yield_now().await;
    let agent = tokio::spawn(async move { backend.evaluate("T1", "1").await });
    tokio::task::yield_now().await;
    assert!(!taking.is_finished() && !agent.is_finished());
    drop(held);
    assert!(taking.await.unwrap().accepted);
    assert!(matches!(
        agent.await.unwrap(),
        Err(BrowserError::HumanHasControl { binding: b }) if *b == binding
    ));
    assert!(control.snapshot().control_epoch > epoch);
    assert_eq!(
        server.received_for("Runtime.evaluate").len(),
        before,
        "no CDP write after the control epoch bump"
    );
}

/// Dispatcher vs lifecycle: a queued input never starts after a whole-grant
/// revoke or a target close, and each intervention yields exactly one
/// InterventionSummary whatever ends it, even when ends race.
#[tokio::test]
async fn queued_input_vs_revoke_or_target_close_never_starts_and_summarizes_once() {
    for end in ["revoke", "target-close", "cede-then-revoke"] {
        let (control, lease, summaries) = fixture();
        take(&control, &lease, 1).await;
        let queued = control.queue_input(&lease, input(&control, 1)).unwrap();
        let epoch = control.snapshot().control_epoch;
        let ws = control.lock().viewers["viewer"].ws_generation;
        let expected = match end {
            "revoke" => {
                control.revoke(&lease, InterventionEnd::Revoked).unwrap();
                // A second end of the same intervention adds nothing.
                control.invalidate(InterventionEnd::TargetClosed).unwrap();
                InterventionEnd::Revoked
            }
            "target-close" => {
                control.invalidate(InterventionEnd::TargetClosed).unwrap();
                control.revoke(&lease, InterventionEnd::Revoked).unwrap();
                InterventionEnd::TargetClosed
            }
            "cede-then-revoke" => {
                assert!(control.cede(&lease, request(&control, 2)).await.accepted);
                control.revoke(&lease, InterventionEnd::Revoked).unwrap();
                InterventionEnd::Cede
            }
            _ => unreachable!(),
        };
        let result = control
            .dispatch(queued, |_| async {
                panic!("queued input dispatched after its intervention ended");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await
            .unwrap();
        assert!(!result.accepted, "{end}");
        assert!(lease.closed(), "{end}: whole-grant end closes the viewer");
        assert!(control.snapshot().control_epoch > epoch, "{end}");
        assert!(control.lock().viewers["viewer"].ws_generation > ws, "{end}");
        let saved = summaries.lock().unwrap();
        assert_eq!(saved.len(), 1, "{end}: exactly one summary");
        assert_eq!(saved[0].ended_by, expected, "{end}");
        assert_eq!(saved[0].destination.run_id, "run");
        assert_eq!(saved[0].destination.session_key, "conversation");
    }
}
