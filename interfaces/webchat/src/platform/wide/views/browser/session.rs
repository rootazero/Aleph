//! Standalone browser-viewer session layer (pure, no I/O).
//!
//! The issuer hands a person a launch URL of the exact shape
//! `{origin}/viewer/browser#grant_id=<id>&nonce=<secret>`. This module holds
//! everything between "the page was opened" and "a decoded frame is held",
//! as plain values a native test can drive:
//!
//! * [`parse_fragment`] — a strict `location.hash` parser. Only `grant_id` and
//!   `nonce` exist; duplicates, unknown keys, a query, empty or oversize input
//!   and any character outside the base64url alphabet are refused.
//! * [`ViewerEndpoints`] — the consume / exchange / WebSocket URLs derived from
//!   the page origin alone. `https` → `wss`; plain `http` → `ws` only for a
//!   loopback host.
//! * [`ViewerSession`] — the state machine
//!   `Idle → Consuming → Exchanging → Exchanged → Authenticated → Closed`.
//!   The exchange snapshot is the **only** authority for which resource and
//!   binding are shown: nothing here reads a URL, a profile or a current tab.
//!
//! # What this module does NOT do
//!
//! It performs no network call. The caller (a transport that does not exist
//! yet in this build — see the module docs of [`super`]) must:
//!
//! 1. `POST` [`ConsumeFragmentNonceRequest`] to [`ViewerEndpoints::consume_url`]
//!    and `POST` [`ExchangeViewerGrantRequest`] to
//!    [`ViewerEndpoints::exchange_url`] (credentials omitted, no-store,
//!    no-referrer), feeding the JSON replies to the matching `accept_*`;
//! 2. open [`ViewerEndpoints::ws_url`] with `binaryType = "arraybuffer"`, send
//!    the text from [`ViewerSession::take_auth_hello`] on open, arm a
//!    [`AUTH_TIMEOUT_MS`] timer that calls [`ViewerSession::on_auth_timeout`],
//!    then route **text** messages to [`ViewerSession::accept_auth_text`] and
//!    **binary** messages to [`ViewerSession::accept_binary_frame`];
//! 3. on *any* `Err` from those calls, or on socket close, call
//!    [`ViewerSession::close`] / [`ViewerSession::on_transport_closed`] and
//!    drop its handlers. `accept_binary_frame` deliberately leaves the session
//!    untouched on `Err` (so one bad frame cannot corrupt held state); closing
//!    is the transport's decision.
//!
//! Every error here is a fixed, payload-free enum. Frame bytes, secrets and
//! server text never reach `Debug`, `Display` or an error value.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use aleph_protocol::browser_live::{
    AuthHello, ConsumeFragmentNonceRequest, ConsumeFragmentNonceResponse, ExchangeTicket,
    ExchangeViewerGrantRequest, ExchangeViewerGrantResponse, FragmentNonce, RedactionStatus,
    ViewerAuthClientMessage, ViewerAuthServerMessage, ViewerSecret, ViewerWsFrame,
    ViewerWsFrameCodecError, ViewportCssPx, VIEWER_AUTH_FIRST_FRAME_MAX_BYTES,
    VIEWER_PROTOCOL_VERSION,
};

use super::model::BoundBrowser;

/// The exact page path of the standalone viewer. No trailing slash, no prefix
/// match: anything else is the ordinary Panel.
pub const VIEWER_PAGE_PATH: &str = "/viewer/browser";
/// The only WebSocket path the exchange may name.
pub const VIEWER_WS_PATH: &str = "/viewer/browser/ws";
const CONSUME_PATH: &str = "/viewer/browser/consume";
const EXCHANGE_PATH: &str = "/viewer/browser/exchange";

/// The gateway closes a socket that has not authenticated within this window.
pub const AUTH_TIMEOUT_MS: u32 = 2000;

/// Upper bound for the raw `location.hash` (including the leading `#`).
pub const FRAGMENT_MAX_BYTES: usize = 512;
const GRANT_ID_MAX_BYTES: usize = 64;
const NONCE_MAX_BYTES: usize = 128;
const TICKET_MAX_BYTES: usize = 512;
/// Cap for a server auth reply (`AuthOk` is ~110 bytes; this is generous).
const AUTH_REPLY_MAX_BYTES: usize = 512;

/// True only for the exact standalone viewer page path.
#[must_use]
pub fn is_standalone_viewer_path(pathname: &str) -> bool {
    pathname == VIEWER_PAGE_PATH
}

// ---------------------------------------------------------------------------
// Fragment
// ---------------------------------------------------------------------------

/// Why a `location.hash` was refused. Carries no input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentError {
    Empty,
    Oversize,
    Malformed,
    Duplicate,
    Unknown,
    Missing,
}

/// A parsed launch fragment. The nonce is a protocol secret wrapper; `Debug`
/// shows neither field.
#[derive(Clone, PartialEq, Eq)]
pub struct ViewerFragment {
    grant_id: String,
    nonce: FragmentNonce,
}

impl ViewerFragment {
    #[must_use]
    pub fn grant_id(&self) -> &str {
        &self.grant_id
    }
}

impl fmt::Debug for ViewerFragment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ViewerFragment { grant_id: [REDACTED], nonce: [REDACTED] }")
    }
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn token(value: &str, max: usize) -> Result<String, FragmentError> {
    if value.is_empty() || value.len() > max || !value.bytes().all(is_token_byte) {
        return Err(FragmentError::Malformed);
    }
    Ok(value.to_owned())
}

