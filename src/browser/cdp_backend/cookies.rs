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
use crate::browser::types::CookieOp;

use super::{map_cdp_err, CdpBackend};

/// The active tab's session and current URL.
///
/// `NoSession` rather than a silent no-op when there is no tab: "there were no
/// cookies" and "nothing looked" must not share a spelling (判据 §8).
/// `no_tab_is_a_named_refusal_not_an_empty_jar` is the falsifier.
async fn active_page(
    be: &CdpBackend,
) -> Result<(std::sync::Arc<EngineHandle>, SessionId, Option<String>), BrowserError> {
    let handle = be.handle().await?;
    let tab_id = {
        let tabs = handle.tabs.lock().await;
        tabs.active
            .clone()
            .or_else(|| tabs.entries.keys().next().cloned())
    };
    let Some(tab_id) = tab_id else {
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
        runtime::evaluate(&handle.conn, Some(&session), &js, false)
            .await
            .map_err(|e| map_cdp_err(be.engine(), "Runtime.evaluate", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aleph_cdp::testkit::{FakeCdpServer, Responder};
    use serde_json::json;

    use crate::browser::backend::BrowserBackend;
    use crate::browser::cdp_backend::test_support::*;
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
}
