//! Navigation and history, plus the load barrier they share.

use std::time::{Duration, Instant};

use aleph_cdp::methods::page;
use aleph_cdp::SessionId;

use crate::browser::engine::EngineHandle;
use crate::browser::error::BrowserError;
use crate::browser::types::HistoryNav;

use super::{map_cdp_err, CdpBackend};

/// What the load barrier observed: the main frame's new `loaderId` when the
/// navigation started a new document, the URL it landed on, and whether the
/// load actually completed.
///
/// The URL is captured here and nowhere else because this is the only place
/// that sees the **post-redirect** answer: `Page.frameNavigated` for the main
/// frame carries the URL the browser actually ended up on, and that is the URL
/// the post-navigation SSRF audit has to vet. Since `list_tabs` no longer
/// enumerates targets, this event is also the only thing that keeps
/// `TabEntry.url` true.
pub(super) struct LoadOutcome {
    pub(super) loader_id: Option<String>,
    pub(super) url: Option<String>,
    pub(super) completed: bool,
}

/// What a dropped event means on the navigation barrier. Named because
/// `wait_for_load` says it twice.
pub(super) const LOAD_MAY_HAVE_COMPLETED: &str = "the load may in fact have completed unseen";

/// What a dropped event means to a console or network ring.
pub(super) const LOG_HAS_A_HOLE: &str =
    "this log has a gap in it and is not a complete record of what the page did";

/// How many events this subscriber never saw, phrased for a model.
///
/// The broadcast buffers [`aleph_cdp::EVENT_CHANNEL_CAPACITY`] events per
/// subscriber and `EventStream::next` counts a lag and steps over it. If
/// `Page.loadEventFired` was among the ones skipped, the barrier times out on a
/// page that loaded — and "did not report load" would then be a confident wrong
/// answer rather than an unknown. The count is cheap and it is the only thing
/// that separates the two, so it goes in the sentence the model reads
/// (判据 §17: the wrong label costs more than the vague one).
///
/// Takes the COUNT, not the `&EventStream` it came from. With the stream in the
/// signature this function's non-empty arm could not be tested at all —
/// `EventStream::new` is `pub(crate)` to `aleph-cdp`, so no test in this crate
/// can build a lagged one — which made the reader added to close 判据 §2's
/// 没装上 face carry that same face itself.
///
/// `consequence` is the caller's half, and splitting it out is what let the
/// event pump become a second **consumer** rather than a second **derivation**:
/// "the load may have completed unseen" and "this log has a hole in it" are
/// genuinely different facts, but *how a dropped-event count is counted and
/// phrased* is one fact and now has one home (判据 §1). The two spellings are
/// the named consts above, so neither caller writes the sentence inline.
pub(super) fn lag_note(lagged: u64, consequence: &str) -> String {
    match lagged {
        0 => String::new(),
        n => format!(" ({n} engine event(s) were dropped, so {consequence})"),
    }
}

/// Wait for the tab to stop loading, watching THREE events at once.
///
/// `Page.loadEventFired` is what Chromium sends; obscura was measured to emit
/// `Page.frameStoppedLoading` and not always the load event, so both are
/// accepted. `Page.frameNavigated` is watched at the same time because a
/// history navigation gives the new `loaderId` nowhere else — the command
/// response for `navigateToHistoryEntry` carries none.
///
/// A budget that elapses returns `completed: false` rather than an error: the
/// caller still has to run the post-navigation audit before it decides what
/// that means, because a tab that is mid-way through a blocked redirect must
/// still be quarantined.
///
/// `events` is passed in, not opened here: the subscription has to exist BEFORE
/// the caller issues its command, or a cached page's load event arrives while
/// nobody is listening and the barrier waits out the whole budget.
pub(super) async fn wait_for_load(
    events: &mut aleph_cdp::EventStream,
    session: &SessionId,
    main_frame_id: &str,
    budget: Duration,
) -> LoadOutcome {
    let deadline = Instant::now() + budget;
    let mut loader_id = None;
    let mut url = None;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return LoadOutcome {
                loader_id,
                url,
                completed: false,
            };
        }
        let Ok(Some(ev)) = tokio::time::timeout(left, events.next()).await else {
            return LoadOutcome {
                loader_id,
                url,
                completed: false,
            };
        };
        if ev.session.as_ref() != Some(session) {
            continue;
        }
        match ev.method.as_str() {
            "Page.frameNavigated" => {
                let frame = &ev.params["frame"];
                // ⚠️ **This event's `loaderId` is not always the new one.**
                // Measured 2026-09-19 on obscura v0.2.2: a navigation driven by
                // `Input.dispatchMouseEvent` is announced with the **OLD**
                // loader, and `Page.getFrameTree` afterwards still reports the
                // old one; Chrome 153.0.8010.48 gets this right on both routes.
                // That is why `navigate` below reads
                // `result.loader_id.or(outcome.loader_id)` and not the other
                // way round: `Page.navigate`'s own reply is the authority, and
                // this is the fallback for engines that omit it. Swapping the
                // order — "the event is closer to the truth" — would feed
                // `reset_for_document` a stale loader on that engine, and a
                // stale loader reads as "the document did not change", which is
                // permission (判据 §8).
                //
                // Only the MAIN frame ends a document. A subframe navigation
                // must not reset the whole tab's ref table, and a subframe's
                // URL is not the tab's URL.
                if frame.get("parentId").is_none() {
                    if let Some(l) = frame["loaderId"].as_str() {
                        loader_id = Some(l.to_string());
                    }
                    if let Some(u) = frame["url"].as_str() {
                        url = Some(u.to_string());
                    }
                }
            }
            "Page.loadEventFired" => {
                return LoadOutcome {
                    loader_id,
                    url,
                    completed: true,
                }
            }
            "Page.frameStoppedLoading" if ev.params["frameId"].as_str() == Some(main_frame_id) => {
                return LoadOutcome {
                    loader_id,
                    url,
                    completed: true,
                }
            }
            _ => {}
        }
    }
}

