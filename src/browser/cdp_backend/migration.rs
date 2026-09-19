//! Carrying a profile's identity from one engine's process to the other's.
//!
//! `--user-data-dir` (Chromium) and `--storage-dir` (obscura) persist different
//! formats and neither reads the other's, so a switch cannot hand over a
//! directory — it has to re-state the state over CDP. The honest definition of
//! "seamless" is spec §5.5's: **the login survives, the page progress does
//! not.** That sentence is written out for the model as a literal in
//! `BrowserSessionTool::DESCRIPTION`; it is prose about a design decision, not a
//! rendering of anything here, so there is deliberately no table for it to
//! drift from. The per-engine facts that ARE table-shaped reach the model the
//! other way — as the result of `browser_session{action:"capabilities"}`.
//!
//! Failure policy, and the reason for it: cookies and tabs are **errors**
//! (a switch that silently drops the session is exactly the "报成功的 no-op"
//! shape, 判据 §11); localStorage and scroll are **warnings** (per-origin, best
//! effort — a `SecurityError` on a sandboxed frame is not a reason to strand
//! the user on a browser they asked to leave). Warnings travel in the return
//! value, never a log line.
//!
//! Every CDP error goes through [`super::map_cdp_err`]. That is not tidiness:
//! it is the only thing that turns obscura's global command barrier into
//! `EngineBusy{engine, method, waited_secs}` instead of a generic failure, and
//! `EngineBusy` is the error whose text names the verb that gets out of it.

use std::collections::HashMap;

use aleph_cdp::methods::{network, page, runtime, target};
use aleph_cdp::SessionId;

use super::evaluate::as_call_expression;
use super::map_cdp_err;
use super::navigate::{apply_document_boundary, wait_for_load};
use crate::browser::engine::{Engine, EngineHandle};
use crate::browser::error::BrowserError;

/// One tab, as it must be re-created on the other engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabMigration {
    pub url: String,
    pub scroll_x: i32,
    pub scroll_y: i32,
    pub active: bool,
}

/// Everything read off the source engine, before anything is written anywhere.
#[derive(Clone, Debug, Default)]
pub(crate) struct MigrationState {
    pub cookies: Vec<network::Cookie>,
    /// **Order is explicitly not guaranteed**, and that is a decision rather
    /// than an oversight: it is `HashMap` iteration order over
    /// `TabTable.entries`, so it differs run to run. Nothing downstream may
    /// depend on it — the target's tab positions are arbitrary, and
    /// `get_all_cookies` is issued on whichever session lands first, which is
    /// safe only because cookies are browser-context-wide rather than
    /// per-session. Making it deterministic would mean choosing an order the
    /// tab table does not have; saying so is cheaper and honest (判据 §12).
    pub tabs: Vec<TabMigration>,
    /// `(origin, [(key, value)])`, one entry per distinct origin among the open
    /// tabs. Two tabs on one origin share one entry — a second read of the same
    /// store would answer the same thing.
    pub local_storage: Vec<(String, Vec<(String, String)>)>,
    /// What did not come across. Never empty for a lossy export.
    pub warnings: Vec<String>,
}

/// What actually landed on the target.
#[derive(Clone, Debug, Default)]
pub(crate) struct ImportReport {
    pub cookies_moved: usize,
    pub tabs_reopened: usize,
    pub local_storage_origins: usize,
    /// The target-side tab id carrying the source's active tab, if any.
    pub active_tab: Option<String>,
    pub warnings: Vec<String>,
}

/// `scheme://host[:port]` for a page URL, or `None` for anything that is not a
/// tuple origin (`about:blank`, `data:`). A non-tuple origin has no
/// localStorage to move, so `None` is a fact and not a failure.
///
/// Without the `is_tuple` filter every `about:blank` tab would serialise as the
/// origin `"null"` and they would all collide into one made-up store.
fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

const READ_SCROLL: &str = "[window.scrollX, window.scrollY]";
const READ_LOCAL_STORAGE: &str = "JSON.stringify(Object.entries(localStorage))";

