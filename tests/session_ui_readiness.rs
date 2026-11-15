//! Real aleph-server handler fixture. This target must not be cfg-elided: the
//! server binary has test=false, so importing the source is the executable seam.

#[path = "../src/bin/aleph-server/server_init.rs"]
mod server_init;
#[path = "session_ui_readiness/support.rs"]
mod support;

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use support::ReadinessRig;

#[tokio::test]
async fn real_handler_fixture_preserves_existing_session_owner() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct real-handler rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let key = "agent:main:readiness-a";
            let user = "readiness-user-a";
            let before = rig.precreate(user, key).await.expect("precreate real row");
            assert_eq!(before.owner_user_id.as_deref(), Some(user));
            assert_eq!(before.scope_id.as_deref(), Some("personal:readiness-user-a"));

            rig.release_prepare();
            rig.release_committed_prepare();
            let response = rig.chat_send(user, key).await;
            assert!(response.error.is_none(), "real handler failed: {response:?}");
            assert!(response.result.is_some(), "success must carry a result");
            assert_eq!(response.result.as_ref().unwrap()["session_key"], key);

            // Wait for a real delegated store operation, even when the handler
            // currently returns before its queued engine reaches the store.
            rig.wait_prepare_entered().await;
            rig.wait_metadata_committed().await;
            let after = rig.metadata(key).await.expect("metadata read").expect("persisted row");
            assert_eq!(after.owner_user_id, before.owner_user_id);
            assert_eq!(after.scope_id, before.scope_id);
            let committed = rig.committed_metadata();
            assert!(!committed.is_empty(), "must observe a real successful store call");
            assert!(committed.iter().all(|m| m.owner_user_id == before.owner_user_id && m.scope_id == before.scope_id));
            assert_eq!(rig.external_provider_effects(), 0);
            eprintln!("R1 persisted key={} owner={:?} scope={:?}; local_provider_calls={} (no external provider installed)", after.key, after.owner_user_id, after.scope_id, rig.local_provider_effects());
        }).await.expect("smoke body must finish within 15s");
    }).catch_unwind().await;

    // Never let a body panic/timeout skip asynchronous gate release and drain.
    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("R1 cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("owned fixture runtime and store calls must be drained");
}

#[tokio::test]
async fn chat_response_waits_for_addressed_metadata_commit() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct real-handler rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let key = "agent:main:readiness-chat-causal";
            let user = "readiness-user-chat";
            let rpc = rig.chat_send(user, key);
            tokio::pin!(rpc);

            tokio::select! {
                biased;
                response = &mut rpc => panic!("chat.send succeeded before preparation: {response:?}"),
                _ = rig.wait_prepare_entered() => {}
            }
            assert!(futures::poll!(&mut rpc).is_pending());
            assert_eq!(rig.queue_or_provider_effects(), 0);

            rig.release_prepare();
            tokio::select! {
                biased;
                response = &mut rpc => panic!("chat.send succeeded before metadata commit: {response:?}"),
                _ = rig.wait_metadata_committed() => {}
            }
            let metadata = rig
                .metadata(key)
                .await
                .expect("read committed chat metadata")
                .expect("chat metadata row must exist before response");
            assert_eq!(metadata.owner_user_id.as_deref(), Some(user));
            assert_eq!(metadata.scope_id.as_deref(), Some("personal:readiness-user-chat"));
            let committed = rig.committed_metadata();
            assert!(!committed.is_empty(), "must observe a real successful store call");
            assert!(committed.iter().all(|row| {
                row.owner_user_id.as_deref() == Some(user)
                    && row.scope_id.as_deref() == Some("personal:readiness-user-chat")
            }));

            rig.release_committed_prepare();
            let response = rpc.await;
            assert!(response.error.is_none(), "real chat.send failed: {response:?}");
            let result = response.result.expect("successful chat.send must carry a result");
            assert_eq!(result["session_key"], key);
        })
        .await
        .expect("chat causal test must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("R2 chat cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("chat fixture runtime and store calls must be drained");
}

