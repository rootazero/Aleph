//! Real transport for the standalone browser viewer.
//!
//! The pure session contract ([`super::session`]) owns every decision: what a
//! consume / exchange reply may contain, when the auth hello leaves, which
//! binary frames are accepted. This module is only the I/O that feeds it:
//!
//! 1. `POST` the consume request, hand the reply to the session;
//! 2. `POST` the exchange request, hand the reply to the session;
//! 3. open the one WebSocket the session's [`ViewerEndpoints`] derive from the
//!    page origin, send the auth hello the session hands out, then forward
//!    every message to the session and show the frame it accepted.
//!
//! Hard rules (restated here because a transport is where they get broken):
//!
//! * The fragment nonce and the session secret exist only inside the session
//!   and the one request / hello body that carries them. Nothing here logs,
//!   formats or displays them, and every failure is one of the payload-free
//!   enums below. Server bodies and decoder text are never shown.
//! * Requests go out with `credentials: omit`, `cache: no-store`,
//!   `referrerPolicy: no-referrer` and `redirect: error`. `mode` is left at its
//!   default (`cors`) on purpose: a same-origin POST with `no-referrer` and a
//!   non-cors mode is serialized with `Origin: null`, which the gateway
//!   refuses (it needs the real `Origin`). The target is always the exact
//!   same-origin endpoint, so `cors` mode never reaches another origin.
//! * The socket URL is the session's derived endpoint, re-derived from the page
//!   origin once more before connecting (see [`ws_target`]). No server-supplied
//!   path or host is ever dialed. No secret rides in the URL, a header or a
//!   subprotocol.
//! * The Gateway stays the only CDP owner: this is a read-only viewer. There is
//!   no input, no control message and no second channel.
//! * Frames are shown as `data:` URLs, not `blob:` URLs: the Panel CSP
//!   (`img-src 'self' data: https:`) has no `blob:` source. Only bytes the
//!   session accepted are ever encoded.
//!
//! The decision helpers at the top are pure and unit-tested natively; the
//! browser glue lives in the wasm-only `io` module.

use aleph_protocol::browser_live::{
    ViewerServerMessage, VIEWER_WS_FRAME_HEADER_MAX_BYTES, VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES,
};
use leptos::prelude::*;
use serde::de::DeserializeOwned;

use super::session::{ViewerEndpoints, ViewerSession};

/// Budget for one consume / exchange round trip (request + body read).
pub const REQUEST_TIMEOUT_MS: u32 = 10_000;

/// Upper bound on a consume / exchange reply body.
pub const RESPONSE_MAX_BYTES: usize = 1024 * 1024;

/// Upper bound on one binary socket message: the 8-byte length prefix, the
/// largest header and the largest JPEG payload the wire format allows.
pub const BINARY_MESSAGE_MAX_BYTES: usize =
    8 + VIEWER_WS_FRAME_HEADER_MAX_BYTES + VIEWER_WS_FRAME_PAYLOAD_MAX_BYTES;

/// Upper bound on one post-auth text message (`ViewerServerMessage`).
pub const TEXT_MESSAGE_MAX_BYTES: usize = 256 * 1024;

/// UI-facing transport state, shared between the driver and the view.
#[derive(Clone, Copy)]
pub struct TransportUi {
    /// A transport-level failure (network, server status, socket error or
    /// pre-auth close). Fixed meaning; carries no detail.
    pub failed: RwSignal<bool>,
    /// `data:` URL of the latest accepted frame, if any.
    pub frame_url: RwSignal<Option<String>>,
}

impl TransportUi {
    #[must_use]
    pub fn new() -> Self {
        Self {
            failed: RwSignal::new(false),
            frame_url: RwSignal::new(None),
        }
    }
}

impl Default for TransportUi {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a consume / exchange step failed. Payload-free by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepError {
    /// Could not reach the gateway, or it did not answer in time.
    Network,
    /// The gateway refused the request (`4xx`): bad / expired / reused link.
    Status4xx,
    /// The gateway answered with a non-success, non-`4xx` status.
    ServerStatus,
    /// The reply body exceeded [`RESPONSE_MAX_BYTES`].
    TooLarge,
    /// The reply was not exactly the expected protocol struct.
    Decode,
}

/// How a failed step is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    /// The link itself is unusable: shown as the invalid-link notice.
    InvalidLink,
    /// The transport failed: shown as the fixed failure notice.
    Failed,
}

