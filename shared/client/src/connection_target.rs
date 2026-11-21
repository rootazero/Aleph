//! Canonical connection target parsing shared by desktop hosts.
//! Storage, certificate trust, credentials and service supervision remain host-owned.
use url::Url;

const DEFAULT_PORT: u16 = 18790;
const LOCAL_PANEL_ORIGIN: &str = "http://127.0.0.1:18790";

#[derive(Clone, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// The conventional loopback Gateway; each host owns its startup policy.
    Local,
    /// Connect to a remote Gateway by origin; never touch the local daemon.
    Remote(Url),
}

impl ConnectionTarget {
    /// The credential endpoint for this target, subject to the existing transport policy.
    pub fn credential_gateway_url(&self) -> crate::CliResult<String> {
        let mut gateway = match self {
            Self::Local => Url::parse(LOCAL_PANEL_ORIGIN).expect("constant Panel origin"),
            Self::Remote(url) => url.clone(),
        };
        let scheme = match gateway.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => {
                return Err(crate::CliError::Config(
                    "unsupported Panel transport".into(),
                ))
            }
        };
        gateway.set_scheme(scheme).expect("hierarchical HTTP URL");
        gateway
            .set_username("")
            .expect("HTTP URL supports authority");
        gateway
            .set_password(None)
            .expect("HTTP URL supports authority");
        gateway.set_path("/ws");
        gateway.set_query(None);
        gateway.set_fragment(None);
        crate::credential::ensure_credential_transport(gateway.as_str())?;
        Ok(gateway.into())
    }

    /// The root hosting the shared Panel, without credentials or bootstrap material.
    pub fn panel_url(&self) -> String {
        match self {
            Self::Local => format!("{LOCAL_PANEL_ORIGIN}/"),
            Self::Remote(url) => format!("{}/", url.origin().ascii_serialization()),
        }
    }

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
            // `LOCAL_PANEL_ORIGIN` is already exactly the loopback origin's
            // serialization (http scheme, non-default port), so a string
            // compare is a parsed-origin compare without parsing a constant.
            Self::Local => url.origin().unicode_serialization() == LOCAL_PANEL_ORIGIN,
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
            let after = tail.get(end..).unwrap_or("");
            end > 0
                && (after.is_empty()
                    || after.starts_with('/')
                    || after.starts_with('?')
                    || after.starts_with('#'))
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
        raw.get(pos + 3..).unwrap_or("")
    } else {
        raw
    };
    // Inspect only the authority's host/port, never a userinfo colon or a
    // lookalike port in the path, query or fragment. Url has validated it above.
    let after_scheme = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
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
            after_scheme.get(idx..).is_some_and(|s| s.starts_with(':'))
                && port_tail_is_explicit(after_scheme.get(idx + 1..).unwrap_or(""))
        }
        // Plain host:port — digits after the `:`, terminated by `/`, `?`,
        // `#`, or end-of-string. `host:80?bt=...` is an explicit port 80
        // followed by a query string, not a port labelled `80?bt=...`.
        Some(colon) => port_tail_is_explicit(after_scheme.get(colon + 1..).unwrap_or("")),
        None => false,
    }
}

