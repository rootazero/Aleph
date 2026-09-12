//! Native JS dialogs. The latch they set lives in `TabEntry::pending_dialog`
//! and is written by exactly one function (`events::apply_event`).

use aleph_cdp::methods::page;
use aleph_cdp::CdpError;

use crate::browser::engine::{EngineCapabilities, EngineHandle};
use crate::browser::error::BrowserError;

use super::{map_cdp_err, CdpBackend};

pub(super) async fn handle_dialog(
    be: &CdpBackend,
    caps: &EngineCapabilities,
    tab_id: &str,
    action: &str,
    prompt_text: Option<&str>,
) -> Result<(), BrowserError> {
    // Before the action is even parsed: on an engine with no dialogs, "accept"
    // and "banana" are the same call, and the engine limit is the more useful
    // thing to say about both.
    super::require(caps, be.engine(), |c| c.js_dialogs, "handle_dialog")?;
    let accept = match action.to_ascii_lowercase().as_str() {
        "accept" | "ok" | "confirm" => true,
        "dismiss" | "cancel" | "reject" => false,
        other => {
            return Err(BrowserError::ActionFailed(format!(
                "unknown dialog action '{other}' — expected 'accept' or 'dismiss'"
            )))
        }
    };
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;

    // **No local pre-check.** `TabEntry::pending_dialog` is a hint; the ENGINE
    // owns whether a dialog is open. A second derivation that can REFUSE the
    // call is the one that ends up disagreeing with the authority (判据 §1,
    // §12), and it disagreed in the expensive direction: after any failed
    // `browser_dialog` the latch was cleared and the retry was then refused
    // with "no dialog is open" while one was still up — and since this function
    // is `page::handle_javascript_dialog`'s only caller, nothing could reach
    // the engine to find out. That is worse than the fail-dead it replaced,
    // which at least kept this door open.
    //
    // The old sentence is not lost. It is produced below from the engine's own
    // answer: the same words, derived from the authority instead of guessed
    // ahead of it.
    let outcome =
        page::handle_javascript_dialog(&handle.conn, Some(&session), accept, prompt_text).await;

    match outcome {
        // Handled — the dialog is gone because this call closed it.
        Ok(()) => {
            clear_latch(&handle, tab_id).await;
            Ok(())
        }
        // The engine says there is no dialog, which ESTABLISHES the fact: the
        // stale echo goes (most likely a `javascriptDialogClosed` the pump
        // dropped to a lag), and the model gets the sentence it needs.
        Err(e) if says_no_dialog(&e) => {
            clear_latch(&handle, tab_id).await;
            Err(BrowserError::ActionFailed(
                "no dialog is open on this tab — the engine confirms it. A \
                 dialog is only pending after the page opens one (e.g. a click \
                 that calls alert/confirm)."
                    .into(),
            ))
        }
        // Everything else — a timeout, a transport failure, a protocol error
        // about something else — establishes NOTHING, and 判据 §8 says an error
        // is only ever entitled to say "I do not know". Reading one as "the
        // dialog is gone" drops `actions::tab_ready`'s gate while the danger is
        // still there. So the latch stays (every other verb keeps refusing by
        // name) and this call stays reachable (the model can try again): the
        // gate up AND the door open, which is what the unconditional clear got
        // wrong in both directions at once.
        Err(e) => Err(map_cdp_err(be.engine(), "Page.handleJavaScriptDialog", e)),
    }
}

async fn clear_latch(handle: &EngineHandle, tab_id: &str) {
    let mut tabs = handle.tabs.lock().await;
    if let Some(entry) = tabs.entries.get_mut(tab_id) {
        entry.pending_dialog = None;
    }
}

