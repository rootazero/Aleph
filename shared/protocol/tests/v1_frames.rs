//! Golden Protocol v1 frames shared with the TSX Panel web host
//! (`interfaces/panel/web/host.test.mjs`).
//!
//! The payloads are literal text, not `serde_json` output, so the bytes do not
//! depend on map ordering features. This test proves Rust accepts each payload
//! as the type it claims to be and that the fixture holds the exact wire bytes
//! `framing` produces. Regenerate with `ALEPH_UPDATE_GOLDEN=1`.

use aleph_protocol::framing::{encode_header, split_frame, HEADER_BYTES};
use aleph_protocol::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use aleph_protocol::panel::PANEL_HOST_UNAVAILABLE;
use aleph_protocol::system_hello::HelloParams;
use serde_json::{json, Value};

/// The Web v1 endpoint's payload cap (`MAX_MESSAGE` in
/// `src/gateway/server/connection/web_v1.rs`).
const WEB_V1_MAX_PAYLOAD: u32 = 1024 * 1024;

/// The first frame the TSX Panel web host sends, byte for byte.
const HELLO: &str = r#"{"jsonrpc":"2.0","id":1,"method":"system.hello","params":{"protocol":{"major":1,"minor":0},"client":{"name":"aleph-panel","version":"0.1.0","platform":"web","ui_runtime":"nsh"},"features":[]}}"#;
const CALL: &str = r#"{"jsonrpc":"2.0","id":2,"method":"sessions.list","params":{}}"#;
/// Multi-byte UTF-8: the length prefix counts bytes, not UTF-16 units.
const RESULT: &str = r#"{"jsonrpc":"2.0","result":{"label":"会话 ✓"},"id":2}"#;
const ERROR: &str =
    r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"method unavailable on Web v1"},"id":3}"#;

fn wire(payload: &str) -> Vec<u8> {
    let mut bytes = encode_header(payload.len(), WEB_V1_MAX_PAYLOAD)
        .expect("golden payloads fit")
        .to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fixture() -> Value {
    let frames: Vec<Value> = [
        ("hello", HELLO),
        ("call", CALL),
        ("result", RESULT),
        ("error", ERROR),
    ]
    .iter()
    .map(|(name, payload)| {
        let bytes = wire(payload);
        let (text, rest) = split_frame(&bytes, WEB_V1_MAX_PAYLOAD)
            .expect("valid frame")
            .expect("complete frame");
        assert_eq!((text, rest.len()), (*payload, 0));
        json!({ "name": name, "payload": payload, "wire_hex": hex(&bytes) })
    })
    .collect();
    let oversize = (WEB_V1_MAX_PAYLOAD + 1).to_be_bytes();
    json!({
        "header_bytes": HEADER_BYTES,
        "max_payload": WEB_V1_MAX_PAYLOAD,
        "frames": frames,
        "oversize_header_hex": hex(&oversize),
        "host_unavailable_code": PANEL_HOST_UNAVAILABLE,
    })
}

#[test]
fn golden_payloads_are_the_types_they_claim() {
    let hello: JsonRpcRequest = serde_json::from_str(HELLO).unwrap();
    assert_eq!(hello.method, "system.hello");
    let params: HelloParams = serde_json::from_value(hello.params.unwrap()).unwrap();
    assert_eq!((params.protocol.major, params.protocol.minor), (1, 0));
    assert!(params.features.is_empty());

    let call: JsonRpcRequest = serde_json::from_str(CALL).unwrap();
    assert_eq!(call.id, Some(json!(2)));
    let result: JsonRpcResponse = serde_json::from_str(RESULT).unwrap();
    assert!(result.error.is_none());
    let error: JsonRpcResponse = serde_json::from_str(ERROR).unwrap();
    assert_eq!(error.error.unwrap().code, -32601);
}

#[test]
fn v1_frame_golden_vectors_are_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v1_frames.json");
    let expected = fixture();
    if std::env::var_os("ALEPH_UPDATE_GOLDEN").is_some() {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&expected).unwrap() + "\n",
        )
        .unwrap();
    }
    let current = std::fs::read_to_string(path)
        .expect("fixture exists; regenerate with ALEPH_UPDATE_GOLDEN=1");
    // Compare values, not text: key order in the file is not part of the contract.
    let current: Value = serde_json::from_str(&current).unwrap();
    assert_eq!(current, expected, "regenerate with ALEPH_UPDATE_GOLDEN=1");
}