#[tokio::test]
async fn agent_run_response_waits_for_addressed_metadata_commit() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct real-handler rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let key = "agent:main:readiness-agent";
            let user = "readiness-user-agent";
            let rpc = rig.agent_run(user, key);
            tokio::pin!(rpc);

            tokio::select! {
                biased;
                response = &mut rpc => panic!("agent.run succeeded before preparation: {response:?}"),
                _ = rig.wait_prepare_entered() => {}
            }
            assert!(futures::poll!(&mut rpc).is_pending());
            assert_eq!(rig.queue_or_provider_effects(), 0);

            rig.release_prepare();
            tokio::select! {
                biased;
                response = &mut rpc => panic!("agent.run succeeded before metadata commit: {response:?}"),
                _ = rig.wait_metadata_committed() => {}
            }
            let metadata = rig
                .metadata(key)
                .await
                .expect("read committed agent metadata")
                .expect("agent metadata row must exist before response");
            assert_eq!(metadata.owner_user_id.as_deref(), Some(user));
            assert_eq!(metadata.scope_id.as_deref(), Some("personal:readiness-user-agent"));
            let committed = rig.committed_metadata();
            assert!(!committed.is_empty(), "must observe a real successful store call");
            assert!(committed.iter().all(|row| {
                row.owner_user_id.as_deref() == Some(user)
                    && row.scope_id.as_deref() == Some("personal:readiness-user-agent")
            }));

            rig.release_committed_prepare();
            let response = rpc.await;
            assert!(response.error.is_none(), "real agent.run failed: {response:?}");
            let result = response.result.expect("successful agent.run must carry a result");
            assert_eq!(result["session_key"], key);
        })
        .await
        .expect("agent causal test must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("R2 agent cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("agent fixture runtime and store calls must be drained");
}

#[tokio::test]
async fn store_failure_does_not_spawn_or_reply_success() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct real-handler rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            for (index, (method, user, key)) in [
                (
                    "chat.send",
                    "readiness-failure-chat",
                    "agent:main:failure-chat",
                ),
                (
                    "agent.run",
                    "readiness-failure-agent",
                    "agent:main:failure-agent",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                rig.fail_next_get_or_create("backend-secret-store-failure");
                let response = if method == "chat.send" {
                    rig.chat_send(user, key).await
                } else {
                    rig.agent_run(user, key).await
                };
                rig.wait_prepare_entries(index + 1).await;
                let error = response.error.expect("store failure must return an error");
                assert_eq!(error.code, alephcore::gateway::protocol::INTERNAL_ERROR);
                assert_eq!(error.message, "Session unavailable");
                assert!(!error.message.contains("backend-secret-store-failure"));
                assert!(response.result.is_none());
                assert_eq!(rig.active_store_operations(), 0);
                assert!(rig.metadata(key).await.expect("metadata read").is_none());
                assert_eq!(rig.queue_or_provider_effects(), 0);
            }
        })
        .await
        .expect("store failure test must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("R2-B failure cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("store failure fixture must drain");
}