/// Read every migratable fact off `handle`. Nothing is written anywhere and the
/// source engine is untouched, so a failure here costs the caller nothing.
pub(crate) async fn export_state(handle: &EngineHandle) -> Result<MigrationState, BrowserError> {
    let engine = handle.engine;
    let conn = &handle.conn;

    // (session, url, active), taken under the lock and then released, so no CDP
    // round trip happens while holding it.
    //
    // The URL comes from `TabEntry.url` — the tab table, which the navigation
    // barrier and the persistent `Page.frameNavigated` arm keep true. It
    // deliberately does NOT come from `Target.getTargets`: obscura answers that
    // with an empty list even on its own connection (R13,
    // `aleph_cdp::methods::target::get_targets`' own doc), so an
    // enumeration-based export would carry no tabs at all in the default
    // direction while every unit test stayed green.
    let open: Vec<(SessionId, String, bool)> = {
        let tabs = handle.tabs.lock().await;
        tabs.entries
            .iter()
            .map(|(id, e)| {
                (
                    e.session.clone(),
                    e.url.clone(),
                    tabs.active.as_deref() == Some(id.as_str()),
                )
            })
            .collect()
    };

    let mut state = MigrationState::default();
    let Some((first_session, _, _)) = open.first().cloned() else {
        // A handle with no tab has no page session, and every cookie method is
        // session-scoped. Say what did not happen instead of returning an empty
        // success that reads like "there was nothing to move".
        state.warnings.push(format!(
            "{engine} had no open tab: no cookies, tabs or localStorage were exported"
        ));
        return Ok(state);
    };

    // Cookies first: the whole browser context in one call, and the one thing
    // whose loss makes the switch pointless.
    state.cookies = network::get_all_cookies(conn, Some(&first_session))
        .await
        .map_err(|e| map_cdp_err(engine, "Network.getAllCookies", e))?;

    let mut seen_origins: Vec<String> = Vec::new();
    for (session, url, active) in &open {
        // Scroll via the page, not `Page.getLayoutMetrics`: the two engines
        // disagree about what that method reports for a scrolled document, and
        // `window.scrollX/Y` is the value the restore side writes back with
        // `window.scrollTo`. One derivation for read and write.
        let (scroll_x, scroll_y) =
            match runtime::evaluate(conn, Some(session), READ_SCROLL, false).await {
                Ok(res) if res.exception.is_none() => decode_scroll(&res.value).unwrap_or((0, 0)),
                Ok(res) => {
                    state.warnings.push(format!(
                        "scroll position of {url} was not readable ({}); it will open at the top",
                        res.exception.unwrap_or_default()
                    ));
                    (0, 0)
                }
                Err(e) => {
                    state.warnings.push(format!(
                        "scroll position of {url} was not readable ({}); it will open at the top",
                        map_cdp_err(engine, "Runtime.evaluate", e)
                    ));
                    (0, 0)
                }
            };
        state.tabs.push(TabMigration {
            url: url.clone(),
            scroll_x,
            scroll_y,
            active: *active,
        });

        let Some(origin) = origin_of(url) else {
            continue;
        };
        if seen_origins.contains(&origin) {
            continue;
        }
        seen_origins.push(origin.clone());
        match runtime::evaluate(conn, Some(session), READ_LOCAL_STORAGE, false).await {
            Ok(res) if res.exception.is_none() => match decode_local_storage(&res.value) {
                Some(pairs) if !pairs.is_empty() => state.local_storage.push((origin, pairs)),
                Some(_) => {}
                None => state.warnings.push(format!(
                    "localStorage for {origin} was not in the expected shape and was not migrated"
                )),
            },
            Ok(res) => state.warnings.push(format!(
                "localStorage for {origin} was not migrated: {}",
                res.exception.unwrap_or_default()
            )),
            Err(e) => state.warnings.push(format!(
                "localStorage for {origin} was not migrated: {}",
                map_cdp_err(engine, "Runtime.evaluate", e)
            )),
        }
    }

    Ok(state)
}

