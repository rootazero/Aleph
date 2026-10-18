//! Opt-in device credential for the `connect` handshake.
//!
//! A [`DeviceCredential`] is how a client asks the Gateway to identify it as a
//! *paired device* (and, through the device's user binding, as that device's
//! user). It is deliberately NOT part of [`crate::CliConfig`]: that struct is
//! serialised to `config.toml`, and a credential must never reach a plaintext
//! file. The credential lives in memory only, is never `Debug`-printed, and
//! there is intentionally no accessor that hands the secret back out — the only
//! consumer is the handshake serialiser inside this crate.
//!
//! Persistence is delegated to [`crate::credential_store`]: a keyring-backed
//! (Windows Credential Manager / macOS Keychain / Linux Secret Service) store
//! that fails closed when no OS keyring is usable — there is no plaintext-file
//! fallback. Only a verified long-lived device token may be persisted; a
//! bootstrap ticket never is. Until a caller opts into that store, the
//! credential is held in memory only (background:
//! `.superpowers/sdd/2026-10-07-panel-browser-workspace/task-27-auth-blocker-report.md`).

use std::fmt;
use std::net::IpAddr;

use serde_json::{Map, Value};

use crate::error::{CliError, CliResult};

/// Which kind of secret a [`DeviceCredential`] carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    DeviceToken,
    BootstrapTicket,
}

/// A paired-device credential: a device id plus EITHER a long-lived
/// `device_token` OR a one-shot `bootstrap_ticket`.
#[derive(Clone)]
pub struct DeviceCredential {
    device_id: String,
    kind: Kind,
    secret: String,
}

impl DeviceCredential {
    /// A long-lived device token previously issued for `device_id`.
    pub fn device_token(device_id: impl Into<String>, token: impl Into<String>) -> CliResult<Self> {
        Self::new(device_id.into(), Kind::DeviceToken, token.into())
    }

    /// A one-shot bootstrap ticket to be exchanged for a device token under
    /// `device_id`. The ticket is consumed by the Gateway on first use.
    pub fn bootstrap_ticket(
        device_id: impl Into<String>,
        ticket: impl Into<String>,
    ) -> CliResult<Self> {
        Self::new(device_id.into(), Kind::BootstrapTicket, ticket.into())
    }

    fn new(device_id: String, kind: Kind, secret: String) -> CliResult<Self> {
        if device_id.trim().is_empty() {
            return Err(CliError::Config(
                "device credential needs a device_id".into(),
            ));
        }
        if secret.trim().is_empty() {
            return Err(CliError::Config("device credential secret is empty".into()));
        }
        Ok(Self {
            device_id,
            kind,
            secret,
        })
    }

    /// The (non-secret) device id this credential was minted for.
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Whether this is a one-shot bootstrap ticket (vs. a device token).
    #[must_use]
    pub fn is_bootstrap_ticket(&self) -> bool {
        self.kind == Kind::BootstrapTicket
    }

    /// The `(device_id, device_token)` pair, only for a long-lived device
    /// token. `None` for a bootstrap ticket: a one-shot ticket must never be
    /// handed to a persistence layer. Crate-private so the secret has exactly
    /// two consumers: the handshake serialiser and the credential store.
    pub(crate) fn persistable_parts(&self) -> Option<(&str, &str)> {
        match self.kind {
            Kind::DeviceToken => Some((&self.device_id, &self.secret)),
            Kind::BootstrapTicket => None,
        }
    }

    /// Insert the credential fields into a `connect` params object.
    pub(crate) fn write_connect_params(&self, params: &mut Map<String, Value>) {
        params.insert("device_id".into(), Value::String(self.device_id.clone()));
        let key = match self.kind {
            Kind::DeviceToken => "device_token",
            Kind::BootstrapTicket => "bootstrap_ticket",
        };
        params.insert(key.into(), Value::String(self.secret.clone()));
    }
}

impl fmt::Debug for DeviceCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCredential")
            .field("device_id", &self.device_id)
            .field(
                "kind",
                &match self.kind {
                    Kind::DeviceToken => "device_token",
                    Kind::BootstrapTicket => "bootstrap_ticket",
                },
            )
            .field("secret", &"[redacted]")
            .finish()
    }
}

