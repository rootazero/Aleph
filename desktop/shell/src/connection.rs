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
    marker_home().map(|h| h.join(".aleph").join(format!("{MARKER_PREFIX}-{name}")))
}

/// Home directory the markers live under. Production: the real user home.
/// Test builds: ONLY the owned temp root installed by [`test_home::TestHome`]
/// on the current thread, otherwise `None` — a test can never fall back to (or
/// be redirected by env vars to) the real home, so marker I/O in tests is
/// structurally confined to a temp dir we created.
#[cfg(all(not(test), not(feature = "qa-owned-root")))]
fn marker_home() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

/// QA build (`qa-owned-root`, never default): markers live under the owned
/// QA root proven by `qa_root::init_from_args` — never the real home, never an
/// env-derived path. `None` (no storage) if the root was not initialised.
#[cfg(all(not(test), feature = "qa-owned-root"))]
fn marker_home() -> Option<std::path::PathBuf> {
    crate::qa_root::try_root().map(std::path::Path::to_path_buf)
}

#[cfg(test)]
fn marker_home() -> Option<std::path::PathBuf> {
    test_home::current()
}

/// Test-only owned home root for marker paths. Deliberately does NOT touch
/// `HOME`/`USERPROFILE` (on Windows `dirs::home_dir` ignores `HOME`, which is
/// how a former env-var-based fixture let a destructive test hit the real
/// profile). The root is a fresh directory we create under the OS temp dir,
/// installed in a thread-local read by `marker_home`; it is the only thing the
/// guard ever deletes.
#[cfg(test)]
pub(crate) mod test_home {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const ROOT_PREFIX: &str = "aleph-shell-marker-test-";