impl StepError {
    /// A refused or malformed exchange means the link / reply is invalid; an
    /// unreachable or erroring gateway is a transport failure.
    #[must_use]
    pub fn outcome(self) -> StepOutcome {
        match self {
            Self::Status4xx | Self::TooLarge | Self::Decode => StepOutcome::InvalidLink,
            Self::Network | Self::ServerStatus => StepOutcome::Failed,
        }
    }
}

/// Map an HTTP status to success or a [`StepError`].
pub fn classify_status(status: u16) -> Result<(), StepError> {
    match status {
        200..=299 => Ok(()),
        400..=499 => Err(StepError::Status4xx),
        _ => Err(StepError::ServerStatus),
    }
}

/// Decode a reply body as exactly `T` (the protocol structs deny unknown
/// fields). The decoder's error text is dropped: it can quote the input.
pub fn decode_response<T: DeserializeOwned>(text: &str) -> Result<T, StepError> {
    if text.len() > RESPONSE_MAX_BYTES {
        return Err(StepError::TooLarge);
    }
    serde_json::from_str(text).map_err(|_| StepError::Decode)
}

/// The socket URL to dial, or `None` if it is not exactly the endpoint
/// re-derived from the page origin. Defence in depth over the session, which
/// already pins `ws_path`: a mismatch here means the session's endpoints were
/// not built from this origin.
#[must_use]
pub fn ws_target(endpoints: &ViewerEndpoints, page_origin: &str) -> Option<String> {
    ViewerEndpoints::from_origin(page_origin)
        .ok()
        .filter(|derived| derived == endpoints)
        .map(|derived| derived.ws_url().to_owned())
}

/// Whether a binary message is within the wire format's absolute ceiling.
#[must_use]
pub fn binary_len_ok(len: usize) -> bool {
    len > 0 && len <= BINARY_MESSAGE_MAX_BYTES
}

/// What a post-auth text message means for the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAction {
    /// A well-formed server message the read-only viewer does not render.
    Ignore,
    /// The server said the viewer is closed: end with the closed notice.
    Closed,
    /// A server error, an oversize message or something undecodable: end with
    /// the failure notice (fail closed).
    Failed,
}

/// Classify one post-auth text message. Nothing from it is kept or shown.
#[must_use]
pub fn classify_text(text: &str) -> TextAction {
    if text.len() > TEXT_MESSAGE_MAX_BYTES {
        return TextAction::Failed;
    }
    match serde_json::from_str::<ViewerServerMessage>(text) {
        Ok(ViewerServerMessage::Closed { .. }) => TextAction::Closed,
        Ok(ViewerServerMessage::Error { .. }) | Err(_) => TextAction::Failed,
        Ok(_) => TextAction::Ignore,
    }
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `data:image/jpeg;base64,...` for session-accepted JPEG bytes.
#[must_use]
pub fn jpeg_data_url(jpeg: &[u8]) -> String {
    const PREFIX: &str = "data:image/jpeg;base64,";
    let mut out = String::with_capacity(PREFIX.len() + jpeg.len().div_ceil(3) * 4);
    out.push_str(PREFIX);
    for chunk in jpeg.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let n = (b0 << 16) | (b1 << 8) | b2;
        let sextet = |shift: u32| char::from(BASE64_ALPHABET[((n >> shift) & 0x3f) as usize]);
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    out
}

/// Start the transport for `session`. Browser-only; native builds have no
/// network I/O and this is a no-op.
///
/// Must be called from a reactive owner (a component body): teardown is bound
/// to that owner. Cleanup closes the socket, drops its closures, clears the
/// shown frame and closes the session.
#[cfg(target_arch = "wasm32")]
pub fn start(session: RwSignal<ViewerSession>, ui: TransportUi, origin: String) {
    io::start(session, ui, origin);
}

/// Native builds have no browser transport.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(session: RwSignal<ViewerSession>, ui: TransportUi, origin: String) {
    let _ = (session, ui, origin);
}