/// Parse `location.hash` (with or without its leading `#`).
///
/// Accepts exactly `grant_id=<token>&nonce=<token>` in either order; tokens
/// are `[A-Za-z0-9_-]` (the raw base64url the issuer emits, never
/// percent-encoded).
pub fn parse_fragment(raw: &str) -> Result<ViewerFragment, FragmentError> {
    if raw.len() > FRAGMENT_MAX_BYTES {
        return Err(FragmentError::Oversize);
    }
    let body = raw.strip_prefix('#').unwrap_or(raw);
    if body.is_empty() {
        return Err(FragmentError::Empty);
    }
    let mut grant_id: Option<String> = None;
    let mut nonce: Option<String> = None;
    for part in body.split('&') {
        let (key, value) = part.split_once('=').ok_or(FragmentError::Malformed)?;
        match key {
            "grant_id" => {
                if grant_id.is_some() {
                    return Err(FragmentError::Duplicate);
                }
                grant_id = Some(token(value, GRANT_ID_MAX_BYTES)?);
            }
            "nonce" => {
                if nonce.is_some() {
                    return Err(FragmentError::Duplicate);
                }
                nonce = Some(token(value, NONCE_MAX_BYTES)?);
            }
            _ => return Err(FragmentError::Unknown),
        }
    }
    match (grant_id, nonce) {
        (Some(grant_id), Some(nonce)) => Ok(ViewerFragment {
            grant_id,
            nonce: FragmentNonce(nonce),
        }),
        _ => Err(FragmentError::Missing),
    }
}

// ---------------------------------------------------------------------------
// Origin / endpoints
// ---------------------------------------------------------------------------

/// Why an origin cannot host the viewer. Carries no input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginError {
    /// Not an `http://` / `https://` origin, or not a bare origin at all.
    Invalid,
    /// Plain `http` for a host that is not loopback.
    InsecureRemote,
}

/// The three endpoint URLs, derived from the page origin only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerEndpoints {
    consume_url: String,
    exchange_url: String,
    ws_url: String,
}

fn valid_dns_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

impl ViewerEndpoints {
    /// Derive endpoints from `window.location.origin`.
    ///
    /// Rejects userinfo, path, query, fragment, whitespace, bad ports and
    /// (for plain `http`) any non-loopback host.
    pub fn from_origin(origin: &str) -> Result<Self, OriginError> {
        let (secure, rest) = if let Some(rest) = origin.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = origin.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(OriginError::Invalid);
        };
        if rest.is_empty()
            || rest
                .bytes()
                .any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | b'\\') || b.is_ascii_whitespace())
        {
            return Err(OriginError::Invalid);
        }

        // Split `host[:port]`, honoring a bracketed IPv6 literal.
        let (host, port, loopback) = if let Some(inner) = rest.strip_prefix('[') {
            let (addr, after) = inner.split_once(']').ok_or(OriginError::Invalid)?;
            let ip: Ipv6Addr = addr.parse().map_err(|_| OriginError::Invalid)?;
            let port = match after {
                "" => None,
                other => Some(other.strip_prefix(':').ok_or(OriginError::Invalid)?),
            };
            (format!("[{addr}]"), port, ip.is_loopback())
        } else {
            let (host, port) = match rest.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (rest, None),
            };
            if !valid_dns_host(host) {
                return Err(OriginError::Invalid);
            }
            (host.to_owned(), port, is_loopback_host(host))
        };
        if let Some(port) = port {
            let valid = !port.is_empty()
                && port.len() <= 5
                && port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|p| p != 0);
            if !valid {
                return Err(OriginError::Invalid);
            }
        }
        if !secure && !loopback {
            return Err(OriginError::InsecureRemote);
        }

        let authority = match port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        let (http, ws) = if secure {
            ("https", "wss")
        } else {
            ("http", "ws")
        };
        Ok(Self {
            consume_url: format!("{http}://{authority}{CONSUME_PATH}"),
            exchange_url: format!("{http}://{authority}{EXCHANGE_PATH}"),
            ws_url: format!("{ws}://{authority}{VIEWER_WS_PATH}"),
        })
    }

    #[must_use]
    pub fn consume_url(&self) -> &str {
        &self.consume_url
    }

    #[must_use]
    pub fn exchange_url(&self) -> &str {
        &self.exchange_url
    }

    #[must_use]
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Why a session could not be started from the page location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    Origin(OriginError),
    Fragment(FragmentError),
}

/// Why a session ended. Terminal; carries nothing the server sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// A consume / exchange reply failed validation.
    InvalidResponse,
    /// The server sent `AuthError` or an unusable auth reply.
    AuthRejected,
    /// No `AuthOk` within [`AUTH_TIMEOUT_MS`].
    AuthTimeout,
    /// The socket closed or errored.
    TransportClosed,
    /// Closed locally (page teardown).
    Local,
}

/// Where the session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// Fragment parsed; nothing sent.
    Idle,
    /// Consume request issued; awaiting the ticket.
    Consuming,
    /// Exchange request issued; awaiting the snapshot.
    Exchanging,
    /// Snapshot bound; awaiting the socket's `AuthOk`.
    Exchanged,
    /// Authenticated; binary frames are accepted.
    Authenticated,
    /// Terminal. Frame, snapshot and secrets are gone.
    Closed(CloseReason),
}

/// Why a frame was refused. Never carries bytes or decoder text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameReject {
    /// Not a decodable `ViewerWsFrame` (or an unsupported header version).
    Malformed,
    /// Resource, binding, connection epoch or stream epoch differs from the
    /// exchange snapshot.
    ScopeMismatch,
    /// `seq` did not increase.
    StaleSeq,
}

