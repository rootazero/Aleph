//! Connection target: local daemon vs remote Gateway.
//!
//! The shell connects to exactly one Gateway at a time — either the
//! same-machine `aleph-server` it launches and supervises (Local, the
//! default and today's behaviour), or a remote Gateway by URL (Remote, which
//! never touches the local daemon). The choice persists in
//! `~/.aleph/<prefix>-target` (`<prefix>` = `.desktop-shell` for the full app,
//! `.desktop-shell-panel` for the lite shell — see [`MARKER_PREFIX`], so both
//! can run on one machine with independent targets); a missing file means Local
//! (zero regression on first run).

use url::Url;

/// Default Gateway port when the user omits one.
const DEFAULT_PORT: u16 = 18790;

/// Filename prefix for this build's shell-state markers under `~/.aleph/`.
/// The full app and the panel-only (lite) shell keep *independent* connection +
/// autostart state so both can run on one machine without clobbering each other
/// (the full app driving its loopback server while the lite shell points at a
/// remote). The full app keeps the historical unprefixed names (zero migration
/// for existing installs); the lite shell namespaces under `-panel`.
#[cfg(feature = "embedded-core")]
const MARKER_PREFIX: &str = ".desktop-shell";
#[cfg(not(feature = "embedded-core"))]
const MARKER_PREFIX: &str = ".desktop-shell-panel";

/// Path to a per-build shell-state marker `~/.aleph/<prefix>-<name>`. Variant-
/// namespaced (see [`MARKER_PREFIX`]) so the full app and lite shell never share
/// connection / autostart state. `None` if the home directory can't be resolved.
pub(crate) fn marker_path(name: &str) -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph").join(format!("{MARKER_PREFIX}-{name}")))
}

/// Where the chosen target persists. Mirrors the sibling autostart marker
/// (`<prefix>-autostart`); the full-app-only `.desktop-shell-daemon-version`
/// stays unprefixed (the lite shell has no daemon to version).
fn target_marker() -> Option<std::path::PathBuf> {
    marker_path("target")
}

/// Where the shared Gateway token for a remote target persists. The remote
/// Panel holds the validated token in webview localStorage, but the native
/// notification bridge runs in Rust and cannot read it (and a remote-origin
/// Panel cannot invoke shell commands — the capability scopes IPC to loopback).
/// So the shell captures the token from the remote URL's `?token=` query when
/// the target is set (see [`persist_token_from_url`]) and the bridge reads it
/// here, letting it present the same token and stay authorized on a remote
/// token-protected Gateway (R5 — desktop banners on remote). Sibling of the
/// other `.desktop-shell-*` markers.
fn gateway_token_marker() -> Option<std::path::PathBuf> {
    marker_path("gateway-token")
}

/// Load the persisted Gateway token, if any. Missing/unreadable/empty → None
/// (the bridge then connects bare; correct under loopback LAN-trust, walled on
/// a remote that requires a token until one is captured from a `?token=` URL).
pub fn load_gateway_token() -> Option<String> {
    let token = std::fs::read_to_string(gateway_token_marker()?).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Persist a Gateway token (already trimmed non-empty by the caller). Written
/// with the same `.aleph` directory bootstrap as the target.
pub(crate) fn store_gateway_token(token: &str) -> Result<(), String> {
    let marker = gateway_token_marker().ok_or("home directory not found")?;
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .aleph dir: {e}"))?;
    }
    // The shared Gateway token grants full operator authority, so keep it
    // owner-only. A `write` + later `set_permissions` would create the file at
    // the umask default (typically 0644 — readable by every local user) and
    // leave a TOCTOU window before the chmod lands; create it 0600 atomically
    // instead, mirroring the repo-wide secret-store convention (gateway TLS key,
    // vault, mcp auth, cli endpoint).
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&marker)
            .map_err(|e| format!("create gateway token: {e}"))?;
        f.write_all(token.as_bytes())
            .map_err(|e| format!("write gateway token: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&marker, token).map_err(|e| format!("write gateway token: {e}"))?;
    }
    Ok(())
}

