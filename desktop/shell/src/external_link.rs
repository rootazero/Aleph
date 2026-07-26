//! External-link routing for the Panel webview.
//!
//! The shell hosts a single webview pinned to the local daemon's Panel
//! origin. Links the Panel renders to the outside world (`target="_blank"`
//! markdown links, "open docs" anchors in settings) must NOT hijack that
//! lone webview — there is no back button to recover with (R5: the shell
//! never steals the user's context). Instead we keep the webview on its own
//! origin and hand external URLs to the OS, which opens them in the user's
//! real browser / mail client.
//!
//! Two cooperating halves, mirroring an Electron shell's `will-navigate` +
//! `setWindowOpenHandler`:
//!
//! - [`route`] is the `WebviewWindowBuilder::on_navigation` guard: it allows
//!   navigations that stay on a trusted local origin and cancels +
//!   externalises everything else. This covers plain in-frame link clicks
//!   and JS `location` changes.
//! - [`CLICK_INTERCEPTOR_JS`] is injected into every document so that
//!   `target="_blank"` anchors — which open no main-frame navigation and
//!   would otherwise be dead in a webview — are turned into a top-level
//!   navigation that [`route`] then catches. The script is shell-only by
//!   construction (only the shell injects it); in a plain browser the
//!   Panel's `_blank` links keep opening real tabs (R6).
//!
//! Pure OS integration, no business logic — the shell is the limb (R1/R10).

use std::sync::RwLock;
use tauri::Url;

/// Hosts whose http(s) origins are the Panel itself (the loopback daemon,
/// on whatever port) and thus safe to navigate the hosting webview to.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// The currently-configured remote Gateway origin (scheme + host + port),
/// if any. `is_internal` treats it as internal so in-Panel navigations on
/// the remote origin are not misrouted to the OS browser. Loopback is
/// always internal regardless. Storing the full origin (not just the host)
/// closes a bypass where a different scheme/port on the same host — e.g.
/// an unrelated `https://gw.example.com` on the same box as the configured
/// `http://gw.example.com:9000` — would also load inside the Panel webview.
static REMOTE_ORIGIN: RwLock<Option<String>> = RwLock::new(None);

/// Update the remote origin allow-list. Pass the remote target's URL when
/// switching to a remote Gateway, or `None` when returning to Local.
pub fn set_remote_host(url: Option<Url>) {
    let origin = url.map(|u| u.origin().unicode_serialization());
    *REMOTE_ORIGIN
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = origin;
}

/// Injected into every document: redirect `target="_blank"` anchor clicks
/// into a top-level navigation so [`route`] can externalise them. Capture
/// phase so it runs before the Panel's own handlers; `closest` walks up
/// from the click target to the enclosing anchor. Setting `location.href`
/// is the only channel the (bridge-free) shell webview has back to the
/// native [`route`] guard — which then cancels the navigation and opens the
/// URL in the OS handler.
pub const CLICK_INTERCEPTOR_JS: &str = r#"document.addEventListener('click',function(ev){var t=ev.target;var a=t&&t.closest?t.closest('a[target="_blank"]'):null;if(!a||!a.href)return;ev.preventDefault();window.location.href=a.href;},true);"#;

/// `WebviewWindowBuilder::on_navigation` guard. Returns `true` to let the
/// webview proceed (internal origins), `false` to cancel — opening the URL
/// in the OS default handler instead.
pub fn route(url: &Url) -> bool {
    if is_internal(url) {
        return true;
    }
    open_external(url.as_str());
    false
}

/// Path prefix of the daemon's artifact byte route
/// (`GET /artifact/{cap}/{id}/{filename}`). Deliberately excluded from the
/// internal allow-list — see [`is_internal`].
const ARTIFACT_PATH_PREFIX: &str = "/artifact/";

/// Whether `url` is part of the Panel/splash surface and may load inside the
/// hosting webview. Trusts the Tauri asset/data schemes and any http(s) URL
/// on a loopback host (the local daemon) or the `tauri.localhost` asset host
/// (Windows `WebView2` serves bundled assets there).
///
/// Carve-out: the daemon's artifact byte route is served from that same
/// trusted origin, but an artifact is a *document* (image, PDF, exported
/// HTML), not Panel surface. Navigating to it would replace the shell's one
/// webview with a file viewer and no back button — and `target="_blank"` is
/// no escape, since [`CLICK_INTERCEPTOR_JS`] rewrites `_blank` anchors into
/// top-level navigations. So `/artifact/...` is reported external and handed
/// to the OS browser, which has tabs, a back button, and a downloads shelf
/// (R5: the shell never steals the user's context).
pub fn is_internal(url: &Url) -> bool {
    match url.scheme() {
        // Splash + bundled assets (`tauri://localhost`), and in-page schemes
        // the Panel may render documents from.
        "tauri" | "about" | "data" | "blob" => true,
        "http" | "https" if url.path().starts_with(ARTIFACT_PATH_PREFIX) => false,
        "http" | "https" => match url.host_str() {
            Some(host) => {
                // `host_str` brackets IPv6 literals (`[::1]`); normalise.
                let host = host.trim_start_matches('[').trim_end_matches(']');
                if LOOPBACK_HOSTS.contains(&host) || host == "tauri.localhost" {
                    return true;
                }
                // Allow the currently-configured remote Gateway origin —
                // full origin match (scheme + host + port) so a different
                // scheme/port on the same host cannot piggy-back.
                *REMOTE_ORIGIN
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    == Some(url.origin().unicode_serialization())
            }
            None => false,
        },
        _ => false,
    }
}

