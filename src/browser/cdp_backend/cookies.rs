//! Cookies, and the storage state that is mostly cookies.
//!
//! `BrowserBackend::cookies` takes no tab id, so every operation runs against
//! the profile's active tab: cookies are scoped to an origin, and the active
//! tab is the only origin this backend can name.
//!
//! The two log readers are NOT here. They read the console and network rings,
//! which `events`' pump is the only writer of; putting them beside the cookie
//! jar would make this file name a small lie to every future grep.

use std::path::Path;

use aleph_cdp::methods::{network, runtime};
use aleph_cdp::SessionId;

use crate::browser::engine::EngineHandle;
use crate::browser::error::BrowserError;
use crate::browser::page_state::quote;
use crate::browser::types::CookieOp;

use super::{map_cdp_err, CdpBackend};

/// The active tab's session and current URL.
///
/// `NoSession` rather than a silent no-op when there is no tab: "there were no
/// cookies" and "nothing looked" must not share a spelling (判据 §8).
/// `no_tab_is_a_named_refusal_not_an_empty_jar` is the falsifier.
///
/// **"Which tab is active" is derived in exactly one place**, and this is not
/// it: [`super::tabs::list_tabs`] orders the table by the id the browser
/// assigned — and says in its own comment why an arbitrary order must not be
/// allowed to decide anything — and `tab_registry::active_tab_id` applies the
/// selected-else-last rule to that order. Every other browser verb resolves the
/// active tab that way.
///
/// This function used to read `entries.keys().next()`, a second answer to the
/// same question over a `HashMap`, free to disagree with the first whenever
/// `active` is `None` — which `tabs::close_tab` makes a **designed, reachable**
/// state when the selected tab is the one closed. The consequence was a
/// domainless cookie attached to a page the model never named, reported as
/// success, and picked by `RandomState` so it could differ between two runs of
/// the same binary (判据 §12: the order has to be established where it is
/// applied; §16: the twin had already answered this).
async fn active_page(
    be: &CdpBackend,
) -> Result<(std::sync::Arc<EngineHandle>, SessionId, Option<String>), BrowserError> {
    let handle = be.handle().await?;
    let listing = super::tabs::list_tabs(be).await?;
    let Some(tab_id) = crate::browser::tab_registry::active_tab_id(&listing) else {
        return Err(BrowserError::NoSession(be.profile_name().to_string()));
    };
    let session = handle.ensure_tab(&tab_id).await?;
    let res = runtime::evaluate(&handle.conn, Some(&session), "location.href", false)
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Runtime.evaluate", e))?;
    let url = res
        .value
        .as_str()
        .filter(|s| s.starts_with("http"))
        .map(str::to_string);
    Ok((handle, session, url))
}

/// One cookie, one line. Self-describing because the model reads it directly,
/// and the attribute names are the ones `browser_cookies` accepts back, so a
/// listing can be copied into a `set`.
fn render_cookie(c: &network::Cookie) -> String {
    let mut line = format!(
        "{}={}; domain={}; path={}",
        c.name, c.value, c.domain, c.path
    );
    if let Some(exp) = c.expires.filter(|e| *e > 0.0) {
        // Printed as the float CDP gave, not cast to `i64`: a cookie expiry is
        // a unix second and would never saturate, but a lossy cast inside a
        // rendered fact is a fact that quietly stops being the one measured.
        line.push_str(&format!("; expires={exp}"));
    }
    if c.http_only {
        line.push_str("; httpOnly");
    }
    if c.secure {
        line.push_str("; secure");
    }
    if let Some(ss) = &c.same_site {
        line.push_str(&format!("; sameSite={ss}"));
    }
    line
}

