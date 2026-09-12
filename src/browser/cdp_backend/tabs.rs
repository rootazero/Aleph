//! Tab lifecycle. A `TabId` is the CDP `targetId` — the browser's own name for
//! the page, not an index Aleph invents, so it cannot be confused with another
//! tab after a close and does not have to be re-derived from a listing.

use aleph_cdp::methods::target;
use aleph_cdp::TargetId;

use crate::browser::error::BrowserError;
use crate::browser::tab_registry::TabLine;
use crate::browser::types::TabId;

use super::{map_cdp_err, CdpBackend};

pub(super) async fn open_tab(be: &CdpBackend, url: &str) -> Result<TabId, BrowserError> {
    be.guard()
        .check_navigation(url)
        .await
        .map_err(|e| BrowserError::NavigationFailed(e.to_string()))?;
    // The one verb that means "give me a browser".
    let handle = be.handle_launching().await?;

    // Created on `about:blank` and navigated afterwards, deliberately:
    // `Target.createTarget{url}` returns as soon as the target exists and
    // reports neither a loaderId nor a load event, so a tab opened that way
    // would have no document boundary to reset the ref table on and no moment
    // at which "the page is there" became true.
    let target_id = target::create_target(&handle.conn, "about:blank")
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Target.createTarget", e))?;
    let tab_id = target_id.0.clone();

    // `attach_tab` is the ONE attach path: it attaches, enables
    // Page/Runtime/Network and inserts the `TabEntry` with a fresh `RefTable`
    // and `url: "about:blank"`. Its twin `ensure_tab` is a pure LOOKUP and
    // would answer `TabNotFound` here — creating a tab is this verb's job, and
    // the split is what keeps a typo from looking like a working page.
    handle.attach_tab(&target_id).await?;
    {
        let mut tabs = handle.tabs.lock().await;
        tabs.active = Some(tab_id.clone());
    }

    // The navigation carries the SSRF pre-check (again — it is cheap and the
    // guard is the kind of thing that must not depend on a caller having run
    // it) and the post-navigation audit, so a redirect onto a blocked origin
    // quarantines this brand-new tab exactly as it would an old one.
    super::navigate::navigate(be, &tab_id, url).await?;
    Ok(tab_id)
}

pub(super) async fn close_tab(be: &CdpBackend, tab_id: &str) -> Result<(), BrowserError> {
    let handle = be.handle().await?;
    let closed = target::close_target(&handle.conn, &TargetId(tab_id.to_string()))
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Target.closeTarget", e))?;
    // The verdict comes BEFORE the state change. Dropping the entry first and
    // then reporting `TabNotFound` would leave a tab that may well still exist
    // unaddressable — the table and the answer disagreeing about the same fact,
    // with the model told the tab was never there.
    if !closed {
        return Err(BrowserError::TabNotFound(tab_id.to_string()));
    }
    {
        let mut tabs = handle.tabs.lock().await;
        tabs.entries.remove(tab_id);
        if tabs.active.as_deref() == Some(tab_id) {
            // The closed tab was the selected one. Naming ANY surviving tab
            // would be a guess; leaving it unset lets `active_tab`'s documented
            // last-listed fallback answer, which is the answer every other
            // marker-less listing already gets.
            tabs.active = None;
        }
    }
    Ok(())
}