/// Why a session call was refused. Payload-free by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// Called in a phase that does not allow it; nothing changed.
    WrongPhase,
    GrantMismatch,
    WsPathMismatch,
    InvalidTicket,
    InvalidCredentials,
    InvalidSnapshot,
    RedactionNotCleared,
    AuthReplyTooLarge,
    MalformedAuthReply,
    UnsupportedVersion,
    AuthRejected,
    HelloTooLarge,
    Frame(FrameReject),
}

/// The serialized `AuthHello` text frame. `Debug` hides the secret inside.
pub struct AuthHelloText(String);

impl AuthHelloText {
    /// The exact text to send as the first WebSocket message.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AuthHelloText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthHelloText([REDACTED])")
    }
}

/// The newest accepted frame. `Debug` shows sequence and length only.
#[derive(Clone, PartialEq, Eq)]
pub struct AcceptedFrame {
    seq: u64,
    captured_at_ms: i64,
    viewport_css_px: ViewportCssPx,
    jpeg: Vec<u8>,
}

impl AcceptedFrame {
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    #[must_use]
    pub fn captured_at_ms(&self) -> i64 {
        self.captured_at_ms
    }

    #[must_use]
    pub fn viewport_css_px(&self) -> &ViewportCssPx {
        &self.viewport_css_px
    }

    #[must_use]
    pub fn jpeg(&self) -> &[u8] {
        &self.jpeg
    }
}

impl fmt::Debug for AcceptedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptedFrame")
            .field("seq", &self.seq)
            .field("jpeg_len", &self.jpeg.len())
            .finish()
    }
}

/// One standalone viewer connection attempt. See the module docs for the
/// contract the transport must honor.
pub struct ViewerSession {
    endpoints: ViewerEndpoints,
    phase: SessionPhase,
    grant_id: String,
    nonce: Option<FragmentNonce>,
    rotation_generation: u64,
    secret: Option<ViewerSecret>,
    bound: Option<BoundBrowser>,
    last_seq: u64,
    frame: Option<AcceptedFrame>,
}

impl fmt::Debug for ViewerSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewerSession")
            .field("phase", &self.phase)
            .field("last_seq", &self.last_seq)
            .field("has_frame", &self.frame.is_some())
            .finish_non_exhaustive()
    }
}

impl ViewerSession {
    /// Build an `Idle` session from `location.origin` and `location.hash`.
    pub fn from_location(origin: &str, hash: &str) -> Result<Self, StartError> {
        let endpoints = ViewerEndpoints::from_origin(origin).map_err(StartError::Origin)?;
        let fragment = parse_fragment(hash).map_err(StartError::Fragment)?;
        Ok(Self::new(endpoints, fragment))
    }

    #[must_use]
    pub fn new(endpoints: ViewerEndpoints, fragment: ViewerFragment) -> Self {
        Self {
            endpoints,
            phase: SessionPhase::Idle,
            grant_id: fragment.grant_id,
            nonce: Some(fragment.nonce),
            rotation_generation: 0,
            secret: None,
            bound: None,
            last_seq: 0,
            frame: None,
        }
    }

    #[must_use]
    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    #[must_use]
    pub fn endpoints(&self) -> &ViewerEndpoints {
        &self.endpoints
    }

    /// The binding authority: set only by a validated exchange snapshot.
    #[must_use]
    pub fn bound(&self) -> Option<&BoundBrowser> {
        self.bound.as_ref()
    }

    #[must_use]
    pub fn frame(&self) -> Option<&AcceptedFrame> {
        self.frame.as_ref()
    }

    /// `Idle → Consuming`. Moves the nonce out; it cannot be sent twice.
    pub fn begin_consume(&mut self) -> Result<ConsumeFragmentNonceRequest, SessionError> {
        if self.phase != SessionPhase::Idle {
            return Err(SessionError::WrongPhase);
        }
        let nonce = self.nonce.take().ok_or(SessionError::WrongPhase)?;
        self.phase = SessionPhase::Consuming;
        Ok(ConsumeFragmentNonceRequest {
            grant_id: self.grant_id.clone(),
            nonce,
        })
    }

    /// `Consuming → Exchanging`. The ticket is forwarded, not retained.
    pub fn accept_consume_response(
        &mut self,
        response: ConsumeFragmentNonceResponse,
    ) -> Result<ExchangeViewerGrantRequest, SessionError> {
        if self.phase != SessionPhase::Consuming {
            return Err(SessionError::WrongPhase);
        }
        let ticket: ExchangeTicket = response.ticket;
        if ticket.0.is_empty() || ticket.0.len() > TICKET_MAX_BYTES {
            self.close(CloseReason::InvalidResponse);
            return Err(SessionError::InvalidTicket);
        }
        self.phase = SessionPhase::Exchanging;
        Ok(ExchangeViewerGrantRequest { ticket })
    }

    /// `Exchanging → Exchanged`. Any invalid reply closes the session.
    pub fn accept_exchange_response(
        &mut self,
        response: ExchangeViewerGrantResponse,
    ) -> Result<(), SessionError> {
        if self.phase != SessionPhase::Exchanging {
            return Err(SessionError::WrongPhase);
        }
        match self.validate_exchange(response) {
            Ok((rotation_generation, secret, bound)) => {
                self.last_seq = bound.last_frame_seq;
                self.rotation_generation = rotation_generation;
                self.secret = Some(secret);
                self.bound = Some(bound.bound);
                self.phase = SessionPhase::Exchanged;
                Ok(())
            }
            Err(error) => {
                self.close(CloseReason::InvalidResponse);
                Err(error)
            }
        }
    }