#[cfg(target_arch = "wasm32")]
mod io {
    use super::*;
    use crate::views::browser::session::{CloseReason, SessionPhase, AUTH_TIMEOUT_MS};
    use aleph_protocol::browser_live::{ConsumeFragmentNonceResponse, ExchangeViewerGrantResponse};
    use futures::future::{select, Either};
    use gloo_timers::future::TimeoutFuture;
    use leptos::task::spawn_local;
    use serde::Serialize;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        BinaryType, Event, Headers, MessageEvent, ReferrerPolicy, RequestCache, RequestCredentials,
        RequestInit, RequestRedirect, Response, WebSocket,
    };

    /// One open socket and the closures wired to it. The closures must outlive
    /// the socket's handlers, so they live here and are dropped only by
    /// [`Socket::dispose`], after the handlers are cleared.
    struct Socket {
        ws: WebSocket,
        _on_open: Closure<dyn FnMut(Event)>,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_close: Closure<dyn FnMut(Event)>,
        _on_error: Closure<dyn FnMut(Event)>,
    }

    impl Socket {
        fn dispose(self) {
            self.ws.set_onopen(None);
            self.ws.set_onmessage(None);
            self.ws.set_onclose(None);
            self.ws.set_onerror(None);
            let _ = self.ws.close();
        }
    }

    type SocketSlot = StoredValue<Option<Socket>, LocalStorage>;

    pub(super) fn start(session: RwSignal<ViewerSession>, ui: TransportUi, origin: String) {
        let slot: SocketSlot = StoredValue::new_local(None);
        on_cleanup(move || {
            if let Some(Some(socket)) = slot.try_update_value(|held| held.take()) {
                socket.dispose();
            }
            let _ = session.try_update(|s| s.close(CloseReason::Local));
            let _ = ui.frame_url.try_set(None);
        });
        spawn_local(async move {
            run(session, ui, origin, slot).await;
        });
    }

    /// Record a failed consume / exchange step. The session closes with a
    /// payload-free reason; `Failed` additionally raises the failure flag.
    fn finish_step(session: RwSignal<ViewerSession>, ui: TransportUi, error: StepError) {
        match error.outcome() {
            StepOutcome::InvalidLink => {
                let _ = session.try_update(|s| s.close(CloseReason::InvalidResponse));
            }
            StepOutcome::Failed => {
                let _ = ui.failed.try_set(true);
                let _ = session.try_update(|s| s.close(CloseReason::TransportClosed));
            }
        }
    }

    async fn run(
        session: RwSignal<ViewerSession>,
        ui: TransportUi,
        origin: String,
        slot: SocketSlot,
    ) {
        let Some(endpoints) = session.try_with(|s| s.endpoints().clone()) else {
            return;
        };
        let Some(Ok(consume_request)) = session.try_update(|s| s.begin_consume()) else {
            let _ = ui.failed.try_set(true);
            let _ = session.try_update(|s| s.close(CloseReason::Local));
            return;
        };
        let consumed =
            post_json::<_, ConsumeFragmentNonceResponse>(endpoints.consume_url(), &consume_request)
                .await;
        drop(consume_request);
        let consumed = match consumed {
            Ok(consumed) => consumed,
            Err(error) => return finish_step(session, ui, error),
        };
        let exchange_request = match session.try_update(|s| s.accept_consume_response(consumed)) {
            Some(Ok(request)) => request,
            // The session already closed itself (or is gone / closed).
            _ => {
                let _ = session.try_update(|s| s.close(CloseReason::InvalidResponse));
                return;
            }
        };
        let exchanged = post_json::<_, ExchangeViewerGrantResponse>(
            endpoints.exchange_url(),
            &exchange_request,
        )
        .await;
        drop(exchange_request);
        let exchanged = match exchanged {
            Ok(exchanged) => exchanged,
            Err(error) => return finish_step(session, ui, error),
        };
        match session.try_update(|s| s.accept_exchange_response(exchanged)) {
            Some(Ok(())) => {}
            _ => {
                let _ = session.try_update(|s| s.close(CloseReason::InvalidResponse));
                return;
            }
        }
        let Some(ws_url) = ws_target(&endpoints, &origin) else {
            let _ = session.try_update(|s| s.close(CloseReason::InvalidResponse));
            return;
        };
        if session.try_with(|s| s.phase()) != Some(SessionPhase::Exchanged) {
            return;
        }
        if open_socket(&ws_url, session, ui, slot).is_err() {
            let _ = ui.failed.try_set(true);
            let _ = session.try_update(|s| s.close(CloseReason::TransportClosed));
        }
    }

    /// `POST` `body` as JSON and decode the reply as exactly `Res`.
    async fn post_json<Req: Serialize, Res: DeserializeOwned>(
        url: &str,
        body: &Req,
    ) -> Result<Res, StepError> {
        let window = web_sys::window().ok_or(StepError::Network)?;
        let payload = serde_json::to_string(body).map_err(|_| StepError::Decode)?;

        let headers = Headers::new().map_err(|_| StepError::Network)?;
        headers
            .set("Content-Type", "application/json")
            .map_err(|_| StepError::Network)?;
        let init = RequestInit::new();
        init.set_method("POST");
        init.set_headers(headers.as_ref());
        init.set_body(&JsValue::from_str(&payload));
        init.set_credentials(RequestCredentials::Omit);
        init.set_cache(RequestCache::NoStore);
        init.set_referrer_policy(ReferrerPolicy::NoReferrer);
        init.set_redirect(RequestRedirect::Error);
        // `mode` stays at its default on purpose; see the module docs.

        let pending = window.fetch_with_str_and_init(url, &init);
        drop(init);
        drop(payload);

        let response = match select(
            Box::pin(JsFuture::from(pending)),
            TimeoutFuture::new(REQUEST_TIMEOUT_MS),
        )
        .await
        {
            Either::Left((Ok(value), _)) => value,
            _ => return Err(StepError::Network),
        };
        let response: Response = response.dyn_into().map_err(|_| StepError::Network)?;
        classify_status(response.status())?;
        let body = response.text().map_err(|_| StepError::Network)?;
        let text = match select(
            Box::pin(JsFuture::from(body)),
            TimeoutFuture::new(REQUEST_TIMEOUT_MS),
        )
        .await
        {
            Either::Left((Ok(value), _)) => value.as_string().ok_or(StepError::Decode)?,
            _ => return Err(StepError::Network),
        };
        decode_response(&text)
    }

    fn open_socket(
        url: &str,
        session: RwSignal<ViewerSession>,
        ui: TransportUi,
        slot: SocketSlot,
    ) -> Result<(), ()> {
        let ws = WebSocket::new(url).map_err(|_| ())?;
        ws.set_binary_type(BinaryType::Arraybuffer);

        let on_open = {
            let ws = ws.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| handle_open(&ws, session, ui))
        };
        let on_message = {
            let ws = ws.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                handle_message(&ws, session, ui, &event);
            })
        };
        let on_close = {
            let ws = ws.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                let failed = pre_auth(session);
                end_session(&ws, session, ui, failed);
            })
        };
        let on_error = {
            let ws = ws.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                let failed = pre_auth(session);
                end_session(&ws, session, ui, failed);
            })
        };
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let mut pending = Some(Socket {
            ws,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
            _on_error: on_error,
        });
        let _ = slot.try_update_value(|held| *held = pending.take());
        // The page was torn down while connecting: nothing owns the socket.
        if let Some(orphan) = pending {
            orphan.dispose();
        }
        Ok(())
    }

    /// True while the session still waits for `AuthOk`: a socket ending now is
    /// a failure, not a normal close.
    fn pre_auth(session: RwSignal<ViewerSession>) -> bool {
        session.try_with(|s| s.phase()) == Some(SessionPhase::Exchanged)
    }

    /// End the viewer: terminal session close, frame dropped, socket closed.
    /// Never touches the socket slot (this can run inside a socket handler).
    fn end_session(
        ws: &WebSocket,
        session: RwSignal<ViewerSession>,
        ui: TransportUi,
        failed: bool,
    ) {
        if failed {
            let _ = ui.failed.try_set(true);
        }
        let _ = session.try_update(|s| s.on_transport_closed());
        let _ = ui.frame_url.try_set(None);
        let _ = ws.close();
    }

    fn handle_open(ws: &WebSocket, session: RwSignal<ViewerSession>, ui: TransportUi) {
        let hello = session
            .try_update(|s| s.take_auth_hello())
            .and_then(|result| result.ok());
        let Some(hello) = hello else {
            return end_session(ws, session, ui, true);
        };
        if ws.send_with_str(hello.as_str()).is_err() {
            return end_session(ws, session, ui, true);
        }
        drop(hello);
        let ws = ws.clone();
        spawn_local(async move {
            TimeoutFuture::new(AUTH_TIMEOUT_MS).await;
            let timed_out = session.try_update(|s| {
                s.on_auth_timeout();
                matches!(s.phase(), SessionPhase::Closed(_))
            });
            if timed_out == Some(true) {
                end_session(&ws, session, ui, true);
            }
        });
    }

    fn handle_message(
        ws: &WebSocket,
        session: RwSignal<ViewerSession>,
        ui: TransportUi,
        event: &MessageEvent,
    ) {
        let data = event.data();
        match session.try_with(|s| s.phase()) {
            Some(SessionPhase::Exchanged) => {
                let accepted = data.as_string().is_some_and(|text| {
                    session
                        .try_update(|s| s.accept_auth_text(&text))
                        .is_some_and(|result| result.is_ok())
                });
                if !accepted {
                    end_session(ws, session, ui, true);
                }
            }
            Some(SessionPhase::Authenticated) => {
                if let Some(text) = data.as_string() {
                    match classify_text(&text) {
                        TextAction::Ignore => {}
                        TextAction::Closed => end_session(ws, session, ui, false),
                        TextAction::Failed => end_session(ws, session, ui, true),
                    }
                } else if let Ok(buffer) = data.dyn_into::<js_sys::ArrayBuffer>() {
                    if !binary_len_ok(buffer.byte_length() as usize) {
                        return end_session(ws, session, ui, true);
                    }
                    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                    // Only bytes the session accepts are ever shown.
                    let url = session
                        .try_update(|s| match s.accept_binary_frame(&bytes) {
                            Ok(_) => s.frame().map(|frame| jpeg_data_url(frame.jpeg())),
                            Err(_) => None,
                        })
                        .flatten();
                    match url {
                        Some(url) => {
                            let _ = ui.frame_url.try_set(Some(url));
                        }
                        None => end_session(ws, session, ui, true),
                    }
                } else {
                    end_session(ws, session, ui, true);
                }
            }
            // Closed (or gone): make sure the socket follows the session.
            _ => {
                let _ = ws.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        BrowserLiveClosed, BrowserTargetBinding, ClosedReason, ConsumeFragmentNonceResponse,
        ConversationBinding, IssuerIdentity, ResourceBinding, ViewerBinding,
    };

    #[test]
    fn status_classification_is_fixed() {
        assert_eq!(classify_status(200), Ok(()));
        assert_eq!(classify_status(204), Ok(()));
        assert_eq!(classify_status(403), Err(StepError::Status4xx));
        assert_eq!(classify_status(400), Err(StepError::Status4xx));
        assert_eq!(classify_status(426), Err(StepError::Status4xx));
        assert_eq!(classify_status(500), Err(StepError::ServerStatus));
        assert_eq!(classify_status(302), Err(StepError::ServerStatus));
    }

    #[test]
    fn step_errors_map_to_fixed_outcomes() {
        for error in [StepError::Status4xx, StepError::TooLarge, StepError::Decode] {
            assert_eq!(error.outcome(), StepOutcome::InvalidLink);
        }
        for error in [StepError::Network, StepError::ServerStatus] {
            assert_eq!(error.outcome(), StepOutcome::Failed);
        }
    }

    #[test]
    fn replies_decode_only_as_the_exact_protocol_struct() {
        let ok: Result<ConsumeFragmentNonceResponse, _> =
            decode_response(r#"{"ticket":"abc","expires_at_ms":5}"#);
        assert!(ok.is_ok());
        // Unknown field, wrong shape and non-JSON are all the same fixed error.
        for body in [
            r#"{"ticket":"abc","expires_at_ms":5,"extra":1}"#,
            r#"{"ticket":1}"#,
            "<html>SECRET-body</html>",
            "",
        ] {
            let result: Result<ConsumeFragmentNonceResponse, _> = decode_response(body);
            assert_eq!(result.unwrap_err(), StepError::Decode);
        }
    }

    #[test]
    fn oversize_replies_are_rejected_before_decoding() {
        let body = format!(
            r#"{{"ticket":"{}","expires_at_ms":5}}"#,
            "a".repeat(RESPONSE_MAX_BYTES)
        );
        let result: Result<ConsumeFragmentNonceResponse, _> = decode_response(&body);
        assert_eq!(result.unwrap_err(), StepError::TooLarge);
    }

    #[test]
    fn step_error_debug_carries_no_payload() {
        let rendered = format!("{:?}", StepError::Decode);
        assert_eq!(rendered, "Decode");
    }

    #[test]
    fn ws_target_is_the_exact_origin_derived_endpoint() {
        let origin = "https://panel.example.com";
        let endpoints = ViewerEndpoints::from_origin(origin).unwrap();
        assert_eq!(
            ws_target(&endpoints, origin).as_deref(),
            Some("wss://panel.example.com/viewer/browser/ws")
        );
        let local = ViewerEndpoints::from_origin("http://localhost:3000").unwrap();
        assert_eq!(
            ws_target(&local, "http://localhost:3000").as_deref(),
            Some("ws://localhost:3000/viewer/browser/ws")
        );
        // Endpoints built for a different origin never dial this one.
        assert_eq!(ws_target(&endpoints, "https://evil.example.com"), None);
        assert_eq!(ws_target(&endpoints, "http://panel.example.com"), None);
        // An unusable page origin yields no target at all.
        assert_eq!(ws_target(&endpoints, "https://panel.example.com/x"), None);
        assert_eq!(ws_target(&endpoints, "wss://panel.example.com"), None);
        assert_eq!(ws_target(&endpoints, ""), None);
    }

    #[test]
    fn binary_messages_are_bounded() {
        assert!(!binary_len_ok(0));
        assert!(binary_len_ok(1));
        assert!(binary_len_ok(BINARY_MESSAGE_MAX_BYTES));
        assert!(!binary_len_ok(BINARY_MESSAGE_MAX_BYTES + 1));
    }

    fn closed_message() -> String {
        let message = ViewerServerMessage::Closed {
            closed: BrowserLiveClosed {
                resource_id: "resource-5".to_string(),
                binding: ResourceBinding::Browser {
                    viewer: ViewerBinding {
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
                    },
                },
                reason: ClosedReason::Revoked,
                at_ms: 1,
            },
        };
        serde_json::to_string(&message).unwrap()
    }

    #[test]
    fn post_auth_text_is_classified_fail_closed() {
        assert_eq!(classify_text(&closed_message()), TextAction::Closed);
        assert_eq!(
            classify_text(r#"{"type":"error","code":"denied"}"#),
            TextAction::Failed
        );
        assert_eq!(classify_text("not json"), TextAction::Failed);
        assert_eq!(classify_text(r#"{"type":"mystery"}"#), TextAction::Failed);
        assert_eq!(
            classify_text(&"x".repeat(TEXT_MESSAGE_MAX_BYTES + 1)),
            TextAction::Failed
        );
    }

    #[test]
    fn jpeg_data_url_is_standard_padded_base64() {
        let prefix = "data:image/jpeg;base64,";
        assert_eq!(jpeg_data_url(b""), prefix);
        assert_eq!(jpeg_data_url(b"f"), format!("{prefix}Zg=="));
        assert_eq!(jpeg_data_url(b"fo"), format!("{prefix}Zm8="));
        assert_eq!(jpeg_data_url(b"foo"), format!("{prefix}Zm9v"));
        assert_eq!(jpeg_data_url(b"foob"), format!("{prefix}Zm9vYg=="));
        assert_eq!(jpeg_data_url(b"fooba"), format!("{prefix}Zm9vYmE="));
        assert_eq!(jpeg_data_url(b"foobar"), format!("{prefix}Zm9vYmFy"));
        assert_eq!(
            jpeg_data_url(&[0xff, 0xd8, 0xff, 0xe0]),
            format!("{prefix}/9j/4A==")
        );
    }

    #[test]
    fn native_start_is_inert() {
        // Compiles and does nothing off-wasm; there is no browser transport.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let owner = Owner::new();
            owner.with(|| {
                let session = ViewerSession::from_location(
                    "https://panel.example.com",
                    "#grant_id=grant-abc_123&nonce=nonce-0123456789abcdef-0123456789abcdef",
                );
                if let Ok(session) = session {
                    let session = RwSignal::new(session);
                    let ui = TransportUi::new();
                    start(session, ui, "https://panel.example.com".to_string());
                    assert!(!ui.failed.get_untracked());
                    assert!(ui.frame_url.get_untracked().is_none());
                }
            });
        }
    }
}
