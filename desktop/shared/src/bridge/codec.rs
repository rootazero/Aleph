//! Line-delimited JSON-RPC codec — one JSON object per line, `\n` terminator.

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{DesktopError, Result};

pub fn encode<T: Serialize>(msg: &T) -> Result<String> {
    let mut s = serde_json::to_string(msg)
        .map_err(|e| DesktopError::BridgeFailed(format!("encode: {e}")))?;
    s.push('\n');
    Ok(s)
}

pub fn decode_line<T: DeserializeOwned>(line: &str) -> Result<T> {
    // Deliberately no `raw={line}` in the error: a bridge line can carry OCR
    // text, window titles, or PIM data, and this error is logged at warn level
    // by the reader loop. The raw line stays available at trace level there.
    serde_json::from_str(line.trim_end_matches('\n'))
        .map_err(|e| DesktopError::BridgeFailed(format!("decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_adds_newline() {
        let line = encode(&"hello").unwrap();
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn decode_parses_line() {
        let v: serde_json::Value = decode_line("{\"jsonrpc\":\"2.0\",\"id\":1}").unwrap();
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn decode_trims_trailing_newline() {
        let v: serde_json::Value = decode_line("{\"id\":7}\n").unwrap();
        assert_eq!(v["id"], 7);
    }
}
