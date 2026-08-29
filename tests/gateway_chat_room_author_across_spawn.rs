//! P2 speaker label: the author survives `Orchestrator::dispatch`'s spawn.
//!
//! This test exists because the first attempt at C2 was green everywhere and
//! wrong in production. The author task-local was seeded in
//! `run_loop::with_request_scope`, but `Orchestrator::dispatch` spawns the
//! harness onto a fresh task, and task-locals do not cross `tokio::spawn` —
//! and the main path's user-message writer (`harness_bridge::session_seed`)
//! runs on the far side. Every unit test nested the scope and the author in
//! ONE task, a sequence production never presents, so all of them passed while
//! every room message was labelled with the session's creator.
//!
//! So the assertion here is deliberately made from inside the spawned task,
//! through the real `Orchestrator::dispatch`, with the real
//! `run_dispatch_and_drain` entry point. A same-task nesting of these
//! task-locals proves nothing about this path.

#[path = "gateway_chat_common/mod.rs"]
mod common;

use std::sync::Arc;

use alephcore::gateway::event_emitter::{CollectingEventEmitter, EventEmitter};
use alephcore::gateway::execution_engine::helpers::run_dispatch_and_drain;
use alephcore::gateway::i18n::Locale;
use alephcore::orchestrator::FlowOutcome;
use alephcore::scope::{ScopeAttribution, ScopeId};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use common::{basic_request, orchestrator_with_stub, StubHarnessRunner};

/// Bob types in a room Alice created. The label must say Bob.
///
/// The two facts are deliberately different, which is the whole point: every
/// run in a room carries the ROOM's attribution (that is what puts both
/// members' memory in one partition), so a label derived from the scope names
/// Alice on Bob's message. `owner_user_id` on the request is Alice because
/// `resolve_attribution` path 1 rebuilds it from the session row; the speaker
/// rides the task-local the gateway seeds from `AUTHOR_USER_KEY`.
#[tokio::test]
async fn the_speaker_survives_the_dispatch_spawn() {
    let seen: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
    let seen_clone = seen.clone();

    let runner = StubHarnessRunner::new(Arc::new(move |ctx| {
        let seen = seen_clone.clone();
        Box::pin(async move {
            // Runs INSIDE `Orchestrator::dispatch`'s `tokio::spawn`, which is
            // exactly where `session_seed` calls this.
            let author = alephcore::scope::ambient_room_author();
            *seen.lock().await = Some(author);

            let outcome = FlowOutcome {
                final_text: "ok".to_string(),
                iterations: 1,
                ..Default::default()
            };
            let _ = ctx
                .events
                .send(alephcore::orchestrator::FlowStreamEvent::Complete(
                    outcome.clone(),
                ));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let mut request = basic_request();
    request.scope = alephcore::scope::FlowScope::resolved(Some(&ScopeAttribution {
        owner_user_id: "u-alice".to_string(),
        scope: ScopeId::Project("p-standup".to_string()),
    }));

    // Reproduce what `run_loop::with_request_scope` establishes around the
    // dispatch call: the ROOM's attribution plus THIS turn's speaker.
    alephcore::scope::with_scope(
        Some(ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: ScopeId::Project("p-standup".to_string()),
        }),
        alephcore::scope::with_room_author(Some("u-bob".to_string()), async {
            run_dispatch_and_drain(
                orch,
                request,
                emitter,
                "run-room-author",
                CancellationToken::new(),
                Locale::En,
            )
            .await
            .expect("dispatch ok");
        }),
    )
    .await;

    let author = seen
        .lock()
        .await
        .clone()
        .expect("the stub runner ran, so the label was resolved");
    assert_eq!(
        author.as_deref(),
        Some("u-bob"),
        "the label must name the speaker across the dispatch spawn; \
         `Some(\"u-alice\")` means the author task-local died at the boundary \
         and the fallback reported the room's creator"
    );
}

/// The control, on the same path: a personal session still gets no label, so a
/// solo conversation's prompt is byte-identical to pre-P2.
///
/// Without this, the test above would also pass for a build that labelled
/// every message everywhere.
#[tokio::test]
async fn a_personal_session_gets_no_label_across_the_same_spawn() {
    let seen: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
    let seen_clone = seen.clone();

    let runner = StubHarnessRunner::new(Arc::new(move |ctx| {
        let seen = seen_clone.clone();
        Box::pin(async move {
            *seen.lock().await = Some(alephcore::scope::ambient_room_author());
            let outcome = FlowOutcome {
                final_text: "ok".to_string(),
                iterations: 1,
                ..Default::default()
            };
            let _ = ctx
                .events
                .send(alephcore::orchestrator::FlowStreamEvent::Complete(
                    outcome.clone(),
                ));
            Ok(outcome)
        })
    }));

    let orch = orchestrator_with_stub(runner);
    let emitter: Arc<dyn EventEmitter> = Arc::new(CollectingEventEmitter::new());

    let mut request = basic_request();
    request.scope =
        alephcore::scope::FlowScope::resolved(Some(&ScopeAttribution::personal("u-alice")));

    alephcore::scope::with_scope(
        Some(ScopeAttribution::personal("u-alice")),
        alephcore::scope::with_room_author(Some("u-alice".to_string()), async {
            run_dispatch_and_drain(
                orch,
                request,
                emitter,
                "run-personal-author",
                CancellationToken::new(),
                Locale::En,
            )
            .await
            .expect("dispatch ok");
        }),
    )
    .await;

    assert_eq!(
        seen.lock().await.clone().expect("the stub runner ran"),
        None,
        "a personal session has no second speaker to name, seeded author or not"
    );
}
