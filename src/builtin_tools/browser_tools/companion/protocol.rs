//! Launch / ready / challenge protocol that proves a managed companion process
//! OWNS the endpoint it claims. Pure parsing + MAC derivation (no process, no
//! sandbox, no browser) so every rejection path is unit-testable.
//!
//! ```text
//! parent -> child stdin : ALEPH-COMPANION-LAUNCH {"v":1,"launch_id":..,"secret":..}\n  (then stdin closes)
//! child  -> parent stdout: ALEPH-COMPANION-READY  {"v":1,"endpoint":..,"ready_mac":..}\n   (<= 4 KiB)
//! parent -> endpoint     : POST /.aleph-companion/challenge {"launch_id":..,"challenge":..}
//! endpoint -> parent     : {"response": hmac(secret, "...challenge.v1\n" id endpoint challenge)}
//! ```
//!
//! The launch secret never appears in argv, env, config, logs or `Debug`
//! output; it is the only thing that can produce a valid MAC, so a process that
//! merely listens on the port (PID guessing, port squatting, a 200 from
//! anything) cannot pass. There is no user-supplied provenance in here: an
//! [`EndpointProof`] can only be minted by [`verify_challenge_response`].

use std::net::IpAddr;

use hmac::{Hmac, Mac};
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use url::{Host, Url};
use zeroize::Zeroizing;

use crate::security::secret_equal_bytes;

pub(crate) const PROTOCOL_VERSION: u8 = 1;
pub(crate) const LAUNCH_PREFIX: &str = "ALEPH-COMPANION-LAUNCH ";
pub(crate) const READY_PREFIX: &str = "ALEPH-COMPANION-READY ";
/// Hard cap on the ready line (without its newline) and on the challenge body.
pub(crate) const MAX_READY_BYTES: usize = 4096;
pub(crate) const MAX_CHALLENGE_BODY_BYTES: usize = 4096;
/// Fixed challenge route; never taken from the child.
pub(crate) const CHALLENGE_PATH: &str = "/.aleph-companion/challenge";
const MAX_ENDPOINT_BYTES: usize = 512;

const READY_DOMAIN: &str = "aleph.companion.ready.v1";
const CHALLENGE_DOMAIN: &str = "aleph.companion.challenge.v1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    #[error("endpoint is not a canonical loopback http(s) origin: {0}")]
    EndpointInvalid(&'static str),
    #[error("ready report exceeds {MAX_READY_BYTES} bytes")]
    ReadyTooLarge,
    #[error("ready report is malformed: {0}")]
    ReadyMalformed(&'static str),
    #[error("ready endpoint differs from the configured endpoint")]
    EndpointMismatch,
    #[error("ready MAC is not bound to this launch")]
    BadReadyMac,
    #[error("challenge response exceeds {MAX_CHALLENGE_BODY_BYTES} bytes")]
    ChallengeTooLarge,
    #[error("challenge response is malformed")]
    ChallengeMalformed,
    #[error("challenge response is not bound to this launch and challenge")]
    ChallengeMismatch,
    #[error("endpoint challenge failed: {0}")]
    Transport(String),
    #[error("the companion process is no longer alive")]
    ChildNotAlive,
}

fn os_rng() -> UnwrapErr<SysRng> {
    UnwrapErr(SysRng)
}

fn random_hex<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    os_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// ---------------------------------------------------------------- endpoint

/// A canonical loopback `http(s)://host:port/path` origin. Construction is the
/// only validation point: scheme http/https, no userinfo, loopback IP literal
/// or `localhost`, explicit port, no query/fragment, a normalized path, and the
/// raw text must equal the canonical re-rendering (so aliases such as
/// `HTTP://LocalHost:1/./x` or `%2e%2e` never compare equal to anything).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalEndpoint {
    scheme: &'static str,
    host: String,
    port: u16,
    path: String,
}