/// Drop any persisted Gateway token. Called when switching to a target that
/// carries no token (a different remote, or Local), so one remote's token is
/// never presented to another or to the local daemon. Best-effort: a missing
/// file is already the desired state. Any other IO error is logged at warn
/// so the operator can see why a token from a previous remote might still be
/// on disk — without that visibility the old credential gets presented to
/// the new gateway on the next reconnect.
fn remove_gateway_token() {
    let Some(marker) = gateway_token_marker() else {
        return;
    };
    match std::fs::remove_file(&marker) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            "could not remove Gateway token at {}: {e}; \
             the prior remote's credential may still be presented to the next target",
            marker.display()
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// Launch + supervise the local daemon; webview → 127.0.0.1:18790.
    Local,
    /// Connect to a remote Gateway by origin; never touch the local daemon.
    Remote(Url),
}

impl ConnectionTarget {
    /// Parse a persisted/user-entered target string. `"local"` (any case) or
    /// empty → Local. Otherwise normalise to a `Remote(Url)`:
    /// accept `host`, `host:port`, `http://host`, `https://host:port`;
    /// default scheme `http`, default port 18790.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let t = raw.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("local") {
            return Ok(Self::Local);
        }
        let with_scheme = if t.contains("://") {
            t.to_string()
        } else {
            format!("http://{t}")
        };
        let mut url = Url::parse(&with_scheme).map_err(|e| format!("invalid target URL: {e}"))?;
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(format!("unsupported scheme: {other}")),
        }
        if url.host().is_none() {
            return Err("target URL has no host".to_string());
        }
        // Apply the default port only when the user did not supply one explicitly.
        // `url::Url::port()` returns None both for "no port written" and for "port
        // equals the scheme default" (e.g. https:443, http:80) — the two cases are
        // indistinguishable after parsing.  We therefore inspect the pre-parse
        // string: if it already contains ":<digits>" after the host, the user made
        // an explicit choice and we honour it; otherwise we inject DEFAULT_PORT.
        let has_explicit_port = has_explicit_port_in_input(t);
        if !has_explicit_port {
            // set_port only errors when the URL cannot have a port (it can here)
            let _ = url.set_port(Some(DEFAULT_PORT));
        }
        Ok(Self::Remote(url))
    }

    /// Serialise for persistence. Local → `"local"`; Remote → the URL origin
    /// with an explicit port (so that `load_target` round-trips correctly even
    /// when the port equals the scheme default and would otherwise be elided by
    /// the `url` crate's normalisation).
    pub fn to_persisted(&self) -> String {
        match self {
            Self::Local => "local".to_string(),
            Self::Remote(url) => {
                let scheme = url.scheme();
                let host = url.host_str().unwrap_or("127.0.0.1");
                // `url::Url::port()` returns None for scheme-default ports (443 for
                // https, 80 for http); use `port_or_known_default()` to recover them.
                let port = url.port_or_known_default().unwrap_or(DEFAULT_PORT);
                format!("{scheme}://{host}:{port}")
            }
        }
    }

    /// Whether `url` sits on the exact origin (scheme + host + port) the Panel
    /// is served from for this target — the loopback default for Local, the
    /// configured URL for Remote. The update sentinel guard
    /// (`update::control_action`) honours shell-control navigations only from
    /// this origin, so a foreign page cannot drive shell actions.
    pub fn serves_origin(&self, url: &Url) -> bool {
        match self {
            // `crate::PANEL_URL` is already exactly the loopback origin's
            // serialization (http scheme, non-default port), so a string
            // compare is a parsed-origin compare without parsing a constant.
            Self::Local => url.origin().unicode_serialization() == crate::PANEL_URL,
            Self::Remote(target) => url.origin() == target.origin(),
        }
    }
}

/// Detect whether the raw user input already contains an explicit port number.
/// Handles forms: `host:port`, `http://host:port`, `https://host:port`,
/// including IPv6 (`[::1]:port`).  Returns false when only a scheme default
/// would apply (e.g. `https://host` with no written port).
fn has_explicit_port_in_input(raw: &str) -> bool {
    // Strip scheme if present so we work on `[host]/path` or `host:port/path`.
    let after_scheme = if let Some(pos) = raw.find("://") {
        &raw[pos + 3..]
    } else {
        raw
    };
    // For IPv6 addresses the host is wrapped in brackets: `[::1]:port`.
    let host_end = if after_scheme.starts_with('[') {
        after_scheme.find(']').map(|i| i + 1)
    } else {
        // Plain hostname or IPv4 — find the first `:`.
        after_scheme.find(':')
    };
    match host_end {
        // IPv6: `idx` is already one past `]` (i.e. pointing at `:` when a
        // port is present), so check for `:` directly at `idx`.
        Some(idx) if after_scheme.starts_with('[') => {
            after_scheme[idx..].starts_with(':')
                && after_scheme[idx + 1..]
                    .split('/')
                    .next()
                    .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        }
        // Plain host:port — digits after the `:` (before any `/`).
        Some(colon) => after_scheme[colon + 1..]
            .split('/')
            .next()
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        None => false,
    }
}