/// Open `url` in the OS default browser/handler. Public entry point for the
/// app menu's external links (repository, issue tracker) — same injection-safe
/// launcher as the `_blank`-link guard, so menu code need not reach into the
/// (full-app-only) daemon module for a common capability (spec §5.1).
// Reached only through the macOS app menu today; dead in other builds.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn open_url(url: &str) {
    open_external(url);
}

/// Open `url` in the OS default handler (browser / mail client), detached.
/// Best-effort: a failure just means the link does nothing, which is no
/// worse than the pre-guard behaviour. No shell is invoked, so the URL is
/// never interpreted as a command.
fn open_external(url: &str) {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };

    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `rundll32 url.dll,FileProtocolHandler <url>` is the canonical
        // injection-safe URL opener — no `cmd /c start` quoting pitfalls.
        let mut c = Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    match cmd.spawn() {
        Ok(_child) => tracing::info!(target: "shell", "opened external URL in OS handler: {url}"),
        Err(e) => tracing::warn!("failed to open external URL {url}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn internal(u: &str) -> bool {
        is_internal(&Url::parse(u).unwrap())
    }

    // All tests in this module share the process-global REMOTE_ORIGIN
    // static. Run them serially so a parallel test thread doesn't observe
    // another test's writes between `set_remote_host` and the assertion.
    use serial_test::serial;

    #[test]
    #[serial(remote_origin)]
    fn loopback_panel_origins_are_internal() {
        assert!(internal("http://127.0.0.1:18790"));
        assert!(internal("http://127.0.0.1:18790/auth/bootstrap?nonce=abc"));
        assert!(internal("http://localhost:18790/chat"));
        assert!(internal("https://127.0.0.1:443/"));
        assert!(internal("http://[::1]:18790/"));
    }

    #[test]
    #[serial(remote_origin)]
    fn bundled_asset_schemes_are_internal() {
        assert!(internal("tauri://localhost/index.html"));
        assert!(internal("http://tauri.localhost/index.html"));
        assert!(internal("about:blank"));
        assert!(internal("data:text/html,hello"));
    }

    #[test]
    #[serial(remote_origin)]
    fn outside_origins_are_external() {
        assert!(!internal("https://github.com/aleph"));
        assert!(!internal("https://example.com/127.0.0.1"));
        assert!(!internal("http://10.0.0.5:18790/"));
        assert!(!internal("mailto:hi@example.com"));
        assert!(!internal("tel:+1555"));
    }

    #[test]
    #[serial(remote_origin)]
    fn artifact_bytes_are_external_even_on_a_trusted_origin() {
        // The carve-out has to beat *every* trusted-origin branch, not just
        // loopback: navigating the shell's single webview to an artifact
        // leaves the user in a file viewer with no back button.
        assert!(!internal(
            "http://127.0.0.1:18790/artifact/cap/id/report.pdf"
        ));
        assert!(!internal(
            "http://localhost:18790/artifact/cap/id/chart.png"
        ));
        assert!(!internal("https://127.0.0.1:443/artifact/cap/id/x.html"));
        assert!(!internal("http://tauri.localhost/artifact/cap/id/x.png"));
        set_remote_host(Some(Url::parse("http://203.0.113.9:18790").unwrap()));
        assert!(!internal("http://203.0.113.9:18790/artifact/cap/id/x.png"));
        set_remote_host(None);

        // Panel routes that merely *mention* the word stay internal — the
        // guard is a path prefix, not a substring match.
        assert!(internal("http://127.0.0.1:18790/chat/artifact/id"));
        assert!(internal("http://127.0.0.1:18790/artifacts"));
    }

    #[test]
    #[serial(remote_origin)]
    fn route_allows_internal_without_side_effects() {
        // Only the internal branch is exercised here: it returns `true`
        // without ever calling `open_external`, so no browser is spawned
        // during tests. The external (cancel) decision is covered by the
        // `is_internal` cases above — `route` simply negates it.
        assert!(route(&Url::parse("http://127.0.0.1:18790/chat").unwrap()));
    }

    #[test]
    #[serial(remote_origin)]
    fn remote_origin_becomes_internal_when_set() {
        // Use TEST-NET-3 (203.0.113.x) — not used in any other test,
        // avoiding concurrent REMOTE_ORIGIN pollution with outside_origins_are_external.
        // default: a routable host is external
        assert!(!internal("http://203.0.113.7:18790/chat"));
        set_remote_host(Some(Url::parse("http://203.0.113.7:18790").unwrap()));
        // now the configured remote origin is internal, loopback still internal,
        // and an unrelated origin stays external
        assert!(internal("http://203.0.113.7:18790/chat"));
        assert!(internal("http://127.0.0.1:18790/"));
        assert!(!internal("https://github.com/aleph"));
        // clearing reverts
        set_remote_host(None);
        assert!(!internal("http://203.0.113.7:18790/chat"));
    }

    #[test]
    #[serial(remote_origin)]
    fn different_scheme_or_port_on_remote_host_stays_external() {
        // Set a remote origin on plain http; the same host on a different
        // scheme (https) or a different port must NOT be auto-trusted —
        // otherwise content from an unrelated service on the same machine
        // would load inside the Panel webview. (Regression test for the
        // host-only allow-list bypass.)
        set_remote_host(Some(Url::parse("http://203.0.113.99:18790").unwrap()));
        assert!(
            !internal("https://203.0.113.99:18790/chat"),
            "https on the configured remote host must not piggy-back"
        );
        assert!(
            !internal("http://203.0.113.99:9000/chat"),
            "different port on the configured remote host must not piggy-back"
        );
        // Same origin still internal.
        assert!(internal("http://203.0.113.99:18790/chat"));
        set_remote_host(None);
    }
}