impl CanonicalEndpoint {
    pub(crate) fn parse(raw: &str) -> Result<Self, ProtocolError> {
        use ProtocolError::EndpointInvalid as Bad;
        if raw.is_empty() || raw.len() > MAX_ENDPOINT_BYTES {
            return Err(Bad("length"));
        }
        if raw
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii())
        {
            return Err(Bad("characters"));
        }
        let url = Url::parse(raw).map_err(|_| Bad("unparseable"))?;
        let scheme = match url.scheme() {
            "http" => "http",
            "https" => "https",
            _ => return Err(Bad("scheme")),
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Bad("userinfo"));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(Bad("query or fragment"));
        }
        let port = url
            .port()
            .ok_or(Bad("explicit non-default port required"))?;
        let host = match url.host().ok_or(Bad("host"))? {
            Host::Domain("localhost") => "localhost".to_owned(),
            Host::Domain(_) => return Err(Bad("host must be loopback")),
            Host::Ipv4(ip) if IpAddr::V4(ip).is_loopback() => ip.to_string(),
            Host::Ipv6(ip) if IpAddr::V6(ip).is_loopback() => format!("[{ip}]"),
            _ => return Err(Bad("host must be loopback")),
        };
        let path = url.path().to_owned();
        if path.contains("//")
            || path.split('/').any(|s| s == "." || s == "..")
            || path.contains('%')
            || path.contains('\\')
        {
            return Err(Bad("path not canonical"));
        }
        let endpoint = Self {
            scheme,
            host,
            port,
            path,
        };
        if endpoint.as_string() != raw {
            return Err(Bad("not in canonical spelling"));
        }
        Ok(endpoint)
    }

    pub(crate) fn as_string(&self) -> String {
        format!("{}://{}:{}{}", self.scheme, self.host, self.port, self.path)
    }

    /// `scheme://host:port` + the FIXED challenge route.
    pub(crate) fn challenge_url(&self) -> String {
        format!(
            "{}://{}:{}{CHALLENGE_PATH}",
            self.scheme, self.host, self.port
        )
    }

    pub(crate) fn host_port(&self) -> (&str, u16) {
        (&self.host, self.port)
    }
}

// ------------------------------------------------------------- credentials

/// Per-launch secret + public launch id. Held by the Gateway for the life of
/// one launch attempt; zeroized on drop; `Debug` never prints the secret.
pub(crate) struct LaunchCredentials {
    launch_id: String,
    secret: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for LaunchCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchCredentials")
            .field("launch_id", &self.launch_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize)]
struct LaunchLine<'a> {
    v: u8,
    launch_id: &'a str,
    secret: &'a str,
}

impl LaunchCredentials {
    pub(crate) fn generate() -> Self {
        let mut secret = Zeroizing::new([0u8; 32]);
        os_rng().fill_bytes(&mut secret[..]);
        Self {
            launch_id: random_hex::<16>(),
            secret,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_parts(launch_id: &str, secret: [u8; 32]) -> Self {
        Self {
            launch_id: launch_id.to_owned(),
            secret: Zeroizing::new(secret),
        }
    }

    pub(crate) fn launch_id(&self) -> &str {
        &self.launch_id
    }

    /// The single stdin line that delivers the secret. Zeroized after the write.
    pub(crate) fn launch_line(&self) -> Zeroizing<Vec<u8>> {
        let secret_hex = Zeroizing::new(hex::encode(&self.secret[..]));
        let body = serde_json::to_string(&LaunchLine {
            v: PROTOCOL_VERSION,
            launch_id: &self.launch_id,
            secret: &secret_hex,
        })
        .unwrap_or_default();
        let mut line = Vec::with_capacity(LAUNCH_PREFIX.len() + body.len() + 1);
        line.extend_from_slice(LAUNCH_PREFIX.as_bytes());
        line.extend_from_slice(body.as_bytes());
        line.push(b'\n');
        Zeroizing::new(line)
    }

    fn mac_hex(&self, parts: &[&str]) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.secret[..])
            .expect("HMAC accepts any key length");
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                mac.update(b"\n");
            }
            mac.update(part.as_bytes());
        }
        hex::encode(mac.finalize().into_bytes())
    }

    pub(crate) fn ready_mac(&self, endpoint: &CanonicalEndpoint) -> String {
        self.mac_hex(&[READY_DOMAIN, &self.launch_id, &endpoint.as_string()])
    }

    pub(crate) fn challenge_response(
        &self,
        endpoint: &CanonicalEndpoint,
        challenge: &str,
    ) -> String {
        self.mac_hex(&[
            CHALLENGE_DOMAIN,
            &self.launch_id,
            &endpoint.as_string(),
            challenge,
        ])
    }
}