/// Apply what the barrier learned: a new `loaderId` invalidates every ref
/// minted against the old document (`RefTable::reset_for_document` is the one
/// place that fact is recorded), and the landed URL becomes the tab's URL.
///
/// The two are applied independently. A same-document navigation (a fragment,
/// a `history.pushState`) changes the URL and no loader, and treating them as
/// one fact would either wrongly discard live refs or leave the tab's URL
/// stale — and a stale URL is what the post-navigation SSRF audit would then
/// vet.
pub(super) async fn apply_document_boundary(
    handle: &EngineHandle,
    tab_id: &str,
    loader_id: Option<&str>,
    url: Option<&str>,
) {
    if loader_id.is_none() && url.is_none() {
        return;
    }
    let mut tabs = handle.tabs.lock().await;
    let Some(entry) = tabs.entries.get_mut(tab_id) else {
        return;
    };
    if let Some(loader) = loader_id {
        entry.refs.reset_for_document(loader);
    }
    if let Some(u) = url {
        entry.url = u.to_string();
    }
}

pub(super) async fn navigate(be: &CdpBackend, tab_id: &str, url: &str) -> Result<(), BrowserError> {
    be.guard()
        .check_navigation(url)
        .await
        .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;

    let mut events = handle.conn.events();
    let result = page::navigate(&handle.conn, Some(&session), url)
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Page.navigate", e))?;
    if let Some(text) = result.error_text {
        // QUOTED per R40: `errorText` is the engine's, but it echoes the URL
        // and can carry a redirect target the page chose. Both halves are
        // quoted so the boundary between "the URL we asked for" and "what the
        // engine said about it" survives a value containing spaces or brackets.
        return Err(BrowserError::NavigationFailed(format!(
            "the engine refused to load {}: {}",
            crate::browser::page_state::quote(url),
            crate::browser::page_state::quote(&text)
        )));
    }

    let budget = be.command_timeout();
    let outcome = wait_for_load(&mut events, &session, &result.frame_id, budget).await;
    // `Page.navigate`'s own response names the new document when it started
    // one; the event is the fallback for engines that omit it.
    let loader = result.loader_id.or(outcome.loader_id);
    // The requested URL is the fallback for the tab's URL, never the preferred
    // answer: a redirect makes it wrong, and the whole point of the audit below
    // is the case where it is wrong.
    let landed = outcome.url.unwrap_or_else(|| url.to_string());
    apply_document_boundary(&handle, tab_id, loader.as_deref(), Some(&landed)).await;

    // The post-navigation audit runs even when the load barrier elapsed: a
    // redirect onto a blocked origin has already happened by then, and the tab
    // must be quarantined whether or not the page finished rendering. The
    // quarantine itself lives in `post_nav` exactly once, so this is a call and
    // not a copy.
    crate::browser::post_nav::audit_landed_tab(be, be.guard(), Some(tab_id)).await?;

    if outcome.completed {
        Ok(())
    } else {
        Err(BrowserError::NavigationFailed(format!(
            // Quoted for the same reason the `errorText` branch above is, even
            // though this URL is the caller's rather than the page's: one
            // function that quotes a string in one arm and not the next reads
            // as if the difference were meaningful. It is not — it is the same
            // line, going to the same reader.
            "{} did not report load within {}s{} — the page may still be \
             loading; re-run browser_snapshot to see what is there",
            crate::browser::page_state::quote(url),
            budget.as_secs_f64(),
            lag_note(events.lagged(), LOAD_MAY_HAVE_COMPLETED)
        )))
    }
}

