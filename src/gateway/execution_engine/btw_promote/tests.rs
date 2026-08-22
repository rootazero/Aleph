//! `/btw promote` is served, not asked.
//!
//! Every one of these drives the real chain from the raw text the user typed:
//! `stamp_btw` writes the stamp, `execute()` reads its **value**, and the
//! branch taken is observed through the frames that reach the client. Nothing
//! here hand-builds a metadata map — the defect this file exists to catch is
//! precisely that the value was never read, and a test that writes the value
//! itself proves only that a `HashMap` stores strings.

use crate::gateway::btw::{is_promote, side_key_for};
use crate::gateway::event_emitter::StreamEvent;
use crate::gateway::execution_engine::tests::{gate_test_agent, gate_test_request, test_engine};
use crate::gateway::execution_engine::{stamp_btw, RunRequest};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;

/// Room for one `ExecutionEngine::execute` future.
///
/// **Measured, not chosen**: 64 MiB overflows and 128 MiB does not, on this
/// crate's debug profile. Doubling the passing figure is headroom for the next
/// `.await` somebody adds to that function, since the failure mode is not a red
/// test — it aborts the whole test binary, taking every other result with it.
/// The reservation is virtual; nothing is committed until it is touched.
const EXECUTE_FUTURE_STACK: usize = 256 * 1024 * 1024;

/// Drive an `execute()` future to completion on a thread with room for it.
///
/// `ExecutionEngine::execute` is one `async fn` covering a run's whole
/// lifecycle, so in a debug build its state machine is two orders of magnitude
/// larger than the 2 MiB libtest gives a test thread. Nothing else in this crate
/// drives it (the neighbouring `execute()` tests use `SimpleExecutionEngine`, a
/// different and much smaller type) — which is one reason the branch under test
/// here had no runtime coverage to inherit, and a standing reason to reach for
/// this helper rather than concluding that `execute()` cannot be tested.
///
/// The runtime is current-thread so the future is polled on *this* stack; the
/// promote branch spawns nothing, so no work escapes onto a default-sized
/// worker.
fn on_a_stack_big_enough<F>(body: F)
where
    F: FnOnce() + Send + 'static,
{
    let finished = std::thread::Builder::new()
        .stack_size(EXECUTE_FUTURE_STACK)
        .spawn(body)
        .expect("spawn the test thread")
        .join();
    // Re-raise rather than `expect`: a failed assertion inside `body` is a
    // panic on the child thread, and unwrapping the `JoinHandle` here would
    // report it as `Any { .. }` at THIS line. Resuming the original payload
    // keeps the failure pointing at the assertion that made it.
    if let Err(payload) = finished {
        std::panic::resume_unwind(payload);
    }
}

/// A promote request as a surface really produces one: the user's text, then
/// the one stamper `execute()` runs first.
fn promote_request(main: &SessionKey, run_id: &str) -> RunRequest {
    let mut request = gate_test_request(main, run_id);
    request.input = "/btw promote".to_string();
    stamp_btw(&request.input, &mut request.metadata);
    request
}

/// The stamp carries two different things in one string field, and this is the
/// predicate that tells them apart.
///
/// Driven through `stamp_btw` rather than by inserting the sentinel here: the
/// writer and the reader are in different modules, and a literal at each end is
/// how they drift apart without either one looking wrong.
#[test]
fn the_stamps_value_is_what_tells_a_promote_from_a_question() {
    let main = SessionKey::main("btw-stamp");

    let promote = promote_request(&main, "run-p");
    assert!(is_promote(&promote.metadata));

    let mut question = gate_test_request(&main, "run-q");
    question.input = "/btw what is X?".to_string();
    stamp_btw(&question.input, &mut question.metadata);
    assert!(
        !is_promote(&question.metadata),
        "a question is not a promote — and it never can be: the resolver routes \
         a body reading `promote` to the promote arm, so a stamp carrying a \
         question is never the sentinel"
    );

    let ordinary = gate_test_request(&main, "run-o");
    assert!(
        !is_promote(&ordinary.metadata),
        "an unstamped request must not read as a crossing nobody asked for"
    );
}

