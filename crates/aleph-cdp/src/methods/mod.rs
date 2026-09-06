//! Thin typed wrappers over the ~30 CDP methods Aleph calls.
//!
//! Hand-written on purpose. Generating the protocol would put thousands of types nobody calls into
//! the build (R3) and would make "which part of CDP does Aleph depend on" unanswerable — this
//! module IS that answer. Each wrapper does exactly three things: build params with `json!`, call,
//! parse the reply into a small struct. No retries (that decision belongs to the caller), no
//! defaults invented for a field the peer did not send, no interpretation of what a value means.

pub mod browser;
pub mod dom;
pub mod dom_snapshot;
pub mod emulation;
pub mod input;
pub mod network;
pub mod page;
pub mod runtime;
pub mod target;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{CdpError, Result};

/// Parse a CDP result into `T`, naming the method and showing the value that would not parse.
pub(crate) fn decode<T: DeserializeOwned>(method: &str, value: Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|e| CdpError::Decode(format!("{method}: {e}; reply was {value}")))
}

/// A required field of a CDP result. Absent is `Decode`, never a default: "the peer did not say"
/// and "the peer said nothing is there" are different facts (判据 §8).
pub(crate) fn field<'a>(method: &str, value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .get(key)
        .ok_or_else(|| CdpError::Decode(format!("{method}: reply has no `{key}`: {value}")))
}

/// Decode the base64 `data` field CDP uses for binary payloads.
pub(crate) fn decode_base64(method: &str, value: &Value) -> Result<Vec<u8>> {
    use base64::Engine as _;
    let data = field(method, value, "data")?
        .as_str()
        .ok_or_else(|| CdpError::Decode(format!("{method}: `data` is not a string: {value}")))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| CdpError::Decode(format!("{method}: `data` is not valid base64: {e}")))
}