// ------------------------------------------------------------------- ready

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyReport {
    v: u8,
    endpoint: String,
    ready_mac: String,
}

/// Parse and verify the child's ready line (WITHOUT its trailing newline).
/// Returns the endpoint the child claims, only after it equals `expected` and
/// carries a MAC bound to this launch's secret.
pub(crate) fn verify_ready_line(
    line: &[u8],
    expected: &CanonicalEndpoint,
    creds: &LaunchCredentials,
) -> Result<CanonicalEndpoint, ProtocolError> {
    if line.len() > MAX_READY_BYTES {
        return Err(ProtocolError::ReadyTooLarge);
    }
    let text = std::str::from_utf8(line).map_err(|_| ProtocolError::ReadyMalformed("utf8"))?;
    let text = text.strip_suffix('\r').unwrap_or(text);
    let json = text
        .strip_prefix(READY_PREFIX)
        .ok_or(ProtocolError::ReadyMalformed("prefix"))?;
    let report: ReadyReport =
        serde_json::from_str(json).map_err(|_| ProtocolError::ReadyMalformed("json"))?;
    if report.v != PROTOCOL_VERSION {
        return Err(ProtocolError::ReadyMalformed("version"));
    }
    let claimed = CanonicalEndpoint::parse(&report.endpoint)?;
    if &claimed != expected {
        return Err(ProtocolError::EndpointMismatch);
    }
    let want = creds.ready_mac(&claimed);
    if !secret_equal_bytes(report.ready_mac.as_bytes(), want.as_bytes()) {
        return Err(ProtocolError::BadReadyMac);
    }
    Ok(claimed)
}

// --------------------------------------------------------------- challenge

#[derive(Serialize)]
pub(crate) struct ChallengeRequest<'a> {
    pub(crate) launch_id: &'a str,
    pub(crate) challenge: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeResponse {
    response: String,
}

/// Fresh 32-byte random challenge (hex).
pub(crate) fn new_challenge() -> String {
    random_hex::<32>()
}

/// Gateway-verified proof that the process behind `endpoint` holds this
/// launch's secret. Private fields, no `Deserialize`, only constructible by
/// [`verify_challenge_response`]: nothing user-supplied can stand in for it.
#[derive(Clone, Debug)]
pub(crate) struct EndpointProof {
    launch_id: String,
    endpoint: CanonicalEndpoint,
}

impl EndpointProof {
    pub(crate) fn endpoint(&self) -> &CanonicalEndpoint {
        &self.endpoint
    }

    pub(crate) fn launch_id(&self) -> &str {
        &self.launch_id
    }
}

/// Verify the endpoint's answer to `challenge` and mint the proof.
pub(crate) fn verify_challenge_response(
    body: &[u8],
    endpoint: &CanonicalEndpoint,
    challenge: &str,
    creds: &LaunchCredentials,
) -> Result<EndpointProof, ProtocolError> {
    if body.len() > MAX_CHALLENGE_BODY_BYTES {
        return Err(ProtocolError::ChallengeTooLarge);
    }
    let parsed: ChallengeResponse =
        serde_json::from_slice(body).map_err(|_| ProtocolError::ChallengeMalformed)?;
    let want = creds.challenge_response(endpoint, challenge);
    if !secret_equal_bytes(parsed.response.as_bytes(), want.as_bytes()) {
        return Err(ProtocolError::ChallengeMismatch);
    }
    Ok(EndpointProof {
        launch_id: creds.launch_id.clone(),
        endpoint: endpoint.clone(),
    })
}