/// `[x, y]` → ints. `None` means "not that shape", which the caller reports.
fn decode_scroll(value: &serde_json::Value) -> Option<(i32, i32)> {
    let arr = value.as_array()?;
    let x = arr.first()?.as_f64()?.round() as i32;
    let y = arr.get(1)?.as_f64()?.round() as i32;
    Some((x, y))
}

/// `[["k","v"],…]` (JSON, possibly nested in a string) → pairs.
fn decode_local_storage(value: &serde_json::Value) -> Option<Vec<(String, String)>> {
    let parsed = match value {
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s).ok()?,
        other => other.clone(),
    };
    let rows = parsed.as_array()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let pair = row.as_array()?;
        out.push((
            pair.first()?.as_str()?.to_string(),
            pair.get(1)?.as_str()?.to_string(),
        ));
    }
    Some(out)
}

/// JS that installs a whole localStorage payload.
///
/// The payload is built with `serde_json` so a key or value containing a quote
/// cannot end the expression — these are page-authored bytes, which is the one
/// place a hand-built string is a bug. The wrapper is
/// [`super::evaluate::as_call_expression`], not a second locally-written IIFE:
/// "how a script becomes a call" has one owner.
fn import_local_storage_js(pairs: &[(String, String)]) -> String {
    let payload = serde_json::Value::Array(
        pairs
            .iter()
            .map(|(k, v)| serde_json::json!([k, v]))
            .collect(),
    );
    as_call_expression(&format!(
        "() => {{ for (const [k, v] of {payload}) localStorage.setItem(k, v); return 'ok'; }}"
    ))
}