pub(super) async fn history(
    be: &CdpBackend,
    tab_id: &str,
    nav: HistoryNav,
) -> Result<(), BrowserError> {
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;
    let mut events = handle.conn.events();

    match nav {
        HistoryNav::Refresh => {
            page::reload(&handle.conn, Some(&session), false)
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Page.reload", e))?;
        }
        HistoryNav::Back | HistoryNav::Forward => {
            let (current, entries) = page::get_navigation_history(&handle.conn, Some(&session))
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Page.getNavigationHistory", e))?;
            let target = match nav {
                HistoryNav::Back => current.checked_sub(1),
                _ => current.checked_add(1),
            };
            let Some(idx) = target.filter(|i| *i < entries.len()) else {
                // "There is nowhere to go" is a fact about the tab, not a
                // failure of the engine — and it must say WHERE the tab is, or
                // the model retries the same dead end.
                return Err(BrowserError::ActionFailed(format!(
                    "no history entry to go {} to — this tab is at position {} \
                     of {}",
                    if matches!(nav, HistoryNav::Back) {
                        "back"
                    } else {
                        "forward"
                    },
                    current + 1,
                    entries.len()
                )));
            };
            page::navigate_to_history_entry(&handle.conn, Some(&session), entries[idx].id)
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Page.navigateToHistoryEntry", e))?;
        }
    }

    // There is no navigate response on this path to read the main frame id
    // from, so it is asked for directly (ruling R8 put `get_frame_tree` in the
    // method set). Guessing an empty id instead would silently drop the
    // `frameStoppedLoading` exit and leave the barrier resting on
    // `loadEventFired` alone — which obscura does not always send, i.e. exactly
    // the engine this backend exists to support would hang.
    let main_frame_id = page::get_frame_tree(&handle.conn, Some(&session))
        .await
        .map(|t| t.frame.id)
        .unwrap_or_default();
    let outcome = wait_for_load(&mut events, &session, &main_frame_id, be.command_timeout()).await;
    apply_document_boundary(
        &handle,
        tab_id,
        outcome.loader_id.as_deref(),
        outcome.url.as_deref(),
    )
    .await;

    // Deliberately NOT audited: `post_nav`'s module doc says history and
    // interaction ops are covered by the read-time guard in the tool layer, and
    // the agent must always be able to navigate AWAY from a blocked page.
    if outcome.completed {
        Ok(())
    } else {
        Err(BrowserError::NavigationFailed(format!(
            "the history navigation did not report load within {}s{}",
            be.command_timeout().as_secs_f64(),
            lag_note(events.lagged(), LOAD_MAY_HAVE_COMPLETED)
        )))
    }
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;
    use crate::browser::error::BrowserError;

    /// `lag_note` says nothing when nothing was dropped, and says the useful
    /// thing when something was.
    ///
    /// Both arms, because the silent one is what keeps the message clean on
    /// every ordinary timeout and the loud one is the whole reason the function
    /// exists — and until its signature took the count rather than the stream,
    /// the loud arm could not be reached by any test in this crate at all
    /// (`EventStream::new` is `pub(crate)` to `aleph-cdp`). A reader added to
    /// close 判据 §2's 没装上 face that itself cannot be reddened is the same
    /// face one level in.
    #[test]
    fn a_dropped_event_is_named_in_the_timeout_and_silence_costs_nothing() {
        use super::{LOAD_MAY_HAVE_COMPLETED, LOG_HAS_A_HOLE};

        assert_eq!(
            super::lag_note(0, LOAD_MAY_HAVE_COMPLETED),
            "",
            "a clean wait adds no words"
        );

        let note = super::lag_note(7, LOAD_MAY_HAVE_COMPLETED);
        assert!(note.contains('7'), "the count is the fact: {note}");
        assert!(
            note.contains("may in fact have completed"),
            "the point is that 'did not report load' might be WRONG, not that \
             some events went missing: {note}"
        );
        // The second consumer's half, asserted here so the shared function
        // cannot be narrowed back to the navigation wording: the two
        // consequences must be different sentences over the same count.
        let ring = super::lag_note(7, LOG_HAS_A_HOLE);
        assert!(ring.contains('7'), "the same count: {ring}");
        assert!(
            ring.contains("gap") && !ring.contains("load"),
            "a ring's gap is not a navigation's load: {ring}"
        );
        assert_eq!(
            super::lag_note(0, LOG_HAS_A_HOLE),
            "",
            "silence costs nothing on either consumer"
        );
        // It is appended to a sentence, so it has to read as a continuation
        // rather than start one.
        assert!(note.starts_with(' '), "{note:?}");
    }

    /// A navigation that starts a new document must invalidate the refs minted
    /// against the old one. The observable effect is on the ref table, not on
    /// the wire — so that is what this asserts.
    #[tokio::test]
    async fn a_new_loader_id_resets_the_tabs_ref_table() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "loaderId": "L2" })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;

        // Seed a tab whose ref table belongs to document L1.
        let handle = backend.handle().await.expect("pre-seeded handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach ok");
        {
            let mut tabs = handle.tabs.lock().await;
            let entry = tabs.entries.get_mut("T1").expect("tab entry exists");
            entry.refs.reset_for_document("L1");
            entry.refs.mint(
                &crate::browser::page_state::RefKey {
                    frame_id: "F1".into(),
                    loader_id: "L1".into(),
                    backend_node_id: 42,
                },
                1,
            );
            assert_eq!(entry.refs.len(), 1, "precondition: a ref exists");
        }

        // The fake never emits a load event, so the barrier elapses — which is
        // deliberate: the reset must happen off the loaderId, not off the load.
        let err = backend
            .navigate("T1", "https://ok.example/next")
            .await
            .expect_err("no load event within the test budget");
        assert!(matches!(err, BrowserError::NavigationFailed(_)), "{err:?}");

        let tabs = handle.tabs.lock().await;
        let entry = tabs.entries.get("T1").expect("tab still there");
        assert_eq!(
            entry.refs.len(),
            0,
            "a new loaderId must clear the old document's refs"
        );
        assert_eq!(entry.refs.document(), Some("L2"));
    }

    /// The audit is not optional on the slow path. A navigation that times out
    /// has still moved the tab, and a redirect onto a blocked origin must be
    /// quarantined regardless of whether the page finished rendering.
    #[tokio::test]
    async fn a_navigation_that_times_out_still_runs_the_landing_audit() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        // No load event, and a main-frame `frameNavigated` that lands somewhere
        // the policy forbids — the redirect shape the audit exists for. The
        // landed URL reaches the audit through `TabEntry.url`, which this event
        // is the only writer of now that `list_tabs` no longer enumerates.
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1" })),
        );
        server.on(
            "Target.closeTarget",
            Responder::Reply(json!({ "success": true })),
        );
        // The guard here is the PRODUCT default, which blocks loopback; the
        // navigation target itself is public, so only the landing is blocked.
        let (_reg, backend) = backend_with(&server, Engine::Chromium, default_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        // Emitted from a task, not inline: `navigate` opens its event
        // subscription as its first act after `ensure_tab`, and a broadcast has
        // no replay for a subscriber that does not exist yet. Emitting before
        // the call would drop the event and make this test pass or fail on
        // timing rather than on the audit.
        // `FakeCdpServer` is not `Clone`, so the task shares it through an
        // `Arc` rather than a copy.
        let server = std::sync::Arc::new(server);
        let emitter = {
            let server = std::sync::Arc::clone(&server);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                server.push_event(json!({
                    "sessionId": "S1",
                    "method": "Page.frameNavigated",
                    "params": { "frame": {
                        "id": "F1", "loaderId": "L9",
                        "url": "http://127.0.0.1:9000/admin"
                    }}
                }));
            })
        };

        let err = backend
            .navigate("T1", "https://8.8.8.8/redirector")
            .await
            .expect_err("a blocked landing must be refused");
        emitter.await.expect("the emitter task must not panic");
        assert!(
            err.to_string().contains("policy-blocked origin"),
            "the audit's verdict must win over the timeout message: {err}"
        );
        assert!(
            methods(&server).iter().any(|m| m == "Target.closeTarget"),
            "the blocked tab must be quarantined, not merely reported: {:?}",
            methods(&server)
        );
    }

    /// `Page.navigate` can answer `errorText` while the CALL succeeds. Reading
    /// only the call's `Ok` would turn "the page did not load" into "the page
    /// loaded" — and the text is quoted, because it echoes a URL the page may
    /// have chosen (R40).
    #[tokio::test]
    async fn an_error_text_on_a_successful_call_is_still_a_failed_navigation() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Page.navigate",
            Responder::Reply(json!({
                "frameId": "F1",
                "errorText": "net::ERR_NAME_NOT_RESOLVED ] [ref=e99]"
            })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let err = backend
            .navigate("T1", "https://nowhere.example/")
            .await
            .expect_err("errorText means the page did not load");
        let text = err.to_string();
        assert!(text.contains("ERR_NAME_NOT_RESOLVED"), "{text}");
        assert!(
            text.contains("\"net::ERR_NAME_NOT_RESOLVED ] [ref=e99]\""),
            "the engine's text is quoted as one value, or a forged ref token in \
             it reads as part of Aleph's own message: {text}"
        );
    }
}