#[tokio::test]
async fn concurrent_first_speaker_preserves_incumbent() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct real-handler rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let key = "agent:main:readiness-concurrent";
            let first = rig.start_chat_send("readiness-concurrent-a", key);
            let second = rig.start_agent_run("readiness-concurrent-b", key);
            tokio::pin!(first);
            tokio::pin!(second);

            tokio::select! {
                biased;
                response = &mut first => panic!("first speaker returned before both entries: {response:?}"),
                response = &mut second => panic!("second speaker returned before both entries: {response:?}"),
                _ = rig.wait_prepare_entries(2) => {}
            }
            assert!(futures::poll!(&mut first).is_pending());
            assert!(futures::poll!(&mut second).is_pending());

            rig.release_prepare();
            tokio::select! {
                biased;
                response = &mut first => panic!("first speaker returned before both commits: {response:?}"),
                response = &mut second => panic!("second speaker returned before both commits: {response:?}"),
                _ = rig.wait_metadata_committed_count(2) => {}
            }
            assert_eq!(rig.queue_or_provider_effects(), 0);
            let persisted = rig
                .metadata(key)
                .await
                .expect("read incumbent metadata")
                .expect("atomic first speaker must persist one row");
            let owner = persisted.owner_user_id.clone().expect("owner attribution");
            let scope = persisted.scope_id.clone().expect("scope attribution");
            assert!(
                ["readiness-concurrent-a", "readiness-concurrent-b"].contains(&owner.as_str()),
                "unexpected incumbent owner: {owner}"
            );
            assert_eq!(scope, format!("personal:{owner}"));
            assert_eq!(rig.committed_metadata().len(), 2);
            assert!(rig.committed_metadata().iter().all(|row| {
                row.owner_user_id.as_deref() == Some(owner.as_str())
                    && row.scope_id.as_deref() == Some(scope.as_str())
            }));

            rig.close_provider();
            rig.release_committed_prepare();
            rig.wait_provider_entered().await;
            let (first, second) = tokio::join!(first, second);
            let first = first.expect("first concurrent handler response");
            let second = second.expect("second concurrent handler response");
            let success_count = usize::from(first.error.is_none()) + usize::from(second.error.is_none());
            assert_eq!(success_count, 1, "exactly the incumbent caller may succeed");
            let loser = if first.error.is_some() { &first } else { &second };
            assert!(loser.result.is_none());
            assert!(loser.error.is_some(), "loser must be denied safely");
            let winner = if first.error.is_none() { &first } else { &second };
            assert_eq!(winner.result.as_ref().expect("winner result")["session_key"], key);
            let winner_run_id = winner
                .result
                .as_ref()
                .and_then(|result| result["run_id"].as_str())
                .expect("winner response must carry the actual run_id");
            let winner_caller = if first.error.is_none() {
                "readiness-concurrent-a"
            } else {
                "readiness-concurrent-b"
            };
            assert_eq!(winner_caller, owner, "successful caller must own the incumbent row");
            let loser_caller = if owner == "readiness-concurrent-a" {
                "readiness-concurrent-b"
            } else {
                "readiness-concurrent-a"
            };
            let winner_marker = format!("readiness-synthetic-caller:{owner}");
            let loser_marker = format!("readiness-synthetic-caller:{loser_caller}");
            let diagnostic = rig.active_run_diagnostic(winner_run_id).await;
            let observations = rig.provider_observations();
            let observation_shape = observations
                .iter()
                .map(|observation| {
                    (
                        observation.name.as_str(),
                        observation.user_texts.iter().any(|text| text.contains(&winner_marker)),
                        observation.user_texts.iter().any(|text| text.contains(&loser_marker)),
                        observation.user_texts.len(),
                    )
                })
                .collect::<Vec<_>>();
            eprintln!(
                "R2-B active-run diagnostic key={key:?} winner_session_key={:?} winner_run_id={winner_run_id:?} incumbent_owner={owner:?} status={diagnostic} observations={observation_shape:?}",
                winner.result.as_ref().and_then(|result| result["session_key"].as_str()),
            );
            let held_run_id = rig
                .active_run_id(key)
                .expect("winner must hold the real session run while provider is gated");
            assert!(rig.pending_run_ids(key).is_empty(), "winner must not leave a foreign queued run");
            assert_eq!(rig.tool_effects(), 0);
            assert!(!observations.is_empty(), "must observe an actual typed provider request");
            assert!(observations.iter().all(|observation| {
                observation.user_texts.iter().any(|text| text.contains(&winner_marker))
                    && observation.user_texts.iter().all(|text| !text.contains(&loser_marker))
            }), "provider input must attribute every actual request to the winner");

            rig.release_provider();
            let after = rig
                .metadata(key)
                .await
                .expect("read incumbent after responses")
                .expect("incumbent row must remain");
            assert_eq!(after.owner_user_id.as_deref(), Some(owner.as_str()));
            assert_eq!(after.scope_id.as_deref(), Some(scope.as_str()));
            assert_eq!(winner_run_id, held_run_id);
            assert!(rig.pending_run_ids(key).is_empty());
            assert_eq!(rig.tool_effects(), 0);
            let observations = rig.provider_observations();
            assert!(observations.iter().all(|observation| {
                observation.user_texts.iter().any(|text| text.contains(&winner_marker))
                    && observation.user_texts.iter().all(|text| !text.contains(&loser_marker))
            }));
            assert_eq!(rig.external_provider_effects(), 0);
            assert_eq!(rig.local_provider_effects(), observations.len());
            assert!(
                rig.queue_or_provider_effects() >= observations.len(),
                "aggregate is diagnostic only; winner provider evidence is checked by typed observations"
            );
        })
        .await
        .expect("concurrent first-speaker test must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("R2-B concurrency cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("concurrent fixture must drain");
}

