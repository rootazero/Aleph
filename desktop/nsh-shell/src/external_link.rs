//! Links the Panel opens outside itself. The window holds one Panel page
//! and has no back button, so a link off the Panel's own pages goes to the
//! system (the browser, the mail app) and the Panel stays where it is.
//!
//! `panel_bridge.js` catches such a click and sends `open-external`; the
//! host decides again here, from the server it shows, and opens only what
//! [`external_target`] returns.

use aleph_client::ConnectionTarget;
use url::Url;

/// The server's artifact bytes (`GET /artifact/{cap}/{id}/{file}`): a
/// document, not a Panel page, so it opens in the browser even though the
/// server serves it.
const ARTIFACT_PATH_PREFIX: &str = "/artifact/";

/// The schemes the host hands to the system; nsh.json's `openUrl`
/// capability grants the same ones.
const OPENABLE_SCHEMES: [&str; 4] = ["https", "http", "mailto", "x-apple.systempreferences"];

/// Whether `url` is one of the Panel's own pages for the server `gateway`:
/// only the exact origin (scheme, host and port) for either target.
pub(crate) fn is_internal(url: &Url, gateway: &ConnectionTarget) -> bool {
    match url.scheme() {
        "about" | "blob" => true,
        "http" | "https" if url.path().starts_with(ARTIFACT_PATH_PREFIX) => false,
        "http" | "https" => gateway.serves_origin(url),
        _ => false,
    }
}

/// What an `open-external` request may open: `raw` parsed, unless it is a
/// Panel page, not a URL, or a scheme the system must not get
/// (`javascript:`, `file:`, `data:` and every other one not listed).
pub(crate) fn external_target(raw: &str, gateway: &ConnectionTarget) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let openable = OPENABLE_SCHEMES.contains(&url.scheme());
    (openable && !is_internal(&url, gateway)).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(raw: &str) -> ConnectionTarget {
        ConnectionTarget::Remote(Url::parse(raw).expect("test URL"))
    }

    fn internal(raw: &str, gateway: &ConnectionTarget) -> bool {
        is_internal(&Url::parse(raw).expect("test URL"), gateway)
    }

    #[test]
    fn only_the_panel_origin_is_internal_for_the_local_server() {
        let local = ConnectionTarget::Local;
        assert!(internal("http://127.0.0.1:18790", &local));
        assert!(internal(
            "http://127.0.0.1:18790/auth/bootstrap?nonce=abc",
            &local
        ));
        assert!(!internal("http://localhost:18790/chat", &local));
        assert!(!internal("http://[::1]:18790/", &local));
        assert!(!internal("http://127.0.0.1:18791/chat", &local));
        assert!(internal("about:blank", &local));
    }

    #[test]
    fn outside_origins_are_external() {
        let local = ConnectionTarget::Local;
        assert!(!internal("https://github.com/rootazero/Aleph", &local));
        assert!(!internal("https://example.com/127.0.0.1", &local));
        assert!(!internal("http://10.0.0.5:18790/", &local));
        assert!(!internal("mailto:hi@example.com", &local));
        assert!(!internal("data:text/html,hello", &local));
    }

    #[test]
    fn the_remote_servers_own_origin_is_internal() {
        let gateway = remote("https://gw.example.com:8443");
        assert!(internal("https://gw.example.com:8443/chat", &gateway));
        assert!(!internal("https://github.com/rootazero/Aleph", &gateway));
        // Another scheme or port on the same host is another service.
        assert!(!internal("http://gw.example.com:8443/chat", &gateway));
        assert!(!internal("https://gw.example.com/chat", &gateway));
        // This computer's pages are not the remote Panel's.
        assert!(!internal("http://127.0.0.1:18790/chat", &gateway));
    }

    #[test]
    fn artifact_bytes_are_external_on_every_server() {
        let local = ConnectionTarget::Local;
        assert!(!internal(
            "http://127.0.0.1:18790/artifact/cap/id/report.pdf",
            &local
        ));
        let gateway = remote("https://gw.example.com");
        assert!(!internal(
            "https://gw.example.com/artifact/cap/id/x.png",
            &gateway
        ));
        // A Panel route that only mentions the word stays internal.
        assert!(internal("http://127.0.0.1:18790/chat/artifact/id", &local));
        assert!(internal("http://127.0.0.1:18790/artifacts", &local));
    }

    #[test]
    fn outside_links_open_and_panel_pages_do_not() {
        let local = ConnectionTarget::Local;
        for raw in [
            "https://github.com/rootazero/Aleph",
            "mailto:hi@example.com",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            "http://127.0.0.1:18790/artifact/cap/id/report.pdf",
        ] {
            assert!(external_target(raw, &local).is_some(), "{raw}");
        }
        assert!(external_target("http://127.0.0.1:18790/chat", &local).is_none());
        assert!(external_target("not a url", &local).is_none());
    }

    #[test]
    fn script_and_file_urls_never_reach_the_system() {
        let local = ConnectionTarget::Local;
        for raw in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
        ] {
            assert!(external_target(raw, &local).is_none(), "{raw}");
        }
    }
}