/// Whether a target has ever been chosen (the marker file exists). Unlike
/// [`load_target`], which collapses "no marker" and "marker says local" into
/// `Local`, this distinguishes first run (no marker) from a deliberate Local
/// choice — the panel-only shell needs that to decide whether to open its
/// first-run connection page. Consumed only by the panel-only variant's
/// first-run flow; the full app supervises a local daemon and never shows a
/// first-run page, so this is gated out of it.
#[cfg(not(feature = "embedded-core"))]
pub fn marker_exists() -> bool {
    target_marker().is_some_and(|m| m.exists())
}

/// Load the persisted target; missing/unreadable/unparsable → Local
/// (fail-safe: a corrupt marker must never strand the user on a broken
/// remote — it falls back to the always-available local daemon).
pub fn load_target() -> ConnectionTarget {
    let Some(marker) = target_marker() else {
        return ConnectionTarget::Local;
    };
    match std::fs::read_to_string(&marker) {
        Ok(s) => ConnectionTarget::parse(&s).unwrap_or(ConnectionTarget::Local),
        Err(_) => ConnectionTarget::Local,
    }
}

/// Persist a target string (already validated by `parse`). Writes the
/// normalised form.
pub fn save_target(target: &ConnectionTarget) -> Result<(), String> {
    let Some(marker) = target_marker() else {
        return Err("home directory not found".to_string());
    };
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .aleph dir: {e}"))?;
    }
    std::fs::write(&marker, target.to_persisted()).map_err(|e| format!("write target: {e}"))
}

/// The bundled connection page (`splash/connect.html`) as a navigable URL for
/// the current platform. Tauri serves app assets from `tauri://localhost` on
/// macOS/Linux, but from `http://tauri.localhost` on Windows — WebView2 has no
/// `tauri://` scheme. A single hardcoded `tauri://localhost/connect.html`
/// therefore navigates to nothing on Windows, leaving the first-run wizard a
/// blank white window. Build it per platform so every navigation target (the
/// first-run wizard, the unreachable-target fallback, and the tray / app-menu
/// "connect" items) resolves on all three. The `route` guard already treats
/// `http://tauri.localhost` as internal (see `external_link::is_internal`), so
/// the corrected navigation is allowed.
pub(crate) fn connect_page_url() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "http://tauri.localhost/connect.html"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "tauri://localhost/connect.html"
    }
}

/// The bundled cert-trust approval page (`splash/cert-trust.html`) as a
/// navigable URL, resolved per platform like [`connect_page_url`]. The
/// TLS-challenge hook navigates here with `window.navigate` (an absolute
/// engine-level load) rather than a relative `location.href` eval: at challenge
/// time the failing remote navigation has left no usable document base, so a
/// relative target resolves against the wrong (or blank) origin.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    allow(dead_code)
)]
pub(crate) fn cert_trust_page_url() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "http://tauri.localhost/cert-trust.html"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "tauri://localhost/cert-trust.html"
    }
}

// ---------------------------------------------------------------------------
// Tauri commands — the shell's *only* invoke surface, strictly limited to
// connection configuration (spec §5.2 explicit exception to "no invoke_handler";
// these are I/O config toggles, not business logic — R2/R4 boundary held).
// ---------------------------------------------------------------------------

/// Return the current target as a string (`"local"` or the remote URL).
#[tauri::command]
pub fn get_connection_target() -> String {
    load_target().to_persisted()
}