/// Write `state` into `handle`'s engine and re-open its tabs.
///
/// Order is load-bearing: cookies are installed **before** the first tab is
/// created, because a tab that loads before its cookie is an unauthenticated
/// page and the model would read the login screen as the site.
///
/// **This deliberately does not re-run the SSRF pre-check** that
/// `cdp_backend::navigate::navigate` runs. These URLs are not model input: they
/// came off tabs the guard already admitted, on this same profile, moments ago,
/// and `apply_document_boundary` + the caller's post-switch snapshot still see
/// wherever they actually land. Re-vetting them would also mean a policy edited
/// between the two engines could strand a switch half-done — the worst state
/// this module can be in. If that trade is ever reversed, the change is to
/// route this through `CdpBackend` rather than to add a second guard call here.
pub(crate) async fn import_state(
    handle: &EngineHandle,
    state: &MigrationState,
) -> Result<ImportReport, BrowserError> {
    let engine = handle.engine;
    let conn = &handle.conn;
    let budget = conn.command_timeout();
    let mut report = ImportReport {
        warnings: state.warnings.clone(),
        ..ImportReport::default()
    };

    if !state.cookies.is_empty() {
        let boot = boot_session(handle).await?;
        // `set_cookies`' fourth argument attributes cookies that carry no
        // domain of their own. Everything `Network.getAllCookies` returns is a
        // cookie already IN a store, so it has one, and `None` is the right
        // answer for the whole batch — passing a URL there would attach an
        // origin to cookies that already name their own and could silently
        // narrow them. The `else` arm exists for a `MigrationState` that did
        // not come from `export_state`; it attributes them to the first tab,
        // which is the only origin this function knows.
        let page_url = if state.cookies.iter().all(|c| !c.domain.is_empty()) {
            None
        } else {
            state.tabs.first().map(|t| t.url.as_str())
        };
        network::set_cookies(conn, Some(&boot), &state.cookies, page_url)
            .await
            .map_err(|e| map_cdp_err(engine, "Network.setCookies", e))?;
        report.cookies_moved = state.cookies.len();
    }

    let by_origin: HashMap<&str, &Vec<(String, String)>> = state
        .local_storage
        .iter()
        .map(|(o, p)| (o.as_str(), p))
        .collect();
    let mut imported_origins: Vec<String> = Vec::new();

    for tab in &state.tabs {
        // `about:blank` then navigate, NOT `createTarget{url}` — the same shape
        // and the same reason as `tabs::open_tab`: `Target.createTarget{url}`
        // returns as soon as the target exists and reports neither a loaderId
        // nor a load event, so a tab opened that way has no document boundary
        // to reset its ref table on. Navigating is also what makes the URL
        // recorded below a post-redirect fact rather than the one we asked for.
        let target_id = target::create_target(conn, "about:blank")
            .await
            .map_err(|e| map_cdp_err(engine, "Target.createTarget", e))?;
        let tab_id = target_id.0.clone();
        // `attach_tab` is the ONE attach path (its twin `ensure_tab` is a pure
        // lookup and would answer `TabNotFound` here): it attaches, enables
        // Page/Runtime/Network and inserts the `TabEntry`, so the target's tab
        // table is populated by the same code path every other verb uses.
        let session = handle.attach_tab(&target_id).await?;

        settle(handle, conn, &session, &tab_id, &tab.url, budget, engine).await?;

        // localStorage is origin-scoped and only reachable from a document on
        // that origin, so it is written after the first load and the page is
        // navigated once more (a fresh navigation, not a reload — reloading a
        // POSTed page prompts). A site that reads localStorage at startup
        // therefore sees it; the cost is one extra load per origin.
        if let Some(origin) = origin_of(&tab.url) {
            if let Some(pairs) = by_origin.get(origin.as_str()) {
                if !imported_origins.contains(&origin) {
                    match runtime::evaluate(
                        conn,
                        Some(&session),
                        &import_local_storage_js(pairs),
                        false,
                    )
                    .await
                    {
                        Ok(res) if res.exception.is_none() => {
                            imported_origins.push(origin.clone());
                            if let Err(e) =
                                settle(handle, conn, &session, &tab_id, &tab.url, budget, engine)
                                    .await
                            {
                                report.warnings.push(format!(
                                    "localStorage for {origin} was written but the reload failed \
                                     ({e}); the page may not have read it"
                                ));
                            }
                        }
                        Ok(res) => report.warnings.push(format!(
                            "localStorage for {origin} could not be restored: {}",
                            res.exception.unwrap_or_default()
                        )),
                        Err(e) => report.warnings.push(format!(
                            "localStorage for {origin} could not be restored: {}",
                            map_cdp_err(engine, "Runtime.evaluate", e)
                        )),
                    }
                }
            }
        }

        if tab.scroll_x != 0 || tab.scroll_y != 0 {
            let js = as_call_expression(&format!(
                "() => {{ window.scrollTo({}, {}); return 'ok'; }}",
                tab.scroll_x, tab.scroll_y
            ));
            if let Err(e) = runtime::evaluate(conn, Some(&session), &js, false).await {
                report.warnings.push(format!(
                    "scroll position of {} was not restored: {}",
                    tab.url,
                    map_cdp_err(engine, "Runtime.evaluate", e)
                ));
            }
        }

        if tab.active {
            let mut tabs = handle.tabs.lock().await;
            tabs.active = Some(tab_id.clone());
            report.active_tab = Some(tab_id.clone());
        }
        report.tabs_reopened += 1;
    }

    report.local_storage_origins = imported_origins.len();
    Ok(report)
}

