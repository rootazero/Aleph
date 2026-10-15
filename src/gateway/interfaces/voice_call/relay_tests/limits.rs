//! Coordinator limits exercised at the fake provider/output boundaries.
use super::*;
use crate::gateway::interfaces::voice_call::coordinator::{
    VOICE_CALL_TIMEOUT, VOICE_MAX_TEXT_BYTES, VOICE_RUN_TIMEOUT, VOICE_SUBMIT_TIMEOUT,
};

async fn still_pending(call: &mut std::pin::Pin<Box<impl Future<Output = VoiceCallReport>>>) {
    std::future::poll_fn(|cx| {
        assert!(
            call.as_mut().poll(cx).is_pending(),
            "call closed before its limit"
        );
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn idle_call_deadline_releases_permit_at_ten_minutes() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("answer");
    let mut call = Box::pin(run_public_voice_call(
        admitted,
        wiring(&rig, runner.clone(), registry.clone(), Some(store())),
    ));
    still_pending(&mut call).await;
    assert_eq!(registry.active_count(), 1);
    tokio::time::advance(VOICE_CALL_TIMEOUT - Duration::from_nanos(1)).await;
    still_pending(&mut call).await;
    assert_eq!(registry.active_count(), 1);
    tokio::time::advance(Duration::from_nanos(1)).await;
    let report = call.await;
    assert_eq!(report.termination, VoiceCallTermination::CallTimeout);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert_eq!(runner.calls.load(Ordering::Acquire), 0);
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert!(!lease.is_active());
    assert_eq!(registry.active_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn provider_and_send_deadlines_suppress_late_effects_and_history() {
    for provider_wait in [true, false] {
        let mut rig = Rig::new().await;
        rig.peer.block_send.store(!provider_wait, Ordering::Release);
        let admitted = rig.admit().await;
        let lease = admitted.lease().clone();
        let session = lease.session().clone();
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let registry = PublicRunRegistry::new(4, 1000);
        let (runner, mut requests) = Runner::new("answer");
        runner.block.store(provider_wait, Ordering::Release);
        let mut call = Box::pin(run_public_voice_call(
            admitted,
            wiring(
                &rig,
                runner.clone(),
                registry.clone(),
                Some(history.clone()),
            ),
        ));
        rig.input
            .send(transcript(&session, 1, 1, "question", false))
            .unwrap();
        if provider_wait {
            before_call_closes(&mut call, requests.recv())
                .await
                .unwrap();
        } else {
            before_call_closes(&mut call, rig.entered_submit.recv())
                .await
                .unwrap();
        }
        let limit = if provider_wait {
            VOICE_RUN_TIMEOUT
        } else {
            VOICE_SUBMIT_TIMEOUT
        };
        tokio::time::advance(limit - Duration::from_nanos(1)).await;
        still_pending(&mut call).await;
        assert_eq!(registry.active_count(), 1);
        tokio::time::advance(Duration::from_nanos(1)).await;
        let report = call.await;
        assert_eq!(
            report.termination,
            if provider_wait {
                VoiceCallTermination::RunnerFailed
            } else {
                VoiceCallTermination::SubmissionFailed
            }
        );
        assert_eq!(report.frames_accepted, 0);
        assert_eq!(report.history_appended, 0);
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert!(!lease.is_active());
        assert_eq!(registry.active_count(), 0);
        if provider_wait {
            assert_eq!(runner.drops.load(Ordering::Acquire), 1);
            assert_eq!(runner.late_effects.load(Ordering::Acquire), 0);
            assert!(runner
                .cancelled_token
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_cancelled());
            assert!(!registry.recheck_spend());
        } else {
            assert!(rig
                .peer
                .submit_cancel
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_cancelled());
        }
        runner.gate.notify_one();
        rig.peer.send_gate.notify_one();
        tokio::task::yield_now().await;
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn text_byte_limit_rejects_oversized_input_and_combined_output_before_send() {
    // Two rejection cases and one positive control exactly at the byte ceiling.
    for case in 0..3 {
        let oversized_input = case == 0;
        let exact_limit = case == 2;
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        let session = admitted.lease().session().clone();
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let registry = PublicRunRegistry::new(4, 1000);
        let answer = if oversized_input {
            "answer".to_owned()
        } else {
            "a".repeat(VOICE_MAX_TEXT_BYTES - usize::from(exact_limit))
        };
        let question = if oversized_input {
            "q".repeat(VOICE_MAX_TEXT_BYTES + 1)
        } else {
            "q".to_owned()
        };
        let (runner, _requests) = Runner::new(&answer);
        rig.input
            .send(transcript(&session, 1, 1, &question, false))
            .unwrap();
        let call = run_public_voice_call(
            admitted,
            wiring(
                &rig,
                runner.clone(),
                registry.clone(),
                Some(history.clone()),
            ),
        );
        let report = if exact_limit {
            let (report, ()) = bounded(async {
                tokio::join!(call, async {
                    let output = as_text(rig.accepted.recv().await.unwrap());
                    assert_eq!(question.len() + output.text.len(), VOICE_MAX_TEXT_BYTES);
                    rig.input.send(hangup(&session)).unwrap();
                })
            })
            .await;
            report
        } else {
            bounded(call).await
        };
        assert_eq!(
            report.termination,
            if exact_limit {
                VoiceCallTermination::Hangup
            } else {
                VoiceCallTermination::LimitExceeded
            }
        );
        assert_eq!(
            runner.calls.load(Ordering::Acquire),
            usize::from(!oversized_input)
        );
        assert_eq!(report.frames_accepted, usize::from(exact_limit));
        assert_eq!(report.history_appended, 0);
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert_eq!(
            rig.peer.accepted_log.lock().unwrap().len(),
            usize::from(exact_limit)
        );
        assert_eq!(registry.active_count(), 0);
    }
}