#[tokio::test]
async fn preparation_cancel_and_committed_row_response_loss_are_distinct() {
    let cancel_rig = ReadinessRig::new()
        .await
        .expect("construct cancellation rig");
    let loss_rig = ReadinessRig::new()
        .await
        .expect("construct response-loss rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let cancel_key = "agent:main:readiness-cancelled";
            let cancelled = cancel_rig.start_chat_send("readiness-cancel", cancel_key);
            tokio::pin!(cancelled);
            cancel_rig.wait_prepare_entered().await;
            assert!(cancel_rig
                .metadata(cancel_key)
                .await
                .expect("metadata read")
                .is_none());
            cancel_rig
                .cancel_handler(cancel_key)
                .await
                .expect("actual handler cancellation must join");
            assert!(cancelled.await.is_err(), "cancelled handler cannot reply");
            assert!(cancel_rig
                .metadata(cancel_key)
                .await
                .expect("metadata read")
                .is_none());
            assert!(cancel_rig.committed_metadata().is_empty());
            assert_eq!(cancel_rig.active_rpc_preparations(), 0);
            assert_eq!(cancel_rig.active_store_operations(), 0);
            assert_eq!(cancel_rig.queue_or_provider_effects(), 0);

            let loss_key = "agent:main:readiness-response-loss";
            let response_lost = loss_rig.start_agent_run("readiness-response-loss", loss_key);
            drop(response_lost);
            loss_rig.wait_prepare_entered().await;
            loss_rig.release_prepare();
            loss_rig.wait_metadata_committed().await;
            let committed = loss_rig
                .metadata(loss_key)
                .await
                .expect("metadata read")
                .expect("canonical row must exist before post-return gate");
            assert_eq!(
                committed.owner_user_id.as_deref(),
                Some("readiness-response-loss")
            );
            assert_eq!(
                committed.scope_id.as_deref(),
                Some("personal:readiness-response-loss")
            );
            assert_eq!(loss_rig.active_store_operations(), 1);
            assert_eq!(loss_rig.active_rpc_preparations(), 1);
            loss_rig.release_committed_prepare();
            loss_rig
                .join_handler(loss_key)
                .await
                .expect("response-loss handler must settle and join");
            assert_eq!(loss_rig.active_rpc_preparations(), 0);
            let after = loss_rig
                .metadata(loss_key)
                .await
                .expect("metadata read after response loss")
                .expect("caller response loss must not rollback committed row");
            assert_eq!(after.owner_user_id, committed.owner_user_id);
            assert_eq!(after.scope_id, committed.scope_id);
        })
        .await
        .expect("cancellation/response-loss test must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cancel_cleanup = cancel_rig.close().await;
    let loss_cleanup = loss_rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cancel_cleanup {
            eprintln!("R2-B cancellation cleanup also failed: {error:#}");
        }
        if let Err(error) = loss_cleanup {
            eprintln!("R2-B response-loss cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cancel_cleanup.expect("cancellation fixture must drain");
    loss_cleanup.expect("response-loss fixture must drain");
}

#[tokio::test]
async fn later_queue_rejection_keeps_legitimate_row_without_model_effect() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct queue rejection rig");
    let key = "agent:main:readiness-queue-rejected";
    let user = "readiness-queue-rejected";
    rig.configure_busy_queue(1, 1).await;
    let before = rig.precreate(user, key).await.expect("precreate row");
    let leading = alephcore::gateway::busy_queue::register(key, 1, "fixture-leading-reject")
        .expect("valid leading ticket");
    rig.release_prepare();
    rig.release_committed_prepare();
    let response = rig.chat_send(user, key).await;
    let run_id = response.result.as_ref().expect("keyed success")["run_id"]
        .as_str()
        .expect("actual handler run id")
        .to_owned();
    let terminal = rig.wait_run_error(&run_id).await;
    assert_eq!(terminal["params"]["error_code"], "AGENT_BUSY");
    let after = rig
        .metadata(key)
        .await
        .expect("metadata read")
        .expect("row remains");
    assert_eq!(after.owner_user_id, before.owner_user_id);
    assert_eq!(after.scope_id, before.scope_id);
    assert_eq!(rig.local_provider_effects(), 0);
    assert_eq!(rig.tool_effects(), 0);
    assert!(rig.pending_run_ids(key).iter().all(|id| id != &run_id));
    drop(leading);
    assert!(rig.pending_run_ids(key).is_empty());
    rig.close().await.expect("queue rejection fixture drains");
}

#[tokio::test]
async fn later_queue_timeout_keeps_legitimate_row_without_model_effect() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct queue timeout rig");
    let key = "agent:main:readiness-queue-timeout";
    let user = "readiness-queue-timeout";
    rig.configure_busy_queue(2, 1).await;
    let before = rig.precreate(user, key).await.expect("precreate row");
    let leading = alephcore::gateway::busy_queue::register(key, 2, "fixture-leading-timeout")
        .expect("valid leading ticket");
    rig.release_prepare();
    rig.release_committed_prepare();
    let response = rig.agent_run(user, key).await;
    let run_id = response.result.as_ref().expect("keyed success")["run_id"]
        .as_str()
        .expect("actual handler run id")
        .to_owned();
    rig.wait_run_queued(&run_id).await;
    let terminal = rig.wait_run_error(&run_id).await;
    assert_eq!(terminal["params"]["error_code"], "AGENT_BUSY");
    let after = rig
        .metadata(key)
        .await
        .expect("metadata read")
        .expect("row remains");
    assert_eq!(after.owner_user_id, before.owner_user_id);
    assert_eq!(after.scope_id, before.scope_id);
    assert_eq!(rig.local_provider_effects(), 0);
    assert_eq!(rig.tool_effects(), 0);
    drop(leading);
    assert!(rig.pending_run_ids(key).is_empty());
    rig.close().await.expect("queue timeout fixture drains");
}