/// Navigate one tab and wait for it, reusing the navigation module's barrier
/// and document boundary.
///
/// `events` is opened BEFORE the command, because a cached page's load event
/// otherwise arrives while nobody is listening and the barrier waits out the
/// whole budget.
#[allow(clippy::too_many_arguments)]
async fn settle(
    handle: &EngineHandle,
    conn: &aleph_cdp::CdpConnection,
    session: &SessionId,
    tab_id: &str,
    url: &str,
    budget: std::time::Duration,
    engine: Engine,
) -> Result<(), BrowserError> {
    let mut events = conn.events();
    let result = page::navigate(conn, Some(session), url)
        .await
        .map_err(|e| map_cdp_err(engine, "Page.navigate", e))?;
    if let Some(text) = result.error_text {
        return Err(BrowserError::EngineFailure {
            engine,
            reason: format!(
                "the new engine refused to load {}: {}",
                crate::browser::page_state::quote(url),
                crate::browser::page_state::quote(&text)
            ),
        });
    }
    let outcome = wait_for_load(&mut events, session, &result.frame_id, budget).await;
    // `Page.navigate`'s own response names the new document when it started
    // one; the event is the fallback for engines that omit it. This order is
    // `navigate`'s and is load-bearing — see the note there: on obscura the
    // event can carry a STALE loader, and a stale loader reads to
    // `RefTable::reset_for_document` as "the document did not change".
    let loader = result.loader_id.or(outcome.loader_id);
    let landed = outcome.url.unwrap_or_else(|| url.to_string());
    apply_document_boundary(handle, tab_id, loader.as_deref(), Some(&landed)).await;
    // A tab that did not finish loading is NOT an error here: the switch has
    // already moved the cookies, and the model's next `browser_snapshot` sees
    // whatever is actually there. Reporting a failure would throw away a
    // migration that mostly worked.
    Ok(())
}