/// The identity the Gateway itself reported for a credential-authenticated
/// connection. Both fields come from the server's connect response; nothing
/// here is the client's own claim.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VerifiedIdentity {
    /// The paired device the Gateway resolved the credential to.
    pub device_id: String,
    /// The user that device is bound to, when the Gateway reported one.
    pub user_id: Option<String>,
}

/// Refuse to put a credential on a transport that exposes it: only `wss://`,
/// or `ws://` to a loopback host, may carry one.
pub(crate) fn ensure_credential_transport(url: &str) -> CliResult<()> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| CliError::Config("gateway url has no scheme".into()))?;
    match scheme.to_ascii_lowercase().as_str() {
        "wss" => Ok(()),
        "ws" if host_is_loopback(rest) => Ok(()),
        _ => Err(CliError::Config(
            "refusing to send a device credential over an unencrypted non-loopback transport"
                .into(),
        )),
    }
}

fn host_is_loopback(after_scheme: &str) -> bool {
    // Fail closed on malformed authorities: a backslash (treated as `/` by
    // WHATWG-style parsers) or an ASCII control char makes the hand-rolled
    // split below disagree with the URL the transport will actually dial.
    if after_scheme
        .chars()
        .any(|c| c == '\\' || c.is_ascii_control())
    {
        return false;
    }
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // Drop userinfo (`user@host`): the real host is what follows the last `@`.
    let hostport = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default()
    } else {
        hostport.split(':').next().unwrap_or_default()
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_secret() {
        let c = DeviceCredential::device_token("dev-1", "aleph-dt-SECRET").unwrap();
        let shown = format!("{c:?} {c:#?}");
        assert!(!shown.contains("aleph-dt-SECRET"), "{shown}");
        assert!(shown.contains("dev-1"));
        let t = DeviceCredential::bootstrap_ticket("dev-1", "aleph-bt-SECRET").unwrap();
        assert!(!format!("{t:?}").contains("aleph-bt-SECRET"));
    }

    #[test]
    fn empty_parts_are_rejected() {
        assert!(DeviceCredential::device_token("", "t").is_err());
        assert!(DeviceCredential::device_token("d", "  ").is_err());
        assert!(DeviceCredential::bootstrap_ticket("d", "").is_err());
    }

    #[test]
    fn connect_params_carry_exactly_the_matching_secret_field() {
        let mut p = Map::new();
        DeviceCredential::device_token("dev-1", "aleph-dt-x")
            .unwrap()
            .write_connect_params(&mut p);
        assert_eq!(p["device_id"], "dev-1");
        assert_eq!(p["device_token"], "aleph-dt-x");
        assert!(!p.contains_key("bootstrap_ticket"));

        let mut p = Map::new();
        DeviceCredential::bootstrap_ticket("dev-1", "aleph-bt-x")
            .unwrap()
            .write_connect_params(&mut p);
        assert_eq!(p["bootstrap_ticket"], "aleph-bt-x");
        assert!(!p.contains_key("device_token"));
    }

    #[test]
    fn credential_transport_policy() {
        for ok in [
            "wss://gw.example.com/ws",
            "WSS://gw.example.com:8443",
            "ws://127.0.0.1:18789",
            "ws://127.0.0.1:18789/",
            "ws://localhost:18789",
            "ws://[::1]:18789/ws",
        ] {
            assert!(ensure_credential_transport(ok).is_ok(), "{ok}");
        }
        for bad in [
            "ws://gw.example.com/ws",
            "ws://192.168.1.5:18789",
            "ws://0.0.0.0:18789",
            "ws://localhost.evil.example:1",
            "ws://127.0.0.1@evil.example/",
            "http://127.0.0.1:18789",
            "127.0.0.1:18789",
            "ws://evil.example\\@127.0.0.1/",
            "ws://127.0.0.1\\@evil.example/",
            "ws://127.0.0.1\t@evil.example/",
            "ws://127.0.0.1\n.evil.example/",
        ] {
            assert!(ensure_credential_transport(bad).is_err(), "{bad}");
        }
    }
}
