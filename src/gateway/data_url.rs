//! RFC 2397 `data:` URL decoder for channel-agnostic use.
//!
//! Originally lived inline in
//! `src/gateway/interfaces/imessage/bluebubbles/api.rs::decode_data_url`.
//! Factored out so any transport that wants to accept a binary blob as a
//! `data:` URL (Telegram, WhatsApp, Slack, …) can share the same parser
//! and the same RFC 2397 contract.
//!
//! ## Contract (RFC 2397)
//!
//! ```text
//! data:[<mediatype>][;base64],<data>
//! ```
//!
//! - Only `;base64,` payloads are accepted. URL-encoded (no `;base64,`)
//!   payloads are **rejected**: every chat-icon image is binary, the
//!   caller always produces base64, and silently URL-decoding bytes can
//!   corrupt them. Returning a clean `DataUrlError::NotBase64` surfaces
//!   the mistake instead of guessing.
//! - The mediatype is optional; absent one defaults to
//!   `application/octet-stream`. A `;charset=...` hint is dropped (binary
//!   image payloads don't care; we only need the bytes to survive
//!   transport).
//!
//! ## Errors
//!
//! Every error returns a structured [`DataUrlError`] that downstream
//! channels can map into their own error type. The original BlueBubbles
//! copy mapped into `BbError::BadResponse`; WhatsApp's set_group_icon
//! maps into `ChannelError::InvalidInput`. Keep the variants narrow so
//! callers don't lose information in the bridge.

use base64::Engine;
use thiserror::Error;

/// All the ways a `data:` URL can be malformed for our purposes.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DataUrlError {
    /// The string does not start with `data:`.
    #[error("missing data: prefix")]
    MissingPrefix,

    /// No comma separating mediatype/metadata from the payload.
    #[error("missing comma in data: URL")]
    MissingComma,

    /// The URL is well-formed but uses `;charset=...` (or no `;base64,`).
    /// Image/icon payloads are always binary; we reject the ambiguous
    /// URL-encoded shape rather than silently corrupt the bytes.
    #[error("only base64 data: URLs are accepted")]
    NotBase64,

    /// The base64 payload could not be decoded.
    #[error("invalid base64: {0}")]
    InvalidBase64(String),
}

/// Decode a `data:` URL into `(mediatype, bytes)`.
///
/// See module docs for the full contract. The default mediatype (when
/// the URL omits one) is `application/octet-stream`.
pub fn decode(s: &str) -> Result<(String, Vec<u8>), DataUrlError> {
    const PREFIX: &str = "data:";
    let rest = s.strip_prefix(PREFIX).ok_or(DataUrlError::MissingPrefix)?;
    let (meta, payload) = rest.split_once(',').ok_or(DataUrlError::MissingComma)?;

    let mut mime = "application/octet-stream".to_string();
    let mut is_base64 = false;
    for piece in meta.split(';') {
        if piece == "base64" {
            is_base64 = true;
        } else if let Some(m) = piece.strip_prefix("charset=") {
            // Binary image payloads don't care about the charset hint.
            // Drop it; keep the mediatype.
            let _ = m;
        } else if !piece.is_empty() && mime == "application/octet-stream" {
            mime = piece.to_string();
        }
    }

    if !is_base64 {
        return Err(DataUrlError::NotBase64);
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| DataUrlError::InvalidBase64(e.to_string()))?;
    Ok((mime, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_accepts_base64_png() {
        let s = "data:image/png;base64,iVBORw0KGgo=";
        let (mime, bytes) = decode(s).expect("valid data URL");
        assert_eq!(mime, "image/png");
        assert!(!bytes.is_empty());
        // PNG magic header
        assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn decode_defaults_mime_when_omitted() {
        let s = "data:;base64,SGVsbG8=";
        let (mime, bytes) = decode(s).expect("mime-defaulted URL");
        assert_eq!(mime, "application/octet-stream");
        assert_eq!(bytes, b"Hello");
    }

    #[test]
    fn decode_ignores_charset_hint() {
        let s = "data:image/jpeg;charset=utf-8;base64,/wo=";
        let (mime, _) = decode(s).expect("charset URL");
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn decode_rejects_url_encoded_payload() {
        let s = "data:text/plain,Hello%20World";
        let err = decode(s).expect_err("URL-encoded should be rejected");
        assert_eq!(err, DataUrlError::NotBase64);
    }

    #[test]
    fn decode_rejects_missing_prefix() {
        let err = decode("http://example.com/x").expect_err("wrong scheme");
        assert_eq!(err, DataUrlError::MissingPrefix);
    }

    #[test]
    fn decode_rejects_missing_comma() {
        let err = decode("data:image/png;base64").expect_err("no separator");
        assert_eq!(err, DataUrlError::MissingComma);
    }

    #[test]
    fn decode_rejects_invalid_base64() {
        let err = decode("data:image/png;base64,not_base64!!!").expect_err("bad b64");
        assert!(matches!(err, DataUrlError::InvalidBase64(_)));
    }
}