/// A page session on `handle` for the browser-context-wide cookie write.
///
/// `EngineHandle::new` takes a `first_tab`, so a freshly launched handle always
/// has one and this always finds it. There is deliberately **no**
/// `create_target("about:blank")` fallback: an arm that can only run when an
/// invariant is already broken is a safety net nobody has ever tested, and it
/// would hide the breakage rather than report it (判据 §17).
async fn boot_session(handle: &EngineHandle) -> Result<SessionId, BrowserError> {
    let tabs = handle.tabs.lock().await;
    tabs.active
        .as_ref()
        .and_then(|id| tabs.entries.get(id))
        .or_else(|| tabs.entries.values().next())
        .map(|e| e.session.clone())
        .ok_or_else(|| BrowserError::EngineFailure {
            engine: handle.engine,
            reason: "the newly launched engine reported no page to install cookies into; \
                     re-run browser_open"
                .to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use crate::browser::cdp_backend::test_support::methods;

    /// A source engine with one tab on https://example.com/a, one cookie, one
    /// localStorage pair, scrolled to y=480.
    ///
    /// `Target.getTargets` answers `[]` on purpose: that is what real obscura
    /// answers even on its own connection (R13, and
    /// `aleph_cdp::methods::target::get_targets`' own doc). Any implementation
    /// that reads tab URLs from the enumeration fails here, which is the whole
    /// reason the fake is written this way.
    async fn source_server() -> FakeCdpServer {
        FakeCdpServer::start(|req| {
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let expr = req
                .get("params")
                .and_then(|p| p.get("expression"))
                .and_then(|e| e.as_str())
                .unwrap_or("");
            match method {
                "Target.getTargets" => Responder::Reply(json!({ "targetInfos": [] })),
                "Network.getAllCookies" => Responder::Reply(json!({
                    "cookies": [{
                        "name": "sid", "value": "abc123", "domain": "example.com",
                        "path": "/", "expires": -1.0, "httpOnly": true,
                        "secure": false, "sameSite": "Lax"
                    }]
                })),
                "Runtime.evaluate" if expr.contains("scrollX") => Responder::Reply(json!({
                    "result": { "type": "object", "value": [0, 480] }
                })),
                "Runtime.evaluate" if expr.contains("localStorage") => Responder::Reply(json!({
                    "result": { "type": "string", "value": "[[\"theme\",\"dark\"]]" }
                })),
                _ => Responder::Reply(json!({})),
            }
        })
        .await
    }

    #[tokio::test]
    async fn export_reads_cookies_tabs_and_local_storage_from_the_source() {
        let server = source_server().await;
        let handle = crate::browser::testkit::fake_handle(
            Engine::Obscura,
            &server,
            &[
                ("T-A", "https://example.com/a", true),
                // A non-tuple origin, so `origin_of`'s filter has something to
                // filter: `about:blank` has no localStorage to move, and an
                // implementation that dropped the filter would report an
                // origin of "null" as a second store.
                ("T-B", "about:blank", false),
            ],
        )
        .await;

        let state = export_state(&handle).await.expect("export");

        assert_eq!(state.cookies.len(), 1);
        assert_eq!(state.cookies[0].name, "sid");
        assert_eq!(state.cookies[0].value, "abc123");
        assert_eq!(state.tabs.len(), 2);
        let a = state
            .tabs
            .iter()
            .find(|t| t.url == "https://example.com/a")
            .expect("the http tab must migrate");
        assert_eq!(a.scroll_y, 480);
        assert!(a.active);
        assert_eq!(
            state.local_storage,
            vec![(
                "https://example.com".to_string(),
                vec![("theme".to_string(), "dark".to_string())]
            )]
        );
        // The URL did NOT come from target enumeration — the fake answered [].
        assert!(
            !methods(&server).iter().any(|m| m == "Target.getTargets"),
            "tab URLs must come from TabEntry.url, not from an enumeration \
             obscura answers falsely (R13); saw {:?}",
            methods(&server)
        );
    }

    #[tokio::test]
    async fn import_sets_the_same_cookies_and_reopens_every_tab_on_the_target() {
        let src = source_server().await;
        let source = crate::browser::testkit::fake_handle(
            Engine::Obscura,
            &src,
            &[("T-A", "https://example.com/a", true)],
        )
        .await;
        let state = export_state(&source).await.expect("export");

        let dst = FakeCdpServer::start(|req| {
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            match method {
                "Target.createTarget" => Responder::Reply(json!({ "targetId": "T-B" })),
                "Target.attachToTarget" => Responder::Reply(json!({ "sessionId": "S-B" })),
                "Target.getTargets" => Responder::Reply(json!({ "targetInfos": [] })),
                "Page.navigate" => Responder::Reply(json!({ "frameId": "F-B", "loaderId": "L-B" })),
                "Runtime.evaluate" => {
                    Responder::Reply(json!({ "result": { "type": "string", "value": "ok" } }))
                }
                _ => Responder::Reply(json!({})),
            }
        })
        .await;
        // The target starts with its launch tab, as a real one does
        // (`EngineHandle::new` takes a `first_tab`).
        let target = crate::browser::testkit::fake_handle(
            Engine::Chromium,
            &dst,
            &[("T-BOOT", "about:blank", true)],
        )
        .await;

        let report = import_state(&target, &state).await.expect("import");

        assert_eq!(report.cookies_moved, 1);
        assert_eq!(report.tabs_reopened, 1);
        assert_eq!(report.local_storage_origins, 1);

        // The cookies that arrive are the ones that left — read off the params
        // the fake received, not off a value this test also wrote.
        let set = dst
            .received()
            .into_iter()
            .find(|m| m.get("method").and_then(|x| x.as_str()) == Some("Network.setCookies"))
            .expect("Network.setCookies must be sent");
        let cookies = set["params"]["cookies"].as_array().expect("cookies array");
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0]["name"], "sid");
        assert_eq!(cookies[0]["value"], "abc123");
        assert_eq!(cookies[0]["domain"], "example.com");
        // The exported cookie carries its own domain, so nothing on the wire
        // may attribute it to a page URL: a `url` here would let the engine
        // re-derive a narrower origin than the one the cookie already names.
        // `network::set_cookies` only writes the key when `domain` is empty.
        assert!(
            cookies[0].get("url").is_none(),
            "a domain-carrying cookie must not be given a url, got {:?}",
            cookies[0]
        );

        // The tab lands on the source's URL. Read off `Page.navigate`, which is
        // the call that carries the load — `Target.createTarget` is issued on
        // `about:blank` here for the same reason `tabs::open_tab` does it:
        // `createTarget{url}` reports neither a loaderId nor a load event, so a
        // tab opened that way has no document boundary to reset refs on.
        let navigated = dst
            .received()
            .into_iter()
            .find(|m| m.get("method").and_then(|x| x.as_str()) == Some("Page.navigate"))
            .expect("Page.navigate must be sent");
        assert_eq!(navigated["params"]["url"], "https://example.com/a");

        // Cookies before tabs: a tab that loads before its cookie is an
        // unauthenticated page, and the model would read the login screen as
        // the site.
        let seen = methods(&dst);
        let cookies_at = seen.iter().position(|m| m == "Network.setCookies").unwrap();
        let target_at = seen
            .iter()
            .position(|m| m == "Target.createTarget")
            .unwrap();
        assert!(
            cookies_at < target_at,
            "cookies must be installed before any tab is opened, saw {seen:?}"
        );

        let tabs = target.tabs.lock().await;
        assert!(tabs.entries.contains_key("T-B"));
        assert_eq!(tabs.entries["T-B"].url, "https://example.com/a");
        assert_eq!(tabs.active.as_deref(), Some("T-B"));
    }

    #[tokio::test]
    async fn a_cookie_export_failure_is_an_error_and_touches_no_target() {
        let src = FakeCdpServer::start(|req| {
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if method == "Network.getAllCookies" {
                return Responder::Error {
                    code: -32000,
                    message: "cookie store unavailable".into(),
                };
            }
            Responder::Reply(json!({}))
        })
        .await;
        let source = crate::browser::testkit::fake_handle(
            Engine::Obscura,
            &src,
            &[("T-A", "https://example.com/a", true)],
        )
        .await;

        let err = export_state(&source).await.expect_err("must fail closed");
        // `map_cdp_err` turns a protocol error into `BrowserError::Cdp`, which
        // carries the method — so the error says WHICH call failed, and a
        // timeout on the same call would have become `EngineBusy` instead.
        assert!(
            matches!(&err, BrowserError::Cdp { method, .. } if method == "Network.getAllCookies"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn an_engine_barrier_during_export_is_engine_busy_not_engine_failure() {
        // obscura's global command barrier arrives as a per-command timeout.
        // Reporting it as `EngineFailure` would tell the model the engine is
        // broken when it is merely wedged, and lose the verb it should use.
        let src = FakeCdpServer::start(|req| {
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if method == "Network.getAllCookies" {
                return Responder::Delay(
                    std::time::Duration::from_secs(30),
                    Box::new(Responder::Reply(json!({ "cookies": [] }))),
                );
            }
            Responder::Reply(json!({}))
        })
        .await;
        let source = crate::browser::testkit::fake_handle(
            Engine::Obscura,
            &src,
            &[("T-A", "https://example.com/a", true)],
        )
        .await;

        let err = export_state(&source)
            .await
            .expect_err("the barrier must surface");
        assert!(
            matches!(&err, BrowserError::EngineBusy { engine, method, .. }
                     if *engine == Engine::Obscura && method == "Network.getAllCookies"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_local_storage_failure_is_a_warning_not_an_abort() {
        let src = FakeCdpServer::start(|req| {
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let expr = req
                .get("params")
                .and_then(|p| p.get("expression"))
                .and_then(|e| e.as_str())
                .unwrap_or("");
            match method {
                "Network.getAllCookies" => Responder::Reply(json!({ "cookies": [] })),
                "Runtime.evaluate" if expr.contains("scrollX") => Responder::Reply(json!({
                    "result": { "type": "object", "value": [0, 0] }
                })),
                "Runtime.evaluate" => Responder::Error {
                    code: -32000,
                    message: "SecurityError: localStorage is not available".into(),
                },
                _ => Responder::Reply(json!({})),
            }
        })
        .await;
        let source = crate::browser::testkit::fake_handle(
            Engine::Obscura,
            &src,
            &[("T-A", "https://example.com/a", true)],
        )
        .await;

        let state = export_state(&source)
            .await
            .expect("localStorage is best effort");
        assert!(state.local_storage.is_empty());
        assert_eq!(state.tabs.len(), 1, "the tab still migrates");
        assert!(
            state.warnings.iter().any(|w| w.contains("localStorage")),
            "the loss must be stated, got {:?}",
            state.warnings
        );
    }
}