#[tokio::test]
async fn btw_prepares_response_key_without_replacing_execution_key() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct /btw handler rig");
    let main_key = "agent:main:readiness-btw-main";
    let user = "readiness-btw-user";
    let main = rig
        .precreate(user, main_key)
        .await
        .expect("precreate main row");
    rig.close_provider();
    rig.release_prepare();
    rig.release_committed_prepare();
    let response = rig
        .start_chat_send_with_input(user, main_key, "/btw explain the current task")
        .await
        .expect("real /btw handler response");
    assert!(response.error.is_none(), "real /btw failed: {response:?}");
    let result = response.result.expect("/btw success must carry a result");
    assert_eq!(result["session_key"], main_key);
    let run_id = result["run_id"].as_str().expect("response run id");
    rig.wait_metadata_committed_count(2).await;
    let main_session = alephcore::gateway::router::SessionKey::from_key_string(main_key)
        .expect("main session key");
    let side_key = alephcore::gateway::btw::side_key_for(&main_session).to_key_string();
    rig.track_key(&side_key);
    let side = rig
        .metadata(&side_key)
        .await
        .expect("read side metadata")
        .expect("real /btw side row");
    assert_eq!(side.owner_user_id.as_deref(), main.owner_user_id.as_deref());
    assert_eq!(side.scope_id.as_deref(), main.scope_id.as_deref());
    rig.wait_provider_entered().await;
    assert_eq!(rig.active_run_id(&side_key).as_deref(), Some(run_id));
    assert_ne!(rig.active_run_id(main_key).as_deref(), Some(run_id));
    let observations = rig.provider_observations();
    assert!(
        !observations.is_empty(),
        "must observe the real side provider request"
    );
    assert!(observations.iter().any(|observation| {
        observation
            .user_texts
            .iter()
            .any(|text| text.contains("/btw explain the current task"))
    }));
    rig.release_provider();
    rig.close().await.expect("/btw fixture drains");
}