/// Test-only mint (the production path is `verify_challenge_response`).
#[cfg(test)]
pub(crate) fn proof_for_test(
    creds: &LaunchCredentials,
    endpoint: &CanonicalEndpoint,
) -> EndpointProof {
    EndpointProof {
        launch_id: creds.launch_id.clone(),
        endpoint: endpoint.clone(),
    }
}

/// Seam for the live endpoint exchange so lifecycle tests can run offline.
#[async_trait::async_trait]
pub(crate) trait EndpointProver: Send + Sync {
    async fn prove(
        &self,
        creds: &LaunchCredentials,
        endpoint: &CanonicalEndpoint,
        process_alive: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<EndpointProof, ProtocolError>;
}

/// Real prover: one bounded POST to the fixed challenge route of the
/// explicitly configured endpoint. No redirects, no proxy, exact 200, body
/// <= 4 KiB, and the child must be alive before AND after the exchange.
pub(crate) struct HttpEndpointProver {
    pub(crate) budget: std::time::Duration,
}

#[async_trait::async_trait]
impl EndpointProver for HttpEndpointProver {
    async fn prove(
        &self,
        creds: &LaunchCredentials,
        endpoint: &CanonicalEndpoint,
        process_alive: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<EndpointProof, ProtocolError> {
        if !process_alive() {
            return Err(ProtocolError::ChildNotAlive);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(self.budget)
            .connect_timeout(self.budget)
            .build()
            .map_err(|e| ProtocolError::Transport(e.to_string()))?;
        let challenge = new_challenge();
        let request = ChallengeRequest {
            launch_id: creds.launch_id(),
            challenge: &challenge,
        };
        let mut response = client
            .post(endpoint.challenge_url())
            .json(&request)
            .send()
            .await
            .map_err(|e| ProtocolError::Transport(e.without_url().to_string()))?;
        if response.status() != reqwest::StatusCode::OK {
            // Includes every 3xx: redirects are never followed.
            return Err(ProtocolError::Transport(format!(
                "status {}",
                response.status().as_u16()
            )));
        }
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| ProtocolError::Transport(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > MAX_CHALLENGE_BODY_BYTES {
                return Err(ProtocolError::ChallengeTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let proof = verify_challenge_response(&body, endpoint, &challenge, creds)?;
        if !process_alive() {
            return Err(ProtocolError::ChildNotAlive);
        }
        Ok(proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> LaunchCredentials {
        LaunchCredentials::from_parts("0123456789abcdef0123456789abcdef", [7u8; 32])
    }

    fn endpoint() -> CanonicalEndpoint {
        CanonicalEndpoint::parse("http://127.0.0.1:5173/app").expect("canonical")
    }

    fn ready_line(creds: &LaunchCredentials, endpoint: &CanonicalEndpoint) -> Vec<u8> {
        format!(
            "{READY_PREFIX}{{\"v\":1,\"endpoint\":\"{}\",\"ready_mac\":\"{}\"}}",
            endpoint.as_string(),
            creds.ready_mac(endpoint)
        )
        .into_bytes()
    }

    #[test]
    fn canonical_endpoint_accepts_only_the_canonical_loopback_spelling() {
        for ok in [
            "http://127.0.0.1:5173/",
            "http://127.0.0.1:5173/app",
            "https://localhost:8443/a/b",
            "http://[::1]:3000/",
        ] {
            assert_eq!(CanonicalEndpoint::parse(ok).expect(ok).as_string(), ok);
        }
        for bad in [
            "",
            "ftp://127.0.0.1:1/",
            "http://127.0.0.1/",           // no explicit port
            "http://127.0.0.1:80/",        // default port is dropped by the parser
            "http://user:pw@127.0.0.1:1/", // userinfo
            "http://127.0.0.1:1/?q=1",     // query
            "http://127.0.0.1:1/#f",       // fragment
            "http://example.com:1/",       // not loopback
            "http://10.0.0.1:1/",          // private, not loopback
            "http://0.0.0.0:1/",           // unspecified
            "HTTP://LOCALHOST:1/",         // non-canonical spelling
            "http://127.0.0.1:1/a/../b",   // dot segments
            "http://127.0.0.1:1//x",       // empty segment
            "http://127.0.0.1:1/%2e%2e/x", // encoded traversal
            "http://127.0.0.1:1",          // missing path
            "http://127.0.0.1:1/ x",       // whitespace
        ] {
            assert!(
                CanonicalEndpoint::parse(bad).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn launch_line_carries_the_secret_only_on_the_stdin_line_and_debug_redacts_it() {
        let creds = creds();
        let line = creds.launch_line();
        let text = std::str::from_utf8(&line).expect("utf8");
        assert!(text.starts_with(LAUNCH_PREFIX) && text.ends_with('\n'));
        assert!(text.contains(&hex::encode([7u8; 32])));
        assert!(text.contains(creds.launch_id()));
        let debug = format!("{creds:?}");
        assert!(
            !debug.contains(&hex::encode([7u8; 32])),
            "debug leaked: {debug}"
        );
        assert!(debug.contains("redacted"));
    }

    #[test]
    fn ready_line_round_trips_and_binds_to_the_launch_secret() {
        let (creds, endpoint) = (creds(), endpoint());
        let claimed = verify_ready_line(&ready_line(&creds, &endpoint), &endpoint, &creds)
            .expect("valid ready");
        assert_eq!(claimed, endpoint);
        // CRLF tolerated; the newline itself is never part of the line.
        let mut crlf = ready_line(&creds, &endpoint);
        crlf.push(b'\r');
        assert!(verify_ready_line(&crlf, &endpoint, &creds).is_ok());
    }

    #[test]
    fn ready_line_rejects_malformed_oversize_wrong_endpoint_and_wrong_nonce() {
        let (creds, endpoint) = (creds(), endpoint());
        let good = String::from_utf8(ready_line(&creds, &endpoint)).expect("utf8");

        // oversize
        let mut big = good.clone();
        big.push_str(&" ".repeat(MAX_READY_BYTES));
        assert_eq!(
            verify_ready_line(big.as_bytes(), &endpoint, &creds),
            Err(ProtocolError::ReadyTooLarge)
        );
        // malformed: bad utf8, bad prefix, bad json, unknown field, wrong version
        assert_eq!(
            verify_ready_line(&[0xff, 0xfe], &endpoint, &creds),
            Err(ProtocolError::ReadyMalformed("utf8"))
        );
        assert_eq!(
            verify_ready_line(b"HELLO {}", &endpoint, &creds),
            Err(ProtocolError::ReadyMalformed("prefix"))
        );
        assert_eq!(
            verify_ready_line(
                format!("{READY_PREFIX}not json").as_bytes(),
                &endpoint,
                &creds
            ),
            Err(ProtocolError::ReadyMalformed("json"))
        );
        let extra = good.replacen("\"v\":1", "\"v\":1,\"pid\":4242", 1);
        assert_eq!(
            verify_ready_line(extra.as_bytes(), &endpoint, &creds),
            Err(ProtocolError::ReadyMalformed("json")),
            "unknown fields (a self-reported pid) are denied, not ignored"
        );
        let v2 = good.replacen("\"v\":1", "\"v\":2", 1);
        assert_eq!(
            verify_ready_line(v2.as_bytes(), &endpoint, &creds),
            Err(ProtocolError::ReadyMalformed("version"))
        );
        // wrong endpoint: validly MACed for ANOTHER canonical endpoint
        let other = CanonicalEndpoint::parse("http://127.0.0.1:9999/app").expect("canonical");
        assert_eq!(
            verify_ready_line(&ready_line(&creds, &other), &endpoint, &creds),
            Err(ProtocolError::EndpointMismatch)
        );
        // wrong nonce: MAC made with a different launch secret / launch id
        let foreign = LaunchCredentials::from_parts("0123456789abcdef0123456789abcdef", [9u8; 32]);
        assert_eq!(
            verify_ready_line(&ready_line(&foreign, &endpoint), &endpoint, &creds),
            Err(ProtocolError::BadReadyMac)
        );
        let other_launch =
            LaunchCredentials::from_parts("ffffffffffffffffffffffffffffffff", [7u8; 32]);
        assert_eq!(
            verify_ready_line(&ready_line(&other_launch, &endpoint), &endpoint, &creds),
            Err(ProtocolError::BadReadyMac),
            "the MAC binds the launch id as well as the secret"
        );
    }

    #[test]
    fn challenge_response_must_be_bound_to_secret_endpoint_and_this_challenge() {
        let (creds, endpoint) = (creds(), endpoint());
        let challenge = new_challenge();
        assert_eq!(challenge.len(), 64);
        assert_ne!(challenge, new_challenge(), "challenges are fresh");
        let body = |resp: &str| format!("{{\"response\":\"{resp}\"}}").into_bytes();

        let ok = body(&creds.challenge_response(&endpoint, &challenge));
        let proof = verify_challenge_response(&ok, &endpoint, &challenge, &creds).expect("proof");
        assert_eq!(proof.endpoint(), &endpoint);
        assert_eq!(proof.launch_id(), creds.launch_id());

        // replay of a response to a DIFFERENT challenge
        let replay = body(&creds.challenge_response(&endpoint, "someotherchallenge"));
        assert_eq!(
            verify_challenge_response(&replay, &endpoint, &challenge, &creds).unwrap_err(),
            ProtocolError::ChallengeMismatch
        );
        // a bare HTTP-200 style answer / wrong secret
        let foreign = LaunchCredentials::from_parts(creds.launch_id(), [1u8; 32]);
        for bad in [
            body("ok"),
            body(&foreign.challenge_response(&endpoint, &challenge)),
        ] {
            assert_eq!(
                verify_challenge_response(&bad, &endpoint, &challenge, &creds).unwrap_err(),
                ProtocolError::ChallengeMismatch
            );
        }
        // malformed / unknown field / oversize
        for (bad, want) in [
            (b"not json".to_vec(), ProtocolError::ChallengeMalformed),
            (b"{}".to_vec(), ProtocolError::ChallengeMalformed),
            (
                b"{\"response\":\"x\",\"extra\":1}".to_vec(),
                ProtocolError::ChallengeMalformed,
            ),
            (
                vec![b' '; MAX_CHALLENGE_BODY_BYTES + 1],
                ProtocolError::ChallengeTooLarge,
            ),
        ] {
            assert_eq!(
                verify_challenge_response(&bad, &endpoint, &challenge, &creds).unwrap_err(),
                want
            );
        }
        // endpoint binding: same secret, same challenge, other endpoint
        let other = CanonicalEndpoint::parse("http://127.0.0.1:9999/app").expect("canonical");
        let cross = body(&creds.challenge_response(&other, &challenge));
        assert_eq!(
            verify_challenge_response(&cross, &endpoint, &challenge, &creds).unwrap_err(),
            ProtocolError::ChallengeMismatch
        );
    }

    #[test]
    fn challenge_url_is_the_fixed_route_on_the_configured_origin() {
        assert_eq!(
            endpoint().challenge_url(),
            "http://127.0.0.1:5173/.aleph-companion/challenge"
        );
    }
}
