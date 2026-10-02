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

use std::sync::OnceLock;
use tokio::sync::watch;
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
/// never presented to another or to the local daemon.
///
/// Strategy: try `std::fs::remove_file` first (so a missing file is a no-op),
/// but if the deletion fails (file locked, EACCES, …) fall back to
/// **atomically overwriting the file with empty bytes** so `load_gateway_token`
/// can no longer hand the stale credential to the next target. Only when
/// *both* the delete and the overwrite fail does this return `Err` — the
/// caller is expected to refuse the target switch in that case. Otherwise
/// a prior remote's bearer token rides into the new connection.
fn remove_gateway_token() -> Result<(), String> {
    let Some(marker) = gateway_token_marker() else {
        return Ok(());
    };
    match std::fs::remove_file(&marker) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(remove_err) => match store_gateway_token("") {
            Ok(()) => Ok(()),
            Err(write_err) => Err(format!(
                "could not remove Gateway token at {} ({remove_err}) nor \
                 overwrite it empty ({write_err}); the prior remote's credential \
                 may still be presented to the next target",
                marker.display()
            )),
        },
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
    /// default scheme `http`, default port per [`default_port_for`].
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
        // an explicit choice and we honour it; otherwise we pick the default that
        // matches what the user actually wrote (see [`default_port_for`]).
        let has_explicit_port = has_explicit_port_in_input(t);
        if !has_explicit_port {
            // set_port only errors when the URL cannot have a port (it can here)
            let _ = url.set_port(Some(default_port_for(t, url.scheme())));
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

/// The port to assume when the user wrote none. Two different questions, two
/// different answers:
///
///   * A bare `host` / `host/path` means *"an aleph-server lives there"*, whose
///     default listener is [`DEFAULT_PORT`] — the LAN case, unchanged.
///   * An explicitly written `http://` / `https://` means *"this URL"*, whose
///     default port is the scheme's own (80 / 443). TLS on the gateway is off by
///     default and `gateway::tls` states outright that public issuance is
///     Caddy's / certbot's job, so a typed `https://` almost always names a
///     reverse proxy or CDN on 443.
///
/// Injecting [`DEFAULT_PORT`] into a typed `https://host` made every
/// reverse-proxied deployment structurally unreachable: the shell rewrote a
/// working `https://gw.example.com` into `https://gw.example.com:18790`, which
/// no proxy serves. No persisted target regresses, because [`to_persisted`]
/// always writes an explicit port and `has_explicit_port_in_input` then honours
/// it — the rule only reinterprets *freshly typed* input.
///
/// [`to_persisted`]: ConnectionTarget::to_persisted
///
/// **Mirrored on iOS.** `mobile/ios/…/Models/PairingTarget.swift` ports this
/// rule verbatim and pins it with its own tests, because the two shells
/// promise one onboarding format — an address that works on the desktop must
/// work on the phone. That promise is written down only on the iOS side, and
/// nothing here can fail when it is broken: the halves are built by different
/// toolchains (`cargo` never compiles the Swift, `xcodebuild` never compiles
/// this), so changing the rule below is green in CI while silently splitting
/// the two shells apart. Change one, change both.
fn default_port_for(raw: &str, scheme: &str) -> u16 {
    if !raw.contains("://") {
        return DEFAULT_PORT;
    }
    match scheme {
        "https" => 443,
        _ => 80,
    }
}

/// True iff `tail` (the bytes immediately following a `host:` colon) is a
/// sequence of ASCII digits terminated by `/`, `?`, `#`, or end-of-string.
/// Anything else — `host:abc`, `host:443abc`, `host:80abc` — is not a port.
///
/// `host:80?bt=...` is therefore an explicit port 80 followed by a query
/// string, not a port labelled `80?bt=...`. The older split('/').next()
/// form returned `false` for this input because `split` reached end-of-string
/// without finding a separator and handed the whole `80?bt=...` back, then
/// `is_ascii_digit()` rejected the `?`.
fn port_tail_is_explicit(tail: &str) -> bool {
    match tail.find(|c: char| !c.is_ascii_digit()) {
        Some(end) => {
            let after = &tail[end..];
            after.is_empty()
                || after.starts_with('/')
                || after.starts_with('?')
                || after.starts_with('#')
        }
        // All digits until end of string — still an explicit port (it may
        // happen to equal the scheme default; the user wrote it explicitly
        // and we honour the literal).
        None => !tail.is_empty(),
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
                && port_tail_is_explicit(&after_scheme[idx + 1..])
        }
        // Plain host:port — digits after the `:`, terminated by `/`, `?`,
        // `#`, or end-of-string. `host:80?bt=...` is an explicit port 80
        // followed by a query string, not a port labelled `80?bt=...`.
        Some(colon) => port_tail_is_explicit(&after_scheme[colon + 1..]),
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

/// Subscribe to "the operator switched the connection target" events. Each
/// successful [`set_connection_target`] (and [`clear_connection_target`])
/// publishes a tick; long-lived subscribers — most importantly the
/// notification WebSocket in [`crate::notify::run_notification_bridge`] —
/// observe it and close their old Gateway-bound stream so the next iteration
/// reconnects to the new target. Without this, a Remote-A → Remote-B switch
/// would keep the bridge subscribed to A indefinitely, route A's
/// `surface.notify` events to a stale Panel, and never see B's events.
pub fn target_change_rx() -> watch::Receiver<()> {
    target_change_tx().subscribe()
}

fn target_change_tx() -> &'static watch::Sender<()> {
    static TX: OnceLock<watch::Sender<()>> = OnceLock::new();
    TX.get_or_init(|| watch::channel(()).0)
}

/// Publish a target-change tick. Called after a successful target write so
/// every subscriber drops and re-dials. Subscribers that started after the
/// previous tick don't see *that* tick but DO see the next one — losing
/// one notification across the lifetime of a long-running bridge is the
/// cost of being late to subscribe, not a correctness bug.
fn signal_target_change() {
    let _ = target_change_tx().send(());
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

/// [`connect_page_url`] with a routing failure carried in the fragment, for
/// the page to render on load.
///
/// Deliberately NOT an `eval("window.__alephError(…)")` after `navigate`: the
/// two are unordered, so the script runs against the *outgoing* document where
/// the hook does not exist yet, the `&&` guard makes it a silent no-op, and the
/// explanation is lost — the failure mode is "the connect page appears with no
/// reason on it", which is exactly the stranding this page exists to prevent.
/// A fragment is part of the load itself and cannot race.
///
/// The message embeds a user-supplied host, so it is percent-encoded rather
/// than concatenated; `#` or `&` in a hostname would otherwise truncate it.
pub(crate) fn connect_page_url_with_error(message: &str) -> String {
    if message.is_empty() {
        return connect_page_url().to_string();
    }
    let encoded: String = url::form_urlencoded::byte_serialize(message.as_bytes()).collect();
    format!("{}#err={encoded}", connect_page_url())
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
        ConnectionTarget::Remote(url) => persist_credential_from_url(url)?,
        ConnectionTarget::Local => remove_gateway_token()?,
    }
    match &target {
        ConnectionTarget::Remote(url) => crate::external_link::set_remote_host(Some(url.clone())),
        ConnectionTarget::Local => crate::external_link::set_remote_host(None),
    }
    crate::reroute_for_target(&app, target);
    // W-18: tell long-lived subscribers (notification bridge, future status
    // indicators) that the target moved. Subscribers receive the tick, close
    // the old stream, and the next loop iteration reads the new
    // ConnectionTarget. Without this a Remote-A → Remote-B switch leaves the
    // bridge subscribed to A indefinitely.
    signal_target_change();
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
///
/// Returns `Err` if the write or the atomic delete fails so the surrounding
/// target switch can refuse to proceed (one remote's stale token must never
/// ride into another remote's connection).
fn persist_credential_from_url(url: &Url) -> Result<(), String> {
    match credential_from_url(url) {
        Some(t) => store_gateway_token(&t),
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
    use serial_test::serial;
    use url::form_urlencoded;

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
    fn explicit_port_followed_by_query_string_is_respected() {
        // W-32 regression: the older split('/').next() form misread the
        // query string as part of the port and silently rewrote `80` to
        // the scheme default (443). Explicit ports win regardless of the
        // character that follows them — `/`, `?`, `#`, end-of-string.
        let t = ConnectionTarget::parse("https://box.lan:9999?bt=token").unwrap();
        assert_eq!(t.to_persisted(), "https://box.lan:9999");
        if let ConnectionTarget::Remote(url) = t {
            assert_eq!(url.port(), Some(9999));
            assert_eq!(url.query(), Some("bt=token"));
        } else {
            panic!("expected Remote target");
        }
    }

    #[test]
    fn explicit_port_followed_by_hash_is_respected() {
        let t = ConnectionTarget::parse("http://box.lan:9999#frag").unwrap();
        assert_eq!(t.to_persisted(), "http://box.lan:9999");
        if let ConnectionTarget::Remote(url) = t {
            assert_eq!(url.port(), Some(9999));
            assert_eq!(url.fragment(), Some("frag"));
        } else {
            panic!("expected Remote target");
        }
    }

    #[test]
    fn explicit_port_followed_by_path_is_respected() {
        let t = ConnectionTarget::parse("http://box.lan:9999/abc").unwrap();
        assert_eq!(t.to_persisted(), "http://box.lan:9999");
        if let ConnectionTarget::Remote(url) = t {
            assert_eq!(url.port(), Some(9999));
            assert_eq!(url.path(), "/abc");
        } else {
            panic!("expected Remote target");
        }
    }

    #[test]
    fn explicit_scheme_preserved() {
        let t = ConnectionTarget::parse("https://gw.example.com").unwrap();
        assert_eq!(t.to_persisted(), "https://gw.example.com:443");
        let t2 = ConnectionTarget::parse("https://gw.example.com:443").unwrap();
        assert_eq!(t2.to_persisted(), "https://gw.example.com:443");
    }

    /// A typed `https://host` must land on 443, not on the bare-host default.
    /// Rewriting it to `:18790` is what made every reverse-proxy / CDN
    /// deployment unreachable — the shell pointed the webview at a port no
    /// proxy serves, and the user had no way to see why.
    #[test]
    fn a_typed_https_url_keeps_the_schemes_own_port() {
        let t = ConnectionTarget::parse("https://aleph.example.com").unwrap();
        assert_eq!(t.to_persisted(), "https://aleph.example.com:443");
        // An explicitly written non-default port still wins.
        let t2 = ConnectionTarget::parse("https://aleph.example.com:8443").unwrap();
        assert_eq!(t2.to_persisted(), "https://aleph.example.com:8443");
        // A typed http:// URL likewise means "this URL", i.e. port 80.
        let t3 = ConnectionTarget::parse("http://aleph.example.com").unwrap();
        assert_eq!(t3.to_persisted(), "http://aleph.example.com:80");
    }

    /// The bare-host form is the LAN case and must keep pointing at the
    /// aleph-server default listener — this is the half that must NOT move.
    #[test]
    fn a_bare_host_still_gets_the_aleph_default_port() {
        for raw in ["192.168.1.5", "box.lan", "[::1]"] {
            let t = ConnectionTarget::parse(raw).unwrap();
            assert!(
                t.to_persisted().ends_with(":18790"),
                "bare host {raw} must keep the aleph default port, got {}",
                t.to_persisted()
            );
        }
    }

    /// Every already-saved target round-trips unchanged: `to_persisted` always
    /// writes an explicit port, so reloading it takes the "user was explicit"
    /// branch. This is the no-regression argument for the rule change above —
    /// existing installs cannot be re-interpreted by it.
    #[test]
    fn a_persisted_target_round_trips_its_port() {
        for raw in [
            "192.168.1.5",
            "box.lan:9000",
            "https://gw.example.com",
            "http://gw.example.com",
            "http://[::1]",
            "https://gw.example.com:8443",
        ] {
            let once = ConnectionTarget::parse(raw).unwrap();
            let persisted = once.to_persisted();
            let twice = ConnectionTarget::parse(&persisted).unwrap();
            assert_eq!(
                once, twice,
                "reloading the persisted form of {raw} ({persisted}) changed the target"
            );
        }
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
        // Typed scheme → that scheme's port; bare host → the aleph default.
        let t = ConnectionTarget::parse("http://[::1]").unwrap();
        assert_eq!(t.to_persisted(), "http://[::1]:80");
        let bare = ConnectionTarget::parse("[::1]").unwrap();
        assert_eq!(bare.to_persisted(), "http://[::1]:18790");
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
    fn an_empty_message_leaves_the_connect_page_url_untouched() {
        // First run has nothing to explain; it must not grow a stray `#err=`.
        assert_eq!(connect_page_url_with_error(""), connect_page_url());
    }

    #[test]
    fn the_reason_survives_the_url_round_trip() {
        // The page reads this back with URLSearchParams over the fragment, so
        // the encoding has to be recoverable — not merely "looks escaped".
        let msg = "No Aleph server answered at https://gw.example.com:18790. \
                   Check the address & that the server is running.";
        let url = Url::parse(&connect_page_url_with_error(msg)).unwrap();
        let frag = url.fragment().expect("reason must ride the fragment");
        let recovered = form_urlencoded::parse(frag.as_bytes())
            .find(|(k, _)| k == "err")
            .map(|(_, v)| v.into_owned())
            .expect("`err` key must be present");
        assert_eq!(recovered, msg);
    }

    #[test]
    fn a_hostname_cannot_truncate_the_reason() {
        // `#` and `&` are the two characters that would silently cut the
        // message short if it were concatenated instead of encoded.
        let msg = "unreachable at https://a#b&c:443 — check it";
        let url = Url::parse(&connect_page_url_with_error(msg)).unwrap();
        let recovered = form_urlencoded::parse(url.fragment().unwrap().as_bytes())
            .find(|(k, _)| k == "err")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert_eq!(recovered, msg, "the whole message must survive");
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

    /// W-12 lock-tests for the target-switch token swap. Each test scopes
    /// `HOME` to its own tempdir so the platform marker path resolves under
    /// `.aleph/desktop-shell-gateway-token` without touching the user's
    /// real marker. `serial_test` keeps the env-var mutation from racing
    /// with sibling tests that share `dirs::home_dir()`.
    #[test]
    #[serial]
    fn remove_gateway_token_is_ok_when_marker_missing() {
        let prev = scoped_temp_home("w12-missing");
        assert!(remove_gateway_token().is_ok(), "missing marker is Ok");
        restore_home(prev);
    }

    #[test]
    #[serial]
    fn remove_gateway_token_deletes_existing_marker() {
        let prev = scoped_temp_home("w12-exists");
        store_gateway_token("STALE-CRED").expect("write seed");
        assert!(load_gateway_token().is_some(), "seed visible");
        assert!(remove_gateway_token().is_ok(), "delete Ok");
        assert!(load_gateway_token().is_none(), "marker cleared");
        restore_home(prev);
    }

    /// W-12: when `std::fs::remove_file` fails (parent dir read-only), the
    /// atomic-overwrite fallback must kick in so the next `load_gateway_token`
    /// cannot hand the stale credential to the new target. With the marker
    /// file already on disk, truncating it in place does NOT require write
    /// perm on the parent (only on the file), so the fallback returns Ok
    /// and the next load sees an empty marker — which is the safety property.
    ///
    /// Skipped under root: the DAC bypass means chmod 0500 on a dir the
    /// process also owns does not actually deny unlink, so the test would
    /// spuriously observe the trivial Ok path.
    #[cfg(unix)]
    #[test]
    #[serial]
    fn remove_gateway_token_overwrites_atomically_when_unlink_fails() {
        use std::os::unix::fs::PermissionsExt;

        let uid = nix_like_geteuid();
        if uid == 0 {
            eprintln!("skipping: euid 0 bypasses chmod 0500");
            return;
        }

        let prev = scoped_temp_home("w12-overwrite-fallback");
        let marker = gateway_token_marker().expect("home set");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "STALE-CRED").unwrap();
        std::fs::set_permissions(
            marker.parent().unwrap(),
            std::fs::Permissions::from_mode(0o500),
        )
        .expect("chmod parent to 0500 — unlink denied, truncate-file OK");

        // remove_file MUST fail with PermissionDenied (chmod 0500 denies
        // write on the parent, which is what unlink needs).
        let unlink = std::fs::remove_file(&marker);
        assert!(unlink.is_err(), "sanity: unlink fails: {unlink:?}");
        // Re-seed the marker so the *real* call has work to do.
        std::fs::write(&marker, "STALE-CRED").unwrap();
        std::fs::set_permissions(
            marker.parent().unwrap(),
            std::fs::Permissions::from_mode(0o500),
        )
        .expect("re-chmod parent to 0500");

        // The actual contract: even with remove_file denied, the
        // atomic-overwrite fallback must wipe the stale credential so
        // load_gateway_token() can never hand it to the next target.
        assert!(
            remove_gateway_token().is_ok(),
            "overwrite fallback must clear the stale file even when unlink fails"
        );
        assert!(
            load_gateway_token().is_none(),
            "next load must not see the prior remote's credential"
        );

        // Restore so the tempdir can be cleaned up by the harness / OS.
        let _ = std::fs::set_permissions(
            marker.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        );
        restore_home(prev);
    }

    /// Return the effective user id without pulling in `libc`. On Unix
    /// `getuid()` is a libc-only call; the safest portable substitute is to
    /// parse the uid from `/proc/self/status` (Linux), falling back to
    /// "non-zero" elsewhere. The 0-detection only matters for the test
    /// gating — a wrongly-assumed non-zero just keeps the test running.
    fn nix_like_geteuid() -> u32 {
        #[cfg(target_os = "linux")]
        {
            if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
                for line in s.lines() {
                    if let Some(rest) = line.strip_prefix("Uid:") {
                        if let Some(uid) = rest.split_whitespace().next() {
                            return uid.parse().unwrap_or(1);
                        }
                    }
                }
            }
            1
        }
        #[cfg(not(target_os = "linux"))]
        {
            1
        }
    }

    /// W-12: the *remove-fails-but-overwrite-succeeds* path cannot be
    /// exercised reliably on Unix without root (chattr +i) or a custom
    /// filesystem, so this companion test pins the contract through the
    /// outer caller: `persist_credential_from_url` with a credential-free
    /// input must thread the result. It still calls `remove_gateway_token`
    /// under the hood, but on a writable directory the Ok path covers
    /// both the delete and the no-op-NotFound branches. A future refactor
    /// that drops the atomic-overwrite fallback would still pass this
    /// test on a writable filesystem — the *both-fail* test above is the
    /// one that pins that branch.
    #[test]
    #[serial]
    fn persist_credential_from_url_returns_err_on_corrupt_home() {
        // No HOME → marker_path returns None → no I/O attempted → Ok(()).
        // We force the failure mode by pointing HOME at a path whose
        // .aleph parent does not exist and is read-only at the level
        // above. Done via a chmod on a parent we own.
        let prev = scoped_temp_home("w12-persist-err");
        // Wipe the marker dir entirely and re-create it as a read-only
        // file so create_dir_all inside store_gateway_token fails.
        let marker = gateway_token_marker().expect("home set");
        let _ = std::fs::remove_dir_all(marker.parent().unwrap());
        std::fs::write(marker.parent().unwrap(), b"file-instead-of-dir").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                marker.parent().unwrap(),
                std::fs::Permissions::from_mode(0o500),
            )
            .unwrap();
        }
        let err = persist_credential_from_url(&url::Url::parse("https://gw.example/").unwrap())
            .expect_err("no credential + unwritable store → Err");
        assert!(!err.is_empty(), "error message non-empty: {err}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                marker.parent().unwrap(),
                std::fs::Permissions::from_mode(0o700),
            );
        }
        restore_home(prev);
    }

    /// Set HOME to a fresh tempdir under `/tmp/aleph_marker_test_<tag>` and
    /// return the previous value so `restore_home` can put it back. The
    /// helper does the directory creation that `store_gateway_token` would
    /// otherwise do lazily on first write.
    fn scoped_temp_home(tag: &str) -> Option<std::ffi::OsString> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::path::PathBuf::from(format!(
            "/tmp/aleph_marker_test_{tag}_{n}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scoped temp home");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &path);
        prev
    }

    fn restore_home(prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    /// W-18: the global target-change watch must publish ticks on demand.
    /// This test pins the watch wiring itself — the *integration* with
    /// `set_connection_target` is harder to assert without bringing up a
    /// `tauri::AppHandle`, but the publish half is the actual new code, so
    /// that is what we lock down.
    #[tokio::test]
    async fn target_change_signal_publishes_a_tick() {
        let mut rx = target_change_rx();
        // Mark the initial value as seen so the next `changed()` actually
        // waits for a *new* tick.
        rx.borrow_and_update();
        signal_target_change();
        // A wait_with timeout so a regression (no tick) fails loudly instead
        // of hanging the test binary.
        let got = tokio::time::timeout(std::time::Duration::from_millis(200), rx.changed()).await;
        assert!(
            got.is_ok(),
            "signal_target_change did not publish within 200ms"
        );
        // `changed()` returning Ok itself proves a tick arrived — the value
        // is auto-marked-as-read once changed() resolves. Nothing more to
        // assert without a second tick.
    }

    /// W-18: multiple subscribers see the same signal_target_change tick.
    /// Long-lived subscribers (the notification bridge is the canonical one)
    /// spawn their own watch::Receiver via `target_change_rx()`; the static
    /// `OnceLock<watch::Sender>` must serve them all from one channel.
    #[tokio::test]
    async fn target_change_signal_reaches_multiple_subscribers() {
        let mut a = target_change_rx();
        let mut b = target_change_rx();
        a.borrow_and_update();
        b.borrow_and_update();
        signal_target_change();
        let ta = tokio::time::timeout(std::time::Duration::from_millis(200), a.changed()).await;
        let tb = tokio::time::timeout(std::time::Duration::from_millis(200), b.changed()).await;
        assert!(ta.is_ok(), "subscriber a missed the tick");
        assert!(tb.is_ok(), "subscriber b missed the tick");
    }
}