    thread_local! {
        static HOME: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    pub(super) fn current() -> Option<PathBuf> {
        HOME.with(|h| h.borrow().clone())
    }

    /// RAII guard: owns a fresh temp root, routes `marker_path` there for the
    /// current thread, restores the previous routing and deletes the root on drop.
    pub(crate) struct TestHome {
        root: PathBuf,
        prev: Option<PathBuf>,
    }

    impl TestHome {
        pub(crate) fn new(tag: &str) -> Self {
            assert!(
                !tag.is_empty()
                    && tag
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "tag must be a plain identifier: {tag:?}"
            );
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let root = std::env::temp_dir().join(format!(
                "{ROOT_PREFIX}{tag}-{}-{n}-{nanos}",
                std::process::id()
            ));
            // `create_dir` (not `_all`): fail loudly if the name already exists
            // rather than adopting a directory we did not create.
            std::fs::create_dir(&root).expect("create owned temp home root");
            let prev = HOME.with(|h| h.borrow_mut().replace(root.clone()));
            Self { root, prev }
        }

        pub(crate) fn root(&self) -> &Path {
            &self.root
        }

        /// True iff `path` is strictly inside this fixture's owned root.
        /// Destructive tests assert this before deleting anything.
        pub(crate) fn contains(&self, path: &Path) -> bool {
            path.starts_with(&self.root) && path != self.root
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            HOME.with(|h| *h.borrow_mut() = self.prev.take());
            let owned = self.root.starts_with(std::env::temp_dir())
                && self
                    .root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(ROOT_PREFIX));
            if owned {
                let _ = std::fs::remove_dir_all(&self.root);
            } else {
                eprintln!("refusing to delete non-owned root {}", self.root.display());
            }
        }
    }
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

/// Write `path` atomically: stream into a uniquely named sibling temp file
/// (`create_new`; owner-only `0600` on Unix so there is no umask/TOCTOU
/// window), `sync_all`, then `rename` over the destination. std's rename
/// replaces an existing file on every supported platform (Windows uses
/// `MOVEFILE_REPLACE_EXISTING`). A failure at any step removes the temp file
/// and leaves the previous destination byte-for-byte intact, so a reader (or
/// the next launch) never sees a half-written token or marker.
fn write_file_atomic(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let tmp = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let stage = || -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        write(&mut f)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    };
    let result = stage();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Persist a Gateway token (already trimmed non-empty by the caller) in the
/// legacy token file, with the same `.aleph` directory bootstrap as the target.
/// This is the *legacy file* store only — not the OS keyring (that is
/// `keyring_bridge`'s domain, and cannot be rolled back from here). The shared
/// Gateway token grants full operator authority, so it is created owner-only,
/// and goes through [`write_file_atomic`]: a failed write never truncates or
/// half-replaces the previous token.
pub(crate) fn store_gateway_token(token: &str) -> Result<(), String> {
    let marker = gateway_token_marker().ok_or("home directory not found")?;
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .aleph dir: {e}"))?;
    }
    write_file_atomic(&marker, |f| {
        use std::io::Write;
        f.write_all(token.as_bytes())
    })
    .map_err(|e| format!("write gateway token: {e}"))
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
        // In-place truncate, deliberately NOT `store_gateway_token`: the atomic
        // writer needs write permission on the parent directory, which is
        // exactly what just denied the unlink. Truncating to empty has no
        // partial state (old bytes or none).
        Err(remove_err) => match std::fs::write(&marker, b"") {
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
            after_scheme[idx..].starts_with(':') && port_tail_is_explicit(&after_scheme[idx + 1..])
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

/// Atomically replace the target marker with the normalised form of `target`.
fn write_target_marker(marker: &std::path::Path, target: &ConnectionTarget) -> Result<(), String> {
    write_file_atomic(marker, |f| {
        use std::io::Write;
        f.write_all(target.to_persisted().as_bytes())
    })
    .map_err(|e| format!("write target: {e}"))
}

/// Snapshot of the legacy gateway-token file, for rollback. `None` = absent.
type TokenSnapshot = Option<Vec<u8>>;

/// Read the raw legacy token file. Any error other than NotFound is returned:
/// without a snapshot we could not undo a credential swap, so the switch must
/// not begin (fail-closed).
fn snapshot_gateway_token() -> Result<TokenSnapshot, String> {
    let Some(path) = gateway_token_marker() else {
        return Ok(None);
    };
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read gateway token before target switch: {e}")),
    }
}

/// Put the legacy token file back to `prior` (atomic write, or removal).
fn restore_gateway_token(prior: TokenSnapshot) -> Result<(), String> {
    match prior {
        Some(bytes) => {
            let path = gateway_token_marker().ok_or("home directory not found")?;
            write_file_atomic(&path, |f| {
                use std::io::Write;
                f.write_all(&bytes)
            })
            .map_err(|e| format!("restore gateway token: {e}"))
        }
        None => remove_gateway_token(),
    }
}

/// Credential + marker commit, in the order that makes failure safe.
///
/// 1. `.aleph` exists and the old token is snapshotted (either failing aborts
///    with nothing touched).
/// 2. `apply_credential` swaps the legacy token file (atomic write / removal).
///    Failing here leaves the old token and the old marker — still consistent.
/// 3. `write_marker` atomically replaces the marker. Failing here rolls the
///    token file back to the snapshot, so the old marker is not left paired
///    with the new target's credential; if the rollback itself fails the error
///    says so (residual: old marker + new credential until the next switch).
///
/// The marker is the commit point and is written LAST: it never names a target
/// whose credential was not prepared. The two files cannot be committed
/// atomically together, hence the rollback; the OS keyring is not touched here
/// and cannot be rolled back from this layer. Caller holds the keyring gate.
fn commit_target_files(
    marker: &std::path::Path,
    apply_credential: impl FnOnce() -> Result<(), String>,
    write_marker: impl FnOnce(&std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .aleph dir: {e}"))?;
    }
    let prior = snapshot_gateway_token()?;
    apply_credential()?;
    if let Err(write_err) = write_marker(marker) {
        return Err(match restore_gateway_token(prior) {
            Ok(()) => write_err,
            Err(restore_err) => format!(
                "{write_err}; additionally could not restore the previous Gateway \
                 token ({restore_err}): the previous target may still be paired \
                 with the new credential"
            ),
        });
    }
    Ok(())
}

/// Commit a validated target: re-derive the legacy Gateway credential for it
/// (a `?bt=` / `?token=` on a Remote URL is stored; Local or a credential-less
/// URL clears it, so one remote's token is never presented to another target or
/// the local daemon) and then persist the marker — all inside the keyring gate
/// so no keyring operation observes the half-way state. See
/// [`commit_target_files`] for ordering and rollback. On `Err` the caller must
/// neither navigate nor signal.
pub fn save_target(target: &ConnectionTarget) -> Result<(), String> {
    let Some(marker) = target_marker() else {
        return Err("home directory not found".to_string());
    };
    crate::keyring_bridge::commit_target_change(|| {
        commit_target_files(
            &marker,
            || match target {
                ConnectionTarget::Remote(url) => persist_credential_from_url(url),
                ConnectionTarget::Local => remove_gateway_token(),
            },
            |m| write_target_marker(m, target),
        )
    })
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

/// Process-wide gate that serialises whole target switches.
///
/// A switch is a multi-step, multi-`await` transaction: persist the marker,
/// re-derive the credential, update the external-link allow-list, then
/// probe + navigate and finally tick subscribers. Two overlapping switches
/// (connect page, tray and menu can all fire one) used to interleave —
/// `A.save, B.save, B.meta, A.meta` — leaving the native config on B while the
/// allow-list / webview followed A. The whole transaction therefore runs
/// inside this async mutex (a `tokio` one: a std mutex must not be held across
/// `.await`, and waiting here parks only the task, never the main thread).
///
/// Cost: a slow target probe delays the *next* switch until it resolves
/// (bounded by the probe's own timeout). Side effects stay ordered, and a
/// persistence failure aborts the switch before any allow-list update,
/// navigation or tick (see [`commit_target_files`] for the legacy-file
/// ordering/rollback and its residual: the OS keyring is not rolled back).
fn target_switch_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Run `f` without interleaving another target switch. It starts only once
/// every earlier switch (including its navigation and tick) has finished.
/// This serializes effects; rollback of the legacy files is
/// [`commit_target_files`]'s job, not the gate's.
pub(crate) async fn serialize_target_switch<F, Fut, T>(f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let _guard = target_switch_gate().lock().await;
    f().await
}

/// Re-route for whatever target is currently persisted (e.g. after a cert is
/// pinned), under the same gate as a switch so it cannot observe, or navigate
/// to, a half-applied one. The target is read INSIDE the gate (after any
/// in-flight switch has committed or rolled back its marker), never captured
/// before waiting. A switch whose persistence failed leaves the old marker, so
/// this re-routes to the previous, still-consistent target.
pub(crate) async fn reroute_current_target(app: tauri::AppHandle) {
    serialize_target_switch(|| async move {
        let target = load_target();
        crate::reroute_for_target(&app, target).await;
    })
    .await
}

/// Validate + persist a new target, update the external-link allow-list, and
/// re-route (navigate + supervise) for it. `raw` accepts the same forms as
/// `ConnectionTarget::parse`.
///
/// `async` on purpose: `save_target` waits on the keyring gate, which can be
/// held across an OS credential prompt for an unbounded time. A sync command
/// would run that wait on the main thread and freeze the UI, so the write runs
/// in `spawn_blocking`; navigation and events still happen only after it
/// succeeded, on this (async) side.
///
/// The whole operation — parse through tick — runs under
/// [`serialize_target_switch`], and `reroute_for_target` is *awaited* (not
/// spawned) so the gate is held until the navigation is committed.
#[tauri::command]
pub async fn set_connection_target(app: tauri::AppHandle, raw: String) -> Result<(), String> {
    serialize_target_switch(|| async move {
        let target = ConnectionTarget::parse(&raw)?;
        let to_save = target.clone();
        // `save_target` re-derives the legacy Gateway credential (so the native
        // notification bridge can authorize against a remote token-protected
        // Gateway too, R5 — desktop banners on remote; `?bt=` bootstrap ticket
        // preferred over legacy `?token=`) and only then commits the marker, with
        // rollback. Any `Err` returns here: no allow-list update, no navigation,
        // no tick — the previous target stays in force.
        tauri::async_runtime::spawn_blocking(move || save_target(&to_save))
            .await
            // Fixed text: a join error carries no secret and no wire detail.
            .map_err(|_| "save target: background task failed".to_string())??;
        match &target {
            ConnectionTarget::Remote(url) => {
                crate::external_link::set_remote_host(Some(url.clone()))
            }
            ConnectionTarget::Local => crate::external_link::set_remote_host(None),
        }
        crate::reroute_for_target(&app, target).await;
        // W-18: tell long-lived subscribers (notification bridge, future status
        // indicators) that the target moved. Subscribers receive the tick, close
        // the old stream, and the next loop iteration reads the new
        // ConnectionTarget. Without this a Remote-A → Remote-B switch leaves the
        // bridge subscribed to A indefinitely.
        signal_target_change();
        Ok(())
    })
    .await
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
pub async fn clear_connection_target(app: tauri::AppHandle) -> Result<(), String> {
    set_connection_target(app, "local".to_string()).await
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

    /// W-12 lock-tests for the target-switch token swap. Each test installs a
    /// [`test_home::TestHome`] (an owned temp root, thread-local; no
    /// `HOME`/`USERPROFILE` mutation) so the marker path resolves under
    /// `<root>/.aleph/...-gateway-token` and can never reach the user's real
    /// marker. Without a guard `marker_path` is `None` in test builds.
    #[test]
    #[serial]
    fn remove_gateway_token_is_ok_when_marker_missing() {
        let _home = test_home::TestHome::new("w12-missing");
        assert!(remove_gateway_token().is_ok(), "missing marker is Ok");
    }

    #[test]
    #[serial]
    fn remove_gateway_token_deletes_existing_marker() {
        let _home = test_home::TestHome::new("w12-exists");
        store_gateway_token("STALE-CRED").expect("write seed");
        assert!(load_gateway_token().is_some(), "seed visible");
        assert!(remove_gateway_token().is_ok(), "delete Ok");
        assert!(load_gateway_token().is_none(), "marker cleared");
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

        let home = test_home::TestHome::new("w12-overwrite-fallback");
        let marker = gateway_token_marker().expect("home set");
        assert!(
            home.contains(&marker),
            "marker must be under the owned root"
        );
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
    }

    /// Return the effective user id without pulling in `libc`. On Unix
    /// `getuid()` is a libc-only call; the safest portable substitute is to
    /// parse the uid from `/proc/self/status` (Linux), falling back to
    /// "non-zero" elsewhere. The 0-detection only matters for the test
    /// gating — a wrongly-assumed non-zero just keeps the test running.
    #[cfg(unix)]
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

    /// W-12: `persist_credential_from_url` must surface store failures so the
    /// surrounding target switch can refuse to proceed. Two failure shapes,
    /// both cross-platform (no chmod) and confined to the owned fixture root:
    ///
    /// * credential present, `<root>/.aleph` is a *file* → `create_dir_all`
    ///   fails → `Err`.
    /// * no credential, the marker path is a *directory* → `remove_file` fails
    ///   with something other than NotFound (on Windows a file-as-parent would
    ///   report NotFound and legitimately return Ok, so that shape cannot be
    ///   used here), and the empty-overwrite fallback fails too → `Err`.
    #[test]
    #[serial]
    fn persist_credential_from_url_returns_err_on_corrupt_home() {
        let home = test_home::TestHome::new("w12-persist-err");
        let marker = gateway_token_marker().expect("home set");
        let dot_aleph = marker.parent().unwrap().to_path_buf();
        assert!(home.contains(&dot_aleph), "must be under the owned root");
        std::fs::write(&dot_aleph, b"file-instead-of-dir").unwrap();
        let err = persist_credential_from_url(
            &url::Url::parse("https://gw.example/?token=SECRET").unwrap(),
        )
        .expect_err("credential + unwritable store → Err");
        assert!(err.contains("create .aleph dir"), "got: {err}");
    }

    #[test]
    #[serial]
    fn persist_credential_from_url_without_credential_errs_when_marker_undeletable() {
        let home = test_home::TestHome::new("w12-persist-nocred-err");
        let marker = gateway_token_marker().expect("home set");
        assert!(home.contains(&marker), "must be under the owned root");
        std::fs::create_dir_all(&marker).unwrap(); // a directory where the file should be
        let err = persist_credential_from_url(&url::Url::parse("https://gw.example/").unwrap())
            .expect_err("no credential + undeletable marker + failed overwrite → Err");
        assert!(err.contains("could not remove Gateway token"), "got: {err}");
    }

    /// Structural guard: without a fixture, test builds resolve NO marker path
    /// (never the real home), and the fixture root is an owned temp dir.
    #[test]
    fn test_builds_never_resolve_a_real_home() {
        assert!(marker_path("target").is_none(), "no fixture → no path");
        let home = test_home::TestHome::new("guard-check");
        let p = marker_path("target").expect("fixture installed");
        assert!(home.contains(&p));
        assert!(home.root().starts_with(std::env::temp_dir()));
        drop(home);
        assert!(marker_path("target").is_none(), "routing restored on drop");
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

    /// Two overlapping switches must not interleave. A's save is parked on a
    /// oneshot while B is started; with the gate, B's whole transaction
    /// (save, metadata, nav, tick) runs strictly after A's — the split
    /// `A.save B.save B.meta A.meta` that left native config on B and the
    /// allow-list on A is impossible.
    #[tokio::test]
    async fn overlapping_target_switches_run_whole_transactions_in_order() {
        use std::sync::{Arc, Mutex};
        let log: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let (release_a, a_parked) = tokio::sync::oneshot::channel::<()>();
        let (a_started_tx, a_started) = tokio::sync::oneshot::channel::<()>();

        let la = log.clone();
        let a = tokio::spawn(serialize_target_switch(move || async move {
            la.lock().unwrap().push("A.save:begin");
            let _ = a_started_tx.send(());
            a_parked.await.unwrap(); // A's save is still in flight here
            la.lock().unwrap().push("A.save:end");
            la.lock().unwrap().push("A.meta");
            tokio::task::yield_now().await;
            la.lock().unwrap().push("A.nav");
            la.lock().unwrap().push("A.tick");
        }));
        a_started.await.unwrap();

        let lb = log.clone();
        let b = tokio::spawn(serialize_target_switch(move || async move {
            lb.lock().unwrap().push("B.save");
            tokio::task::yield_now().await;
            lb.lock().unwrap().push("B.meta");
            lb.lock().unwrap().push("B.nav");
            lb.lock().unwrap().push("B.tick");
        }));

        // Give B every chance to barge in while A is parked.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(*log.lock().unwrap(), vec!["A.save:begin"]);

        release_a.send(()).unwrap();
        a.await.unwrap();
        b.await.unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "A.save:begin",
                "A.save:end",
                "A.meta",
                "A.nav",
                "A.tick",
                "B.save",
                "B.meta",
                "B.nav",
                "B.tick"
            ]
        );
    }

    /// Source pin: the production command must route its whole body through
    /// the gate and await (not spawn) the re-route, else the helper test above
    /// proves nothing about the real path.
    #[test]
    fn set_connection_target_is_gated_end_to_end() {
        let src = include_str!("connection.rs").replace('\r', "");
        let start = src.find("pub async fn set_connection_target(").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let gate = body.find("serialize_target_switch(").expect("not gated");
        for step in [
            "ConnectionTarget::parse(",
            "save_target(",
            "set_remote_host(",
            "reroute_for_target(&app, target).await",
            "signal_target_change()",
        ] {
            let at = body.find(step).unwrap_or_else(|| panic!("missing {step}"));
            assert!(at > gate, "{step} runs outside the target-switch gate");
        }
    }

    /// Source pin for the fail-closed ordering: the persistence step (legacy
    /// credential + marker, via `save_target`) is propagated with `?` BEFORE
    /// the allow-list update, navigation and tick, and the credential is no
    /// longer applied by a separate step outside `save_target` (which would
    /// re-open the "new marker + old token" window).
    #[test]
    fn set_connection_target_persists_before_any_side_effect() {
        let src = include_str!("connection.rs").replace('\r', "");
        let start = src.find("pub async fn set_connection_target(").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };
        let save = at("save_target(");
        let hosts = at("set_remote_host(");
        let nav = at("reroute_for_target(&app, target).await");
        let tick = at("signal_target_change()");
        assert!(save < hosts && hosts < nav && nav < tick, "order broken");
        // The save's error must short-circuit (`??` = join error, then Err).
        assert!(
            body[save..hosts].contains("??;"),
            "save_target result must be propagated before any side effect"
        );
        for forbidden in ["persist_credential_from_url(", "remove_gateway_token("] {
            assert!(
                !body.contains(forbidden),
                "{forbidden} must run inside save_target (credential-first, \
                 marker-last), not as a separate step of the command"
            );
        }
    }

    /// Source pin: `reroute_current_target` (used by cert approval) shares the
    /// switch gate and reads the persisted target only after acquiring it.
    #[test]
    fn reroute_current_target_is_gated_and_reads_target_inside_the_gate() {
        let src = include_str!("connection.rs").replace('\r', "");
        let start = src
            .find("pub(crate) async fn reroute_current_target(")
            .unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let gate = body.find("serialize_target_switch(").expect("not gated");
        let load = body.find("load_target()").expect("does not reload target");
        let nav = body.find("reroute_for_target(").expect("does not reroute");
        assert!(gate < load && load < nav, "target must be read inside gate");
    }

    // -- atomic writer / fail-closed commit (TestHome only: no real home, no
    //    OS keyring, no AppHandle) ---------------------------------------

    fn tmp_siblings(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".tmp"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    #[serial]
    fn store_gateway_token_replaces_an_existing_token() {
        let _home = test_home::TestHome::new("t44-replace");
        store_gateway_token("OLD").unwrap();
        store_gateway_token("NEW").unwrap();
        assert_eq!(load_gateway_token().as_deref(), Some("NEW"));
        let dir = gateway_token_marker()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(tmp_siblings(&dir).is_empty(), "temp file leaked");
    }

    #[test]
    #[serial]
    fn failed_atomic_write_keeps_the_old_file_and_leaves_no_temp() {
        let _home = test_home::TestHome::new("t44-atomic-write-fail");
        store_gateway_token("OLD").unwrap();
        let path = gateway_token_marker().unwrap();
        let err = write_file_atomic(&path, |f| {
            use std::io::Write;
            f.write_all(b"PARTIAL-NEW")?;
            Err(std::io::Error::other("disk full"))
        })
        .expect_err("writer failure must surface");
        assert!(err.to_string().contains("disk full"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "OLD");
        assert!(tmp_siblings(path.parent().unwrap()).is_empty());
    }

    #[test]
    #[serial]
    fn failed_atomic_rename_keeps_destination_and_leaves_no_temp() {
        let _home = test_home::TestHome::new("t44-atomic-rename-fail");
        // A directory at the destination makes the final rename fail on every
        // platform, after the temp file was fully written.
        let path = gateway_token_marker().unwrap();
        std::fs::create_dir_all(&path).unwrap();
        let res = write_file_atomic(&path, |f| {
            use std::io::Write;
            f.write_all(b"NEW")
        });
        assert!(res.is_err(), "rename onto a directory must fail");
        assert!(path.is_dir(), "destination untouched");
        assert!(tmp_siblings(path.parent().unwrap()).is_empty());
        let err = store_gateway_token("NEW").expect_err("store must report it");
        assert!(err.contains("write gateway token"), "got: {err}");
    }

    #[test]
    #[serial]
    fn credential_failure_leaves_marker_and_token_untouched() {
        let _home = test_home::TestHome::new("t44-cred-fail");
        let marker = target_marker().unwrap();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "http://old.lan:18790").unwrap();
        store_gateway_token("OLD").unwrap();
        let mut marker_written = false;
        let err = commit_target_files(
            &marker,
            || Err("credential boom".to_string()),
            |_| {
                marker_written = true;
                Ok(())
            },
        )
        .expect_err("credential failure must fail the commit");
        assert_eq!(err, "credential boom");
        assert!(
            !marker_written,
            "marker must not be written after a credential failure"
        );
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "http://old.lan:18790"
        );
        assert_eq!(load_gateway_token().as_deref(), Some("OLD"));
    }

    #[test]
    #[serial]
    fn real_credential_store_failure_keeps_old_marker() {
        let _home = test_home::TestHome::new("t44-cred-real-fail");
        let marker = target_marker().unwrap();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "local").unwrap();
        // Token path is a directory: the snapshot read (not NotFound) fails.
        std::fs::create_dir_all(gateway_token_marker().unwrap()).unwrap();
        let new_target =
            ConnectionTarget::parse("https://gw.example.com:8443/?token=SECRET").unwrap();
        let url = match &new_target {
            ConnectionTarget::Remote(u) => u.clone(),
            ConnectionTarget::Local => unreachable!(),
        };
        let res = commit_target_files(
            &marker,
            || persist_credential_from_url(&url),
            |m| write_target_marker(m, &new_target),
        );
        assert!(res.is_err(), "must not report success");
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "local");
    }

    #[test]
    #[serial]
    fn marker_write_failure_restores_the_previous_token() {
        let _home = test_home::TestHome::new("t44-marker-fail-restore");
        let marker = target_marker().unwrap();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "http://old.lan:18790").unwrap();
        store_gateway_token("OLD").unwrap();
        let err = commit_target_files(
            &marker,
            || store_gateway_token("NEW"),
            |_| Err("write target: boom".to_string()),
        )
        .expect_err("marker failure must not report success");
        assert_eq!(err, "write target: boom");
        assert_eq!(
            load_gateway_token().as_deref(),
            Some("OLD"),
            "token rolled back"
        );
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "http://old.lan:18790"
        );
    }

    #[test]
    #[serial]
    fn marker_write_failure_removes_a_token_that_did_not_exist_before() {
        let _home = test_home::TestHome::new("t44-marker-fail-absent");
        let marker = target_marker().unwrap();
        let res = commit_target_files(
            &marker,
            || store_gateway_token("NEW"),
            |_| Err("write target: boom".to_string()),
        );
        assert!(res.is_err());
        assert!(
            !gateway_token_marker().unwrap().exists(),
            "a token created for the failed switch must not survive it"
        );
        assert!(!marker.exists());
    }

    #[test]
    #[serial]
    fn real_marker_write_failure_is_reported_and_rolls_back() {
        let _home = test_home::TestHome::new("t44-marker-dir");
        let marker = target_marker().unwrap();
        // A directory where the marker file belongs: atomic rename must fail.
        std::fs::create_dir_all(&marker).unwrap();
        store_gateway_token("OLD").unwrap();
        let target = ConnectionTarget::parse("https://gw.example.com:8443").unwrap();
        let err = commit_target_files(
            &marker,
            || store_gateway_token("NEW"),
            |m| write_target_marker(m, &target),
        )
        .expect_err("must not report success");
        assert!(err.contains("write target"), "got: {err}");
        assert_eq!(load_gateway_token().as_deref(), Some("OLD"));
    }

    #[test]
    #[serial]
    fn failed_rollback_is_reported_not_hidden() {
        let _home = test_home::TestHome::new("t44-rollback-fail");
        let marker = target_marker().unwrap();
        store_gateway_token("OLD").unwrap();
        let token_path = gateway_token_marker().unwrap();
        let err = commit_target_files(
            &marker,
            || store_gateway_token("NEW"),
            |_| {
                // Sabotage the rollback: replace the token file with a
                // directory so the restore's rename must fail.
                std::fs::remove_file(&token_path).unwrap();
                std::fs::create_dir_all(&token_path).unwrap();
                Err("write target: boom".to_string())
            },
        )
        .expect_err("must fail");
        assert!(err.starts_with("write target: boom"), "got: {err}");
        assert!(
            err.contains("could not restore the previous Gateway token"),
            "got: {err}"
        );
    }

    #[test]
    #[serial]
    fn successful_commit_writes_credential_then_marker() {
        let _home = test_home::TestHome::new("t44-commit-ok");
        let marker = target_marker().unwrap();
        let target = ConnectionTarget::parse("https://gw.example.com:8443").unwrap();
        let order = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
        let cred_order = std::rc::Rc::clone(&order);
        let marker_order = std::rc::Rc::clone(&order);
        commit_target_files(
            &marker,
            move || {
                cred_order.borrow_mut().push("credential");
                store_gateway_token("NEW")
            },
            move |m| {
                marker_order.borrow_mut().push("marker");
                write_target_marker(m, &target)
            },
        )
        .unwrap();
        assert_eq!(*order.borrow(), vec!["credential", "marker"]);
        assert_eq!(load_gateway_token().as_deref(), Some("NEW"));
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "https://gw.example.com:8443"
        );
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