pub(super) async fn cookies(be: &CdpBackend, op: &CookieOp) -> Result<String, BrowserError> {
    let (handle, session, url) = active_page(be).await?;
    let s = Some(&session);
    match op {
        CookieOp::List { domain, path } => {
            let all = network::get_all_cookies(&handle.conn, s)
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Network.getAllCookies", e))?;
            let matched: Vec<String> = all
                .iter()
                .filter(|c| {
                    domain.as_ref().is_none_or(|d| {
                        c.domain.trim_start_matches('.') == d.trim_start_matches('.')
                    }) && path.as_ref().is_none_or(|p| &c.path == p)
                })
                .map(render_cookie)
                .collect();
            // An empty jar says so in words: an empty string would read as "the
            // call did nothing".
            Ok(if matched.is_empty() {
                "no cookies match".to_string()
            } else {
                matched.join("\n")
            })
        }
        CookieOp::Get { name } => {
            let all = network::get_all_cookies(&handle.conn, s)
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Network.getAllCookies", e))?;
            Ok(all
                .iter()
                .find(|c| &c.name == name)
                .map_or_else(|| format!("no cookie named '{name}'"), render_cookie))
        }
        CookieOp::Set {
            name,
            value,
            domain,
            path,
            expires,
            http_only,
            secure,
            same_site,
        } => {
            // With no domain the cookie belongs to the page the browser is on;
            // CDP expresses that as the URL, and guessing a domain would put the
            // cookie somewhere the page cannot read it.
            let cookie = network::Cookie {
                name: name.clone(),
                value: value.clone(),
                domain: domain.clone().unwrap_or_default(),
                path: path.clone().unwrap_or_else(|| "/".to_string()),
                expires: expires.map(|e| e as f64),
                http_only: http_only.unwrap_or(false),
                secure: secure.unwrap_or(false),
                same_site: same_site.map(|ss| ss.as_cli().to_string()),
            };
            if cookie.domain.is_empty() && url.is_none() {
                return Err(BrowserError::ActionFailed(
                    "setting a cookie needs either a domain or a page on an \
                     http(s) URL to attach it to"
                        .into(),
                ));
            }
            network::set_cookies(
                &handle.conn,
                s,
                std::slice::from_ref(&cookie),
                url.as_deref(),
            )
            .await
            .map_err(|e| map_cdp_err(be.engine(), "Network.setCookies", e))?;
            Ok(format!(
                "set cookie '{name}' on {}",
                if cookie.domain.is_empty() {
                    url.clone().unwrap_or_default()
                } else {
                    cookie.domain.clone()
                }
            ))
        }
        CookieOp::Delete { name } => {
            network::delete_cookies(&handle.conn, s, name, None, None)
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Network.deleteCookies", e))?;
            Ok(format!("deleted cookie '{name}'"))
        }
        CookieOp::Clear => {
            network::clear_browser_cookies(&handle.conn, s)
                .await
                .map_err(|e| map_cdp_err(be.engine(), "Network.clearBrowserCookies", e))?;
            Ok("cleared all cookies".to_string())
        }
    }
}

/// Read the active page's `localStorage`. One origin — the one the browser is
/// actually on — and the file records WHICH origin it was, so a restore can tell
/// "this origin had nothing" from "this origin was never captured".
async fn local_storage(
    be: &CdpBackend,
    handle: &EngineHandle,
    session: &SessionId,
) -> Result<Option<(String, Vec<(String, String)>)>, BrowserError> {
    let js = "(() => { try { return [location.origin, \
              Object.entries(localStorage)]; } catch (e) { return null; } })()";
    let res = runtime::evaluate(&handle.conn, Some(session), js, false)
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Runtime.evaluate", e))?;
    let Some(arr) = res.value.as_array() else {
        return Ok(None);
    };
    let Some(origin) = arr.first().and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let items = arr
        .get(1)
        .and_then(|v| v.as_array())
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|p| {
                    let k = p.get(0)?.as_str()?.to_string();
                    let v = p.get(1)?.as_str()?.to_string();
                    Some((k, v))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some((origin.to_string(), items)))
}