/// Validate + persist a new target, update the external-link allow-list, and
/// ask the shell to re-route (navigate + supervise) for it. `raw` accepts the
/// same forms as `ConnectionTarget::parse`.
#[tauri::command]
pub fn set_connection_target(app: tauri::AppHandle, raw: String) -> Result<(), String> {
    let target = ConnectionTarget::parse(&raw)?;
    save_target(&target)?;
    // Capture (or clear) the Gateway credential from the target so the native
    // notification bridge can authorize against a remote token-protected
    // Gateway too (R5 — desktop banners on remote). A QR / shared-link onboarding
    // URL may carry either `?bt=` (bootstrap ticket, preferred) or `?token=`
    // (legacy shared token); a manual address or Local has none. Switching always
    // re-derives it, so one remote's credential is never presented to another or
    // to the local daemon.
    match &target {
        ConnectionTarget::Remote(url) => persist_credential_from_url(url),
        ConnectionTarget::Local => remove_gateway_token(),
    }
    match &target {
        ConnectionTarget::Remote(url) => crate::external_link::set_remote_host(Some(url.clone())),
        ConnectionTarget::Local => crate::external_link::set_remote_host(None),
    }
    crate::reroute_for_target(&app, target);
    Ok(())
}

/// Extract a non-empty credential from a URL's query, if present.
///
/// Priority: `?bt=` (bootstrap ticket) > `?token=` (legacy shared token).
/// Pure (no I/O) so the extraction is unit-testable without touching the filesystem.
fn credential_from_url(url: &Url) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k.as_ref() == "bt")
        .map(|(_, v)| v.into_owned())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            url.query_pairs()
                .find(|(k, _)| k.as_ref() == "token")
                .map(|(_, v)| v.into_owned())
                .filter(|t| !t.is_empty())
        })
}

/// Persist the credential (`?bt=` or `?token=`) from a remote Gateway URL for
/// the notification bridge, or clear the store when the URL carries none.
/// Bootstrap tickets are short-lived, but the bridge needs to present the same
/// value the remote Panel URL carried until it is exchanged for a device token.
fn persist_credential_from_url(url: &Url) {
    match credential_from_url(url) {
        Some(t) => {
            if let Err(e) = store_gateway_token(&t) {
                tracing::warn!(
                    "could not persist Gateway credential for the notification bridge: {e}"
                );
            }
        }
        None => remove_gateway_token(),
    }
}

/// Reset to Local (launch + supervise the local daemon).
#[tauri::command]
pub fn clear_connection_target(app: tauri::AppHandle) -> Result<(), String> {
    set_connection_target(app, "local".to_string())
}