/// The tabs this profile has, read from the handle's tab table — **never** from
/// `Target.getTargets` (controller ruling: no tab discovery by enumeration).
///
/// Measured on real obscura: `Target.getTargets` answers `{"targetInfos":[]}`
/// on a fresh connection — including the first one — while HTTP `/json/list`
/// on the same process lists `page-1`. A backend that enumerated would decide
/// there were no tabs, and every verb that resolves "the active tab" would
/// answer "no tabs open" for a browser with a page in it. An empty enumeration
/// is not evidence of an empty browser (判据 §8).
///
/// So the table is the single source of truth: it is populated by our own
/// `Target.createTarget` calls (above) and by `Target.targetCreated` events
/// whose `openerId` is a tab we already own (popups — handled in the event
/// pump, Task 13), and by nothing else.
pub(super) async fn list_tabs(be: &CdpBackend) -> Result<Vec<TabLine>, BrowserError> {
    let handle = be.handle().await?;
    let tabs = handle.tabs.lock().await;
    let mut lines: Vec<TabLine> = tabs
        .entries
        .iter()
        .map(|(id, entry)| TabLine {
            selected: tabs.active.as_deref() == Some(id.as_str()),
            id: id.clone(),
            url: entry.url.clone(),
        })
        .collect();
    // `HashMap` iteration order is arbitrary, and `tab_registry::active_tab`
    // falls back to LAST-listed when nothing carries the marker. An arbitrary
    // order would make that fallback pick an arbitrary tab, so the listing is
    // ordered by the id the browser assigned — stable across calls, which is
    // the only property the fallback needs.
    lines.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(lines)
}