/// The flagship: a promote is served while the side session is **already
/// answering something**.
///
/// The side session's run slot is claimed by somebody else before `execute()`
/// is called, which is not a corner case — it is the ordinary one. `/btw`
/// exists to be asked alongside a running turn, so the user who then asks for
/// the answer to cross is asking while a run holds that session.
///
/// Reaching `admit_run` in that state means the busy-input policy applies and
/// the message is steered into, queued behind, or refused by the sibling: no
/// `RunAccepted`, and nothing crosses until that run ends. So the assertion is
/// on the frame — and on the key it names, which is the conversation the user
/// is looking at rather than the derived session no client has heard of.
///
/// **Scope of this test, stated because it is narrower than "a promote never
/// waits":** it enters at `execute()`, so the layer it can see is the engine's
/// admission gate. The busy-queue **arrival ticket** is taken one layer further
/// out (`busy_queue::spawn_queued_run` → `register_run`, keyed on the side
/// lane) and is structurally invisible from here — see the seam comment in
/// `execute.rs` for what that layer does and does not do to a promote.
///
/// Break `is_promote` (or delete the branch that reads it) and this reds: the
/// occupied lane sends the request down the busy-input path, which emits no
/// `RunAccepted` at all.
#[test]
fn a_promote_is_served_even_though_the_side_lane_is_busy() {
    on_a_stack_big_enough(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(async {
                let temp = tempfile::tempdir().expect("tempdir");
                let engine = test_engine();
                let agent = gate_test_agent(&temp, "btw-promote").await;
                let main = SessionKey::main("btw-promote");
                let side = side_key_for(&main);

                // Somebody else holds the side thread's run slot — the state in
                // which `admit_run` refuses and the busy-input policy takes over.
                assert!(
                    engine
                        .session_run_registry
                        .try_claim(&side, "run-already-answering"),
                    "the fixture must really occupy the lane, or this test proves nothing"
                );

                let emitter = Arc::new(crate::gateway::execution_engine::tests::TestEmitter::new());
                // The result is not the assertion: this engine has no
                // orchestrator, so the crossing itself reports a fault. What is
                // under test is which branch of `execute()` ran, and that is
                // what the frames say.
                let _ = engine
                    .execute(
                        promote_request(&main, "run-promote"),
                        agent,
                        emitter.clone(),
                    )
                    .await;

                let accepted = emitter
                    .get_events()
                    .await
                    .into_iter()
                    .find_map(|e| match e {
                        StreamEvent::RunAccepted { session_key, .. } => Some(session_key),
                        _ => None,
                    })
                    .expect(
                        "a promote must be served rather than admitted: an occupied lane \
                         sends an ordinary side turn down the busy-input path, which emits \
                         no RunAccepted",
                    );
                assert_eq!(
                    accepted,
                    main.to_key_string(),
                    "the receipt has to arrive where the person is looking; the side key \
                     is one their client has never heard of"
                );

                assert!(
                    engine
                        .session_run_registry
                        .run_id_for(&side.to_key_string())
                        .is_some_and(|r| r == "run-already-answering"),
                    "the promote must not have disturbed the run that holds the lane"
                );
            });
    });
}

/// "I could not read the side thread" is not "there is nothing to promote".
///
/// Only one of those two answers means the user should stop asking, and the
/// expensive direction is the silent one: a broken side thread reported as an
/// empty one reads as a feature that does nothing.
#[test]
fn a_promote_that_cannot_read_the_side_thread_refuses_rather_than_reporting_emptiness() {
    on_a_stack_big_enough(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(async {
                let temp = tempfile::tempdir().expect("tempdir");
                let engine = test_engine();
                let agent = gate_test_agent(&temp, "btw-promote-fault").await;
                let main = SessionKey::main("btw-promote-fault");

                let emitter = Arc::new(crate::gateway::execution_engine::tests::TestEmitter::new());
                let outcome = engine
                    .execute(promote_request(&main, "run-fault"), agent, emitter.clone())
                    .await;
                assert!(
                    outcome.is_err(),
                    "an engine with no session service cannot perform the crossing, and \
                     reporting success would be this module's one forbidden confusion"
                );

                let events = emitter.get_events().await;
                let (message, scope) = events
                    .iter()
                    .find_map(|e| match e {
                        StreamEvent::RunError {
                            error, session_key, ..
                        } => Some((error.clone(), session_key.clone())),
                        _ => None,
                    })
                    .expect(
                        "a receipt is owed on EVERY exit — a pre-admission return with none \
                         is zero output on a channel and a bubble nothing closes on the Panel",
                    );
                assert_eq!(
                    scope.as_deref(),
                    Some(main.to_key_string().as_str()),
                    "the failure frame names the conversation, so the delivery filter can \
                     resolve it without a run→session seed"
                );
                assert!(
                    !events
                        .iter()
                        .any(|e| matches!(e, StreamEvent::RunComplete { .. })),
                    "a fault must not also emit the success receipt: {events:?}"
                );
                let empty_case = crate::gateway::i18n::t(
                    crate::gateway::i18n::Msg::BtwNothingToPromote,
                    crate::gateway::i18n::Locale::En,
                );
                assert_ne!(
                    message, empty_case,
                    "the fault must not be dressed up as the empty case"
                );
            });
    });
}