impl std::fmt::Debug for ConnectionTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("Local"),
            Self::Remote(_) => f.debug_tuple("Remote").field(&self.to_persisted()).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectionTarget;
    use url::Url;

    #[test]
    fn panel_url_is_an_origin_root_without_secret_material() {
        assert_eq!(
            ConnectionTarget::Local.panel_url(),
            "http://127.0.0.1:18790/"
        );
        let target =
            ConnectionTarget::parse("https://user:password@box.lan:8443/ignored?bt=synthetic#frag")
                .unwrap();
        assert_eq!(target.panel_url(), "https://box.lan:8443/");
        let target = ConnectionTarget::parse("https://[::1]").unwrap();
        assert_eq!(target.panel_url(), "https://[::1]/");
    }

    #[test]
    fn credential_endpoint_reuses_the_canonical_transport_policy() {
        for (raw, expected) in [
            ("local", "ws://127.0.0.1:18790/ws"),
            ("https://box.lan:8443", "wss://box.lan:8443/ws"),
            ("https://[::1]", "wss://[::1]/ws"),
            ("http://localhost:9876", "ws://localhost:9876/ws"),
            ("http://[::1]:9876", "ws://[::1]:9876/ws"),
        ] {
            assert_eq!(
                ConnectionTarget::parse(raw)
                    .unwrap()
                    .credential_gateway_url()
                    .unwrap(),
                expected
            );
        }
        for raw in ["http://box.lan", "box.lan", "http://127.0.0.1.evil.example"] {
            let error = ConnectionTarget::parse(raw)
                .unwrap()
                .credential_gateway_url()
                .unwrap_err();
            assert!(!error.to_string().contains(raw));
        }
    }

    #[test]
    fn local_spellings_remain_local() {
        for input in ["", "  ", "local", "LOCAL"] {
            assert_eq!(
                ConnectionTarget::parse(input).unwrap(),
                ConnectionTarget::Local
            );
        }
    }

    #[test]
    fn explicit_scheme_and_explicit_ports_keep_the_existing_contract() {
        for (input, expected) in [
            ("box.lan", "http://box.lan:18790"),
            ("box.lan:9000", "http://box.lan:9000"),
            ("https://gw.example.com", "https://gw.example.com:443"),
            ("http://gw.example.com", "http://gw.example.com:80"),
            ("https://box.lan:80?bt=synthetic", "https://box.lan:80"),
            ("http://box.lan:443#frag", "http://box.lan:443"),
            ("https://box.lan:9999/abc", "https://box.lan:9999"),
            ("[::1]", "http://[::1]:18790"),
            ("http://[::1]", "http://[::1]:80"),
            ("https://[::1]:8443", "https://[::1]:8443"),
        ] {
            let target = ConnectionTarget::parse(input).unwrap();
            assert_eq!(target.to_persisted(), expected);
            let persisted = ConnectionTarget::parse(&target.to_persisted()).unwrap();
            assert_eq!(persisted.to_persisted(), expected);
        }
    }

    #[test]
    fn parser_retains_bootstrap_material_for_existing_host_consumer_but_persistence_does_not() {
        let ConnectionTarget::Remote(url) =
            ConnectionTarget::parse("https://box.lan:9999/abc?bt=synthetic#frag").unwrap()
        else {
            panic!("expected remote")
        };
        assert_eq!(url.query(), Some("bt=synthetic"));
        assert_eq!(url.path(), "/abc");
        assert_eq!(url.fragment(), Some("frag"));
    }

    #[test]
    fn unsupported_schemes_fail_without_echoing_the_input() {
        for raw in ["ftp://host", "ws://host", "file:///tmp/data"] {
            assert!(ConnectionTarget::parse(raw).is_err());
        }
    }

    #[test]
    fn origin_guard_checks_scheme_host_and_port_not_path() {
        let local = ConnectionTarget::Local;
        assert!(local.serves_origin(&Url::parse("http://127.0.0.1:18790/chat").unwrap()));
        for raw in [
            "http://evil.example",
            "http://127.0.0.1:9999",
            "https://127.0.0.1:18790",
            "tauri://localhost",
        ] {
            assert!(!local.serves_origin(&Url::parse(raw).unwrap()));
        }
        let remote = ConnectionTarget::parse("https://box.lan:8443").unwrap();
        assert!(remote.serves_origin(&Url::parse("https://box.lan:8443/chat").unwrap()));
        for raw in [
            "http://box.lan:8443",
            "https://box.lan:443",
            "https://other.lan:8443",
        ] {
            assert!(!remote.serves_origin(&Url::parse(raw).unwrap()));
        }
    }

    #[test]
    fn authority_boundaries_do_not_let_userinfo_or_path_change_the_port() {
        for (input, expected) in [
            ("https://user:pass@box.lan:8443/", "https://box.lan:8443"),
            ("user:pass@box.lan:9000", "http://box.lan:9000"),
            ("https://user:pass@[::1]:8443/", "https://[::1]:8443"),
            ("box.lan/path:9000", "http://box.lan:18790"),
            ("box.lan?redirect=user@other:9000", "http://box.lan:18790"),
            ("box.lan:/path", "http://box.lan:18790"),
        ] {
            assert_eq!(
                ConnectionTarget::parse(input).unwrap().to_persisted(),
                expected
            );
        }
    }

    #[test]
    fn debug_never_prints_userinfo_query_or_fragment() {
        let target = ConnectionTarget::Remote(Url::parse(
            "https://sensitive-user:sensitive-pass@box.lan/secret-path?bt=sensitive-ticket#sensitive-fragment").unwrap());
        let debug = format!("{target:?}");
        assert!(!debug.contains("sensitive"));
        assert!(!debug.contains("secret-path"));
        assert!(debug.contains("https://box.lan:443"));
    }
}