    fn validate_exchange(
        &self,
        response: ExchangeViewerGrantResponse,
    ) -> Result<(u64, ViewerSecret, ExchangeBinding), SessionError> {
        let ExchangeViewerGrantResponse {
            grant_id,
            rotation_generation,
            current_secret,
            ws_path,
            snapshot,
            ..
        } = response;
        if grant_id != self.grant_id {
            return Err(SessionError::GrantMismatch);
        }
        if ws_path != VIEWER_WS_PATH {
            return Err(SessionError::WsPathMismatch);
        }
        // Reuse the protocol's own shape rule so a hello we could not send is
        // refused here, before any socket exists.
        AuthHello {
            protocol_version: VIEWER_PROTOCOL_VERSION,
            grant_id,
            rotation_generation,
            secret: current_secret.0.clone(),
        }
        .validate_shape()
        .map_err(|_| SessionError::InvalidCredentials)?;
        if !matches!(
            snapshot.resource.redaction,
            RedactionStatus::Safe | RedactionStatus::Redacted
        ) {
            return Err(SessionError::RedactionNotCleared);
        }
        let last_frame_seq = snapshot.last_frame_seq;
        let bound =
            BoundBrowser::from_snapshot(snapshot).map_err(|_| SessionError::InvalidSnapshot)?;
        Ok((
            rotation_generation,
            current_secret,
            ExchangeBinding {
                bound,
                last_frame_seq,
            },
        ))
    }

    /// The first (text) socket message. One-shot: the secret leaves the
    /// session here, so a second call is [`SessionError::WrongPhase`].
    pub fn take_auth_hello(&mut self) -> Result<AuthHelloText, SessionError> {
        if self.phase != SessionPhase::Exchanged {
            return Err(SessionError::WrongPhase);
        }
        let secret = self.secret.take().ok_or(SessionError::WrongPhase)?;
        let message = ViewerAuthClientMessage::AuthHello {
            hello: AuthHello {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                grant_id: self.grant_id.clone(),
                rotation_generation: self.rotation_generation,
                secret: secret.0,
            },
        };
        let text = serde_json::to_string(&message).map_err(|_| {
            self.close(CloseReason::InvalidResponse);
            SessionError::InvalidCredentials
        })?;
        if text.len() > VIEWER_AUTH_FIRST_FRAME_MAX_BYTES {
            self.close(CloseReason::InvalidResponse);
            return Err(SessionError::HelloTooLarge);
        }
        Ok(AuthHelloText(text))
    }

    /// `Exchanged → Authenticated` on `AuthOk`; anything else closes. The
    /// reply's `next_secret` is dropped: reconnect rotation is unsupported.
    pub fn accept_auth_text(&mut self, text: &str) -> Result<(), SessionError> {
        // The hello must have left (secret taken) before any reply counts.
        if self.phase != SessionPhase::Exchanged || self.secret.is_some() {
            return Err(SessionError::WrongPhase);
        }
        if text.len() > AUTH_REPLY_MAX_BYTES {
            self.close(CloseReason::AuthRejected);
            return Err(SessionError::AuthReplyTooLarge);
        }
        let Ok(message) = serde_json::from_str::<ViewerAuthServerMessage>(text) else {
            self.close(CloseReason::AuthRejected);
            return Err(SessionError::MalformedAuthReply);
        };
        match message {
            ViewerAuthServerMessage::AuthOk { ok } => {
                if ok.protocol_version != VIEWER_PROTOCOL_VERSION {
                    self.close(CloseReason::AuthRejected);
                    return Err(SessionError::UnsupportedVersion);
                }
                self.phase = SessionPhase::Authenticated;
                Ok(())
            }
            ViewerAuthServerMessage::AuthError { .. } => {
                self.close(CloseReason::AuthRejected);
                Err(SessionError::AuthRejected)
            }
        }
    }

    /// The auth timer fired. Closes only a session still waiting for `AuthOk`.
    pub fn on_auth_timeout(&mut self) {
        if self.phase == SessionPhase::Exchanged {
            self.close(CloseReason::AuthTimeout);
        }
    }

    /// The socket closed or errored.
    pub fn on_transport_closed(&mut self) {
        self.close(CloseReason::TransportClosed);
    }

    /// Accept one binary socket message.
    ///
    /// Valid only when `Authenticated`. The frame must decode under the
    /// snapshot's exact resource id, binding and both epochs, and its `seq`
    /// must be strictly greater than the last accepted one (the snapshot's
    /// `last_frame_seq` at first). On any `Err` the session is **unchanged**;
    /// the transport is expected to close.
    pub fn accept_binary_frame(&mut self, bytes: &[u8]) -> Result<u64, SessionError> {
        if self.phase != SessionPhase::Authenticated {
            return Err(SessionError::WrongPhase);
        }
        let bound = self.bound.as_ref().ok_or(SessionError::WrongPhase)?;
        let live = bound.live().ok_or(SessionError::WrongPhase)?;
        let frame = ViewerWsFrame::decode_for(
            bytes,
            &bound.resource().resource_id,
            bound.viewer(),
            live.connection_epoch,
            live.stream_epoch,
        )
        .map_err(|error| {
            SessionError::Frame(match error {
                ViewerWsFrameCodecError::AttachedScopeMismatch => FrameReject::ScopeMismatch,
                _ => FrameReject::Malformed,
            })
        })?;
        if u16::from(frame.header.version) != VIEWER_PROTOCOL_VERSION {
            return Err(SessionError::Frame(FrameReject::Malformed));
        }
        if frame.header.seq <= self.last_seq {
            return Err(SessionError::Frame(FrameReject::StaleSeq));
        }
        let seq = frame.header.seq;
        self.last_seq = seq;
        self.frame = Some(AcceptedFrame {
            seq,
            captured_at_ms: frame.header.captured_at_ms,
            viewport_css_px: frame.header.viewport_css_px,
            jpeg: frame.jpeg_payload,
        });
        Ok(seq)
    }