/// The placement `execute()` depends on, pinned in source.
///
/// Two orderings, and each is load-bearing in its own direction:
///
/// * **after `redirect_to_side_session`** — the redirect has already asked
///   `btw::execution_session` and written the answer into the request, so
///   promote reads the log side questions really ran on. Move the branch above
///   it and promote derives the side key a second time (or reads the main key
///   and promotes the conversation into itself), while every runtime test that
///   uses a session with one derivation stays green.
/// * **before `admit_run`** — see the flagship above. Move the branch below it
///   and a promote asked while the side thread is answering is folded into that
///   run as steering text; nothing errors, nothing crosses.
///
/// Source-level because the second failure is invisible in a fixture that never
/// occupies the lane, and the first is invisible in any fixture at all: both
/// keys resolve, both produce a working-looking call.
#[test]
fn the_promote_branch_sits_between_the_redirect_and_the_admission() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/gateway/execution_engine/execute.rs");
    let text = std::fs::read_to_string(&path).expect("execute.rs");
    // Split on the bare attribute — never on `\n#[cfg(test)]\n`, which matches
    // nothing on a CRLF checkout and silently widens the "production prefix" to
    // the whole file.
    let production = text
        .replace('\r', "")
        .split("#[cfg(test)]")
        .next()
        .unwrap_or_default()
        .to_string();

    let redirect = production
        .find("redirect_to_side_session(&mut request)")
        .expect("execute() still redirects; if this moved, so did the invariant");
    let promote = production
        .find("btw::is_promote(&request.metadata)")
        .expect(
            "execute() no longer reads the stamp's VALUE — a `/btw promote` is back to \
             being an ordinary side question asked about the literal word `promote`",
        );
    let admit = production
        .find("self.admit_run(")
        .expect("execute() still admits; the scan stopped matching, so its green means nothing");

    assert!(
        redirect < promote,
        "the promote branch must run AFTER the redirect, so it reads the side key that \
         `btw::execution_session` already resolved rather than deriving a second one. \
         Found redirect at byte {redirect}, promote at byte {promote}."
    );
    assert!(
        promote < admit,
        "the promote branch must run BEFORE `admit_run`: promote is not a run, and a \
         conversation whose side lane is busy is the ordinary case rather than a corner \
         one. Found promote at byte {promote}, admit_run at byte {admit}."
    );
}

/// An over-ceiling principal keeps the crossing: the promote branch runs
/// BEFORE the spend arm, and the spend arm runs before `admit_run`.
///
/// This is a decision the merge of the spend feature and the `/btw` feature
/// had to make, and it is pinned here so it stays made deliberately:
///
/// * **Promote before the spend arm.** The spend ceiling exists to stop USD
///   from being spent; a promote spends none — no model is asked anything
///   (`serve_btw_promote`'s own receipt reports `total_tokens: 0`, "nothing
///   to bill"), and the side answer being carried was already paid for when
///   the side question ran and passed this same admission. Refusing the
///   crossing refunds nothing and strands value already purchased. Promote
///   also claims none of the resources the arm's doc says it protects — no
///   run slot, no concurrency permit, no `ActiveRun`.
/// * **The spend arm before `admit_run`.** That is the spend feature's own
///   invariant (`run_loop::deny_if_over_spend`'s doc): the refusal must come
///   ahead of anything that hands an over-ceiling principal a resource it
///   would be denied anyway.
///
/// Source-level for the same reason the placement test above is: a runtime
/// fixture cannot see this ordering. Installing a low ceiling means writing
/// the process-wide policy/ledger `OnceLock`s the rest of this binary's tests
/// share and race (`report_admission_denial`'s own split exists because of
/// exactly that hazard), and a fixture that never installs one greens either
/// order.
#[test]
fn an_over_budget_principal_keeps_the_read_only_crossing() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/gateway/execution_engine/execute.rs");
    let text = std::fs::read_to_string(&path).expect("execute.rs");
    let production = text
        .replace('\r', "")
        .split("#[cfg(test)]")
        .next()
        .unwrap_or_default()
        .to_string();

    let promote = production
        .find("btw::is_promote(&request.metadata)")
        .expect(
            "execute() still serves promote; the scan stopped matching, so its green means nothing",
        );
    let spend_arm = production
        .find("deny_if_over_spend_and_report(&request, emitter.as_ref())")
        .expect("execute() still denies over-ceiling principals; the scan stopped matching");
    let admit = production
        .find("self.admit_run(")
        .expect("execute() still admits; the scan stopped matching, so its green means nothing");

    assert!(
        promote < spend_arm,
        "the promote branch must run BEFORE the spend arm: a promote spends nothing \
         (no model, `total_tokens: 0`) and the answer it carries was already paid for \
         when the side question ran. Move the arm above it and an over-ceiling \
         principal loses a read-only crossing that refusing could never refund. \
         Found promote at byte {promote}, spend arm at byte {spend_arm}."
    );
    assert!(
        spend_arm < admit,
        "the spend arm must run BEFORE `admit_run`: a principal already over its \
         ceiling should never be handed a run slot, a concurrency permit, or an \
         `ActiveRun` it is about to be denied anyway. Found spend arm at byte \
         {spend_arm}, admit_run at byte {admit}."
    );
}