/// Does this engine error ESTABLISH that the tab has no dialog?
///
/// Only a protocol error saying so does. `Timeout`, `Transport`, `Disconnected`
/// and `Decode` establish nothing, and a protocol error about something else
/// establishes something else — the same distinction `actions::resolve_target`
/// draws when it turns "No node with given id" into a stale ref rather than a
/// protocol failure.
///
/// ⚠️ **The spelling is a guess, not a measurement.** obscura never reaches
/// here (`js_dialogs: Unsupported` refuses first), so only Chromium's wording
/// matters, and it is matched case-insensitively on the two words that carry
/// the meaning rather than on a full sentence — engines have shipped the same
/// message with and without trailing punctuation before (`NO_BOX_MESSAGE` in
/// `aleph-cdp` is a prefix match for that reason).
///
/// **A guess is acceptable here only because its failure direction is the safe
/// one**: guess wrong and the latch stays set, so the gate keeps refusing by
/// name while this call stays reachable to try again. The cost is an extra
/// refusal, never a wedge. Task 16's real-machine prober is where the spelling
/// stops being a guess.
fn says_no_dialog(err: &CdpError) -> bool {
    match err {
        CdpError::Protocol { message, .. } => message.to_ascii_lowercase().contains("no dialog"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use super::{CdpBackend, EngineHandle};
    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::{Cap, Engine};

    /// Both halves of the latch, through the public verb: accepting a pending
    /// dialog reaches the wire with the right flag AND clears the latch, so a
    /// second accept is refused rather than answering a dialog that is gone.
    #[tokio::test]
    async fn accepting_a_pending_dialog_clears_the_latch() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on("Page.handleJavaScriptDialog", Responder::Reply(json!({})));
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        {
            let mut tabs = handle.tabs.lock().await;
            tabs.entries
                .get_mut("T1")
                .expect("tab entry")
                .pending_dialog = Some("alert: are you sure".into());
        }

        backend
            .handle_dialog("T1", "accept", None)
            .await
            .expect("a pending dialog can be accepted");
        let call = server
            .received()
            .into_iter()
            .find(|m| m["method"].as_str() == Some("Page.handleJavaScriptDialog"))
            .expect("the accept reached the wire");
        assert_eq!(call["params"]["accept"], json!(true));

        assert!(
            handle.tabs.lock().await.entries["T1"]
                .pending_dialog
                .is_none(),
            "a handled dialog clears the latch, so the gate comes down"
        );
    }

    /// "There is no dialog" is the ENGINE's answer, not a local guess. The
    /// sentence the model reads is the same one the removed pre-check produced;
    /// what changed is that it is now derived from the authority rather than
    /// from a second copy of the fact that could disagree with it.
    #[tokio::test]
    async fn the_engines_own_answer_is_what_says_there_is_no_dialog() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Page.handleJavaScriptDialog",
            Responder::Error {
                code: -32000,
                message: "No dialog is showing".into(),
            },
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        {
            let mut tabs = handle.tabs.lock().await;
            tabs.entries
                .get_mut("T1")
                .expect("tab entry")
                .pending_dialog = Some("alert: stale".into());
        }

        let err = backend
            .handle_dialog("T1", "accept", None)
            .await
            .expect_err("the engine says there is nothing to accept");
        assert!(
            err.to_string().contains("no dialog is open"),
            "the model gets the sentence that tells it what to do next: {err}"
        );
        assert!(
            handle.tabs.lock().await.entries["T1"]
                .pending_dialog
                .is_none(),
            "an answer that ESTABLISHES there is no dialog clears the stale echo"
        );
        // The call reached the wire, which is the half a local pre-check used
        // to prevent: the sentence above is the engine's, not ours.
        assert!(
            methods(&server)
                .iter()
                .any(|m| m == "Page.handleJavaScriptDialog"),
            "the authority has to be asked: {:?}",
            methods(&server)
        );
    }

    /// A tab with a pending dialog and an engine that never answers — the setup
    /// both halves of the timeout claim need.
    ///
    /// Returned rather than inlined twice: the two halves are separate tests so
    /// a mutation names the half it broke, and two copies of a fixture are
    /// where two tests start disagreeing about what "the same setup" means.
    async fn a_tab_whose_engine_will_not_answer() -> (FakeCdpServer, CdpBackend, Arc<EngineHandle>)
    {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        // `Delay`, NOT `Hang`, and the reason is a property of the instrument
        // rather than a preference: `Hang` parks the connection's own read loop
        // (its doc says so — "the read loop cannot observe `ctl_rx` again"), so
        // the retry below would never even be READ by the fake, and the "door
        // is open" assertion would fail for a reason that has nothing to do
        // with this code. Measured before it was believed: with `Hang` the
        // second call recorded 1 frame where 2 were expected. `Delay` spawns,
        // so the connection keeps serving.
        //
        // The duration is DERIVED from the command budget rather than picked,
        // so it cannot be "tidied" down into a slow reply: this is a peer that
        // does not answer within any budget this test could set.
        server.on(
            "Page.handleJavaScriptDialog",
            Responder::Delay(TEST_TIMEOUT * 100, Box::new(Responder::Reply(json!({})))),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        {
            let mut tabs = handle.tabs.lock().await;
            tabs.entries
                .get_mut("T1")
                .expect("tab entry")
                .pending_dialog = Some("confirm: really?".into());
        }

        let err = backend
            .handle_dialog("T1", "accept", None)
            .await
            .expect_err("the engine never answered");
        assert!(
            matches!(err, crate::browser::error::BrowserError::EngineBusy { .. }),
            "a peer that does not answer is a timeout, not a verdict: {err:?}"
        );
        (server, backend, handle)
    }

    /// **Half one: the gate stays up.** An engine that does not answer
    /// establishes nothing (判据 §8), so a timeout must not be read as "the
    /// dialog is gone" — the dialog is still there and every other verb still
    /// has to refuse.
    #[tokio::test]
    async fn an_engine_timeout_keeps_the_gate_up() {
        let (_server, _backend, handle) = a_tab_whose_engine_will_not_answer().await;
        assert_eq!(
            handle.tabs.lock().await.entries["T1"]
                .pending_dialog
                .as_deref(),
            Some("confirm: really?"),
            "a timeout must not take the gate down while the danger is still there"
        );
    }

    /// **Half two: the door stays open.** A retry REACHES the engine instead of
    /// being refused by a local copy of the fact.
    ///
    /// Its own test rather than a second assertion, so a mutation names the
    /// half it broke — the unconditional clear plus the pre-check got both
    /// directions wrong at once, and "one name, one claim" is what keeps a
    /// mutation run able to say which.
    #[tokio::test]
    async fn an_engine_timeout_leaves_the_door_open() {
        let (server, backend, _handle) = a_tab_whose_engine_will_not_answer().await;
        let before = methods(&server)
            .iter()
            .filter(|m| *m == "Page.handleJavaScriptDialog")
            .count();
        let _ = backend.handle_dialog("T1", "accept", None).await;
        let after = methods(&server)
            .iter()
            .filter(|m| *m == "Page.handleJavaScriptDialog")
            .count();
        assert_eq!(
            after,
            before + 1,
            "the retry must reach the engine — this call is the only caller of \
             Page.handleJavaScriptDialog, so a local refusal here is a wedge \
             with no verb that recovers it"
        );
    }

    /// The engine-limit half, injected (R42). It runs BEFORE the action string
    /// is parsed and before the latch is read: on an engine with no dialogs,
    /// "accept" and "banana" are the same call, and the engine limit is the
    /// more useful thing to say about either.
    #[tokio::test]
    async fn handle_dialog_refuses_when_the_table_says_the_engine_cannot() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Obscura, open_guard()).await;

        let mut caps = all_supported();
        caps.js_dialogs = Cap::Unsupported;
        let err = super::handle_dialog(&backend, &caps, "T1", "accept", None)
            .await
            .expect_err("a table saying the engine cannot must refuse");
        match err {
            crate::browser::error::BrowserError::UnsupportedByEngine {
                engine,
                verb,
                supported_by,
            } => {
                assert_eq!(engine, Engine::Obscura);
                assert_eq!(verb, "handle_dialog");
                assert_eq!(supported_by, Some(Engine::Chromium));
            }
            other => panic!("expected UnsupportedByEngine, got {other:?}"),
        }
        assert!(
            methods(&server).is_empty(),
            "the refusal must precede the wire: {:?}",
            methods(&server)
        );

        // …and it wins over a malformed action, because the engine limit is the
        // fact that makes both calls impossible.
        let err = super::handle_dialog(&backend, &caps, "T1", "banana", None)
            .await
            .expect_err("still refused");
        assert!(
            matches!(
                err,
                crate::browser::error::BrowserError::UnsupportedByEngine { .. }
            ),
            "got {err:?}"
        );

        // The control for the arm above: with the row Supported, the SAME
        // malformed action is refused for its own reason. Without this, an
        // `Err(_)`-shaped guard would look identical to one that refuses
        // everything (判据 §2 恒红).
        let err = super::handle_dialog(&backend, &all_supported(), "T1", "banana", None)
            .await
            .expect_err("a malformed action is still refused");
        assert!(
            err.to_string().contains("unknown dialog action 'banana'"),
            "with the capability present the refusal must be about the action: \
             {err}"
        );
    }
}