    /// End the session. Terminal and idempotent (the first reason wins).
    /// Synchronously drops the held frame, snapshot binding and every secret.
    pub fn close(&mut self, reason: CloseReason) {
        self.nonce = None;
        self.secret = None;
        self.bound = None;
        self.frame = None;
        if !matches!(self.phase, SessionPhase::Closed(_)) {
            self.phase = SessionPhase::Closed(reason);
        }
    }
}

/// Validated exchange output, moved into the session only on success.
struct ExchangeBinding {
    bound: BoundBrowser,
    last_frame_seq: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        AuthError, AuthErrorCode, AuthOk, BrowserLiveSnapshot, BrowserTargetBinding, Controller,
        ConversationBinding, DisplayPolicy, IssuerIdentity, PreviewKind, PreviewProvenance,
        PreviewResource, PreviewSource, ResourceBinding, ViewerBinding, ViewerWsFrameHeader,
    };

    const GRANT: &str = "grant-abc_123";
    const NONCE: &str = "nonce-SECRET_value-0123456789abcdef";
    const SECRET43: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const JPEG: &[u8] = b"SECRETJPEG";

    fn binding() -> ViewerBinding {
        ViewerBinding {
            principal: IssuerIdentity::User {
                user_id: "user-42".to_string(),
            },
            task: ConversationBinding {
                session_key: "agent:operator:browser".to_string(),
                run_id: "run-7".to_string(),
            },
            target: BrowserTargetBinding {
                profile: "default".to_string(),
                browser_session_id: "browser-session-3".to_string(),
                target_id: "target-9".to_string(),
            },
        }
    }

    fn snapshot(redaction: RedactionStatus) -> BrowserLiveSnapshot {
        BrowserLiveSnapshot {
            resource: PreviewResource {
                resource_id: "resource-5".to_string(),
                revision: 1,
                binding: ResourceBinding::Browser { viewer: binding() },
                kind: PreviewKind::Page,
                display: DisplayPolicy::AutoRevealCurrentRun,
                source: PreviewSource::BrowserTool,
                provenance: PreviewProvenance::NotManaged,
                expires_at_ms: 2_000,
                redaction,
                companion_candidates: Vec::new(),
            },
            connection_epoch: 3,
            stream_epoch: 4,
            last_frame_seq: 0,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            controller: Controller::Agent,
            control_epoch: 1,
        }
    }

    fn exchange(redaction: RedactionStatus) -> ExchangeViewerGrantResponse {
        ExchangeViewerGrantResponse {
            viewer_session_id: "vs-1".to_string(),
            grant_id: GRANT.to_string(),
            rotation_generation: 1,
            current_secret: ViewerSecret(SECRET43.to_string()),
            ws_path: VIEWER_WS_PATH.to_string(),
            expires_at_ms: 9_000,
            snapshot: snapshot(redaction),
        }
    }

    fn idle() -> ViewerSession {
        ViewerSession::from_location(
            "https://panel.example.com",
            &format!("#grant_id={GRANT}&nonce={NONCE}"),
        )
        .unwrap()
    }

    fn exchanged() -> ViewerSession {
        let mut s = idle();
        s.begin_consume().unwrap();
        s.accept_consume_response(ConsumeFragmentNonceResponse {
            ticket: ExchangeTicket("ticket-1".to_string()),
            expires_at_ms: 1,
        })
        .unwrap();
        s.accept_exchange_response(exchange(RedactionStatus::Safe))
            .unwrap();
        s
    }

    fn auth_ok_text() -> String {
        serde_json::to_string(&ViewerAuthServerMessage::AuthOk {
            ok: AuthOk {
                protocol_version: VIEWER_PROTOCOL_VERSION,
                ws_generation: 1,
                next_secret: "NEXT_SECRET_VALUE".to_string(),
            },
        })
        .unwrap()
    }

    fn authenticated() -> ViewerSession {
        let mut s = exchanged();
        s.take_auth_hello().unwrap();
        s.accept_auth_text(&auth_ok_text()).unwrap();
        assert_eq!(s.phase(), SessionPhase::Authenticated);
        s
    }

    fn header(seq: u64) -> ViewerWsFrameHeader {
        ViewerWsFrameHeader {
            version: 1,
            resource_id: "resource-5".to_string(),
            binding: binding(),
            connection_epoch: 3,
            stream_epoch: 4,
            seq,
            captured_at_ms: 10,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 720,
            },
            content_type: "image/jpeg".to_string(),
        }
    }

    fn encode(header: ViewerWsFrameHeader) -> Vec<u8> {
        ViewerWsFrame {
            header,
            jpeg_payload: JPEG.to_vec(),
        }
        .encode()
        .unwrap()
    }

    // --- fragment ---------------------------------------------------------

    #[test]
    fn fragment_accepts_the_issuer_shape_in_either_order() {
        let a = parse_fragment(&format!("#grant_id={GRANT}&nonce={NONCE}")).unwrap();
        assert_eq!(a.grant_id(), GRANT);
        let b = parse_fragment(&format!("nonce={NONCE}&grant_id={GRANT}")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn fragment_refuses_everything_else() {
        let cases: &[(&str, FragmentError)] = &[
            ("", FragmentError::Empty),
            ("#", FragmentError::Empty),
            ("#grant_id=a", FragmentError::Missing),
            ("#nonce=a", FragmentError::Missing),
            ("#grant_id=a&grant_id=b&nonce=c", FragmentError::Duplicate),
            ("#grant_id=a&nonce=b&nonce=c", FragmentError::Duplicate),
            ("#grant_id=a&nonce=b&extra=c", FragmentError::Unknown),
            ("#?grant_id=a&nonce=b", FragmentError::Unknown),
            ("#grant_id=a&nonce=b&", FragmentError::Malformed),
            ("#grant_id&nonce=b", FragmentError::Malformed),
            ("#grant_id=&nonce=b", FragmentError::Malformed),
            ("#grant_id=a%20b&nonce=c", FragmentError::Malformed),
            ("#grant_id=a b&nonce=c", FragmentError::Malformed),
            ("#grant_id=a&nonce=b#c", FragmentError::Malformed),
            ("#grant_id=a&nonce=b=c", FragmentError::Malformed),
            ("##grant_id=a&nonce=b", FragmentError::Unknown),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_fragment(input).unwrap_err(), *expected, "{input}");
        }
        let long_grant = "g".repeat(GRANT_ID_MAX_BYTES + 1);
        assert_eq!(
            parse_fragment(&format!("#grant_id={long_grant}&nonce=b")).unwrap_err(),
            FragmentError::Malformed
        );
        let long_nonce = "n".repeat(NONCE_MAX_BYTES + 1);
        assert_eq!(
            parse_fragment(&format!("#grant_id=a&nonce={long_nonce}")).unwrap_err(),
            FragmentError::Malformed
        );
        let oversize = "x".repeat(FRAGMENT_MAX_BYTES + 1);
        assert_eq!(
            parse_fragment(&oversize).unwrap_err(),
            FragmentError::Oversize
        );
    }

    #[test]
    fn fragment_debug_and_errors_never_echo_input() {
        let fragment = parse_fragment(&format!("#grant_id={GRANT}&nonce={NONCE}")).unwrap();
        let debug = format!("{fragment:?}");
        assert!(!debug.contains(NONCE) && !debug.contains(GRANT));
        let err = parse_fragment(&format!("#grant_id={GRANT}&nonce=bad value")).unwrap_err();
        assert!(!format!("{err:?}").contains("bad"));
    }

    // --- origin / ws ------------------------------------------------------

    #[test]
    fn https_derives_wss_and_http_loopback_derives_ws() {
        let e = ViewerEndpoints::from_origin("https://panel.example.com").unwrap();
        assert_eq!(
            e.consume_url(),
            "https://panel.example.com/viewer/browser/consume"
        );
        assert_eq!(
            e.exchange_url(),
            "https://panel.example.com/viewer/browser/exchange"
        );
        assert_eq!(e.ws_url(), "wss://panel.example.com/viewer/browser/ws");

        let e = ViewerEndpoints::from_origin("http://127.0.0.1:18789").unwrap();
        assert_eq!(e.ws_url(), "ws://127.0.0.1:18789/viewer/browser/ws");
        let e = ViewerEndpoints::from_origin("http://localhost:3000").unwrap();
        assert_eq!(e.ws_url(), "ws://localhost:3000/viewer/browser/ws");
        let e = ViewerEndpoints::from_origin("http://[::1]:8080").unwrap();
        assert_eq!(e.ws_url(), "ws://[::1]:8080/viewer/browser/ws");
        let e = ViewerEndpoints::from_origin("https://[2001:db8::1]:8443").unwrap();
        assert_eq!(e.ws_url(), "wss://[2001:db8::1]:8443/viewer/browser/ws");
    }

    #[test]
    fn remote_plain_http_is_refused() {
        for origin in [
            "http://panel.example.com",
            "http://192.168.1.10:18789",
            "http://10.0.0.1",
            "http://[2001:db8::1]",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
        ] {
            assert_eq!(
                ViewerEndpoints::from_origin(origin).unwrap_err(),
                OriginError::InsecureRemote,
                "{origin}"
            );
        }
    }

    #[test]
    fn malformed_origins_are_refused() {
        for origin in [
            "",
            "ftp://x",
            "ws://127.0.0.1",
            "https://",
            "https://a.com/",
            "https://a.com/path",
            "https://a.com?x=1",
            "https://a.com#f",
            "https://user@a.com",
            "https://a.com:0",
            "https://a.com:99999",
            "https://a.com:",
            "https://a.com:80a",
            "https://a b.com",
            "https://-a.com",
            "http://[::1",
            "http://[::1]x",
            "HTTPS://a.com",
        ] {
            assert_eq!(
                ViewerEndpoints::from_origin(origin).unwrap_err(),
                OriginError::Invalid,
                "{origin}"
            );
        }
    }

    #[test]
    fn only_the_exact_path_is_the_viewer_route() {
        assert!(is_standalone_viewer_path("/viewer/browser"));
        for path in [
            "/",
            "/viewer/browser/",
            "/viewer/browser/ws",
            "/viewer",
            "/Viewer/browser",
        ] {
            assert!(!is_standalone_viewer_path(path), "{path}");
        }
    }

    #[test]
    fn from_location_reports_which_half_failed() {
        assert_eq!(
            ViewerSession::from_location("http://example.com", "#grant_id=a&nonce=b").unwrap_err(),
            StartError::Origin(OriginError::InsecureRemote)
        );
        assert_eq!(
            ViewerSession::from_location("https://example.com", "").unwrap_err(),
            StartError::Fragment(FragmentError::Empty)
        );
    }

    // --- state machine ----------------------------------------------------

    #[test]
    fn happy_path_walks_every_phase() {
        let mut s = idle();
        assert_eq!(s.phase(), SessionPhase::Idle);
        let req = s.begin_consume().unwrap();
        assert_eq!(req.grant_id, GRANT);
        assert_eq!(req.nonce, FragmentNonce(NONCE.to_string()));
        assert_eq!(s.phase(), SessionPhase::Consuming);

        let next = s
            .accept_consume_response(ConsumeFragmentNonceResponse {
                ticket: ExchangeTicket("ticket-1".to_string()),
                expires_at_ms: 1,
            })
            .unwrap();
        assert_eq!(next.ticket, ExchangeTicket("ticket-1".to_string()));
        assert_eq!(s.phase(), SessionPhase::Exchanging);

        s.accept_exchange_response(exchange(RedactionStatus::Redacted))
            .unwrap();
        assert_eq!(s.phase(), SessionPhase::Exchanged);
        assert_eq!(s.bound().unwrap().resource().resource_id, "resource-5");

        let hello = s.take_auth_hello().unwrap();
        assert!(hello.as_str().len() <= VIEWER_AUTH_FIRST_FRAME_MAX_BYTES);
        assert!(hello.as_str().contains("auth_hello"));
        assert_eq!(s.take_auth_hello().unwrap_err(), SessionError::WrongPhase);

        s.accept_auth_text(&auth_ok_text()).unwrap();
        assert_eq!(s.phase(), SessionPhase::Authenticated);
    }

    #[test]
    fn calls_out_of_phase_change_nothing() {
        let mut s = idle();
        assert_eq!(
            s.accept_consume_response(ConsumeFragmentNonceResponse {
                ticket: ExchangeTicket("t".to_string()),
                expires_at_ms: 1,
            })
            .unwrap_err(),
            SessionError::WrongPhase
        );
        assert_eq!(
            s.accept_exchange_response(exchange(RedactionStatus::Safe))
                .unwrap_err(),
            SessionError::WrongPhase
        );
        assert_eq!(s.take_auth_hello().unwrap_err(), SessionError::WrongPhase);
        assert_eq!(
            s.accept_auth_text("{}").unwrap_err(),
            SessionError::WrongPhase
        );
        assert_eq!(
            s.accept_binary_frame(&encode(header(1))).unwrap_err(),
            SessionError::WrongPhase
        );
        assert_eq!(s.phase(), SessionPhase::Idle);
        s.begin_consume().unwrap();
        assert_eq!(s.begin_consume().unwrap_err(), SessionError::WrongPhase);
    }

    #[test]
    fn invalid_exchange_replies_close_the_session() {
        let cases: Vec<(ExchangeViewerGrantResponse, SessionError)> = vec![
            (
                {
                    let mut r = exchange(RedactionStatus::Safe);
                    r.grant_id = "other".to_string();
                    r
                },
                SessionError::GrantMismatch,
            ),
            (
                {
                    let mut r = exchange(RedactionStatus::Safe);
                    r.ws_path = "/viewer/browser/ws/".to_string();
                    r
                },
                SessionError::WsPathMismatch,
            ),
            (
                {
                    let mut r = exchange(RedactionStatus::Safe);
                    r.current_secret = ViewerSecret("short".to_string());
                    r
                },
                SessionError::InvalidCredentials,
            ),
            (
                exchange(RedactionStatus::Pending),
                SessionError::RedactionNotCleared,
            ),
            (
                exchange(RedactionStatus::Refused),
                SessionError::RedactionNotCleared,
            ),
        ];
        for (reply, expected) in cases {
            let mut s = idle();
            s.begin_consume().unwrap();
            s.accept_consume_response(ConsumeFragmentNonceResponse {
                ticket: ExchangeTicket("t".to_string()),
                expires_at_ms: 1,
            })
            .unwrap();
            assert_eq!(s.accept_exchange_response(reply).unwrap_err(), expected);
            assert_eq!(
                s.phase(),
                SessionPhase::Closed(CloseReason::InvalidResponse)
            );
            assert!(s.bound().is_none());
        }
    }

    #[test]
    fn an_empty_ticket_closes() {
        let mut s = idle();
        s.begin_consume().unwrap();
        assert_eq!(
            s.accept_consume_response(ConsumeFragmentNonceResponse {
                ticket: ExchangeTicket(String::new()),
                expires_at_ms: 1,
            })
            .unwrap_err(),
            SessionError::InvalidTicket
        );
        assert_eq!(
            s.phase(),
            SessionPhase::Closed(CloseReason::InvalidResponse)
        );
    }

    #[test]
    fn auth_failures_close() {
        // AuthError
        let mut s = exchanged();
        s.take_auth_hello().unwrap();
        let text = serde_json::to_string(&ViewerAuthServerMessage::AuthError {
            error: AuthError {
                code: AuthErrorCode::Unauthorized,
                close: true,
            },
        })
        .unwrap();
        assert_eq!(
            s.accept_auth_text(&text).unwrap_err(),
            SessionError::AuthRejected
        );
        assert_eq!(s.phase(), SessionPhase::Closed(CloseReason::AuthRejected));

        // malformed / oversize / unsupported version
        let mut s = exchanged();
        s.take_auth_hello().unwrap();
        assert_eq!(
            s.accept_auth_text("not json").unwrap_err(),
            SessionError::MalformedAuthReply
        );
        let mut s = exchanged();
        s.take_auth_hello().unwrap();
        assert_eq!(
            s.accept_auth_text(&"x".repeat(AUTH_REPLY_MAX_BYTES + 1))
                .unwrap_err(),
            SessionError::AuthReplyTooLarge
        );
        let mut s = exchanged();
        s.take_auth_hello().unwrap();
        let text = serde_json::to_string(&ViewerAuthServerMessage::AuthOk {
            ok: AuthOk {
                protocol_version: 2,
                ws_generation: 1,
                next_secret: String::new(),
            },
        })
        .unwrap();
        assert_eq!(
            s.accept_auth_text(&text).unwrap_err(),
            SessionError::UnsupportedVersion
        );
    }

    #[test]
    fn an_auth_reply_before_the_hello_left_is_out_of_phase() {
        let mut s = exchanged();
        assert_eq!(
            s.accept_auth_text(&auth_ok_text()).unwrap_err(),
            SessionError::WrongPhase
        );
        assert_eq!(s.phase(), SessionPhase::Exchanged);
    }

    #[test]
    fn auth_timeout_closes_only_while_waiting() {
        let mut s = exchanged();
        s.take_auth_hello().unwrap();
        s.on_auth_timeout();
        assert_eq!(s.phase(), SessionPhase::Closed(CloseReason::AuthTimeout));

        let mut s = authenticated();
        s.on_auth_timeout();
        assert_eq!(s.phase(), SessionPhase::Authenticated);
    }

    #[test]
    fn close_clears_everything_and_keeps_the_first_reason() {
        let mut s = authenticated();
        s.accept_binary_frame(&encode(header(1))).unwrap();
        assert!(s.frame().is_some());
        s.on_transport_closed();
        assert_eq!(
            s.phase(),
            SessionPhase::Closed(CloseReason::TransportClosed)
        );
        assert!(s.frame().is_none() && s.bound().is_none());
        s.close(CloseReason::Local);
        assert_eq!(
            s.phase(),
            SessionPhase::Closed(CloseReason::TransportClosed)
        );
        assert_eq!(
            s.accept_binary_frame(&encode(header(2))).unwrap_err(),
            SessionError::WrongPhase
        );
    }

    // --- frames -----------------------------------------------------------

    #[test]
    fn frames_with_increasing_seq_are_accepted() {
        let mut s = authenticated();
        assert_eq!(s.accept_binary_frame(&encode(header(1))).unwrap(), 1);
        assert_eq!(s.accept_binary_frame(&encode(header(5))).unwrap(), 5);
        let frame = s.frame().unwrap();
        assert_eq!(frame.seq(), 5);
        assert_eq!(frame.jpeg(), JPEG);
    }

    #[test]
    fn mismatched_frames_are_rejected_without_mutation() {
        let mut s = authenticated();
        s.accept_binary_frame(&encode(header(2))).unwrap();

        let mut wrong_resource = header(3);
        wrong_resource.resource_id = "resource-6".to_string();
        let mut wrong_binding = header(3);
        wrong_binding.binding.target.target_id = "target-X".to_string();
        let mut wrong_conn = header(3);
        wrong_conn.connection_epoch = 99;
        let mut wrong_stream = header(3);
        wrong_stream.stream_epoch = 99;
        for h in [wrong_resource, wrong_binding, wrong_conn, wrong_stream] {
            assert_eq!(
                s.accept_binary_frame(&encode(h)).unwrap_err(),
                SessionError::Frame(FrameReject::ScopeMismatch)
            );
        }
        for seq in [0, 1, 2] {
            assert_eq!(
                s.accept_binary_frame(&encode(header(seq))).unwrap_err(),
                SessionError::Frame(FrameReject::StaleSeq)
            );
        }
        let mut wrong_version = header(3);
        wrong_version.version = 2;
        assert_eq!(
            s.accept_binary_frame(&encode(wrong_version)).unwrap_err(),
            SessionError::Frame(FrameReject::Malformed)
        );
        for bytes in [&b""[..], &b"garbage-bytes"[..], &encode(header(3))[..10]] {
            assert_eq!(
                s.accept_binary_frame(bytes).unwrap_err(),
                SessionError::Frame(FrameReject::Malformed)
            );
        }

        // Nothing moved: still authenticated, still the seq-2 frame, and the
        // next valid frame is accepted.
        assert_eq!(s.phase(), SessionPhase::Authenticated);
        assert_eq!(s.frame().unwrap().seq(), 2);
        assert_eq!(s.accept_binary_frame(&encode(header(3))).unwrap(), 3);
    }

    // --- redaction --------------------------------------------------------

    #[test]
    fn debug_output_never_contains_secrets_or_frame_bytes() {
        let mut s = idle();
        let req = s.begin_consume().unwrap();
        s.accept_consume_response(ConsumeFragmentNonceResponse {
            ticket: ExchangeTicket("TICKET_VALUE".to_string()),
            expires_at_ms: 1,
        })
        .map(|r| assert!(!format!("{r:?}").contains("TICKET_VALUE")))
        .unwrap();
        assert!(!format!("{req:?}").contains(NONCE));
        s.accept_exchange_response(exchange(RedactionStatus::Safe))
            .unwrap();
        let hello = s.take_auth_hello().unwrap();
        assert!(hello.as_str().contains(SECRET43));
        assert!(!format!("{hello:?}").contains(SECRET43));
        s.accept_auth_text(&auth_ok_text()).unwrap();
        s.accept_binary_frame(&encode(header(1))).unwrap();

        let mut dump = format!("{s:?} {:?}", s.frame().unwrap());
        dump.push_str(&format!("{:?}", s.accept_binary_frame(b"junk")));
        dump.push_str(&format!("{:?}", s.accept_binary_frame(&encode(header(1)))));
        for needle in [
            "SECRETJPEG",
            "[83, 69",
            NONCE,
            SECRET43,
            "NEXT_SECRET_VALUE",
        ] {
            assert!(!dump.contains(needle), "leaked {needle}");
        }
    }
}