pub(super) async fn save_state(be: &CdpBackend, path: &Path) -> Result<(), BrowserError> {
    let (handle, session, _url) = active_page(be).await?;
    let cookies = network::get_all_cookies(&handle.conn, Some(&session))
        .await
        .map_err(|e| map_cdp_err(be.engine(), "Network.getAllCookies", e))?;
    let origins = match local_storage(be, &handle, &session).await? {
        Some((origin, items)) => serde_json::json!([{
            "origin": origin,
            "localStorage": items.iter()
                .map(|(k, v)| serde_json::json!({"name": k, "value": v}))
                .collect::<Vec<_>>(),
        }]),
        // An origin that could not be read is ABSENT, not empty. A readable
        // origin with nothing in it is `[{origin, localStorage: []}]`; an
        // unreadable one is `[]`. Collapsing those would make a restore unable
        // to tell "this origin had nothing" from "this origin was never
        // captured" (判据 §8).
        None => serde_json::json!([]),
    };
    // Playwright's `storageState` shape on purpose: a state saved by this driver
    // loads in the managed one and vice versa, so switching drivers does not
    // cost the login.
    let doc = serde_json::json!({
        "cookies": cookies.iter().map(|c| serde_json::json!({
            "name": c.name, "value": c.value, "domain": c.domain, "path": c.path,
            "expires": c.expires.unwrap_or(-1.0), "httpOnly": c.http_only,
            "secure": c.secure,
            "sameSite": c.same_site.clone().unwrap_or_else(|| "Lax".into()),
        })).collect::<Vec<_>>(),
        "origins": origins,
    });
    tokio::fs::write(
        path,
        serde_json::to_vec_pretty(&doc).map_err(|e| BrowserError::Io(std::io::Error::other(e)))?,
    )
    .await
    .map_err(BrowserError::Io)
}

