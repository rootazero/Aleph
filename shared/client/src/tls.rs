//! How a `wss://` connection decides whom to trust.
//!
//! A gateway with `[gateway.tls] enabled = true` and no operator-supplied
//! `cert_path` serves a **self-signed** certificate. No system root vouches for
//! it, so before this module the `aleph` CLI could not reach a TLS-enabled
//! server *at all* — not even its own, on loopback, on the same machine that
//! generated the certificate. The documented workaround for a LAN deployment
//! (`allow_insecure_remote`) does not help either: the Panel refuses plaintext
//! for a remote origin regardless.
//!
//! The answer here is a **pin**, not a bypass. There is deliberately no
//! `--insecure`: the failure being fixed is "I cannot talk to my own server",
//! and a switch that turns verification off would turn it off for the remote
//! case too — the one the whole TLS tier exists for.
//!
//! Two sources, in order:
//!
//! 1. `--ca-cert <path>` / `ca_cert` in the CLI config. Explicit, works for a
//!    remote server, and an unreadable path is an error rather than a silent
//!    fall-through to "no pin" — a pin that quietly evaporates is worse than
//!    no pin, because the operator believes one is in place.
//! 2. For a **loopback** URL only, the server's own
//!    `<aleph_home>/data/tls/cert.pem`. Same machine, same filesystem, same
//!    trust domain: if an attacker can write that file they already own the
//!    box. Absent file ⇒ no pin, and the connection then fails the way it did
//!    before, with the system roots' own error.
//!
//! The `<aleph_home>` derivation is [`aleph_protocol::paths`], shared with
//! `alephcore` rather than restated — a second copy of the `ALEPH_HOME` rule
//! agrees byte for byte on any machine where the variable is unset, i.e. on
//! every machine anyone would test it on.

use tokio_tungstenite::Connector;

use crate::error::{CliError, CliResult};

/// Build the TLS connector for `url`, or `None` for a plaintext `ws://`.
///
/// `None` means "let tokio-tungstenite do its default thing", which for a
/// `ws://` URL is no TLS at all and for a `wss://` URL is the system roots.
///
/// Takes the certificate path rather than a `CliConfig` because this crate has
/// **two** socket-opening clients — the persistent [`crate::AlephClient`] and
/// the one-shot [`crate::GatewayClient`] behind `aleph-server gateway call` —
/// and only one of them has a `CliConfig` to hand. Both hit the same wall
/// against a TLS-enabled gateway, so both go through here.
pub fn connector_for(url: &str, ca_cert: Option<&str>) -> CliResult<Option<Connector>> {
    if !is_wss(url) {
        return Ok(None);
    }

    let pem = match ca_cert {
        Some(path) => Some(std::fs::read(path).map_err(|e| {
            CliError::Config(format!(
                "ca_cert: cannot read '{path}': {e} — remove the setting to fall back to \
                 the system roots, or point it at the server's data/tls/cert.pem"
            ))
        })?),
        None if host_is_loopback(url) => local_self_signed_cert(),
        None => None,
    };

    let Some(pem) = pem else {
        return Ok(None);
    };

    let cert = native_tls::Certificate::from_pem(&pem)
        .map_err(|e| CliError::Config(format!("ca_cert: not a PEM certificate: {e}")))?;
    let connector = native_tls::TlsConnector::builder()
        .add_root_certificate(cert)
        .build()
        .map_err(|e| CliError::Connection(format!("TLS connector: {e}")))?;
    Ok(Some(Connector::NativeTls(connector)))
}

/// Read the gateway's own self-signed certificate, if this machine has one.
///
/// Returns `None` for every failure — no home directory, no file, unreadable.
/// This is a best-effort convenience for the same-machine case, and it must not
/// turn "you have no local server" into a hard error on a CLI that is about to
/// produce a perfectly good connection error of its own.
fn local_self_signed_cert() -> Option<Vec<u8>> {
    let path = aleph_protocol::paths::self_signed_cert_path()?;
    std::fs::read(path).ok()
}

fn is_wss(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("wss://") || lower.starts_with("https://")
}

/// Whether the URL's host is loopback.
///
/// Deliberately a string check rather than a DNS resolve: this decides whether
/// to *offer* a locally-stored certificate, and a name that resolves to
/// 127.0.0.1 today is not the same trust statement as a literal `localhost`.
/// Unknown or unparseable ⇒ not loopback, which is the direction that offers
/// nothing.
fn host_is_loopback(url: &str) -> bool {
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    // strip userinfo, then take up to the first `/`, `?` or `#`
    let authority = rest.rsplit_once('@').map_or(rest, |(_, a)| a);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        // IPv6 literal: `[::1]:18790`
        stripped.split(']').next().unwrap_or(stripped)
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    let host = host.trim().to_ascii_lowercase();
    host == "localhost"
        || host == "::1"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_urls_never_get_a_connector() {
        // `Connector` has no `Debug`, so these are matched rather than
        // `unwrap()`ed — the shape of the assertion, not its meaning.
        assert!(matches!(
            connector_for("ws://127.0.0.1:18790/ws", None),
            Ok(None)
        ));
        assert!(
            matches!(
                connector_for("ws://127.0.0.1:18790/ws", Some("/nope.pem")),
                Ok(None)
            ),
            "a pin on a plaintext URL is not an error — it is simply unused"
        );
    }

    /// The pin must fail loudly. A `ca_cert` pointing at nothing is exactly the
    /// kind of setting that reads as "configured" while doing nothing at all.
    #[test]
    fn an_unreadable_ca_cert_is_an_error_not_a_silent_fallback() {
        let Err(err) = connector_for(
            "wss://example.invalid/ws",
            Some("/definitely/not/here/cert.pem"),
        ) else {
            panic!("a ca_cert that cannot be read must not resolve to 'no pin'");
        };
        assert!(
            err.to_string().contains("ca_cert"),
            "the error must name the setting: {err}"
        );
    }

    #[test]
    fn loopback_is_recognised_in_every_spelling_the_gateway_binds() {
        for url in [
            "wss://127.0.0.1:18790/ws",
            "wss://localhost:18790/ws",
            "wss://[::1]:18790/ws",
            "wss://127.0.0.5/ws",
            "wss://user:pw@localhost:18790/ws",
        ] {
            assert!(host_is_loopback(url), "{url} is loopback");
        }
        for url in [
            "wss://10.10.10.6:18790/ws",
            "wss://aleph.example.com/ws",
            // `localhost.evil.com` must NOT read as loopback — the check is a
            // suffix on `.localhost`, not a substring anywhere.
            "wss://localhost.evil.com/ws",
        ] {
            assert!(!host_is_loopback(url), "{url} is not loopback");
        }
    }
}