pub(super) async fn switch_tab(be: &CdpBackend, tab_id: &str) -> Result<(), BrowserError> {
    let handle = be.handle().await?;
    // Attach first: switching to a tab the page opened itself (`target=_blank`)
    // must make it addressable, not merely foregrounded. `attach_tab` is
    // idempotent, so a tab already in the table costs one map lookup.
    handle.attach_tab(&TargetId(tab_id.to_string())).await?;
    target::activate_target(&handle.conn, &TargetId(tab_id.to_string()))
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Target.activateTarget", e))?;
    // The selection is only real if whoever asks "which tab is active" next
    // sees it — `list_tabs` above marks exactly this id `selected`, which is
    // the marker `tab_registry::active_tab` prefers.
    handle.tabs.lock().await.active = Some(tab_id.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::Engine;

    /// The measurement this whole design rests on: real obscura answers
    /// `Target.getTargets` with an EMPTY list on a fresh connection while its
    /// own `/json/list` shows a page. A backend that enumerated would conclude
    /// the browser has no tabs and answer every verb with "No tabs open".
    ///
    /// So the fake scripts exactly that hostile answer and the test requires
    /// `open_tab` to succeed anyway and `list_tabs` to return the tab we
    /// created — i.e. the table is the source of truth and the enumeration is
    /// not consulted at all.
    #[tokio::test]
    async fn an_empty_target_enumeration_does_not_hide_the_tab_we_created() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Target.createTarget",
            Responder::Reply(json!({ "targetId": "T-NEW" })),
        );
        // The hostile answer, scripted. If anything consults it, this test goes
        // red — which is the point.
        server.on(
            "Target.getTargets",
            Responder::Reply(json!({ "targetInfos": [] })),
        );
        server.on(
            "Page.navigate",
            Responder::Reply(json!({ "frameId": "F1", "loaderId": "L1" })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;

        // The load barrier elapses (the fake emits no load event), so
        // `open_tab` surfaces the navigate's timeout — but the TAB exists,
        // which is what this test is about. Assert on the table, not on the
        // navigation.
        let err = backend.open_tab("https://ok.example/").await.err();
        assert!(
            err.is_some(),
            "the fake emits no load event, so the navigation reports a timeout"
        );

        let tabs = backend.list_tabs().await.expect("list ok");
        assert_eq!(
            tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["T-NEW"],
            "the created tab must be listed even though the enumeration is \
             empty: {tabs:?}"
        );
        // This URL claim rests on an ordering that is easy to lose: `navigate`
        // runs `apply_document_boundary` — with the requested URL as the
        // fallback when no `frameNavigated` arrived — BEFORE it returns the
        // barrier's timeout error. Move that call after the error return and
        // this assertion goes red while the one above stays green.
        assert_eq!(
            tabs[0].url, "https://ok.example/",
            "and it must carry the URL it landed on: {tabs:?}"
        );
    }

    /// `switch_tab` is only real if the NEXT reader of "which tab is active"
    /// sees it. The two halves live in different functions, so the test spans
    /// both rather than asserting that `Target.activateTarget` was called
    /// (判据 §4: assert the effect arrived, not that the call happened).
    #[tokio::test]
    async fn a_switched_tab_is_the_one_the_listing_marks_selected() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on("Target.activateTarget", Responder::Reply(json!({})));
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;

        // Two tabs in the table — the only way tabs come to exist now.
        let handle = backend.handle().await.expect("pre-seeded handle");
        for id in ["AAA", "BBB"] {
            handle
                .attach_tab(&aleph_cdp::TargetId(id.to_string()))
                .await
                .expect("attach ok");
        }

        let tabs = backend.list_tabs().await.expect("list ok");
        assert_eq!(tabs.len(), 2, "both tabs are listed: {tabs:?}");

        backend.switch_tab("BBB").await.expect("switch ok");
        let tabs = backend.list_tabs().await.expect("list ok");
        let selected: Vec<&str> = tabs
            .iter()
            .filter(|t| t.selected)
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(
            selected,
            vec!["BBB"],
            "exactly the switched tab must carry the marker: {tabs:?}"
        );

        // And back the other way, so a `selected: true` written unconditionally
        // cannot pass by coincidence with one tab marked.
        backend.switch_tab("AAA").await.expect("switch ok");
        let tabs = backend.list_tabs().await.expect("list ok");
        let selected: Vec<&str> = tabs
            .iter()
            .filter(|t| t.selected)
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(selected, vec!["AAA"], "the marker must MOVE: {tabs:?}");
    }

    /// **The listing is ordered**, because `tab_registry::active_tab` falls back
    /// to LAST-listed when nothing carries the `[selected]` marker — so an
    /// arbitrary order makes that fallback pick an arbitrary tab.
    ///
    /// # Why six tabs and not two
    ///
    /// This claim lived inside the switch test with **two** tabs, and against a
    /// dropped `sort_by` that is a coin flip rather than a guard: a two-entry
    /// `HashMap` walks in sorted order about half the time. **Measured** — the
    /// mutation was applied and the test binary run 12 times under
    /// `--test-threads=1`: 6 FAILED, 6 ok. A guard that fires on half its runs
    /// reports the allocator's mood, not the code (判据 §2 — it looks like it is
    /// working, and the only question that separates it from one that is not is
    /// "in what case does this go red", to which the honest answer was "some of
    /// them").
    ///
    /// Six ids inserted in reverse leaves a 1-in-720 accidental pass, and the
    /// assertion is the whole sequence rather than a spot check.
    #[tokio::test]
    async fn the_listing_is_ordered_whatever_order_the_tabs_were_inserted_in() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("pre-seeded handle");

        let want = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"];
        for id in want.iter().rev() {
            handle
                .attach_tab(&aleph_cdp::TargetId((*id).to_string()))
                .await
                .expect("attach ok");
        }

        let tabs = backend.list_tabs().await.expect("list ok");
        assert_eq!(
            tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            want.to_vec(),
            "the listing must be ordered by the id the browser assigned, \
             whatever order the table happens to walk in: {tabs:?}"
        );
    }

    /// `Target.closeTarget` answering `success: false` is the engine saying it
    /// did NOT close the tab. Dropping the entry anyway would leave a page open
    /// that nothing can address again (判据 §8: a refusal is not an absence).
    #[tokio::test]
    async fn a_refused_close_leaves_the_tab_in_the_table() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Target.closeTarget",
            Responder::Reply(json!({ "success": false })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");

        let err = backend
            .close_tab("T1")
            .await
            .expect_err("the engine refused the close");
        assert!(
            matches!(err, crate::browser::error::BrowserError::TabNotFound(ref id) if id == "T1"),
            "got {err:?}"
        );
        let tabs = backend.list_tabs().await.expect("list ok");
        assert_eq!(
            tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["T1"],
            "a tab the engine would not close must stay addressable: {tabs:?}"
        );
    }
}