#[tokio::test]
async fn btw_initial_side_preparation_failure_reports_to_main_owner() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct /btw preparation-failure rig");
    let outcome = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), async {
            let main_key = "agent:main:readiness-btw-preparation-failure";
            let user = "readiness-btw-preparation-failure";
            let main = rig
                .precreate(user, main_key)
                .await
                .expect("precreate attributed main row");
            assert_eq!(main.owner_user_id.as_deref(), Some(user));
            assert_eq!(main.scope_id.as_deref(), Some("personal:readiness-btw-preparation-failure"));

            let rpc = rig.start_chat_send_with_input(user, main_key, "/btw induce preparation failure");
            tokio::pin!(rpc);
            rig.wait_prepare_entered().await;
            rig.release_prepare();
            rig.wait_metadata_committed().await;
            let committed_main = rig
                .metadata(main_key)
                .await
                .expect("read committed main metadata")
                .expect("main row must remain present");
            assert_eq!(committed_main.owner_user_id, main.owner_user_id);
            assert_eq!(committed_main.scope_id, main.scope_id);

            // The first canonical get_or_create has committed. The next call is
            // the side-session preparation and is the one forced to fail.
            rig.fail_next_get_or_create("side-session-preparation-failure");
            rig.release_committed_prepare();
            let response = rpc
                .await
                .expect("real /btw handler must finish or fixture must report failure");
            assert!(response.error.is_none(), "real /btw handler failed: {response:?}");
            let result = response.result.expect("/btw failure still returns keyed success");
            assert_eq!(result["session_key"], main_key);
            let run_id = result["run_id"].as_str().expect("keyed response run id");

            let main_session = alephcore::gateway::router::SessionKey::from_key_string(main_key)
                .expect("main session key");
            let side_key = alephcore::gateway::btw::side_key_for(&main_session).to_key_string();
            rig.track_key(&side_key);
            let terminal = rig.wait_visible_run_error(run_id, user).await;
            assert_eq!(terminal["params"]["session_key"], main_key);
            assert_eq!(terminal["params"]["run_id"], run_id);
            assert_eq!(
                terminal["params"]["error_code"],
                "FAILED",
                "preparation failure must remain a sanitized terminal error"
            );
            assert!(
                rig.metadata(&side_key)
                    .await
                    .expect("read side metadata")
                    .is_none(),
                "failed side preparation must not create the side row"
            );
            assert_eq!(rig.local_provider_effects(), 0);
            assert_eq!(rig.external_provider_effects(), 0);
            assert_eq!(rig.tool_effects(), 0);
            assert!(rig.pending_run_ids(main_key).is_empty());
            assert!(rig.pending_run_ids(&side_key).is_empty());
            rig.join_handler(main_key)
                .await
                .expect("real /btw handler must settle");
            assert_eq!(rig.active_store_operations(), 0);
        })
        .await
        .expect("/btw preparation-failure test must finish within 15s");
    })
    .catch_unwind()
    .await;

    let cleanup = rig.close().await;
    if let Err(panic) = outcome {
        if let Err(error) = cleanup {
            eprintln!("/btw preparation-failure cleanup also failed: {error:#}");
        }
        std::panic::resume_unwind(panic);
    }
    cleanup.expect("/btw preparation-failure fixture must drain");
}

#[tokio::test]
async fn later_queue_purge_keeps_legitimate_row_without_model_effect() {
    let rig = ReadinessRig::new()
        .await
        .expect("construct queue purge rig");
    let key = "agent:main:readiness-queue-purged";
    let user = "readiness-queue-purged";
    rig.configure_busy_queue(2, 30).await;
    let before = rig.precreate(user, key).await.expect("precreate row");
    let leading = alephcore::gateway::busy_queue::register(key, 2, "fixture-leading-purge")
        .expect("valid leading ticket");
    rig.release_prepare();
    rig.release_committed_prepare();
    let response = rig.chat_send(user, key).await;
    let run_id = response.result.as_ref().expect("keyed success")["run_id"]
        .as_str()
        .expect("actual handler run id")
        .to_owned();
    rig.wait_run_queued(&run_id).await;
    assert_eq!(alephcore::gateway::busy_queue::purge(key), 2);
    let terminal = rig.wait_run_error(&run_id).await;
    assert_eq!(terminal["params"]["error_code"], "CANCELLED");
    let after = rig
        .metadata(key)
        .await
        .expect("metadata read")
        .expect("row remains");
    assert_eq!(after.owner_user_id, before.owner_user_id);
    assert_eq!(after.scope_id, before.scope_id);
    assert_eq!(rig.local_provider_effects(), 0);
    assert_eq!(rig.tool_effects(), 0);
    drop(leading);
    assert!(rig.pending_run_ids(key).is_empty());
    rig.close().await.expect("queue purge fixture drains");
}