pub(super) async fn load_state(be: &CdpBackend, path: &Path) -> Result<(), BrowserError> {
    let bytes = tokio::fs::read(path).await.map_err(BrowserError::Io)?;
    let doc: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        BrowserError::ActionFailed(format!("{} is not a storage state: {e}", path.display()))
    })?;
    let (handle, session, _url) = active_page(be).await?;

    let cookies: Vec<network::Cookie> = doc["cookies"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(network::Cookie {
                        name: c["name"].as_str()?.to_string(),
                        value: c["value"].as_str()?.to_string(),
                        domain: c["domain"].as_str().unwrap_or_default().to_string(),
                        path: c["path"].as_str().unwrap_or("/").to_string(),
                        expires: c["expires"].as_f64(),
                        http_only: c["httpOnly"].as_bool().unwrap_or(false),
                        secure: c["secure"].as_bool().unwrap_or(false),
                        same_site: c["sameSite"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if !cookies.is_empty() {
        network::set_cookies(&handle.conn, Some(&session), &cookies, None)
            .await
            .map_err(|e| map_cdp_err(be.engine(), "Network.setCookies", e))?;
    }

    // localStorage can only be written from a page that is already on the
    // origin. Restoring the rest is a real restore; skipping an origin is
    // LOGGED rather than swallowed, because a silently skipped restore is
    // indistinguishable from one that happened.
    let here = local_storage(be, &handle, &session).await?.map(|(o, _)| o);
    for origin in doc["origins"].as_array().into_iter().flatten() {
        let Some(name) = origin["origin"].as_str() else {
            continue;
        };
        if here.as_deref() != Some(name) {
            tracing::warn!(
                origin = %name,
                "browser_session load: localStorage for this origin was not \
                 restored — no open tab is on it"
            );
            continue;
        }
        let items = origin["localStorage"].clone();
        let js = format!(
            "(() => {{ for (const it of {items}) {{ \
             localStorage.setItem(it.name, it.value); }} return true; }})()"
        );
        let res = runtime::evaluate(&handle.conn, Some(&session), &js, false)
            .await
            .map_err(|e| map_cdp_err(be.engine(), "Runtime.evaluate", e))?;
        // **The `EvalResult` is read, not dropped.** A script that throws is an
        // `Ok(EvalResult { exception: Some(..) })`, never an `Err` — the
        // wrapper's own doc says so and calls it 判据 §8 — so discarding the
        // result turned `localStorage.setItem` throwing (quota exceeded, site
        // data blocked for the origin, a sandboxed origin where storage access
        // raises `SecurityError`) into `success: true, "Loaded session 'x'"`.
        // The model then navigates believing it is authenticated, meets a login
        // wall, and has no thread back to the restore — while the cookie half
        // above very likely DID land, so the state is half-restored and the
        // symptom is site-specific.
        if let Some(detail) = res.exception {
            return Err(BrowserError::ActionFailed(format!(
                "restoring localStorage for origin {} threw: {} — the cookies in \
                 this state file were applied first, so the session is \
                 HALF-restored; re-run browser_session{{action:\"load\"}} after \
                 clearing site data for that origin, or continue knowing only \
                 the cookies are in place",
                quote(name),
                // QUOTED (R40): the page picks this message, and it reaches the
                // model outside the untrusted-content fence — the same channel
                // `actions::call_on_node` quotes for.
                quote(&detail)
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use super::{quote, CdpBackend};
    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
    use crate::browser::engine::registry::EngineRegistry;
    use crate::browser::engine::Engine;
    use crate::browser::error::BrowserError;
    use crate::browser::types::CookieOp;

    /// A cookie set with no domain belongs to the page the browser is on. CDP
    /// expresses that as the URL; omitting both puts the cookie on a domain the
    /// page cannot read, and `document.cookie` stays empty while the tool
    /// reports success.
    #[tokio::test]
    async fn a_domainless_cookie_is_attached_to_the_pages_url() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Runtime.evaluate",
            Responder::Reply(json!({ "result": { "type": "string",
                "value": "http://127.0.0.1:18898/tools.html" } })),
        );
        server.on("Network.setCookies", Responder::Reply(json!({})));
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        handle.tabs.lock().await.active = Some("T1".into());

        let out = backend
            .cookies(&CookieOp::Set {
                name: "qa_cookie".into(),
                value: "v".into(),
                domain: None,
                path: Some("/".into()),
                expires: None,
                http_only: None,
                secure: None,
                same_site: None,
            })
            .await
            .expect("set ok");
        assert!(out.contains("qa_cookie"), "{out}");

        let call = server
            .received()
            .into_iter()
            .find(|m| m["method"].as_str() == Some("Network.setCookies"))
            .expect("the set reached the wire");
        assert_eq!(
            call["params"]["cookies"][0]["url"],
            json!("http://127.0.0.1:18898/tools.html"),
            "a domainless cookie must carry the page URL: {call}"
        );
    }

    /// No tab is `NoSession` naming the profile, not "no cookies match".
    ///
    /// The two are one `else` apart and they mean opposite things: an empty
    /// listing says the jar was looked in, and this says nothing looked
    /// (判据 §8). Every verb in this file goes through `active_page`, so this
    /// one assertion covers `cookies`, `save_state` and `load_state`.
    #[tokio::test]
    async fn no_tab_is_a_named_refusal_not_an_empty_jar() {
        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        // No `attach_tab`: the handle exists and has no tabs, which is the
        // state a freshly-parked engine is in.

        let err = backend
            .cookies(&CookieOp::List {
                domain: None,
                path: None,
            })
            .await
            .expect_err("with no tab there is no origin to answer about");
        match err {
            BrowserError::NoSession(profile) => assert_eq!(profile, "default"),
            other => panic!("expected NoSession naming the profile, got {other:?}"),
        }
        assert!(
            !methods(&server)
                .iter()
                .any(|m| m.starts_with("Network.getAllCookies")),
            "nothing may be read when there is nothing to read it from: {:?}",
            methods(&server)
        );
    }

    /// With no `active` marker and several tabs, the tab a tab-less verb acts
    /// on is the one the ORDERED listing names — not whichever the `HashMap`
    /// happens to yield first.
    ///
    /// `close_tab` clears `active` when the selected tab is the one closed
    /// (deliberately: naming a survivor "would be a guess"), so this is a
    /// reachable state and not a corner. Every fixture in this file used to set
    /// `active` and hold one entry, which left the fallback arm both unexecuted
    /// and — at one entry — unable to expose its non-determinism if it had run.
    ///
    /// ## Why the loop, and what it does and does not buy
    ///
    /// The property under test is deterministic; the INSTRUMENT is not. Under
    /// the defect (`entries.keys().next()`) the answer is drawn from a
    /// per-`HashMap` `RandomState`, so one round would agree with the correct
    /// answer 1 time in 16. A fresh table per round makes those draws
    /// independent, so a defect survives all 12 rounds with probability
    /// 16^-11 ≈ 6e-14. That is the "strengthen until the mutation is
    /// deterministically red" rule spent on a `HashMap`-order instrument rather
    /// than reported as a flake rate.
    #[tokio::test]
    async fn with_no_active_marker_the_tab_is_the_one_the_ordered_listing_names() {
        use crate::browser::engine::{TabEntry, TabTable};
        use aleph_cdp::SessionId;

        let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
        wire_session(&server, "S1");
        server.on(
            "Runtime.evaluate",
            Responder::Reply(json!({ "result": { "type": "string", "value": "about:blank" } })),
        );
        server.on(
            "Network.getAllCookies",
            Responder::Reply(json!({ "cookies": [] })),
        );
        let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");

        // Zero-padded so the id order and the string order agree: "T9" sorts
        // after "T16", and a fixture that read naturally would have pinned the
        // wrong expectation.
        let ids: Vec<String> = (1..=16).map(|i| format!("T{i:02}")).collect();
        let expected_session = format!("S-{}", ids.last().expect("16 ids"));

        for round in 0..12 {
            {
                // A FRESH table each round: `RandomState` is per-instance, so
                // this is what makes the 12 draws independent.
                let mut tabs = handle.tabs.lock().await;
                let mut entries = std::collections::HashMap::new();
                for id in &ids {
                    entries.insert(id.clone(), TabEntry::new(SessionId(format!("S-{id}"))));
                }
                *tabs = TabTable {
                    entries,
                    active: None,
                };
                assert!(
                    tabs.entries.len() > 1 && tabs.active.is_none(),
                    "precondition: with one tab, or with a marker, the two \
                     derivations agree and this test cannot fail"
                );
            }

            let before = server.received().len();
            backend
                .cookies(&CookieOp::List {
                    domain: None,
                    path: None,
                })
                .await
                .expect("listing succeeds");
            let session_used = server
                .received()
                .into_iter()
                .skip(before)
                .find(|m| m["method"].as_str() == Some("Network.getAllCookies"))
                .and_then(|m| m["sessionId"].as_str().map(str::to_string))
                .expect("the listing reached the wire on some session");
            assert_eq!(
                session_used, expected_session,
                "round {round}: the verb acted on a tab the ordered listing \
                 does not name"
            );
        }
    }

    /// An origin that could not be read is ABSENT from the saved state; one
    /// that was read is present even when it holds nothing.
    ///
    /// Both halves, because `[]` and `[{…, localStorage: []}]` are the two
    /// answers that would collapse into one if the `None` arm ever started
    /// writing a synthesised entry — and a restore reading the collapsed form
    /// cannot tell "this origin had nothing" from "this origin was never
    /// captured".
    #[tokio::test]
    async fn an_unreadable_origin_is_absent_from_the_saved_state_not_empty() {
        async fn save_with(eval: serde_json::Value) -> serde_json::Value {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("state.json");
            let server = FakeCdpServer::start(FakeCdpServer::scripted(vec![])).await;
            wire_session(&server, "S1");
            server.on("Runtime.evaluate", Responder::Reply(eval));
            server.on(
                "Network.getAllCookies",
                Responder::Reply(json!({ "cookies": [] })),
            );
            let (_reg, backend) = backend_with(&server, Engine::Chromium, open_guard()).await;
            let handle = backend.handle().await.expect("handle");
            handle
                .attach_tab(&aleph_cdp::TargetId("T1".into()))
                .await
                .expect("attach");
            handle.tabs.lock().await.active = Some("T1".into());
            backend.save_state(&path).await.expect("save ok");
            serde_json::from_slice(&std::fs::read(&path).expect("the file was written"))
                .expect("and it is JSON")
        }

        // `null` is what the page returns when `localStorage` throws — a
        // sandboxed or opaque origin. Absent.
        let unreadable = save_with(json!({ "result": { "type": "object", "value": null } })).await;
        assert_eq!(
            unreadable["origins"],
            json!([]),
            "an unreadable origin must not appear at all: {unreadable}"
        );

        // A readable origin with an empty store. Present, and named.
        let readable = save_with(json!({ "result": { "type": "object",
            "value": ["https://example.test", []] } }))
        .await;
        assert_eq!(
            readable["origins"],
            json!([{ "origin": "https://example.test", "localStorage": [] }]),
            "a readable-but-empty origin must be recorded, or a restore cannot \
             tell it from one never captured: {readable}"
        );
    }

    /// The origin every state-file fixture below is on.
    const ORIGIN: &str = "https://example.test";

    /// A server whose `Runtime.evaluate` answers the origin probe with
    /// [`ORIGIN`] and the restore script with either `true` or a thrown error.
    ///
    /// The branch is on the EXPRESSION, not on a call counter: `active_page`
    /// and `local_storage` both evaluate before the restore does, and an
    /// order-based fixture would silently re-target the day either of them
    /// stops asking. `Responder` is a value and cannot vary per call, so this
    /// lives in the constructor closure — the override table `on` writes to is
    /// consulted first, so `wire_session`'s entries still win.
    async fn server_with_restore(throwing: Option<&'static str>) -> FakeCdpServer {
        FakeCdpServer::start(move |frame: &serde_json::Value| {
            match frame["method"].as_str() {
                Some("Runtime.evaluate") => {
                    let expr = frame["params"]["expression"].as_str().unwrap_or("");
                    if expr.contains("localStorage.setItem") {
                        return match throwing {
                            Some(detail) => Responder::Reply(json!({
                                "result": { "type": "undefined" },
                                "exceptionDetails": {
                                    "exception": { "description": detail }
                                }
                            })),
                            None => Responder::Reply(
                                json!({ "result": { "type": "boolean", "value": true } }),
                            ),
                        };
                    }
                    // The origin probe — and `active_page`'s `location.href`,
                    // which reads this as a non-string and honestly answers
                    // "no page URL".
                    Responder::Reply(json!({ "result": { "type": "object",
                        "value": [ORIGIN, [["tok", "v1"]]] } }))
                }
                Some("Network.getAllCookies") => Responder::Reply(json!({ "cookies": [{
                    "name": "sid", "value": "abc", "domain": ".example.test",
                    "path": "/", "httpOnly": true, "secure": true
                }]})),
                _ => Responder::Reply(json!({})),
            }
        })
        .await
    }

    async fn backend_on_a_tab(server: &FakeCdpServer) -> (Arc<EngineRegistry>, CdpBackend) {
        let (reg, backend) = backend_with(server, Engine::Chromium, open_guard()).await;
        let handle = backend.handle().await.expect("handle");
        handle
            .attach_tab(&aleph_cdp::TargetId("T1".into()))
            .await
            .expect("attach");
        handle.tabs.lock().await.active = Some("T1".into());
        (reg, backend)
    }

    /// The OUTBOUND half of a round trip: what `save_state` wrote, `load_state`
    /// puts back on the wire — the cookies as a `Network.setCookies` payload
    /// and the origin's items as a `localStorage.setItem` script.
    ///
    /// ⚠️ **What this does NOT constrain.** The fake answers whatever it is
    /// told to, so this pins the payload Aleph emits and nothing about how a
    /// real engine READS it. `expires: -1` is the case that matters — CDP's
    /// session-cookie sentinel — and whether Chromium and obscura agree on it
    /// is a real-machine question, deferred to Task 16's prober. A round trip
    /// against a fake is not evidence about an engine.
    #[tokio::test]
    async fn a_saved_state_is_put_back_on_the_wire_when_it_is_loaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");

        let server = server_with_restore(None).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_on_a_tab(&server).await;

        backend.save_state(&path).await.expect("save ok");
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("written")).expect("json");
        assert_eq!(doc["cookies"][0]["name"], json!("sid"), "{doc}");
        assert_eq!(doc["origins"][0]["origin"], json!(ORIGIN), "{doc}");
        assert_eq!(
            doc["origins"][0]["localStorage"][0]["name"],
            json!("tok"),
            "the captured item must be in the file: {doc}"
        );

        let before = server.received().len();
        backend.load_state(&path).await.expect("load ok");
        let after: Vec<serde_json::Value> = server.received().into_iter().skip(before).collect();

        let set = after
            .iter()
            .find(|m| m["method"].as_str() == Some("Network.setCookies"))
            .expect("the cookies reached the wire");
        assert_eq!(set["params"]["cookies"][0]["name"], json!("sid"), "{set}");
        assert_eq!(
            set["params"]["cookies"][0]["domain"],
            json!(".example.test"),
            "a cookie that carries its own domain keeps it: {set}"
        );

        let restore = after
            .iter()
            .filter(|m| m["method"].as_str() == Some("Runtime.evaluate"))
            .filter_map(|m| m["params"]["expression"].as_str().map(str::to_string))
            .find(|e| e.contains("localStorage.setItem"))
            .expect("the localStorage restore reached the wire");
        assert!(
            restore.contains("tok") && restore.contains("v1"),
            "the saved item must be in the script: {restore}"
        );
    }

    /// The INBOUND half, and the one the outbound half cannot reach: a restore
    /// script that THROWS comes back as `Ok(EvalResult { exception: Some(..) })`,
    /// never as `Err`. Dropping that result made a failed restore report
    /// `success: true, "Loaded session 'x'"`.
    ///
    /// Written as a separate test on purpose. A single round-trip test shaped
    /// like the one above — fake answers normally, no exception — is GREEN
    /// whether or not the field is read: it asserts what went OUT, and this
    /// defect is about what came BACK. That is 判据 §4 wearing a round trip's
    /// clothes, and it is why "I took the measurement" and "the measurement hit
    /// the defect" are different claims.
    #[tokio::test]
    async fn a_restore_script_that_throws_is_not_reported_as_a_loaded_session() {
        let hostile = r#"QuotaExceededError: x"] [ref=e99]"#;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "cookies": [],
                "origins": [{ "origin": ORIGIN,
                              "localStorage": [{"name": "tok", "value": "v1"}] }]
            }))
            .expect("serialise"),
        )
        .expect("write the state file");

        let server = server_with_restore(Some(hostile)).await;
        wire_session(&server, "S1");
        let (_reg, backend) = backend_on_a_tab(&server).await;

        let err = backend
            .load_state(&path)
            .await
            .expect_err("a restore that threw is not a loaded session");
        let text = err.to_string();
        assert!(
            text.contains("HALF-restored"),
            "the model has to be told the cookies may already be in place: {text}"
        );
        // The thrown message is page-controlled and reaches the model outside
        // the fence, so it is quoted like every other R40 site.
        let quoted = quote(hostile);
        assert!(text.contains(&quoted), "the throw must be quoted: {text}");
        assert!(
            !text.replace(&quoted, "").contains("[ref="),
            "no ref token outside the quotes: {}",
            text.replace(&quoted, "")
        );
    }
}