/// Whether this build is the panel-only (lite) shell variant. Registered in
/// *both* variants (the full app answers `false`) so the connect page can
/// determine its mode deterministically at load — which connect command to
/// call and whether to show the mDNS discovery section — instead of inferring
/// the variant from whether a discovery scan happened to succeed.
#[tauri::command]
pub fn is_lite_shell() -> bool {
    cfg!(not(feature = "embedded-core"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lite_flag_matches_build_variant() {
        // Each matrix asserts its own constant: the full app must answer
        // `false`, the panel-only shell `true` — the connect page keys its
        // command choice and discovery UI off this.
        #[cfg(feature = "embedded-core")]
        assert!(!is_lite_shell());
        #[cfg(not(feature = "embedded-core"))]
        assert!(is_lite_shell());
    }

    #[test]
    fn empty_and_local_parse_to_local() {
        assert_eq!(
            ConnectionTarget::parse("").unwrap(),
            ConnectionTarget::Local
        );
        assert_eq!(
            ConnectionTarget::parse("  ").unwrap(),
            ConnectionTarget::Local
        );
        assert_eq!(
            ConnectionTarget::parse("local").unwrap(),
            ConnectionTarget::Local
        );
        assert_eq!(
            ConnectionTarget::parse("LOCAL").unwrap(),
            ConnectionTarget::Local
        );
    }

    #[test]
    fn bare_host_gets_http_and_default_port() {
        let t = ConnectionTarget::parse("192.168.1.5").unwrap();
        assert_eq!(t.to_persisted(), "http://192.168.1.5:18790");
    }

    #[test]
    fn host_port_gets_http() {
        let t = ConnectionTarget::parse("box.lan:9000").unwrap();
        assert_eq!(t.to_persisted(), "http://box.lan:9000");
    }

    #[test]
    fn explicit_scheme_preserved() {
        let t = ConnectionTarget::parse("https://gw.example.com").unwrap();
        assert_eq!(t.to_persisted(), "https://gw.example.com:18790");
        let t2 = ConnectionTarget::parse("https://gw.example.com:443").unwrap();
        assert_eq!(t2.to_persisted(), "https://gw.example.com:443");
    }

    #[test]
    fn unsupported_scheme_rejected() {
        assert!(ConnectionTarget::parse("ftp://host").is_err());
        assert!(ConnectionTarget::parse("ws://host").is_err());
    }

    #[test]
    fn ipv6_with_port_keeps_user_port() {
        let t = ConnectionTarget::parse("http://[::1]:9000").unwrap();
        assert_eq!(t.to_persisted(), "http://[::1]:9000");
    }

    #[test]
    fn ipv6_without_port_gets_default_port() {
        let t = ConnectionTarget::parse("http://[::1]").unwrap();
        assert_eq!(t.to_persisted(), "http://[::1]:18790");
    }

    #[test]
    fn serves_origin_matches_only_the_target_origin() {
        let local = ConnectionTarget::Local;
        assert!(local.serves_origin(&Url::parse("http://127.0.0.1:18790/chat").unwrap()));
        assert!(local.serves_origin(&Url::parse("http://127.0.0.1:18790").unwrap()));
        // A foreign origin, another loopback port, and an opaque (non-http)
        // origin are all not the Panel origin.
        assert!(!local.serves_origin(&Url::parse("http://evil.com/").unwrap()));
        assert!(!local.serves_origin(&Url::parse("http://127.0.0.1:9999/").unwrap()));
        assert!(!local.serves_origin(&Url::parse("tauri://localhost/index.html").unwrap()));

        let remote = ConnectionTarget::parse("https://gw.example.com:8443").unwrap();
        assert!(remote.serves_origin(&Url::parse("https://gw.example.com:8443/x").unwrap()));
        assert!(!remote.serves_origin(&Url::parse("http://gw.example.com:8443/").unwrap()));
        assert!(!remote.serves_origin(&Url::parse("https://gw.example.com:443/").unwrap()));
    }

    #[test]
    fn credential_from_url_reads_bootstrap_ticket_first() {
        let url = Url::parse("https://gw.example.com:8443/?bt=aleph-bt-abc123&token=aleph-legacy")
            .unwrap();
        assert_eq!(
            credential_from_url(&url).as_deref(),
            Some("aleph-bt-abc123")
        );
    }

    #[test]
    fn credential_from_url_falls_back_to_legacy_token() {
        let url = Url::parse("https://gw.example.com:8443/?token=aleph-abc123").unwrap();
        assert_eq!(credential_from_url(&url).as_deref(), Some("aleph-abc123"));
    }

    #[test]
    fn connect_page_url_is_platform_correct_and_parses() {
        let url = connect_page_url();
        // Must parse as a valid URL the webview can navigate to.
        assert!(Url::parse(url).is_ok());
        // macOS/Linux use the `tauri://` asset scheme; Windows (WebView2) has
        // no `tauri://` scheme and serves bundled assets over http.
        #[cfg(target_os = "windows")]
        assert_eq!(url, "http://tauri.localhost/connect.html");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(url, "tauri://localhost/connect.html");
    }

    #[test]
    fn cert_trust_page_url_is_platform_correct_and_parses() {
        let url = cert_trust_page_url();
        assert!(Url::parse(url).is_ok());
        #[cfg(target_os = "windows")]
        assert_eq!(url, "http://tauri.localhost/cert-trust.html");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(url, "tauri://localhost/cert-trust.html");
    }

    #[test]
    fn credential_from_url_none_without_credential() {
        // No query, an empty credential, and an unrelated param all yield None.
        assert!(credential_from_url(&Url::parse("https://gw.example.com/").unwrap()).is_none());
        assert!(credential_from_url(&Url::parse("https://gw.example.com/?bt=").unwrap()).is_none());
        assert!(
            credential_from_url(&Url::parse("https://gw.example.com/?token=").unwrap()).is_none()
        );
        assert!(
            credential_from_url(&Url::parse("https://gw.example.com/?foo=bar").unwrap()).is_none()
        );
    }
}
