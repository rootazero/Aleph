//! Failure modes of the CDP transport.
//!
//! Every variant names the method it belongs to. "cdp timed out" on its own tells a model nothing
//! it can act on; "DOM.getDocument timed out after 30s" tells it which verb is stuck and how long
//! the engine has been holding it (spec §7.1). Nothing here decides what to do about a failure —
//! that judgement belongs to the caller (R7).

use std::time::Duration;

use thiserror::Error;

/// Why the socket is no longer usable.
///
/// The three are kept apart because they are three different operator facts: the peer went away,
/// the transport broke, or we hung up. Collapsing them would make "the browser died" and "we
/// closed the tab" answer identically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The peer sent a close frame or ended the stream cleanly.
    PeerClosed,
    /// The websocket or the TCP stream errored; the string is the transport's own words.
    TransportError(String),
    /// [`crate::CdpConnection::close`] was called, or the last handle was dropped.
    LocalClose,
}

/// Everything this crate can fail with.
///
/// `Clone` is load-bearing: when the socket dies, one `CloseReason` is handed to every pending
/// call at once. `PartialEq` lets a test assert a whole value instead of matching a shape and then
/// re-reading its fields.
#[derive(Clone, Debug, PartialEq, Error)]
pub enum CdpError {
    /// The peer answered with an `error` object. `data` is kept verbatim: some engines put the
    /// only actionable detail there, and dropping it would leave the caller with a message that
    /// says less than the wire did.
    #[error("cdp {method}: {code} {message}")]
    Protocol {
        method: String,
        code: i64,
        message: String,
        data: Option<serde_json::Value>,
    },

    /// The per-command budget expired with no reply. `waited` is the measured elapsed time, not
    /// the configured budget — the caller reports it to the model as a runtime fact (spec §3.4.2),
    /// and a configured number would be a different claim from the one it makes.
    #[error("cdp {method} timed out after {waited:?}")]
    Timeout { method: String, waited: Duration },

    /// The socket closed while this call was outstanding, or before it was sent.
    #[error("cdp connection closed: {0:?}")]
    Disconnected(CloseReason),

    /// The websocket could not be established or could not carry a frame.
    #[error("cdp transport: {0}")]
    Transport(String),

    /// A reply arrived but did not have the shape the method promises. Never downgraded to a
    /// default value: a missing `sessionId` means "we do not know the session", never "no session"
    /// (判据 §8).
    #[error("cdp decode: {0}")]
    Decode(String),
}

/// The crate's result alias.
pub type Result<T> = std::result::Result<T, CdpError>;
