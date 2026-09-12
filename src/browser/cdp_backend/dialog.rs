//! Native JS dialogs. The latch they set lives in `TabEntry::pending_dialog`
//! and is written by exactly one function (`events::apply_event`).

use aleph_cdp::methods::page;

use crate::browser::engine::EngineCapabilities;
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

    // Refuse when nothing is open. `Page.handleJavaScriptDialog` on a tab with
    // no dialog is a protocol error whose text says nothing about the page, and
    // the model's next move (click the thing that opens one) depends on being
    // told which of the two it is.
    {
        let tabs = handle.tabs.lock().await;
        let entry = tabs
            .entries
            .get(tab_id)
            .ok_or_else(|| BrowserError::TabNotFound(tab_id.to_string()))?;
        if entry.pending_dialog.is_none() {
            return Err(BrowserError::ActionFailed(
                "no dialog is open on this tab — a dialog is only pending after \
                 the page opens one (e.g. a click that calls alert/confirm)"
                    .into(),
            ));
        }
    }

    page::handle_javascript_dialog(&handle.conn, Some(&session), accept, prompt_text)
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Page.handleJavaScriptDialog", e))?;

    // Clear the latch here rather than waiting for `javascriptDialogClosed`: the
    // command has succeeded, and leaving the latch set would make the next
    // `handle_dialog` believe a dialog is still open.
    let mut tabs = handle.tabs.lock().await;
    if let Some(entry) = tabs.entries.get_mut(tab_id) {
        entry.pending_dialog = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

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

        let err = backend
            .handle_dialog("T1", "accept", None)
            .await
            .expect_err("the latch must be clear");
        assert!(
            err.to_string().contains("no dialog is open"),
            "the second accept must say the tab is clear, not fail on the \
             engine: {err}"
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
